//! Работа с Telegram: подключение и фоновый обработчик команд интерфейса.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow};
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

/// Сколько диалогов держим в памяти. Небольшое число = мало ОЗУ.
const MAX_DIALOGS: usize = 120;

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
        Presence::Online(exp) if exp > now => "в сети".to_string(),
        Presence::Online(_) => "был(а) только что".to_string(),
        Presence::Offline(ts) => {
            let (Some(t), Some(n)) = (local(ts, tz), local(now, tz)) else {
                return "был(а) в сети".to_string();
            };
            let hm = t.strftime("%H:%M");
            if t.date() == n.date() {
                format!("был(а) в {hm}")
            } else if n.date().yesterday().ok() == Some(t.date()) {
                format!("был(а) вчера в {hm}")
            } else {
                format!("был(а) {} в {hm}", t.strftime("%d.%m"))
            }
        }
        Presence::Recently => "был(а) недавно".to_string(),
        Presence::LastWeek => "был(а) на этой неделе".to_string(),
        Presence::LastMonth => "был(а) в этом месяце".to_string(),
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
    /// Только для личных чатов.
    pub presence: Option<Presence>,
    pub is_contact: bool,
    pub is_bot: bool,
    pub is_self: bool,
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

/// Откуда переслано сообщение.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fwd {
    /// Имя, если Telegram его прислал (скрытый автор, подпись канала).
    pub name: Option<String>,
    /// Отправитель, если он известен: имя подставляется из списка чатов.
    pub from: Option<PeerId>,
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
    pub time: String,
    /// Сообщение исправлено после отправки.
    pub edited: bool,
    /// Номер сообщения, на которое это является ответом.
    pub reply_to: Option<i32>,
    pub fwd: Option<Fwd>,
}

/// Команды от интерфейса к фоновому обработчику.
pub enum Cmd {
    /// Список чатов (а следом и папки).
    LoadDialogs,
    LoadHistory(ChatInfo, usize),
    Send {
        chat: ChatInfo,
        text: String,
        markdown: bool,
        /// Отложенная публикация: через сколько минут отправить.
        delay_minutes: Option<u32>,
        /// Ответ на сообщение с этим номером.
        reply_to: Option<i32>,
    },
    /// Переслать сообщение `id` из `src` в `dst`.
    Forward { src: ChatInfo, dst: ChatInfo, id: i32 },
    MarkRead(ChatInfo),
    /// Исправить своё сообщение.
    Edit { chat: ChatInfo, id: i32, text: String, markdown: bool },
    /// Удалить своё сообщение (у всех).
    Delete { chat: ChatInfo, id: i32 },
}

/// События от обработчика к интерфейсу.
pub enum Event {
    Dialogs(Vec<ChatInfo>),
    Folders(Vec<Folder>),
    History { chat: PeerId, msgs: Vec<ChatMsg> },
    Incoming { chat: PeerId, chat_name: String, msg: ChatMsg },
    Sent { chat: PeerId, msg: ChatMsg, scheduled: bool },
    /// Отправка не удалась: текст возвращается в поле ввода, чтобы не пропал.
    SendFailed { text: String },
    Edited { chat: PeerId, msg: ChatMsg },
    /// `chat` = None для личных чатов и обычных групп (id сообщений общие на аккаунт).
    Deleted { chat: Option<PeerId>, ids: Vec<i32> },
    Presence { user: PeerId, presence: Presence },
    Status(String),
    Error(String),
}

pub struct Connection {
    pub client: Client,
    pub quit: grammers_mtsender::SenderPoolFatHandle,
    pub updates: mpsc::Receiver<grammers_session::updates::UpdatesLike>,
}

/// Подключается к Telegram, используя сессию из хранилища.
/// `proxy` — необязательный SOCKS5-прокси вида `socks5://127.0.0.1:1080`.
pub fn connect(session: Arc<VaultSession>, api_id: i32, proxy: Option<String>) -> Connection {
    let params = grammers_mtsender::ConnectionParams {
        proxy_url: proxy,
        retry_policy: Box::new(FloodPolicy),
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
    if z.date() == now.date() {
        z.strftime("%H:%M").to_string()
    } else {
        z.strftime("%d.%m %H:%M").to_string()
    }
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
            InvocationError::Rpc(RpcError { code: 420, value: Some(secs), .. })
                if ctx.fail_count.get() <= 2 && *secs <= 300 =>
            {
                FLOOD_UNTIL.store(jiff::Timestamp::now().as_second() + i64::from(*secs), Ordering::Relaxed);
                ControlFlow::Continue(Duration::from_secs(u64::from(*secs)))
            }
            InvocationError::Io(_) if ctx.fail_count.get() <= 1 => ControlFlow::Continue(Duration::from_secs(1)),
            _ => ControlFlow::Break(()),
        }
    }
}

/// Понятное сообщение вместо технического кода ошибки Telegram.
pub fn friendly_error(e: &str) -> String {
    let wait = || {
        e.split(|c: char| !c.is_ascii_digit()).filter(|x| !x.is_empty()).next_back().unwrap_or("?").to_string()
    };
    if e.contains("FLOOD_WAIT") || e.contains("SLOWMODE_WAIT") {
        return format!("Telegram просит подождать {} с и не принимает запросы", wait());
    }
    let known = [
        ("MESSAGE_TOO_LONG", "сообщение слишком длинное (лимит Telegram 4096 символов)"),
        ("CHAT_WRITE_FORBIDDEN", "в этот чат писать нельзя"),
        ("CHAT_ADMIN_REQUIRED", "нужны права администратора"),
        ("USER_BANNED_IN_CHANNEL", "вам запрещено писать в этот чат"),
        ("MESSAGE_NOT_MODIFIED", "текст не изменился"),
        ("MESSAGE_EDIT_TIME_EXPIRED", "время для правки сообщения истекло"),
        ("MESSAGE_ID_INVALID", "такого сообщения уже нет"),
        ("PEER_FLOOD", "Telegram ограничил отправку, попробуйте позже"),
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
        format!("{} ГБ", trim_zero(format!("{:.1}", b / gb)))
    } else if b >= mb {
        format!("{} МБ", trim_zero(format!("{:.1}", b / mb)))
    } else if b >= kb {
        format!("{:.0} КБ", b / kb)
    } else {
        format!("{bytes} Б")
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
            A::Audio(au) if au.voice => return format!("[голосовое {}]", fmt_duration(f64::from(au.duration))),
            A::Audio(au) => {
                let who = match (&au.performer, &au.title) {
                    (Some(p), Some(t)) => format!(" {} — {}", crate::util::clean(p), crate::util::clean(t)),
                    (None, Some(t)) => format!(" {}", crate::util::clean(t)),
                    _ => String::new(),
                };
                return with_size(format!("аудио {}{who}", fmt_duration(f64::from(au.duration))));
            }
            A::Video(v) if v.round_message => return format!("[видеосообщение {}]", fmt_duration(v.duration)),
            A::Video(v) => return with_size(format!("видео {}", fmt_duration(v.duration))),
            _ => {}
        }
    }
    let name = attrs.iter().find_map(|a| match a {
        A::Filename(f) => Some(crate::util::clean(&f.file_name)),
        _ => None,
    });
    if mime.starts_with("image/") && name.is_none() {
        return with_size("изображение".into());
    }
    match name {
        Some(n) if !n.trim().is_empty() => with_size(format!("файл {}", n.trim())),
        _ => with_size("файл".into()),
    }
}

/// Подпись вложения. Пустая строка — подписывать нечего (например, превью ссылки).
pub fn media_label(media: &grammers_client::media::Media) -> String {
    use grammers_client::media::Media as M;
    match media {
        M::Photo(_) => "[фото]".into(),
        M::Sticker(_) => "[стикер]".into(),
        M::Document(d) => match &d.raw.document {
            Some(tl::enums::Document::Document(doc)) => doc_label(&doc.attributes, &doc.mime_type, Some(doc.size.max(0) as usize)),
            _ => "[файл]".into(),
        },
        M::Contact(c) => {
            let name = crate::util::clean(&format!("{} {}", c.first_name(), c.last_name()));
            format!("[контакт {}]", name.trim()).replace("[контакт ]", "[контакт]")
        }
        M::Poll(p) => {
            let tl::enums::TextWithEntities::Entities(q) = p.question();
            format!("[опрос: {}]", crate::util::clean(&q.text).trim())
        }
        M::Geo(_) | M::GeoLive(_) => "[геопозиция]".into(),
        M::Venue(v) => format!("[место: {}]", crate::util::clean(v.title()).trim()),
        M::Dice(_) => "[кубик]".into(),
        M::WebPage(_) => String::new(),
        _ => "[вложение]".into(),
    }
}

/// Склеивает подпись вложения и текст сообщения.
pub fn compose_text(label: &str, text: &str) -> String {
    match (label.is_empty(), text.is_empty()) {
        (true, true) => "[служебное сообщение]".to_string(),
        (true, false) => text.to_string(),
        (false, true) => label.to_string(),
        (false, false) => format!("{label} {text}"),
    }
}

pub fn to_chat_msg(m: &Message, fallback_name: &str) -> ChatMsg {
    let text = crate::util::clean_or(m.text(), "[эмодзи]");
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
        "Я".to_string()
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
        time: format_time(m.date()),
        edited: m.edit_date().is_some() && !m.edit_hide(),
        reply_to: m.reply_to_message_id(),
        fwd,
    }
}

fn dialog_unread(d: &grammers_client::peer::Dialog) -> i32 {
    match &d.raw {
        tl::enums::Dialog::Dialog(x) => x.unread_count,
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
                title: crate::util::clean_or(&t.text, "Папка").trim().to_string(),
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
                title: crate::util::clean_or(&t.text, "Папка").trim().to_string(),
                include,
                include_self: f.pinned_peers.iter().chain(f.include_peers.iter()).any(|p| matches!(p, tl::enums::InputPeer::PeerSelf)),
                ..Default::default()
            })
        }
        _ => None,
    }
}

// ------------------------------------------------------------ операции

pub async fn load_dialogs(client: &Client) -> Result<Vec<ChatInfo>> {
    let mut it = client.iter_dialogs();
    let mut out = Vec::new();
    while let Some(d) = it.next().await? {
        let (name, presence, is_contact, is_bot, is_self) = match d.peer() {
            Peer::User(u) => {
                let is_self = u.is_self();
                let name = if is_self { "Избранное".to_string() } else { crate::util::clean(&u.full_name()) };
                let presence = if is_self { None } else { Some(presence_from(u.status())) };
                (name, presence, u.contact(), u.is_bot(), is_self)
            }
            p => (crate::util::clean(p.name().unwrap_or("(без имени)")), None, false, false, false),
        };
        out.push(ChatInfo {
            id: d.peer_id(),
            peer: d.peer_ref(),
            name: if name.trim().is_empty() { "(без имени)".to_string() } else { name.trim().to_string() },
            kind: kind_of(d.peer()),
            unread: dialog_unread(&d),
            presence,
            is_contact,
            is_bot,
            is_self,
        });
        if out.len() >= MAX_DIALOGS {
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
                    let _ = events.send(Event::Error(format!("не удалось получать обновления: {e}")));
                    return;
                }
            };
            let mut was_down = false;
            loop {
                let update = stream.next().await;
                if was_down && update.is_ok() {
                    was_down = false;
                    let _ = events.send(Event::Status("связь восстановлена".into()));
                }
                match update {
                    Ok(Update::NewMessage(m)) => {
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
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        log(&format!("updates: {e}"));
                        was_down = true;
                        let _ = events.send(Event::Status(format!("связь потеряна: {e}; переподключаюсь…")));
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });
    }

    // 2) команды интерфейса. Отправка идёт строго по порядку, остальное параллельно.
    tokio::spawn(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                Cmd::LoadDialogs => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match load_dialogs(&client).await {
                            Ok(d) => {
                                let _ = events.send(Event::Dialogs(d));
                            }
                            Err(e) => {
                                log(&format!("dialogs: {e}"));
                                let _ = events.send(Event::Error(format!("список чатов: {e}")));
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
                Cmd::LoadHistory(chat, limit) => {
                    let client = client.clone();
                    let events = events.clone();
                    tokio::spawn(async move {
                        match load_history(&client, &chat, limit).await {
                            Ok(msgs) => {
                                let _ = events.send(Event::History { chat: chat.id, msgs });
                            }
                            Err(e) => {
                                log(&format!("history: {e}"));
                                let _ = events.send(Event::Error(format!("история: {e}")));
                            }
                        }
                    });
                }
                Cmd::Forward { src, dst, id } => {
                    match client.forward_messages(dst.peer, &[id], src.peer).await {
                        Ok(_) => {
                            let _ = events.send(Event::Status(format!("Переслано в «{}»", dst.name)));
                        }
                        Err(e) => {
                            log(&format!("forward: {e}"));
                            let _ = events.send(Event::Error(format!("не переслано: {}", friendly_error(&e.to_string()))));
                        }
                    }
                }
                Cmd::Send { chat, text, markdown, delay_minutes, reply_to } => {
                    match send(&client, &chat, &text, markdown, delay_minutes, reply_to).await {
                        Ok(msg) => {
                            let _ = events.send(Event::Sent { chat: chat.id, msg, scheduled: delay_minutes.is_some() });
                        }
                        Err(e) => {
                            log(&format!("send: {e}"));
                            let _ = events.send(Event::Error(format!("не отправлено: {}", friendly_error(&e.to_string()))));
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
                                sender: "Я".into(),
                                text: crate::util::clean_or(&text, "[эмодзи]"),
                                edited: true,
                                ..Default::default()
                            };
                            let _ = events.send(Event::Edited { chat: chat.id, msg: m });
                            let _ = events.send(Event::Status("Сообщение исправлено".into()));
                        }
                        Err(e) => {
                            log(&format!("edit: {e}"));
                            let _ = events.send(Event::Error(format!("не исправлено: {}", friendly_error(&e.to_string()))));
                        }
                    }
                }
                Cmd::Delete { chat, id } => match client.delete_messages(chat.peer, &[id]).await {
                    Ok(_) => {
                        let _ = events.send(Event::Deleted { chat: Some(chat.id), ids: vec![id] });
                        let _ = events.send(Event::Status("Сообщение удалено".into()));
                    }
                    Err(e) => {
                        log(&format!("delete: {e}"));
                        let _ = events.send(Event::Error(format!("не удалено: {}", friendly_error(&e.to_string()))));
                    }
                },
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
        }
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
}
