//! Работа с Telegram: подключение и фоновый обработчик команд интерфейса.

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow};
use grammers_client::media::{Downloadable, Media};
use grammers_client::message::{InputMessage, Message};
use grammers_client::peer::Peer;
use grammers_client::session::types::{PeerId, PeerRef};
use grammers_client::tl;
use grammers_client::update::Update;
use grammers_client::{Client, SenderPool};
use grammers_mtsender::{InvocationError, RetryContext, RetryPolicy, RpcError, UpdatesConfiguration};
use tokio::sync::mpsc;

use crate::util::log;
use crate::vault::VaultSession;
use crate::{tr, trf};

/// Сколько диалогов держим в памяти. Небольшое число = мало ОЗУ.
/// Первая страница списка чатов: приходит быстро и обновляется каждые пару минут.
const QUICK_DIALOGS: usize = 100;
/// Предел полного списка чатов (страницами по 100): защита от аккаунтов с десятками тысяч диалогов.
const MAX_DIALOGS: usize = 3000;
/// Сколько результатов поиска показывать.
const MAX_HITS: usize = 40;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    User,
    Group,
    Channel,
}

/// Когда пользователь был в сети (то, что Telegram разрешает видеть).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Presence {
    /// Unix-время, до которого считаем «в сети».
    Online(i64),
    /// Unix-время последнего визита.
    Offline(i64),
    Recently,
    LastWeek,
    LastMonth,
    /// Скрыто настройками приватности.
    Hidden,
}

pub fn presence_from(st: &tl::enums::UserStatus) -> Presence {
    use tl::enums::UserStatus as S;
    match st {
        S::Online(o) => Presence::Online(i64::from(o.expires)),
        S::Offline(o) => Presence::Offline(i64::from(o.was_online)),
        S::Recently(_) => Presence::Recently,
        S::LastWeek(_) => Presence::LastWeek,
        S::LastMonth(_) => Presence::LastMonth,
        _ => Presence::Hidden,
    }
}

pub fn is_online(p: Presence, now: i64) -> bool {
    matches!(p, Presence::Online(exp) if exp > now)
}

fn local(ts: i64, tz: &jiff::tz::TimeZone) -> Option<jiff::Zoned> {
    Some(jiff::Timestamp::from_second(ts).ok()?.to_zoned(tz.clone()))
}

/// Подпись для заголовка чата: «в сети», «был(а) в 14:32», «был(а) недавно».
pub fn presence_label(p: Presence, now: i64, tz: &jiff::tz::TimeZone) -> String {
    match p {
        Presence::Online(exp) if exp > now => tr!("в сети", "online").to_string(),
        Presence::Online(_) => tr!("был(а) только что", "last seen just now").to_string(),
        Presence::Offline(ts) => {
            let (Some(t), Some(n)) = (local(ts, tz), local(now, tz)) else {
                return tr!("был(а) в сети", "last seen online").to_string();
            };
            let hm = t.strftime("%H:%M");
            if t.date() == n.date() {
                trf!("был(а) в {hm}", "last seen at {hm}")
            } else if n.date().yesterday().ok() == Some(t.date()) {
                trf!("был(а) вчера в {hm}", "last seen yesterday at {hm}")
            } else {
                trf!("был(а) {} в {hm}", "last seen {} at {hm}", t.strftime("%d.%m"))
            }
        }
        Presence::Recently => tr!("был(а) недавно", "last seen recently").to_string(),
        Presence::LastWeek => tr!("был(а) на этой неделе", "last seen this week").to_string(),
        Presence::LastMonth => tr!("был(а) в этом месяце", "last seen this month").to_string(),
        Presence::Hidden => String::new(),
    }
}

#[derive(Clone, Debug)]
pub struct ChatInfo {
    pub id: PeerId,
    pub peer: PeerRef,
    pub name: String,
    pub kind: Kind,
    pub unread: i32,
    /// Непрочитанные упоминания (@).
    pub mentions: i32,
    /// Только для личных чатов.
    pub presence: Option<Presence>,
    pub is_contact: bool,
    pub is_bot: bool,
    pub is_self: bool,
    /// Самое большое прочитанное собеседником ваше сообщение (для отметок прочтения в личных чатах).
    pub read_out: i32,
    /// Черновик, сохранённый в Telegram (набран на телефоне или в другом клиенте).
    pub draft: String,
}

/// Папка чатов из аккаунта Telegram.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Folder {
    pub id: i32,
    pub title: String,
    pub include: Vec<PeerId>,
    pub exclude: Vec<PeerId>,
    pub include_self: bool,
    pub contacts: bool,
    pub non_contacts: bool,
    pub groups: bool,
    pub broadcasts: bool,
    pub bots: bool,
    pub exclude_read: bool,
}

impl Folder {
    pub fn matches(&self, c: &ChatInfo) -> bool {
        if self.exclude.contains(&c.id) {
            return false;
        }
        if self.exclude_read && c.unread == 0 {
            return false;
        }
        if self.include.contains(&c.id) || (self.include_self && c.is_self) {
            return true;
        }
        match c.kind {
            Kind::User if c.is_self => false,
            Kind::User if c.is_bot => self.bots,
            Kind::User if c.is_contact => self.contacts,
            Kind::User => self.non_contacts,
            Kind::Group => self.groups,
            Kind::Channel => self.broadcasts,
        }
    }
}

/// Что делает собеседник в данный момент.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TypingKind {
    Text,
    Voice,
    File,
    Stop,
}

impl TypingKind {
    pub fn label(self) -> &'static str {
        match self {
            TypingKind::Text => tr!("печатает…", "typing…"),
            TypingKind::Voice => tr!("записывает голосовое…", "recording voice…"),
            TypingKind::File => tr!("отправляет файл…", "sending a file…"),
            TypingKind::Stop => "",
        }
    }
}

/// Отложенное сообщение.
#[derive(Clone, Debug, PartialEq)]
pub struct ScheduledMsg {
    pub id: i32,
    /// Unix-время отправки.
    pub date: i64,
    pub text: String,
}

/// Разбирает «сырое» обновление о наборе текста.
pub fn typing_from_raw(u: &tl::enums::Update) -> Option<(PeerId, Option<PeerId>, TypingKind)> {
    use tl::enums::SendMessageAction as A;
    let kind = |a: &A| match a {
        A::SendMessageCancelAction => TypingKind::Stop,
        A::SendMessageTypingAction => TypingKind::Text,
        A::SendMessageRecordAudioAction | A::SendMessageRecordRoundAction | A::SendMessageRecordVideoAction => TypingKind::Voice,
        A::SpeakingInGroupCallAction => TypingKind::Stop,
        _ => TypingKind::File,
    };
    match u {
        tl::enums::Update::UserTyping(x) => Some((PeerId::user(x.user_id)?, None, kind(&x.action))),
        tl::enums::Update::ChatUserTyping(x) => Some((PeerId::chat(x.chat_id)?, Some(PeerId::from(x.from_id.clone())), kind(&x.action))),
        tl::enums::Update::ChannelUserTyping(x) if x.top_msg_id.is_none() => {
            Some((PeerId::channel(x.channel_id)?, Some(PeerId::from(x.from_id.clone())), kind(&x.action)))
        }
        _ => None,
    }
}

/// Откуда переслано сообщение.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fwd {
    /// Имя, если Telegram его прислал (скрытый автор, подпись канала).
    pub name: Option<String>,
    /// Отправитель, если он известен: имя подставляется из списка чатов.
    pub from: Option<PeerId>,
}

/// Реакция под сообщением: смайлик, сколько раз поставили и ставили ли вы.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reaction {
    pub emoji: String,
    pub count: i32,
    pub mine: bool,
}

/// Что делает кнопка под сообщением бота.
#[derive(Clone, Debug, PartialEq)]
pub enum BtnAction {
    /// Нажатие, которое сообщается боту.
    Callback(Vec<u8>),
    Url(String),
    /// Текст, который кнопка предлагает скопировать.
    Copy(String),
    /// Остальные виды (игры, платежи, веб-приложения, вход через inline): здесь не поддерживаются.
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Btn {
    pub text: String,
    pub action: BtnAction,
}

/// Вариант ответа в опросе.
#[derive(Clone, Debug, PartialEq)]
pub struct PollOpt {
    pub text: String,
    pub data: Vec<u8>,
    pub voters: i32,
    pub chosen: bool,
    pub correct: bool,
}

/// Опрос или викторина под сообщением.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PollView {
    pub question: String,
    pub quiz: bool,
    pub closed: bool,
    pub multiple: bool,
    pub total: i32,
    pub options: Vec<PollOpt>,
}

#[derive(Clone, Debug, Default)]
pub struct ChatMsg {
    pub id: i32,
    /// Unix-время (секунды) для разделителей по датам.
    pub ts: i64,
    pub outgoing: bool,
    pub sender: String,
    pub text: String,
    /// Текст без подписи вложения (для правки): `Some`, если у сообщения есть вложение.
    pub caption: Option<String>,
    /// Исходный текст из Telegram (с эмодзи, без подписи вложения): им заполняется поле при правке.
    pub raw: String,
    pub time: String,
    /// Сообщение исправлено после отправки.
    pub edited: bool,
    /// Номер сообщения, на которое это является ответом.
    pub reply_to: Option<i32>,
    pub fwd: Option<Fwd>,
    /// В сообщении упомянули вас (@).
    pub mentioned: bool,
    /// Просмотры поста канала (на момент загрузки).
    pub views: Option<i32>,
    /// Реакции под сообщением (на момент загрузки).
    pub reactions: Vec<Reaction>,
    /// Inline-кнопки под сообщением бота, по рядам.
    pub buttons: Vec<Vec<Btn>>,
    /// Опрос (если сообщение — опрос).
    pub poll: Option<PollView>,
}

/// Найденное сообщение.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub chat: PeerId,
    pub chat_name: String,
    pub id: i32,
    pub ts: i64,
    pub when: String,
    pub sender: String,
    pub text: String,
}

/// Команды от интерфейса к фоновому обработчику.
pub enum Cmd {
    /// Обновить начало списка чатов (первые 100) и папки; новое вливается в имеющийся список.
    LoadDialogs,
    /// Загрузить весь список чатов страницами по 100 (при запуске и по F5).
    LoadAllDialogs,
    LoadHistory(ChatInfo, usize),
    /// Подгрузить сообщения старше `before` (PgUp у самого верха переписки).
    LoadOlder {
        chat: ChatInfo,
        before: i32,
        limit: usize,
    },
    /// Загрузить окружение найденного сообщения: оно и соседние с ним.
    LoadAround {
        chat: ChatInfo,
        id: i32,
        ts: i64,
    },
    /// Найти текст в одном чате.
    SearchChat {
        chat: ChatInfo,
        query: String,
    },
    /// Найти текст во всех чатах.
    SearchAll {
        query: String,
    },
    Send {
        chat: ChatInfo,
        text: String,
        markdown: bool,
        /// Отложенная публикация: через сколько минут отправить.
        delay_minutes: Option<u32>,
        /// Ответ на сообщение с этим номером.
        reply_to: Option<i32>,
    },
    /// Переслать сообщения `ids` из `src` в `dst` (одно или несколько, по порядку).
    Forward {
        src: ChatInfo,
        dst: ChatInfo,
        ids: Vec<i32>,
    },
    MarkRead(ChatInfo),
    /// Поставить реакцию на сообщение (`None` снимает вашу реакцию).
    React {
        chat: ChatInfo,
        id: i32,
        emoji: Option<String>,
    },
    /// Нажать inline-кнопку бота.
    Press {
        chat: ChatInfo,
        id: i32,
        data: Vec<u8>,
    },
    /// Скачать вложение сообщения `id` в папку `downloads` рядом с программой.
    Save {
        chat: ChatInfo,
        id: i32,
    },
    /// Скачать вложение во временную папку `view` и открыть его программой по умолчанию.
    Open {
        chat: ChatInfo,
        id: i32,
    },
    /// Отправить файл с диска.
    SendFile {
        chat: ChatInfo,
        path: PathBuf,
        caption: String,
    },
    /// Показать отложенные сообщения чата.
    ListScheduled(ChatInfo),
    /// Отменить отложенное сообщение.
    DeleteScheduled {
        chat: ChatInfo,
        id: i32,
    },
    /// Исправить своё сообщение.
    Edit {
        chat: ChatInfo,
        id: i32,
        text: String,
        markdown: bool,
    },
    /// Удалить своё сообщение (у всех).
    Delete {
        chat: ChatInfo,
        id: i32,
    },
    /// Проголосовать в опросе: вариант `option` сообщения `id`.
    Vote {
        chat: ChatInfo,
        id: i32,
        option: Vec<u8>,
    },
    /// Удалить несколько своих сообщений (у всех).
    DeleteMany {
        chat: ChatInfo,
        ids: Vec<i32>,
    },
    /// Показать все закреплённые сообщения чата.
    ListPinned(ChatInfo),
    /// Список тем форума (супергруппы с темами).
    ListTopics(ChatInfo),
    /// Прочитать небольшой текстовый файл из сообщения прямо в окне.
    ViewText {
        chat: ChatInfo,
        id: i32,
    },
    /// Несколько фото или файлов одним сообщением (альбом, 2–10 штук).
    SendAlbum {
        chat: ChatInfo,
        paths: Vec<PathBuf>,
        caption: String,
    },
    /// Проверка связи: DNS, прокси и дата-центры Telegram.
    NetCheck(Box<crate::settings::Settings>),
    /// Скачать уменьшенную копию фото для просмотра в консоли.
    Preview {
        chat: ChatInfo,
        id: i32,
    },
    /// Найти человека или канал по @имени и открыть чат с ним (в том числе без переписки).
    OpenTarget(Target),
    /// Вступить в группу или канал по ссылке-приглашению или публичному имени.
    Join(Target),
    /// Выйти из группы или канала.
    Leave(ChatInfo),
    /// Сохранить черновик в Telegram.
    SaveDraft {
        chat: ChatInfo,
        text: String,
    },
}

/// События от обработчика к интерфейсу.
pub enum Event {
    Dialogs(Vec<ChatInfo>),
    Folders(Vec<Folder>),
    History {
        chat: PeerId,
        msgs: Vec<ChatMsg>,
    },
    Incoming {
        chat: PeerId,
        chat_name: String,
        msg: ChatMsg,
    },
    Sent {
        chat: PeerId,
        msg: ChatMsg,
        scheduled: bool,
    },
    /// Отправка не удалась: текст возвращается в поле ввода, чтобы не пропал.
    SendFailed {
        text: String,
    },
    Edited {
        chat: PeerId,
        msg: ChatMsg,
    },
    /// `chat` = None для личных чатов и обычных групп (id сообщений общие на аккаунт).
    Deleted {
        chat: Option<PeerId>,
        ids: Vec<i32>,
    },
    Presence {
        user: PeerId,
        presence: Presence,
    },
    /// Кто-то печатает (или перестал) в чате.
    Typing {
        chat: PeerId,
        from: Option<PeerId>,
        kind: TypingKind,
    },
    /// Закреплённое сообщение чата (текст) или его отсутствие.
    Pinned {
        chat: PeerId,
        text: Option<String>,
    },
    Scheduled {
        chat: PeerId,
        items: Vec<ScheduledMsg>,
    },
    /// Часть списка чатов (самые свежие): вливается в имеющийся список, ничего не удаляя.
    DialogsPage(Vec<ChatInfo>),
    /// Более старые сообщения чата; пустой список значит, что история закончилась.
    Older {
        chat: PeerId,
        msgs: Vec<ChatMsg>,
    },
    /// Окружение найденного сообщения `focus`.
    Around {
        chat: PeerId,
        msgs: Vec<ChatMsg>,
        focus: i32,
    },
    /// Результаты поиска; `scope` — чат поиска или `None` для поиска по всем чатам.
    SearchResults {
        query: String,
        scope: Option<PeerId>,
        hits: Vec<SearchHit>,
    },
    /// Файл для просмотра скачан: интерфейс открывает его.
    Open(PathBuf),
    /// Уменьшенная копия фото (JPEG) для показа в консоли.
    Preview {
        chat: PeerId,
        jpeg: Vec<u8>,
    },
    /// Задержка до Telegram в миллисекундах; `None` — связи нет.
    Latency(Option<u32>),
    /// Сообщение не ушло из-за отсутствия связи и ждёт в очереди.
    Queued {
        chat: PeerId,
        text: String,
        count: usize,
    },
    /// Сообщение из очереди отправлено.
    QueueSent {
        chat: PeerId,
        text: String,
        left: usize,
    },
    /// Сообщение из очереди отклонено Telegram (не из-за связи).
    QueueDropped {
        chat: PeerId,
        text: String,
        reason: String,
    },
    /// Обновились реакции сообщения.
    Reacted {
        chat: PeerId,
        msg: ChatMsg,
    },
    /// Бот ответил на нажатие кнопки (текст может быть пустым).
    ButtonAnswer(String),
    /// Собеседник прочитал ваши сообщения в личном чате до номера `max_id`.
    ReadOutbox {
        chat: PeerId,
        max_id: i32,
    },
    /// Темы форума: (номер, название, непрочитанных).
    /// Содержимое текстового файла для окна чтения.
    TextFile {
        chat: PeerId,
        name: String,
        lines: Vec<String>,
    },
    /// Результат проверки связи, строками.
    NetCheck(Vec<String>),
    Topics {
        chat: PeerId,
        items: Vec<(i32, String, i32)>,
    },
    /// Закреплённые сообщения чата (новые сверху).
    PinnedList {
        chat: PeerId,
        hits: Vec<SearchHit>,
    },
    /// Найденный или присоединённый чат: интерфейс добавляет его в список и открывает.
    Found(ChatInfo),
    /// Вы вышли из чата.
    Left(PeerId),
    /// Черновик изменён на другом устройстве (пустой текст — стёрт).
    Draft {
        chat: PeerId,
        text: String,
    },
    Status(String),
    Error(String),
}

pub struct Connection {
    pub client: Client,
    pub quit: grammers_mtsender::SenderPoolFatHandle,
    pub updates: mpsc::Receiver<grammers_session::updates::UpdatesLike>,
}

/// Название устройства для списка сеансов Telegram, например «Tg-Lite v1.3.0 (Windows)».
pub fn device_name() -> String {
    let os = match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    };
    format!("Tg-Lite v{} ({os})", env!("CARGO_PKG_VERSION"))
}

/// Подключается к Telegram, используя сессию из хранилища.
/// `proxy` — необязательный SOCKS5-прокси вида `socks5://127.0.0.1:1080`.
pub fn connect(session: Arc<VaultSession>, api_id: i32, proxy: Option<String>) -> Connection {
    // Так клиент называется в Telegram → Настройки → Устройства.
    let params = grammers_mtsender::ConnectionParams {
        proxy_url: proxy,
        retry_policy: Box::new(FloodPolicy),
        device_model: device_name(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        ..Default::default()
    };
    let SenderPool { runner, updates, handle } = SenderPool::with_configuration(session, api_id, params);
    let client = Client::new(handle.clone());
    tokio::spawn(runner.run());
    Connection { client, quit: handle, updates }
}

// ------------------------------------------------------------ преобразования

pub fn format_time(ts: jiff::Timestamp) -> String {
    let z = ts.to_zoned(jiff::tz::TimeZone::system());
    let now = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::system());
    if z.date() == now.date() { z.strftime("%H:%M").to_string() } else { z.strftime("%d.%m %H:%M").to_string() }
}

fn kind_of(peer: &Peer) -> Kind {
    match peer {
        Peer::User(_) => Kind::User,
        Peer::Group(_) => Kind::Group,
        Peer::Channel(_) => Kind::Channel,
        _ => Kind::Group,
    }
}

// ------------------------------------------------------------ ограничения Telegram

/// До какого момента (unix, секунды) Telegram просит подождать (FLOOD_WAIT).
static FLOOD_UNTIL: AtomicI64 = AtomicI64::new(0);

/// Сколько секунд ещё нужно ждать из-за ограничения Telegram (0 — ограничений нет).
pub fn flood_left() -> i64 {
    (FLOOD_UNTIL.load(Ordering::Relaxed) - jiff::Timestamp::now().as_second()).max(0)
}

/// Тихое ожидание при FLOOD_WAIT: до пяти минут запрос просто повторяется,
/// а интерфейс показывает обратный отсчёт вместо ошибки.
struct FloodPolicy;

impl RetryPolicy for FloodPolicy {
    fn should_retry(&self, ctx: &RetryContext) -> ControlFlow<(), Duration> {
        match &ctx.error {
            InvocationError::Rpc(RpcError { code: 420, value: Some(secs), .. }) if ctx.fail_count.get() <= 2 && *secs <= 300 => {
                FLOOD_UNTIL.fetch_max(jiff::Timestamp::now().as_second() + i64::from(*secs), Ordering::Relaxed);
                ControlFlow::Continue(Duration::from_secs(u64::from(*secs)))
            }
            InvocationError::Io(_) if ctx.fail_count.get() <= 1 => ControlFlow::Continue(Duration::from_secs(1)),
            _ => ControlFlow::Break(()),
        }
    }
}

/// Ошибка «нет связи» (а не отказ Telegram): такое сообщение можно отправить позже, когда связь вернётся.
/// Отказы Telegram (`rpc error …`, в том числе ожидание FloodWait) сюда не относятся.
pub fn is_network_error(e: &str) -> bool {
    let e = e.to_lowercase();
    !e.contains("rpc error")
        && (e.contains("request error") || e.contains("dropped") || e.contains("connection") || e.contains("timed out"))
}

/// Сообщение, которое ждёт появления связи.
#[derive(Clone, Debug)]
pub struct Queued {
    pub chat: ChatInfo,
    pub text: String,
    pub markdown: bool,
    pub reply_to: Option<i32>,
}

/// Сколько сообщений держим в очереди без связи (только текст, в памяти).
pub const MAX_QUEUED: usize = 30;

/// Пытается отправить очередь по порядку. На первой же сетевой ошибке останавливается и оставляет остаток.
async fn flush_queue(client: &Client, events: &mpsc::UnboundedSender<Event>, queue: &mut std::collections::VecDeque<Queued>) {
    flush_with(queue, events, |q: Queued| {
        let client = client.clone();
        async move {
            match tokio::time::timeout(Duration::from_secs(15), send(&client, &q.chat, &q.text, q.markdown, None, q.reply_to)).await {
                Ok(r) => r,
                Err(_) => Err(anyhow!("request error: timed out")),
            }
        }
    })
    .await;
}

/// Основа `flush_queue`: отдельно от сети, чтобы её можно было проверить тестом.
pub(crate) async fn flush_with<Fut>(
    queue: &mut std::collections::VecDeque<Queued>,
    events: &mpsc::UnboundedSender<Event>,
    mut send_one: impl FnMut(Queued) -> Fut,
) where
    Fut: std::future::Future<Output = Result<ChatMsg>>,
{
    while let Some(q) = queue.front().cloned() {
        match send_one(q.clone()).await {
            Ok(msg) => {
                queue.pop_front();
                let _ = events.send(Event::Sent { chat: q.chat.id, msg, scheduled: false });
                let _ = events.send(Event::QueueSent { chat: q.chat.id, text: q.text, left: queue.len() });
            }
            Err(e) if !is_network_error(&e.to_string()) => {
                queue.pop_front();
                log(&format!("queued send: {e}"));
                let reason = friendly_error(&e.to_string());
                let _ = events.send(Event::QueueDropped { chat: q.chat.id, text: q.text, reason });
            }
            Err(_) => break, // связи всё ещё нет: попробуем позже
        }
    }
}

/// Раз в несколько секунд измеряет задержку до Telegram и сообщает интерфейсу (`None` — нет связи).
async fn ping_loop(client: Client, events: mpsc::UnboundedSender<Event>) {
    let mut first = true;
    loop {
        tokio::time::sleep(Duration::from_secs(if first { 2 } else { 20 })).await;
        first = false;
        let id = jiff::Timestamp::now().as_nanosecond() as i64;
        let started = std::time::Instant::now();
        let res = tokio::time::timeout(Duration::from_secs(8), client.invoke(&tl::functions::Ping { ping_id: id })).await;
        let ms = match res {
            Ok(Ok(_)) => Some(started.elapsed().as_millis().min(99_999) as u32),
            _ => None,
        };
        if events.send(Event::Latency(ms)).is_err() {
            break;
        }
    }
}

/// Понятное сообщение вместо технического кода ошибки Telegram.
pub fn friendly_error(e: &str) -> String {
    let wait = || e.split(|c: char| !c.is_ascii_digit()).rfind(|x| !x.is_empty()).unwrap_or("?").to_string();
    if e.contains("FLOOD_WAIT") || e.contains("SLOWMODE_WAIT") {
        return trf!(
            "Telegram просит подождать {} с и не принимает запросы",
            "Telegram asks to wait {} s and is not accepting requests",
            wait()
        );
    }
    let known = [
        (
            "MESSAGE_TOO_LONG",
            tr!("сообщение слишком длинное (лимит Telegram 4096 символов)", "message is too long (Telegram limit is 4096 characters)"),
        ),
        ("CHAT_WRITE_FORBIDDEN", tr!("в этот чат писать нельзя", "you can't write in this chat")),
        ("CHAT_ADMIN_REQUIRED", tr!("нужны права администратора", "admin rights required")),
        ("USER_BANNED_IN_CHANNEL", tr!("вам запрещено писать в этот чат", "you are banned from writing in this chat")),
        ("MESSAGE_NOT_MODIFIED", tr!("текст не изменился", "text was not changed")),
        ("MESSAGE_EDIT_TIME_EXPIRED", tr!("время для правки сообщения истекло", "time to edit the message has expired")),
        ("MESSAGE_ID_INVALID", tr!("такого сообщения уже нет", "this message no longer exists")),
        ("PEER_FLOOD", tr!("Telegram ограничил отправку, попробуйте позже", "Telegram limited sending, try again later")),
    ];
    for (code, text) in known {
        if e.contains(code) {
            return text.to_string();
        }
    }
    e.to_string()
}

// ------------------------------------------------------------ подписи вложений

fn trim_zero(s: String) -> String {
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

/// «340 КБ», «2 МБ», «1.5 ГБ».
pub fn fmt_size(bytes: usize) -> String {
    let b = bytes as f64;
    let (kb, mb, gb) = (1024.0, 1024.0 * 1024.0, 1024.0 * 1024.0 * 1024.0);
    if b >= gb {
        trf!("{} ГБ", "{} GB", trim_zero(format!("{:.1}", b / gb)))
    } else if b >= mb {
        trf!("{} МБ", "{} MB", trim_zero(format!("{:.1}", b / mb)))
    } else if b >= kb {
        trf!("{:.0} КБ", "{:.0} KB", b / kb)
    } else {
        trf!("{bytes} Б", "{bytes} B")
    }
}

/// «0:12», «3:05», «1:02:03».
pub fn fmt_duration(secs: f64) -> String {
    let t = secs.max(0.0).round() as u64;
    let (h, m, s) = (t / 3600, t % 3600 / 60, t % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Подпись файла-документа по его атрибутам: голосовое, видео, GIF, аудио или просто файл.
pub fn doc_label(attrs: &[tl::enums::DocumentAttribute], mime: &str, size: Option<usize>) -> String {
    use tl::enums::DocumentAttribute as A;
    let size_s = size.map(fmt_size);
    let with_size = |base: String| match &size_s {
        Some(sz) => format!("[{base}, {sz}]"),
        None => format!("[{base}]"),
    };
    if attrs.iter().any(|a| matches!(a, A::Animated)) {
        return with_size("GIF".into());
    }
    for a in attrs {
        match a {
            A::Audio(au) if au.voice => return trf!("[голосовое {}]", "[voice {}]", fmt_duration(f64::from(au.duration))),
            A::Audio(au) => {
                let who = match (&au.performer, &au.title) {
                    (Some(p), Some(t)) => format!(" {} — {}", crate::util::clean(p), crate::util::clean(t)),
                    (None, Some(t)) => format!(" {}", crate::util::clean(t)),
                    _ => String::new(),
                };
                return with_size(trf!("аудио {}{who}", "audio {}{who}", fmt_duration(f64::from(au.duration))));
            }
            A::Video(v) if v.round_message => return trf!("[видеосообщение {}]", "[video message {}]", fmt_duration(v.duration)),
            A::Video(v) => return with_size(trf!("видео {}", "video {}", fmt_duration(v.duration))),
            _ => {}
        }
    }
    let name = attrs.iter().find_map(|a| match a {
        A::Filename(f) => Some(crate::util::clean(&f.file_name)),
        _ => None,
    });
    if mime.starts_with("image/") && name.is_none() {
        return with_size(tr!("изображение", "image").into());
    }
    match name {
        Some(n) if !n.trim().is_empty() => with_size(trf!("файл {}", "file {}", n.trim())),
        _ => with_size(tr!("файл", "file").into()),
    }
}

/// Подпись вложения. Пустая строка — подписывать нечего (например, превью ссылки).
pub fn media_label(media: &grammers_client::media::Media) -> String {
    use grammers_client::media::Media as M;
    match media {
        M::Photo(_) => tr!("[фото]", "[photo]").into(),
        M::Sticker(_) => tr!("[стикер]", "[sticker]").into(),
        M::Document(d) => match &d.raw.document {
            Some(tl::enums::Document::Document(doc)) => doc_label(&doc.attributes, &doc.mime_type, Some(doc.size.max(0) as usize)),
            _ => tr!("[файл]", "[file]").into(),
        },
        M::Contact(c) => {
            let name = crate::util::clean(&format!("{} {}", c.first_name(), c.last_name()));
            if name.trim().is_empty() {
                tr!("[контакт]", "[contact]").to_string()
            } else {
                trf!("[контакт {}]", "[contact {}]", name.trim())
            }
        }
        M::Poll(p) => {
            let tl::enums::TextWithEntities::Entities(q) = p.question();
            trf!("[опрос: {}]", "[poll: {}]", crate::util::clean(&q.text).trim())
        }
        M::Geo(_) | M::GeoLive(_) => tr!("[геопозиция]", "[location]").into(),
        M::Venue(v) => trf!("[место: {}]", "[place: {}]", crate::util::clean(v.title()).trim()),
        M::Dice(_) => tr!("[кубик]", "[dice]").into(),
        M::WebPage(_) => String::new(),
        _ => tr!("[вложение]", "[attachment]").into(),
    }
}

/// Склеивает подпись вложения и текст сообщения.
pub fn compose_text(label: &str, text: &str) -> String {
    match (label.is_empty(), text.is_empty()) {
        (true, true) => tr!("[служебное сообщение]", "[service message]").to_string(),
        (true, false) => text.to_string(),
        (false, true) => label.to_string(),
        (false, false) => format!("{label} {text}"),
    }
}

pub fn to_chat_msg(m: &Message, fallback_name: &str) -> ChatMsg {
    let text = crate::util::clean_or(m.text(), tr!("[эмодзи]", "[emoji]"));
    let label = m.media().map(|md| media_label(&md)).unwrap_or_default();
    let caption = if label.is_empty() { None } else { Some(text.clone()) };
    let text = compose_text(&label, &text);
    let fwd = match m.forward_header() {
        Some(tl::enums::MessageFwdHeader::Header(h)) => Some(Fwd {
            name: h.from_name.as_deref().or(h.post_author.as_deref()).map(|n| crate::util::clean(n).trim().to_string()),
            from: h.from_id.clone().map(PeerId::from),
        }),
        None => None,
    };
    let sender = if m.outgoing() {
        tr!("Я", "Me").to_string()
    } else {
        m.sender().and_then(|p| p.name().map(crate::util::clean)).unwrap_or_else(|| fallback_name.to_string())
    };
    ChatMsg {
        id: m.id(),
        ts: m.date().as_second(),
        outgoing: m.outgoing(),
        sender,
        text,
        caption,
        raw: m.text().to_string(),
        time: format_time(m.date()),
        edited: m.edit_date().is_some() && !m.edit_hide(),
        reply_to: m.reply_to_message_id(),
        fwd,
        mentioned: m.mentioned() && !m.outgoing(),
        views: m.view_count().filter(|v| *v > 0),
        reactions: reactions_of(m),
        buttons: buttons_of(m),
        poll: poll_of(m),
    }
}

/// Опрос сообщения: вопрос, варианты и текущие результаты.
pub fn poll_of(m: &Message) -> Option<PollView> {
    let Some(grammers_client::media::Media::Poll(p)) = m.media() else { return None };
    let tl::enums::TextWithEntities::Entities(q) = p.question();
    let results: Vec<&tl::types::PollAnswerVoters> = p.iter_voters_summary().map(|i| i.collect()).unwrap_or_default();
    let options = p
        .iter_answers()
        .filter_map(|a| {
            let tl::enums::PollAnswer::Answer(a) = a else { return None };
            let tl::enums::TextWithEntities::Entities(t) = &a.text;
            let r = results.iter().find(|r| r.option == a.option);
            Some(PollOpt {
                text: crate::util::clean(&t.text).trim().to_string(),
                data: a.option.clone(),
                voters: r.and_then(|r| r.voters).unwrap_or(0),
                chosen: r.is_some_and(|r| r.chosen),
                correct: r.is_some_and(|r| r.correct),
            })
        })
        .collect();
    Some(PollView {
        question: crate::util::clean(&q.text).trim().to_string(),
        quiz: p.is_quiz(),
        closed: p.closed(),
        multiple: p.raw.multiple_choice,
        total: p.total_voters().unwrap_or(0),
        options,
    })
}

/// Реакции сообщения: только обычные смайлики, остальные (свои эмодзи, платные) показываются как «?».
pub fn reactions_of(m: &Message) -> Vec<Reaction> {
    let tl::enums::Message::Message(x) = &m.raw else { return Vec::new() };
    let Some(tl::enums::MessageReactions::Reactions(r)) = &x.reactions else { return Vec::new() };
    r.results
        .iter()
        .map(|c| {
            let tl::enums::ReactionCount::Count(c) = c;
            let emoji = match &c.reaction {
                tl::enums::Reaction::Emoji(e) => e.emoticon.clone(),
                _ => String::new(),
            };
            Reaction { emoji, count: c.count, mine: c.chosen_order.is_some() }
        })
        .collect()
}

/// Inline-кнопки сообщения. Тексты очищаются от эмодзи, как и остальной текст.
pub fn buttons_of(m: &Message) -> Vec<Vec<Btn>> {
    let Some(tl::enums::ReplyMarkup::ReplyInlineMarkup(mk)) = m.reply_markup() else { return Vec::new() };
    mk.rows
        .iter()
        .map(|row| {
            let tl::enums::KeyboardInlineButtonRow::Row(row) = row;
            row.buttons
                .iter()
                .map(|b| {
                    let tl::enums::KeyboardInlineButton::Button(b) = b;
                    let text = crate::util::clean_or(&b.text, "•");
                    let action = match &b.r#type {
                        tl::enums::InlineButtonType::Callback(c) if !c.requires_password => BtnAction::Callback(c.data.clone()),
                        tl::enums::InlineButtonType::Url(u) => BtnAction::Url(u.url.clone()),
                        tl::enums::InlineButtonType::Copy(c) => BtnAction::Copy(c.copy_text.clone()),
                        _ => BtnAction::Other,
                    };
                    Btn { text, action }
                })
                .collect()
        })
        .collect()
}

fn dialog_unread(d: &grammers_client::peer::Dialog) -> i32 {
    match &d.raw {
        tl::enums::Dialog::Dialog(x) => x.unread_count,
        _ => 0,
    }
}

fn dialog_mentions(d: &grammers_client::peer::Dialog) -> i32 {
    match &d.raw {
        tl::enums::Dialog::Dialog(x) => x.unread_mentions_count,
        _ => 0,
    }
}

fn input_peer_id(p: &tl::enums::InputPeer) -> Option<PeerId> {
    use tl::enums::InputPeer as P;
    match p {
        P::User(u) => PeerId::user(u.user_id),
        P::Chat(c) => PeerId::chat(c.chat_id),
        P::Channel(c) => PeerId::channel(c.channel_id),
        _ => None,
    }
}

fn folder_from_raw(f: &tl::enums::DialogFilter) -> Option<Folder> {
    use tl::enums::DialogFilter as D;
    match f {
        D::Filter(f) => {
            let tl::enums::TextWithEntities::Entities(t) = &f.title;
            let mut include: Vec<PeerId> = f.pinned_peers.iter().chain(f.include_peers.iter()).filter_map(input_peer_id).collect();
            include.dedup();
            Some(Folder {
                id: f.id,
                title: crate::util::clean_or(&t.text, tr!("Папка", "Folder")).trim().to_string(),
                include,
                exclude: f.exclude_peers.iter().filter_map(input_peer_id).collect(),
                include_self: f.pinned_peers.iter().chain(f.include_peers.iter()).any(|p| matches!(p, tl::enums::InputPeer::PeerSelf)),
                contacts: f.contacts,
                non_contacts: f.non_contacts,
                groups: f.groups,
                broadcasts: f.broadcasts,
                bots: f.bots,
                exclude_read: f.exclude_read,
            })
        }
        D::Chatlist(f) => {
            let tl::enums::TextWithEntities::Entities(t) = &f.title;
            let mut include: Vec<PeerId> = f.pinned_peers.iter().chain(f.include_peers.iter()).filter_map(input_peer_id).collect();
            include.dedup();
            Some(Folder {
                id: f.id,
                title: crate::util::clean_or(&t.text, tr!("Папка", "Folder")).trim().to_string(),
                include,
                include_self: f.pinned_peers.iter().chain(f.include_peers.iter()).any(|p| matches!(p, tl::enums::InputPeer::PeerSelf)),
                ..Default::default()
            })
        }
        _ => None,
    }
}

// ------------------------------------------------------------ операции

fn chat_info(p: &Peer, id: PeerId, peer: PeerRef, unread: i32, mentions: i32, read_out: i32, draft: String) -> ChatInfo {
    let (name, presence, is_contact, is_bot, is_self) = match p {
        Peer::User(u) => {
            let is_self = u.is_self();
            let name = if is_self { tr!("Избранное", "Saved Messages").to_string() } else { crate::util::clean(&u.full_name()) };
            let presence = if is_self { None } else { Some(presence_from(u.status())) };
            (name, presence, u.contact(), u.is_bot(), is_self)
        }
        p => (crate::util::clean(p.name().unwrap_or(tr!("(без имени)", "(no name)"))), None, false, false, false),
    };
    ChatInfo {
        id,
        peer,
        name: if name.trim().is_empty() { tr!("(без имени)", "(no name)").to_string() } else { name.trim().to_string() },
        kind: kind_of(p),
        unread,
        mentions,
        presence,
        is_contact,
        is_bot,
        is_self,
        read_out,
        draft,
    }
}

fn dialog_info(d: &grammers_client::peer::Dialog) -> ChatInfo {
    chat_info(d.peer(), d.peer_id(), d.peer_ref(), dialog_unread(d), dialog_mentions(d), dialog_read_out(d), dialog_draft(d))
}

/// Текст черновика из диалога.
fn dialog_draft(d: &grammers_client::peer::Dialog) -> String {
    match &d.raw {
        tl::enums::Dialog::Dialog(x) => x.draft.as_ref().map(draft_text).unwrap_or_default(),
        _ => String::new(),
    }
}

fn draft_text(d: &tl::enums::DraftMessage) -> String {
    match d {
        tl::enums::DraftMessage::Message(m) => m.message.clone(),
        _ => String::new(),
    }
}

fn dialog_read_out(d: &grammers_client::peer::Dialog) -> i32 {
    match &d.raw {
        tl::enums::Dialog::Dialog(x) => x.read_outbox_max_id,
        _ => 0,
    }
}

/// Загружает чаты страницами (Telegram отдаёт по 100) до `limit`. Если передан `progress`, первая страница уходит
/// интерфейсу сразу, а дальше сообщается, сколько чатов уже загружено.
pub async fn load_dialogs(client: &Client, limit: usize, progress: Option<&mpsc::UnboundedSender<Event>>) -> Result<Vec<ChatInfo>> {
    let mut it = client.iter_dialogs();
    let mut out = Vec::new();
    while let Some(d) = it.next().await? {
        out.push(dialog_info(&d));
        if let Some(tx) = progress
            && out.len() % QUICK_DIALOGS == 0
        {
            if out.len() == QUICK_DIALOGS {
                let _ = tx.send(Event::DialogsPage(out.clone()));
            } else {
                let _ = tx.send(Event::Status(trf!("Загружено чатов: {}…", "Chats loaded: {}…", out.len())));
            }
        }
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

pub async fn load_folders(client: &Client) -> Result<Vec<Folder>> {
    let res = client.invoke(&tl::functions::messages::GetDialogFilters {}).await.map_err(|e| anyhow!("{e}"))?;
    let tl::enums::messages::DialogFilters::Filters(f) = res;
    Ok(f.filters.iter().filter_map(folder_from_raw).collect())
}

pub async fn load_history(client: &Client, chat: &ChatInfo, limit: usize) -> Result<Vec<ChatMsg>> {
    let mut it = client.iter_messages(chat.peer).limit(limit);
    let mut out = Vec::new();
    while let Some(m) = it.next().await? {
        out.push(to_chat_msg(&m, &chat.name));
    }
    out.reverse(); // от старых к новым
    Ok(out)
}

/// Сообщения старше `before` (не включая его), от старых к новым.
pub async fn load_older(client: &Client, chat: &ChatInfo, before: i32, limit: usize) -> Result<Vec<ChatMsg>> {
    let mut it = client.iter_messages(chat.peer).offset_id(before).limit(limit);
    let mut out = Vec::new();
    while let Some(m) = it.next().await? {
        out.push(to_chat_msg(&m, &chat.name));
    }
    out.reverse();
    Ok(out)
}

/// Сообщение `id` и его окружение: сначала по дате (чуть позже найденного), а если оно не попало, то по номеру.
pub async fn load_around(client: &Client, chat: &ChatInfo, id: i32, ts: i64) -> Result<Vec<ChatMsg>> {
    let after = (ts + 2 * 3600).clamp(0, i64::from(i32::MAX)) as i32;
    let mut it = client.iter_messages(chat.peer).offset_date(after).limit(60);
    let mut out = Vec::new();
    while let Some(m) = it.next().await? {
        out.push(to_chat_msg(&m, &chat.name));
    }
    if !out.iter().any(|m| m.id == id) {
        out.clear();
        let mut it = client.iter_messages(chat.peer).offset_id(id.saturating_add(1)).limit(60);
        while let Some(m) = it.next().await? {
            out.push(to_chat_msg(&m, &chat.name));
        }
    }
    out.reverse();
    Ok(out)
}

fn hit_from(m: &Message, chat_name: &str) -> SearchHit {
    let c = to_chat_msg(m, chat_name);
    SearchHit { chat: m.peer_id(), chat_name: chat_name.to_string(), id: c.id, ts: c.ts, when: c.time, sender: c.sender, text: c.text }
}

/// Поиск текста в одном чате (сервер ищет нечётко: учитывает формы слов).
pub async fn search_chat(client: &Client, chat: &ChatInfo, query: &str) -> Result<Vec<SearchHit>> {
    let mut it = client.search_messages(chat.peer).query(query);
    let mut out = Vec::new();
    while let Some(m) = it.next().await? {
        out.push(hit_from(&m, &chat.name));
        if out.len() >= MAX_HITS {
            break;
        }
    }
    Ok(out)
}

/// Поиск текста во всех чатах.
pub async fn search_all(client: &Client, query: &str) -> Result<Vec<SearchHit>> {
    let mut it = client.search_all_messages().query(query);
    let mut out = Vec::new();
    while let Some(m) = it.next().await? {
        let name = m.peer().and_then(|p| p.name().map(crate::util::clean)).unwrap_or_default();
        out.push(hit_from(&m, &name));
        if out.len() >= MAX_HITS {
            break;
        }
    }
    Ok(out)
}

pub async fn send(
    client: &Client,
    chat: &ChatInfo,
    text: &str,
    markdown: bool,
    delay_minutes: Option<u32>,
    reply_to: Option<i32>,
) -> Result<ChatMsg> {
    let mut msg = if markdown { InputMessage::new().markdown(text) } else { InputMessage::new().text(text) };
    if let Some(id) = reply_to {
        msg = msg.reply_to(Some(id));
    }
    if let Some(min) = delay_minutes {
        let when = SystemTime::now() + Duration::from_secs(u64::from(min) * 60);
        msg = msg.schedule_date(Some(when));
    }
    let sent = client.send_message(chat.peer, msg).await.map_err(|e| anyhow!("{e}"))?;
    Ok(to_chat_msg(&sent, &chat.name))
}

fn ext_from_mime(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "video/mp4" => "mp4",
        "audio/ogg" => "ogg",
        "audio/mpeg" => "mp3",
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        _ => "",
    }
}

/// Папка для скачанных файлов: `downloads` рядом с программой.
pub fn download_dir() -> PathBuf {
    crate::util::data_dir().join("downloads")
}

/// Скачивает вложение сообщения. Прогресс сообщается в строку состояния.
pub async fn save_attachment(
    client: &Client,
    chat: &ChatInfo,
    id: i32,
    events: &mpsc::UnboundedSender<Event>,
    for_view: bool,
) -> Result<PathBuf> {
    use tokio::io::AsyncWriteExt;
    let msgs = client.get_messages_by_id(chat.peer, &[id]).await.map_err(|e| anyhow!("{e}"))?;
    let msg = msgs.into_iter().next().flatten().ok_or_else(|| anyhow!(tr!("сообщение не найдено", "message not found")))?;
    let media = msg.media().ok_or_else(|| anyhow!(tr!("в сообщении нет вложения", "the message has no attachment")))?;
    let name = match &media {
        Media::Photo(p) => format!("photo_{}.jpg", p.id()),
        Media::Sticker(_) => format!("sticker_{id}.webp"),
        Media::Document(d) => {
            let base = d.name().map(str::to_string).filter(|n| !n.trim().is_empty());
            match base {
                // Имя без расширения, но с известным типом: добавляем расширение, чтобы файл можно было открыть.
                Some(n) if !n.contains('.') && !ext_from_mime(d.mime_type().unwrap_or("")).is_empty() => {
                    format!("{n}.{}", ext_from_mime(d.mime_type().unwrap_or("")))
                }
                Some(n) => n,
                None => {
                    let ext = ext_from_mime(d.mime_type().unwrap_or(""));
                    if ext.is_empty() { format!("file_{id}") } else { format!("file_{id}.{ext}") }
                }
            }
        }
        _ => return Err(anyhow!(tr!("это вложение нельзя скачать", "this attachment cannot be downloaded"))),
    };
    if for_view {
        if !crate::util::is_openable(&crate::util::safe_file_name(&name)) {
            return Err(anyhow!(tr!(
                "такой тип файла не открывается из чата (только картинки, PDF, текст, звук, видео): сохраните его через /save",
                "this file type is not opened from the chat (only images, PDF, text, audio, video): save it with /save"
            )));
        }
        if Downloadable::size(&media).is_some_and(|n| n as u64 > crate::util::MAX_OPEN_BYTES) {
            return Err(anyhow!(tr!("файл больше 50 МБ: сохраните его через /save", "the file is larger than 50 MB: save it with /save")));
        }
    }
    let dir = if for_view { crate::util::view_dir() } else { download_dir() };
    std::fs::create_dir_all(&dir)?;
    let path = crate::util::reserve_unique(&dir, &crate::util::safe_file_name(&name))?;
    // Имя `path` уже занято пустым файлом, поэтому и временное имя уникально.
    let part = PathBuf::from(format!("{}.part", path.display()));
    let total = Downloadable::size(&media);
    let mut file = match tokio::fs::File::create(&part).await {
        Ok(f) => f,
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            return Err(e.into());
        }
    };
    let mut it = client.iter_download(&media);
    let (mut done, mut last_pct) = (0usize, 101u64);
    let short = crate::util::safe_file_name(&name);
    let result: Result<()> = async {
        while let Some(chunk) = it.next().await.map_err(|e| anyhow!("{e}"))? {
            file.write_all(&chunk).await?;
            done += chunk.len();
            if for_view && done as u64 > crate::util::MAX_OPEN_BYTES {
                return Err(anyhow!(tr!(
                    "файл больше 50 МБ: сохраните его через /save",
                    "the file is larger than 50 MB: save it with /save"
                )));
            }
            let pct = total.filter(|t| *t > 0).map(|t| (done as u64 * 100 / t as u64).min(100));
            if let Some(p) = pct
                && p / 5 != last_pct / 5
            {
                last_pct = p;
                let _ = events.send(Event::Status(trf!("Скачиваю {short}: {p}%", "Downloading {short}: {p}%")));
            }
        }
        file.flush().await?;
        Ok(())
    }
    .await;
    drop(file);
    match result {
        Ok(()) => {
            if let Err(e) = std::fs::rename(&part, &path) {
                let _ = std::fs::remove_file(&part);
                let _ = std::fs::remove_file(&path);
                return Err(e.into());
            }
            Ok(path)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            let _ = std::fs::remove_file(&path); // и пустой «заказ» имени
            Err(e)
        }
    }
}

/// Предел размера файла для обычного аккаунта.
pub const MAX_UPLOAD: u64 = 2000 * 1024 * 1024;

pub async fn send_file(client: &Client, chat: &ChatInfo, path: &Path, caption: &str) -> Result<ChatMsg> {
    let meta = check_upload(path)?;
    let up = client.upload_file(path).await.map_err(|e| anyhow!("{e}"))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let msg = InputMessage::new().text(caption);
    // Обычные картинки уходят как фото, всё остальное как файл без пережатия.
    let msg = if matches!(ext.as_str(), "jpg" | "jpeg" | "png") && meta.len() < 10 * 1024 * 1024 { msg.photo(up) } else { msg.file(up) };
    let sent = client.send_message(chat.peer, msg).await.map_err(|e| anyhow!("{e}"))?;
    Ok(to_chat_msg(&sent, &chat.name))
}

/// Проверяет файл перед отправкой: существует, не пустой, не больше 2 ГБ.
fn check_upload(path: &Path) -> Result<std::fs::Metadata> {
    let meta = std::fs::metadata(path).map_err(|e| anyhow!(trf!("не удалось открыть файл: {e}", "could not open file: {e}")))?;
    if !meta.is_file() {
        return Err(anyhow!(tr!("это не файл", "this is not a file")));
    }
    if meta.len() == 0 {
        return Err(anyhow!(tr!("файл пустой", "the file is empty")));
    }
    if meta.len() > MAX_UPLOAD {
        return Err(anyhow!(tr!("файл больше 2 ГБ, Telegram его не примет", "file is larger than 2 GB, Telegram will not accept it")));
    }
    Ok(meta)
}

/// Отправляет несколько файлов одним альбомом: картинки как фото, остальное как файлы.
/// Подпись ставится на первый элемент.
pub async fn send_album(client: &Client, chat: &ChatInfo, paths: &[PathBuf], caption: &str) -> Result<Vec<ChatMsg>> {
    use grammers_client::media::InputMedia;
    if !(2..=10).contains(&paths.len()) {
        return Err(anyhow!(tr!("в альбоме должно быть от 2 до 10 файлов", "an album needs 2 to 10 files")));
    }
    let mut medias = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        let meta = check_upload(path)?;
        let up = client.upload_file(path).await.map_err(|e| anyhow!("{e}"))?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let media = InputMedia::new().caption(if i == 0 { caption } else { "" });
        medias.push(if matches!(ext.as_str(), "jpg" | "jpeg" | "png") && meta.len() < 10 * 1024 * 1024 {
            media.photo(up)
        } else {
            media.document(up)
        });
    }
    let sent = client.send_album(chat.peer, medias).await.map_err(|e| anyhow!("{e}"))?;
    Ok(sent.into_iter().flatten().map(|m| to_chat_msg(&m, &chat.name)).collect())
}

pub async fn list_scheduled(client: &Client, chat: &ChatInfo) -> Result<Vec<ScheduledMsg>> {
    use tl::enums::messages::Messages as M;
    let res = client
        .invoke(&tl::functions::messages::GetScheduledHistory { peer: tl::enums::InputPeer::from(chat.peer), hash: 0 })
        .await
        .map_err(|e| anyhow!("{e}"))?;
    let msgs = match res {
        M::Messages(m) => m.messages,
        M::Slice(m) => m.messages,
        M::ChannelMessages(m) => m.messages,
        M::NotModified(_) => Vec::new(),
    };
    let mut out: Vec<ScheduledMsg> = msgs
        .into_iter()
        .filter_map(|m| match m {
            tl::enums::Message::Message(x) => {
                let text = crate::util::clean(&x.message);
                let text = if x.media.is_some() { trf!("[вложение] {text}", "[attachment] {text}") } else { text };
                Some(ScheduledMsg { id: x.id, date: i64::from(x.date), text: text.trim().to_string() })
            }
            _ => None,
        })
        .collect();
    out.sort_by_key(|m| m.date);
    Ok(out)
}

/// Закреплённое сообщение чата (его текст), если есть.
pub async fn load_pinned(client: &Client, chat: &ChatInfo) -> Result<Option<String>> {
    use tl::enums::messages::Messages as M;
    let res = client
        .invoke(&tl::functions::messages::Search {
            peer: tl::enums::InputPeer::from(chat.peer),
            q: String::new(),
            from_id: None,
            saved_peer_id: None,
            saved_reaction: None,
            top_msg_id: None,
            filter: tl::enums::MessagesFilter::InputMessagesFilterPinned,
            min_date: 0,
            max_date: 0,
            offset_id: 0,
            add_offset: 0,
            limit: 1,
            max_id: 0,
            min_id: 0,
            hash: 0,
        })
        .await
        .map_err(|e| anyhow!("{e}"))?;
    let msgs = match res {
        M::Messages(m) => m.messages,
        M::Slice(m) => m.messages,
        M::ChannelMessages(m) => m.messages,
        M::NotModified(_) => Vec::new(),
    };
    Ok(msgs.into_iter().find_map(|m| match m {
        tl::enums::Message::Message(x) => {
            let t = crate::util::clean(&x.message).replace('\n', " ");
            Some(if t.trim().is_empty() { tr!("[вложение]", "[attachment]").to_string() } else { t.trim().to_string() })
        }
        _ => None,
    }))
}

// ------------------------------------------------------------ ссылки, @имена, закреплённые

/// Куда ведёт введённая пользователем ссылка или @имя.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Публичное имя пользователя, группы или канала (без @).
    Username(String),
    /// Хэш ссылки-приглашения (t.me/+хэш).
    Invite(String),
}

fn valid_username(s: &str) -> bool {
    (4..=32).contains(&s.len())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
}

fn valid_invite(s: &str) -> bool {
    (6..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Служебные пути t.me, которые не являются именами.
const RESERVED_PATHS: [&str; 12] =
    ["joinchat", "addstickers", "addemoji", "c", "proxy", "socks", "share", "iv", "bg", "login", "setlanguage", "addlist"];

/// Разбирает `@имя`, `имя`, `t.me/имя`, `t.me/+хэш`, `t.me/joinchat/хэш`, `tg://resolve?domain=имя`, `tg://join?invite=хэш`.
pub fn parse_target(input: &str) -> Option<Target> {
    let s = input.trim();
    if s.is_empty() || s.chars().any(char::is_whitespace) {
        return None;
    }
    if let Some(name) = s.strip_prefix('@') {
        return valid_username(name).then(|| Target::Username(name.to_string()));
    }
    if let Some(rest) = s.strip_prefix("tg://") {
        let (path, query) = rest.split_once('?')?;
        let get = |key: &str| query.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='));
        return match path {
            "resolve" => get("domain").filter(|d| valid_username(d)).map(|d| Target::Username(d.to_string())),
            "join" => get("invite").filter(|h| valid_invite(h)).map(|h| Target::Invite(h.to_string())),
            _ => None,
        };
    }
    let lower = s.to_ascii_lowercase();
    let no_scheme = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://")).unwrap_or(&lower);
    let no_www = no_scheme.strip_prefix("www.").unwrap_or(no_scheme);
    let host_len = ["t.me/", "telegram.me/", "telegram.dog/"].iter().find(|h| no_www.starts_with(**h)).map(|h| h.len());
    let Some(host_len) = host_len else {
        // голое имя без @ допускается: `/join durov`
        return valid_username(s).then(|| Target::Username(s.to_string()));
    };
    // регистр в хэшах приглашений важен, поэтому режем оригинальную строку по тем же смещениям
    let offset = s.len() - no_www.len() + host_len;
    let path = s.get(offset..)?.split(['?', '#']).next()?;
    let mut parts = path.split('/').filter(|p| !p.is_empty());
    let first = parts.next()?;
    if let Some(h) = first.strip_prefix('+') {
        return valid_invite(h).then(|| Target::Invite(h.to_string()));
    }
    let first_lower = first.to_ascii_lowercase();
    if first_lower == "joinchat" {
        let h = parts.next()?;
        return valid_invite(h).then(|| Target::Invite(h.to_string()));
    }
    let name = if first_lower == "s" { parts.next()? } else { first };
    if RESERVED_PATHS.contains(&name.to_ascii_lowercase().as_str()) {
        return None;
    }
    valid_username(name).then(|| Target::Username(name.to_string()))
}

/// Понятное объяснение ошибки Telegram при поиске, вступлении и выходе.
pub fn rpc_hint(e: &InvocationError) -> String {
    let known: [(&str, &str, &str); 9] = [
        ("USER_ALREADY_PARTICIPANT", "вы уже состоите в этом чате", "you are already a member of this chat"),
        ("INVITE_HASH_EXPIRED", "ссылка-приглашение устарела или отозвана", "the invite link has expired or was revoked"),
        ("INVITE_HASH_INVALID", "ссылка-приглашение неверна", "the invite link is not valid"),
        (
            "INVITE_REQUEST_SENT",
            "заявка на вступление отправлена, ждите одобрения администратора",
            "join request sent, wait for an admin to approve it",
        ),
        ("CHANNELS_TOO_MUCH", "вы состоите в максимальном числе групп и каналов", "you have reached the limit of groups and channels"),
        ("USERNAME_INVALID", "неверное имя", "invalid username"),
        ("USERNAME_NOT_OCCUPIED", "такого имени нет", "no such username"),
        ("CHANNEL_PRIVATE", "чат закрытый: нужна ссылка-приглашение", "the chat is private: an invite link is needed"),
        ("USER_BANNED_IN_CHANNEL", "вам запрещено вступать в этот чат", "you are banned from this chat"),
    ];
    for (name, ru, en) in known {
        if e.is(name) {
            return tr!(ru, en).to_string();
        }
    }
    e.to_string()
}

/// Находит чат по публичному имени.
pub async fn resolve_chat(client: &Client, name: &str) -> Result<Option<ChatInfo>> {
    let Some(peer) = client.resolve_username(name).await.map_err(|e| anyhow!("{}", rpc_hint(&e)))? else {
        return Ok(None);
    };
    let Some(r) = peer.to_ref().await.map_err(|e| anyhow!("{e}"))? else {
        return Ok(None);
    };
    Ok(Some(chat_info(&peer, peer.id(), r, 0, 0, 0, String::new())))
}

/// Вступает в чат; возвращает чат, если он известен сразу (по публичному имени).
pub async fn join_target(client: &Client, target: &Target) -> Result<Option<ChatInfo>> {
    match target {
        Target::Invite(hash) => match client.invoke(&tl::functions::messages::ImportChatInvite { hash: hash.clone() }).await {
            Ok(_) => Ok(None),
            Err(e) => Err(anyhow!("{}", rpc_hint(&e))),
        },
        Target::Username(name) => {
            let Some(info) = resolve_chat(client, name).await? else {
                return Err(anyhow!("{}", tr!("такого имени нет", "no such username")));
            };
            if info.kind == Kind::User {
                return Ok(Some(info));
            }
            match client.join_chat(info.peer).await {
                Ok(_) => Ok(Some(info)),
                Err(e) if e.is("USER_ALREADY_PARTICIPANT") => Ok(Some(info)),
                Err(e) => Err(anyhow!("{}", rpc_hint(&e))),
            }
        }
    }
}

/// Все закреплённые сообщения чата, новые сверху.
pub async fn list_pinned(client: &Client, chat: &ChatInfo) -> Result<Vec<SearchHit>> {
    let mut it = client.search_messages(chat.peer).filter(tl::enums::MessagesFilter::InputMessagesFilterPinned);
    let mut out = Vec::new();
    while let Some(m) = it.next().await? {
        out.push(hit_from(&m, &chat.name));
        if out.len() >= MAX_HITS {
            break;
        }
    }
    Ok(out)
}

/// Сохраняет черновик в Telegram (пустой текст стирает его); он появится на других устройствах.
pub async fn save_draft(client: &Client, chat: &ChatInfo, text: &str) -> Result<()> {
    client
        .invoke(&tl::functions::messages::SaveDraft {
            no_webpage: false,
            invert_media: false,
            reply_to: None,
            peer: chat.peer.into(),
            message: text.to_string(),
            entities: None,
            media: None,
            effect: None,
            suggested_post: None,
            rich_message: None,
        })
        .await?;
    Ok(())
}

/// Ставит реакцию (или снимает, если `emoji` = None) и возвращает обновлённое сообщение.
pub async fn react(client: &Client, chat: &ChatInfo, id: i32, emoji: Option<&str>) -> Result<ChatMsg> {
    use grammers_client::message::InputReactions;
    let msg = fetch_message(client, chat, id).await?;
    let reactions = match emoji {
        Some(e) => InputReactions::emoticon(e),
        None => InputReactions::remove(),
    };
    msg.react(reactions).await.map_err(|e| anyhow!("{e}"))?;
    Ok(to_chat_msg(&fetch_message(client, chat, id).await?, &chat.name))
}

/// Скачивает небольшой текстовый файл из сообщения в память (не на диск) и возвращает имя и строки.
pub async fn fetch_text_file(client: &Client, chat: &ChatInfo, id: i32) -> Result<(String, Vec<String>)> {
    let msg = fetch_message(client, chat, id).await?;
    let Some(Media::Document(d)) = msg.media() else {
        return Err(anyhow!(tr!("в сообщении нет файла", "this message has no file")));
    };
    let name = d.name().map(str::to_string).unwrap_or_default();
    if !crate::util::is_text_file(&name) {
        return Err(anyhow!(tr!("это не текстовый файл", "this is not a text file")));
    }
    let media = Media::Document(d);
    if Downloadable::size(&media).is_some_and(|n| n > crate::util::MAX_TEXT_BYTES) {
        return Err(anyhow!(tr!(
            "файл больше 1 МБ: откройте его через O или сохраните через S",
            "the file is larger than 1 MB: open it with O or save it with S"
        )));
    }
    let mut it = client.iter_download(&media);
    let mut out = Vec::new();
    while let Some(chunk) = it.next().await.map_err(|e| anyhow!("{e}"))? {
        out.extend_from_slice(&chunk);
        if out.len() > crate::util::MAX_TEXT_BYTES {
            return Err(anyhow!(tr!("файл больше 1 МБ", "the file is larger than 1 MB")));
        }
    }
    Ok((crate::util::clean(&name), crate::util::text_for_view(&out)))
}

/// Скачивает самую крупную копию фото до 800 точек (она уже сжата Telegram, это быстро и недорого).
pub async fn fetch_preview(client: &Client, chat: &ChatInfo, id: i32) -> Result<Vec<u8>> {
    use grammers_client::media::{Media, PhotoSize};
    let msg = fetch_message(client, chat, id).await?;
    let thumbs = match msg.media() {
        Some(Media::Photo(p)) => p.thumbs(),
        Some(Media::Document(d)) => d.thumbs(),
        _ => return Err(anyhow!(tr!("в сообщении нет фото", "this message has no photo"))),
    };
    let best = thumbs
        .into_iter()
        .filter_map(|t| match &t {
            PhotoSize::Size(s) if s.width > 0 && s.width <= 800 && s.height <= 800 => Some((s.width * s.height, t)),
            _ => None,
        })
        .max_by_key(|(area, _)| *area)
        .map(|(_, t)| t)
        .ok_or_else(|| anyhow!(tr!("у этого вложения нет превью", "this attachment has no preview")))?;
    let mut it = client.iter_download(&best);
    let mut out = Vec::new();
    while let Some(chunk) = it.next().await.map_err(|e| anyhow!("{e}"))? {
        out.extend_from_slice(&chunk);
        if out.len() > 4 * 1024 * 1024 {
            return Err(anyhow!(tr!("превью слишком большое", "the preview is too large")));
        }
    }
    Ok(out)
}

/// Темы форума: от новых к старым, не больше 100.
pub async fn list_topics(client: &Client, chat: &ChatInfo) -> Result<Vec<(i32, String, i32)>> {
    let res = client
        .invoke(&tl::functions::messages::GetForumTopics {
            peer: chat.peer.into(),
            q: None,
            offset_date: 0,
            offset_id: 0,
            offset_topic: 0,
            limit: 100,
        })
        .await
        .map_err(|e| anyhow!("{e}"))?;
    let tl::enums::messages::ForumTopics::Topics(t) = res;
    Ok(t.topics
        .into_iter()
        .filter_map(|x| match x {
            tl::enums::ForumTopic::Topic(t) => Some((t.id, crate::util::clean(&t.title).trim().to_string(), t.unread_count)),
            _ => None,
        })
        .collect())
}

/// Голосует в опросе и возвращает обновлённое сообщение с результатами.
pub async fn vote(client: &Client, chat: &ChatInfo, id: i32, option: Vec<u8>) -> Result<ChatMsg> {
    client
        .invoke(&tl::functions::messages::SendVote { peer: chat.peer.into(), msg_id: id, options: vec![option] })
        .await
        .map_err(|e| anyhow!("{e}"))?;
    Ok(to_chat_msg(&fetch_message(client, chat, id).await?, &chat.name))
}

async fn fetch_message(client: &Client, chat: &ChatInfo, id: i32) -> Result<Message> {
    let msgs = client.get_messages_by_id(chat.peer, &[id]).await.map_err(|e| anyhow!("{e}"))?;
    msgs.into_iter().next().flatten().ok_or_else(|| anyhow!(tr!("сообщение не найдено", "message not found")))
}

/// Нажимает inline-кнопку бота; возвращает ответ бота (всплывающий текст), если он есть.
pub async fn press_button(client: &Client, chat: &ChatInfo, id: i32, data: Vec<u8>) -> Result<String> {
    let ans = client
        .invoke(&tl::functions::messages::GetBotCallbackAnswer {
            game: false,
            peer: chat.peer.into(),
            msg_id: id,
            data: Some(data),
            password: None,
        })
        .await
        .map_err(|e| anyhow!("{e}"))?;
    let tl::enums::messages::BotCallbackAnswer::Answer(a) = ans;
    Ok(a.message.as_deref().map(|t| crate::util::clean(t).trim().to_string()).unwrap_or_default())
}

/// Разбирает «сырое» обновление о черновике (набран на другом устройстве); темы форумов не учитываем.
pub fn draft_from_raw(u: &tl::enums::Update) -> Option<(PeerId, String)> {
    match u {
        tl::enums::Update::DraftMessage(x) if x.top_msg_id.is_none() && x.saved_peer_id.is_none() => {
            Some((PeerId::from(x.peer.clone()), draft_text(&x.draft)))
        }
        _ => None,
    }
}

/// Разбирает «сырое» обновление о прочтении ваших сообщений собеседником в личном чате.
pub fn read_outbox_from_raw(u: &tl::enums::Update) -> Option<(PeerId, i32)> {
    match u {
        tl::enums::Update::ReadHistoryOutbox(x) => match &x.peer {
            tl::enums::Peer::User(p) => Some((PeerId::user(p.user_id)?, x.max_id)),
            _ => None,
        },
        _ => None,
    }
}

// ------------------------------------------------------------ фоновые задачи

/// Запускает обработку команд и входящих обновлений. Возвращает канал команд.
pub fn spawn_worker(conn: Connection, events: mpsc::UnboundedSender<Event>) -> mpsc::UnboundedSender<Cmd> {
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Cmd>();
    let client = conn.client.clone();

    // 1) входящие обновления
    {
        let client = client.clone();
        let events = events.clone();
        let updates = conn.updates;
        tokio::spawn(async move {
            let mut stream = match client.stream_updates(updates, UpdatesConfiguration { catch_up: false }).await {
                Ok(s) => s,
                Err(e) => {
                    log(&format!("stream_updates: {e}"));
                    let _ = events.send(Event::Error(trf!("не удалось получать обновления: {e}", "failed to receive updates: {e}")));
                    return;
                }
            };
            let mut was_down = false;
            loop {
                let update = stream.next().await;
                if was_down && update.is_ok() {
                    was_down = false;
                    let _ = events.send(Event::Status(tr!("связь восстановлена", "connection restored").into()));
                }
                match update {
                    Ok(Update::NewMessage(m)) => {
                        crate::util::debug("update: new message");
                        let name = crate::util::clean(m.peer().and_then(|p| p.name()).unwrap_or("?")).trim().to_string();
                        let msg = to_chat_msg(&m, &name);
                        let _ = events.send(Event::Incoming { chat: m.peer_id(), chat_name: name, msg });
                    }
                    Ok(Update::MessageEdited(m)) => {
                        let name = crate::util::clean(m.peer().and_then(|p| p.name()).unwrap_or("?")).trim().to_string();
                        let msg = to_chat_msg(&m, &name);
                        let _ = events.send(Event::Edited { chat: m.peer_id(), msg });
                    }
                    Ok(Update::MessageDeleted(d)) => {
                        let chat = d.channel_id().map(PeerId::channel_unchecked);
                        let _ = events.send(Event::Deleted { chat, ids: d.messages().to_vec() });
                    }
                    Ok(Update::Raw(raw)) => {
                        if let tl::enums::Update::UserStatus(u) = &raw.raw {
                            if let Some(user) = PeerId::user(u.user_id) {
                                let _ = events.send(Event::Presence { user, presence: presence_from(&u.status) });
                            }
                        } else if let Some((chat, text)) = draft_from_raw(&raw.raw) {
                            let _ = events.send(Event::Draft { chat, text });
                        } else if let Some((chat, max_id)) = read_outbox_from_raw(&raw.raw) {
                            let _ = events.send(Event::ReadOutbox { chat, max_id });
                        } else if let Some((chat, from, kind)) = typing_from_raw(&raw.raw) {
                            let _ = events.send(Event::Typing { chat, from, kind });
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        log(&format!("updates: {e}"));
                        was_down = true;
                        let _ = events
                            .send(Event::Status(trf!("связь потеряна: {e}; переподключаюсь…", "connection lost: {e}; reconnecting…")));
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });
    }

    // 1б) задержка до Telegram для строки статуса
    tokio::spawn(ping_loop(client.clone(), events.clone()));

    // 2) команды интерфейса. Отправка идёт строго по порядку, остальное параллельно.
    tokio::spawn(async move {
        let mut queue: std::collections::VecDeque<Queued> = std::collections::VecDeque::new();
        loop {
            // Пока есть очередь, раз в 5 секунд пробуем её отправить.
            let next = if queue.is_empty() {
                cmd_rx.recv().await
            } else {
                match tokio::time::timeout(Duration::from_secs(5), cmd_rx.recv()).await {
                    Ok(c) => c,
                    Err(_) => {
                        flush_queue(&client, &events, &mut queue).await;
                        continue;
                    }
                }
            };
            let Some(cmd) = next else { break };
            match cmd {
                Cmd::LoadDialogs | Cmd::LoadAllDialogs => {
                    let all = matches!(cmd, Cmd::LoadAllDialogs);
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        let loaded = if all {
                            load_dialogs(&client, MAX_DIALOGS, Some(&events)).await
                        } else {
                            load_dialogs(&client, QUICK_DIALOGS, None).await
                        };
                        match loaded {
                            Ok(d) => {
                                let _ = events.send(if all { Event::Dialogs(d) } else { Event::DialogsPage(d) });
                            }
                            Err(e) => {
                                log(&format!("dialogs: {e}"));
                                let _ = events.send(Event::Error(trf!("список чатов: {e}", "chat list: {e}")));
                                return;
                            }
                        }
                        match load_folders(&client).await {
                            Ok(f) => {
                                let _ = events.send(Event::Folders(f));
                            }
                            // Папки не критичны: без них остаются вкладки «Все / Личные / Группы / Каналы».
                            Err(e) => log(&format!("folders: {e}")),
                        }
                    });
                }
                Cmd::LoadOlder { chat, before, limit } => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match load_older(&client, &chat, before, limit).await {
                            Ok(msgs) => {
                                let _ = events.send(Event::Older { chat: chat.id, msgs });
                            }
                            Err(e) => {
                                log(&format!("older: {e}"));
                                let _ = events.send(Event::Error(trf!("история: {e}", "history: {e}")));
                            }
                        }
                    });
                }
                Cmd::LoadAround { chat, id, ts } => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match load_around(&client, &chat, id, ts).await {
                            Ok(msgs) => {
                                let _ = events.send(Event::Around { chat: chat.id, msgs, focus: id });
                            }
                            Err(e) => {
                                log(&format!("around: {e}"));
                                let _ = events.send(Event::Error(trf!("история: {e}", "history: {e}")));
                            }
                        }
                    });
                }
                Cmd::SearchChat { chat, query } => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match search_chat(&client, &chat, &query).await {
                            Ok(hits) => {
                                let _ = events.send(Event::SearchResults { query, scope: Some(chat.id), hits });
                            }
                            Err(e) => {
                                log(&format!("search: {e}"));
                                let _ = events.send(Event::Error(trf!("поиск: {e}", "search: {e}")));
                            }
                        }
                    });
                }
                Cmd::SearchAll { query } => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match search_all(&client, &query).await {
                            Ok(hits) => {
                                let _ = events.send(Event::SearchResults { query, scope: None, hits });
                            }
                            Err(e) => {
                                log(&format!("search: {e}"));
                                let _ = events.send(Event::Error(trf!("поиск: {e}", "search: {e}")));
                            }
                        }
                    });
                }
                Cmd::LoadHistory(chat, limit) => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match load_history(&client, &chat, limit).await {
                            Ok(msgs) => {
                                let _ = events.send(Event::History { chat: chat.id, msgs });
                                // Закреплённое сообщение не критично: ошибка только в журнал.
                                match load_pinned(&client, &chat).await {
                                    Ok(text) => {
                                        let _ = events.send(Event::Pinned { chat: chat.id, text });
                                    }
                                    Err(e) => crate::util::debug(&format!("pinned: {e}")),
                                }
                            }
                            Err(e) => {
                                log(&format!("history: {e}"));
                                let _ = events.send(Event::Error(trf!("история: {e}", "history: {e}")));
                            }
                        }
                    });
                }
                Cmd::Save { chat, id } => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match save_attachment(&client, &chat, id, &events, false).await {
                            Ok(p) => {
                                let _ = events.send(Event::Status(trf!("Сохранено: {}", "Saved: {}", p.display())));
                            }
                            Err(e) => {
                                log(&format!("save: {e}"));
                                let _ =
                                    events.send(Event::Error(trf!("не скачано: {}", "not downloaded: {}", friendly_error(&e.to_string()))));
                            }
                        }
                    });
                }
                Cmd::Open { chat, id } => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match save_attachment(&client, &chat, id, &events, true).await {
                            Ok(p) => {
                                let _ = events.send(Event::Open(p));
                            }
                            Err(e) => {
                                log(&format!("open: {e}"));
                                let _ = events.send(Event::Error(trf!("не открыто: {}", "not opened: {}", friendly_error(&e.to_string()))));
                            }
                        }
                    });
                }
                Cmd::SendFile { chat, path, caption } => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        let _ = events.send(Event::Status(tr!("Загружаю файл…", "Uploading file…").into()));
                        match send_file(&client, &chat, &path, &caption).await {
                            Ok(msg) => {
                                let _ = events.send(Event::Sent { chat: chat.id, msg, scheduled: false });
                            }
                            Err(e) => {
                                log(&format!("send_file: {e}"));
                                let _ =
                                    events.send(Event::Error(trf!("не отправлено: {}", "not sent: {}", friendly_error(&e.to_string()))));
                            }
                        }
                    });
                }
                Cmd::ViewText { chat, id } => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match fetch_text_file(&client, &chat, id).await {
                            Ok((name, lines)) => {
                                let _ = events.send(Event::TextFile { chat: chat.id, name, lines });
                            }
                            Err(e) => {
                                log(&format!("view text: {e}"));
                                let _ = events.send(Event::Error(trf!("не прочитано: {}", "not read: {}", friendly_error(&e.to_string()))));
                            }
                        }
                    });
                }
                Cmd::SendAlbum { chat, paths, caption } => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        let _ = events.send(Event::Status(tr!("Загружаю альбом…", "Uploading the album…").into()));
                        match send_album(&client, &chat, &paths, &caption).await {
                            Ok(msgs) => {
                                for msg in msgs {
                                    let _ = events.send(Event::Sent { chat: chat.id, msg, scheduled: false });
                                }
                            }
                            Err(e) => {
                                log(&format!("send_album: {e}"));
                                let _ =
                                    events.send(Event::Error(trf!("не отправлено: {}", "not sent: {}", friendly_error(&e.to_string()))));
                            }
                        }
                    });
                }
                Cmd::ListScheduled(chat) => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match list_scheduled(&client, &chat).await {
                            Ok(items) => {
                                let _ = events.send(Event::Scheduled { chat: chat.id, items });
                            }
                            Err(e) => {
                                log(&format!("scheduled: {e}"));
                                let _ = events.send(Event::Error(trf!("отложенные: {}", "scheduled: {}", friendly_error(&e.to_string()))));
                            }
                        }
                    });
                }
                Cmd::DeleteScheduled { chat, id } => {
                    let r = client
                        .invoke(&tl::functions::messages::DeleteScheduledMessages {
                            peer: tl::enums::InputPeer::from(chat.peer),
                            id: vec![id],
                        })
                        .await;
                    match r {
                        Ok(_) => {
                            let _ = events.send(Event::Status(tr!("Отложенное сообщение отменено", "Scheduled message cancelled").into()));
                        }
                        Err(e) => {
                            log(&format!("delete scheduled: {e}"));
                            let _ = events.send(Event::Error(trf!("не отменено: {}", "not cancelled: {}", friendly_error(&e.to_string()))));
                        }
                    }
                }
                Cmd::Forward { src, dst, ids } => match client.forward_messages(dst.peer, &ids, src.peer).await {
                    Ok(_) => {
                        let _ = events.send(Event::Status(if ids.len() > 1 {
                            trf!("Переслано сообщений: {} в «{}»", "Forwarded {} messages to «{}»", ids.len(), dst.name)
                        } else {
                            trf!("Переслано в «{}»", "Forwarded to «{}»", dst.name)
                        }));
                    }
                    Err(e) => {
                        log(&format!("forward: {e}"));
                        let _ = events.send(Event::Error(trf!("не переслано: {}", "not forwarded: {}", friendly_error(&e.to_string()))));
                    }
                },
                Cmd::Send { chat, text, markdown, delay_minutes, reply_to } => {
                    // Пока в очереди есть сообщения, новые встают за ними: порядок не нарушается.
                    if !queue.is_empty() && delay_minutes.is_none() {
                        if queue.len() < MAX_QUEUED {
                            queue.push_back(Queued { chat: chat.clone(), text: text.clone(), markdown, reply_to });
                            let _ = events.send(Event::Queued { chat: chat.id, text, count: queue.len() });
                        } else {
                            let _ = events.send(Event::Error(
                                tr!("очередь переполнена, сообщение не принято", "queue is full, message rejected").into(),
                            ));
                            let _ = events.send(Event::SendFailed { text });
                        }
                        continue;
                    }
                    match send(&client, &chat, &text, markdown, delay_minutes, reply_to).await {
                        Ok(msg) => {
                            let _ = events.send(Event::Sent { chat: chat.id, msg, scheduled: delay_minutes.is_some() });
                        }
                        Err(e) if delay_minutes.is_none() && is_network_error(&e.to_string()) => {
                            log(&format!("send (queued): {e}"));
                            queue.push_back(Queued { chat: chat.clone(), text: text.clone(), markdown, reply_to });
                            let _ = events.send(Event::Queued { chat: chat.id, text, count: queue.len() });
                        }
                        Err(e) => {
                            log(&format!("send: {e}"));
                            let _ = events.send(Event::Error(trf!("не отправлено: {}", "not sent: {}", friendly_error(&e.to_string()))));
                            let _ = events.send(Event::SendFailed { text });
                        }
                    }
                }
                Cmd::Edit { chat, id, text, markdown } => {
                    let msg = if markdown { InputMessage::new().markdown(&text) } else { InputMessage::new().text(&text) };
                    match client.edit_message(chat.peer, id, msg).await {
                        Ok(()) => {
                            let m = ChatMsg {
                                id,
                                outgoing: true,
                                sender: tr!("Я", "Me").into(),
                                text: crate::util::clean_or(&text, tr!("[эмодзи]", "[emoji]")),
                                edited: true,
                                ..Default::default()
                            };
                            let _ = events.send(Event::Edited { chat: chat.id, msg: m });
                            let _ = events.send(Event::Status(tr!("Сообщение исправлено", "Message edited").into()));
                        }
                        Err(e) => {
                            log(&format!("edit: {e}"));
                            let _ = events.send(Event::Error(trf!("не исправлено: {}", "not edited: {}", friendly_error(&e.to_string()))));
                        }
                    }
                }
                Cmd::React { chat, id, emoji } => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match react(&client, &chat, id, emoji.as_deref()).await {
                            Ok(msg) => {
                                let _ = events.send(Event::Reacted { chat: chat.id, msg });
                            }
                            Err(e) => {
                                log(&format!("react: {e}"));
                                let _ = events.send(Event::Error(trf!(
                                    "реакция не поставлена: {}",
                                    "reaction not set: {}",
                                    friendly_error(&e.to_string())
                                )));
                            }
                        }
                    });
                }
                Cmd::Vote { chat, id, option } => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match vote(&client, &chat, id, option).await {
                            Ok(msg) => {
                                let _ = events.send(Event::Reacted { chat: chat.id, msg });
                            }
                            Err(e) => {
                                log(&format!("vote: {e}"));
                                let _ = events.send(Event::Error(trf!(
                                    "голос не принят: {}",
                                    "vote not counted: {}",
                                    friendly_error(&e.to_string())
                                )));
                            }
                        }
                    });
                }
                Cmd::Press { chat, id, data } => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match press_button(&client, &chat, id, data).await {
                            Ok(text) => {
                                let _ = events.send(Event::ButtonAnswer(text));
                            }
                            Err(e) => {
                                log(&format!("button: {e}"));
                                let _ = events.send(Event::Error(trf!(
                                    "кнопка не сработала: {}",
                                    "button failed: {}",
                                    friendly_error(&e.to_string())
                                )));
                            }
                        }
                    });
                }
                Cmd::Delete { chat, id } => match client.delete_messages(chat.peer, &[id]).await {
                    Ok(_) => {
                        let _ = events.send(Event::Deleted { chat: Some(chat.id), ids: vec![id] });
                        let _ = events.send(Event::Status(tr!("Сообщение удалено", "Message deleted").into()));
                    }
                    Err(e) => {
                        log(&format!("delete: {e}"));
                        let _ = events.send(Event::Error(trf!("не удалено: {}", "not deleted: {}", friendly_error(&e.to_string()))));
                    }
                },
                Cmd::DeleteMany { chat, ids } => match client.delete_messages(chat.peer, &ids).await {
                    Ok(_) => {
                        let n = ids.len();
                        let _ = events.send(Event::Deleted { chat: Some(chat.id), ids });
                        let _ = events.send(Event::Status(trf!("Удалено сообщений: {n}", "Messages deleted: {n}")));
                    }
                    Err(e) => {
                        log(&format!("delete many: {e}"));
                        let _ = events.send(Event::Error(trf!("не удалено: {}", "not deleted: {}", friendly_error(&e.to_string()))));
                    }
                },
                Cmd::Preview { chat, id } => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match fetch_preview(&client, &chat, id).await {
                            Ok(jpeg) => {
                                let _ = events.send(Event::Preview { chat: chat.id, jpeg });
                            }
                            Err(e) => {
                                log(&format!("preview: {e}"));
                                let _ = events.send(Event::Error(trf!("превью: {}", "preview: {}", friendly_error(&e.to_string()))));
                            }
                        }
                    });
                }
                Cmd::NetCheck(settings) => {
                    let events = events.clone();
                    tokio::spawn(async move {
                        let lines = crate::proxy::netcheck(&settings).await;
                        let _ = events.send(Event::NetCheck(lines));
                    });
                }
                Cmd::ListTopics(chat) => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match list_topics(&client, &chat).await {
                            Ok(items) => {
                                let _ = events.send(Event::Topics { chat: chat.id, items });
                            }
                            Err(e) => {
                                log(&format!("topics: {e}"));
                                let text = e.to_string();
                                let _ = events.send(Event::Error(if text.contains("FORUM") || text.contains("PEER_ID_INVALID") {
                                    tr!("в этом чате нет тем (это не форум)", "this chat has no topics (it is not a forum)").into()
                                } else {
                                    trf!("темы: {}", "topics: {}", friendly_error(&text))
                                }));
                            }
                        }
                    });
                }
                Cmd::ListPinned(chat) => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match list_pinned(&client, &chat).await {
                            Ok(hits) => {
                                let _ = events.send(Event::PinnedList { chat: chat.id, hits });
                            }
                            Err(e) => {
                                log(&format!("pinned list: {e}"));
                                let _ = events.send(Event::Error(trf!("закреплённые: {e}", "pinned: {e}")));
                            }
                        }
                    });
                }
                Cmd::OpenTarget(target) => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        let Target::Username(name) = target else { return };
                        match resolve_chat(&client, &name).await {
                            Ok(Some(info)) => {
                                let _ = events.send(Event::Found(info));
                            }
                            Ok(None) => {
                                let _ = events.send(Event::Error(trf!("@{} не найден", "@{} not found", name)));
                            }
                            Err(e) => {
                                log(&format!("open @{name}: {e}"));
                                let _ = events.send(Event::Error(trf!("@{}: {}", "@{}: {}", name, e)));
                            }
                        }
                    });
                }
                Cmd::Join(target) => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match join_target(&client, &target).await {
                            Ok(Some(info)) => {
                                let _ = events.send(Event::Found(info));
                            }
                            Ok(None) => {
                                let _ = events.send(Event::Status(tr!("Вы вступили в чат", "You joined the chat").into()));
                                if let Ok(d) = load_dialogs(&client, QUICK_DIALOGS, None).await {
                                    let _ = events.send(Event::DialogsPage(d));
                                }
                            }
                            Err(e) => {
                                log(&format!("join: {e}"));
                                let _ = events.send(Event::Error(trf!("не удалось вступить: {e}", "could not join: {e}")));
                            }
                        }
                    });
                }
                Cmd::SaveDraft { chat, text } => {
                    let client = client.clone();
                    tokio::spawn(async move {
                        // Черновик не критичен: ошибка только в журнал.
                        if let Err(e) = save_draft(&client, &chat, &text).await {
                            log(&format!("save draft: {e}"));
                        }
                    });
                }
                Cmd::Leave(chat) => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        // личный чат так «покинуть» нельзя: Telegram удалил бы переписку
                        if chat.kind == Kind::User {
                            return;
                        }
                        match client.delete_dialog(chat.peer).await {
                            Ok(()) => {
                                let _ = events.send(Event::Left(chat.id));
                            }
                            Err(e) => {
                                log(&format!("leave: {e}"));
                                let _ = events.send(Event::Error(trf!("не удалось выйти: {e}", "could not leave: {e}")));
                            }
                        }
                    });
                }
                Cmd::MarkRead(chat) => {
                    let client = client.clone();
                    tokio::spawn(async move {
                        let _ = client.mark_as_read(chat.peer).await;
                    });
                }
            }
        }
    });

    cmd_tx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: i64, kind: Kind) -> ChatInfo {
        let pid = match kind {
            Kind::User => PeerId::user(id).unwrap(),
            Kind::Group => PeerId::chat(id).unwrap(),
            Kind::Channel => PeerId::channel(id).unwrap(),
        };
        ChatInfo {
            id: pid,
            peer: pid.to_ambient_ref(),
            name: "x".into(),
            kind,
            unread: 0,
            presence: None,
            is_contact: false,
            is_bot: false,
            is_self: false,
            mentions: 0,
            read_out: 0,
            draft: String::new(),
        }
    }

    #[test]
    fn targets_are_parsed_from_links_and_names() {
        let user = |n: &str| Some(Target::Username(n.to_string()));
        let invite = |h: &str| Some(Target::Invite(h.to_string()));
        for ok in [
            "@durov",
            "durov",
            "t.me/durov",
            "https://t.me/durov",
            "http://t.me/durov/",
            "HTTPS://T.ME/durov",
            "t.me/durov/123",
            "t.me/durov?start=x",
            "https://telegram.me/durov",
            "tg://resolve?domain=durov",
            "t.me/s/durov",
            "www.t.me/durov",
            "  @durov  ",
        ] {
            assert_eq!(parse_target(ok), user("durov"), "{ok}");
        }
        for ok in [
            "t.me/+AbCdEfGh12345",
            "https://t.me/+AbCdEfGh12345",
            "t.me/joinchat/AbCdEfGh12345",
            "tg://join?invite=AbCdEfGh12345",
            "t.me/+AbCdEfGh12345?x=1",
        ] {
            assert_eq!(parse_target(ok), invite("AbCdEfGh12345"), "{ok}");
        }
        // регистр хэша сохраняется
        assert_eq!(parse_target("https://T.me/+aBcDeF_gh-1"), invite("aBcDeF_gh-1"));
        for bad in [
            "",
            "   ",
            "@",
            "@ab",
            "@1abcde",
            "@du rov",
            "durov channel",
            "t.me/",
            "t.me/+",
            "t.me/+abc",
            "t.me/joinchat",
            "t.me/addstickers/pack",
            "t.me/c/123/4",
            "https://example.com/durov",
            "tg://unknown?x=1",
            "tg://resolve",
            "tg://join?invite=ab",
            "@имя_на_русском",
            "@a_very_long_name_that_is_longer_than_thirty_two_characters",
        ] {
            assert_eq!(parse_target(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn read_receipts_come_only_from_private_chats() {
        let upd = |peer: tl::enums::Peer| {
            tl::enums::Update::ReadHistoryOutbox(tl::types::UpdateReadHistoryOutbox { peer, max_id: 42, pts: 1, pts_count: 1 })
        };
        assert_eq!(
            read_outbox_from_raw(&upd(tl::enums::Peer::User(tl::types::PeerUser { user_id: 7 }))),
            Some((PeerId::user(7).unwrap(), 42))
        );
        assert_eq!(read_outbox_from_raw(&upd(tl::enums::Peer::Chat(tl::types::PeerChat { chat_id: 7 }))), None);
        assert_eq!(read_outbox_from_raw(&upd(tl::enums::Peer::Channel(tl::types::PeerChannel { channel_id: 7 }))), None);
    }

    #[test]
    fn drafts_come_from_updates_and_dialogs() {
        let upd = |top: Option<i32>, draft: tl::enums::DraftMessage| {
            tl::enums::Update::DraftMessage(tl::types::UpdateDraftMessage {
                peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: 9 }),
                top_msg_id: top,
                saved_peer_id: None,
                draft,
            })
        };
        let text = tl::enums::DraftMessage::Message(tl::types::DraftMessage {
            no_webpage: false,
            invert_media: false,
            reply_to: None,
            message: "привет".into(),
            entities: None,
            media: None,
            date: 1,
            effect: None,
            suggested_post: None,
            rich_message: None,
        });
        assert_eq!(draft_from_raw(&upd(None, text.clone())), Some((PeerId::user(9).unwrap(), "привет".to_string())));
        assert_eq!(draft_text(&text), "привет");
        let empty = tl::enums::DraftMessage::Empty(tl::types::DraftMessageEmpty { date: None });
        assert_eq!(draft_from_raw(&upd(None, empty.clone())), Some((PeerId::user(9).unwrap(), String::new())));
        assert_eq!(draft_text(&empty), "");
        assert_eq!(draft_from_raw(&upd(Some(5), text)), None, "черновики тем форумов не учитываем");
    }

    #[test]
    fn folder_matching_rules() {
        let mut work = Folder { id: 1, title: "Работа".into(), groups: true, ..Default::default() };
        assert!(work.matches(&chat(5, Kind::Group)));
        assert!(!work.matches(&chat(5, Kind::Channel)));

        // явно добавленный чат попадает в папку, исключённый нет
        let ch = chat(7, Kind::Channel);
        work.include.push(ch.id);
        assert!(work.matches(&ch));
        work.exclude.push(ch.id);
        assert!(!work.matches(&ch));

        // «Личные»: контакты, не-контакты и боты настраиваются отдельно
        let mut f = Folder { contacts: true, ..Default::default() };
        let mut u = chat(10, Kind::User);
        assert!(!f.matches(&u));
        u.is_contact = true;
        assert!(f.matches(&u));
        u.is_bot = true;
        assert!(!f.matches(&u));
        f.bots = true;
        assert!(f.matches(&u));

        // «без прочитанных»
        let f = Folder { groups: true, exclude_read: true, ..Default::default() };
        let mut g = chat(11, Kind::Group);
        assert!(!f.matches(&g));
        g.unread = 3;
        assert!(f.matches(&g));
    }

    #[test]
    fn presence_labels() {
        let tz = jiff::tz::TimeZone::UTC;
        let now = jiff::Timestamp::from_second(1_761_700_000).unwrap().as_second(); // 2025-10-29 01:06 UTC
        assert_eq!(presence_label(Presence::Online(now + 60), now, &tz), "в сети");
        assert!(is_online(Presence::Online(now + 60), now));
        assert!(!is_online(Presence::Online(now - 1), now));
        assert_eq!(presence_label(Presence::Online(now - 5), now, &tz), "был(а) только что");
        assert_eq!(presence_label(Presence::Recently, now, &tz), "был(а) недавно");
        assert_eq!(presence_label(Presence::LastWeek, now, &tz), "был(а) на этой неделе");
        assert_eq!(presence_label(Presence::Hidden, now, &tz), "");
        // сегодня, вчера, давно
        assert_eq!(presence_label(Presence::Offline(now - 600), now, &tz), "был(а) в 00:56");
        assert_eq!(presence_label(Presence::Offline(now - 86_400), now, &tz), "был(а) вчера в 01:06");
        assert_eq!(presence_label(Presence::Offline(now - 5 * 86_400), now, &tz), "был(а) 24.10 в 01:06");
    }

    #[test]
    fn status_conversion() {
        let off = tl::enums::UserStatus::Offline(tl::types::UserStatusOffline { was_online: 123 });
        assert_eq!(presence_from(&off), Presence::Offline(123));
        assert_eq!(presence_from(&tl::enums::UserStatus::Empty), Presence::Hidden);
    }

    #[test]
    fn sizes_and_durations() {
        assert_eq!(fmt_size(500), "500 Б");
        assert_eq!(fmt_size(340 * 1024), "340 КБ");
        assert_eq!(fmt_size(2 * 1024 * 1024), "2 МБ");
        assert_eq!(fmt_size(1_572_864), "1.5 МБ");
        assert_eq!(fmt_size(3 * 1024 * 1024 * 1024), "3 ГБ");
        assert_eq!(fmt_duration(12.0), "0:12");
        assert_eq!(fmt_duration(185.4), "3:05");
        assert_eq!(fmt_duration(3723.0), "1:02:03");
    }

    #[test]
    fn document_labels() {
        use tl::enums::DocumentAttribute as A;
        let voice = A::Audio(tl::types::DocumentAttributeAudio { voice: true, duration: 12, title: None, performer: None, waveform: None });
        assert_eq!(doc_label(&[voice], "audio/ogg", Some(20_000)), "[голосовое 0:12]");
        let song = A::Audio(tl::types::DocumentAttributeAudio {
            voice: false,
            duration: 200,
            title: Some("Песня".into()),
            performer: Some("Группа".into()),
            waveform: None,
        });
        assert_eq!(doc_label(&[song], "audio/mpeg", Some(3 * 1024 * 1024)), "[аудио 3:20 Группа — Песня, 3 МБ]");
        let video = |round| {
            A::Video(tl::types::DocumentAttributeVideo {
                round_message: round,
                supports_streaming: false,
                nosound: false,
                duration: 30.0,
                w: 640,
                h: 480,
                preload_prefix_size: None,
                video_start_ts: None,
                video_codec: None,
            })
        };
        assert_eq!(doc_label(&[video(false)], "video/mp4", Some(5 * 1024 * 1024)), "[видео 0:30, 5 МБ]");
        assert_eq!(doc_label(&[video(true)], "video/mp4", Some(1000)), "[видеосообщение 0:30]");
        assert_eq!(doc_label(&[A::Animated, video(false)], "video/mp4", Some(1024 * 1024)), "[GIF, 1 МБ]");
        let name = A::Filename(tl::types::DocumentAttributeFilename { file_name: "отчёт.pdf".into() });
        assert_eq!(doc_label(&[name], "application/pdf", Some(2 * 1024 * 1024)), "[файл отчёт.pdf, 2 МБ]");
        assert_eq!(doc_label(&[], "application/zip", None), "[файл]");
        assert_eq!(doc_label(&[], "image/png", Some(1024)), "[изображение, 1 КБ]");
    }

    #[test]
    fn label_and_text_are_combined() {
        assert_eq!(compose_text("[фото]", ""), "[фото]");
        assert_eq!(compose_text("[фото]", "смотри"), "[фото] смотри");
        assert_eq!(compose_text("", "просто текст"), "просто текст");
        assert_eq!(compose_text("", ""), "[служебное сообщение]");
    }

    #[test]
    fn telegram_errors_are_explained() {
        assert!(friendly_error("rpc error 420: FLOOD_WAIT (value: 31)").contains("31"));
        assert!(friendly_error("MESSAGE_TOO_LONG").contains("4096"));
        assert!(friendly_error("CHAT_WRITE_FORBIDDEN").contains("нельзя"));
        assert_eq!(friendly_error("что-то непонятное"), "что-то непонятное");
    }

    #[test]
    fn flood_policy_waits_quietly_but_not_forever() {
        use std::num::NonZeroU32;
        let ctx = |secs: u32, fails: u32| RetryContext {
            fail_count: NonZeroU32::new(fails).unwrap(),
            slept_so_far: Duration::ZERO,
            error: InvocationError::Rpc(RpcError { code: 420, name: "FLOOD_WAIT".into(), value: Some(secs), caused_by: None }),
        };
        assert_eq!(FloodPolicy.should_retry(&ctx(30, 1)), ControlFlow::Continue(Duration::from_secs(30)));
        assert!(flood_left() > 0, "обратный отсчёт запущен");
        assert_eq!(FloodPolicy.should_retry(&ctx(30, 3)), ControlFlow::Break(()), "после двух попыток сдаёмся");
        assert_eq!(FloodPolicy.should_retry(&ctx(900, 1)), ControlFlow::Break(()), "слишком долго: показываем ошибку");
    }

    #[test]
    fn typing_updates_are_recognised() {
        use tl::enums::SendMessageAction as A;
        let user =
            tl::enums::Update::UserTyping(tl::types::UpdateUserTyping { user_id: 5, top_msg_id: None, action: A::SendMessageTypingAction });
        let (chat, from, kind) = typing_from_raw(&user).unwrap();
        assert_eq!(chat, PeerId::user(5).unwrap());
        assert!(from.is_none());
        assert_eq!(kind, TypingKind::Text);
        let stop =
            tl::enums::Update::UserTyping(tl::types::UpdateUserTyping { user_id: 5, top_msg_id: None, action: A::SendMessageCancelAction });
        assert_eq!(typing_from_raw(&stop).unwrap().2, TypingKind::Stop);
        let voice = tl::enums::Update::UserTyping(tl::types::UpdateUserTyping {
            user_id: 5,
            top_msg_id: None,
            action: A::SendMessageRecordAudioAction,
        });
        assert_eq!(typing_from_raw(&voice).unwrap().2, TypingKind::Voice);
        assert_eq!(TypingKind::Text.label(), "печатает…");
    }

    fn no_cyrillic(s: &str) -> bool {
        !s.chars().any(|c| ('\u{400}'..='\u{4ff}').contains(&c))
    }

    #[test]
    fn english_labels_have_no_russian_text() {
        crate::i18n::set(1);
        let tz = jiff::tz::TimeZone::UTC;
        let now = 1_800_000_000;
        for p in [
            Presence::Online(now + 60),
            Presence::Online(now - 60),
            Presence::Offline(now - 10),
            Presence::Offline(now - 86_400),
            Presence::Offline(now - 5 * 86_400),
            Presence::Recently,
            Presence::LastWeek,
            Presence::LastMonth,
        ] {
            let l = presence_label(p, now, &tz);
            assert!(no_cyrillic(&l), "{l}");
        }
        for n in [0usize, 500, 2048, 5 * 1024 * 1024, 3 * 1024 * 1024 * 1024] {
            assert!(no_cyrillic(&fmt_size(n)));
        }
        assert_eq!(fmt_size(2 * 1024 * 1024), "2 MB");
        assert_eq!(fmt_size(1536), "2 KB");
        for e in ["FLOOD_WAIT_30", "MESSAGE_TOO_LONG", "CHAT_WRITE_FORBIDDEN", "MESSAGE_ID_INVALID", "PEER_FLOOD"] {
            let m = friendly_error(e);
            assert!(no_cyrillic(&m), "{m}");
        }
        assert!(no_cyrillic(&compose_text("", "")));
        for k in [TypingKind::Text, TypingKind::Voice, TypingKind::File] {
            assert!(no_cyrillic(k.label()));
        }
        crate::i18n::set(0);
    }

    #[test]
    fn unknown_errors_are_passed_through_unchanged() {
        assert_eq!(friendly_error("SOMETHING_NEW_42"), "SOMETHING_NEW_42");
        assert!(friendly_error("FLOOD_WAIT_17").contains("17"));
        assert!(friendly_error("420 SLOWMODE_WAIT_5").contains('5'));
    }

    #[test]
    fn duration_formats_edge_cases() {
        assert_eq!(fmt_duration(-5.0), "0:00");
        assert_eq!(fmt_duration(59.6), "1:00");
        assert_eq!(fmt_duration(3725.0), "1:02:05");
    }

    #[test]
    fn chat_msg_default_is_empty_and_safe() {
        let m = ChatMsg::default();
        assert!(m.text.is_empty() && m.raw.is_empty() && m.caption.is_none() && !m.edited && m.reply_to.is_none());
    }
}
