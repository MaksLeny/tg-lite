//! Работа с Telegram: подключение и фоновый обработчик команд интерфейса.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow};
use grammers_client::message::{InputMessage, Message};
use grammers_client::peer::Peer;
use grammers_client::session::types::{PeerId, PeerRef};
use grammers_client::tl;
use grammers_client::update::Update;
use grammers_client::{Client, SenderPool};
use grammers_mtsender::UpdatesConfiguration;
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

#[derive(Clone, Debug)]
pub struct ChatMsg {
    pub id: i32,
    /// Unix-время (секунды) для разделителей по датам.
    pub ts: i64,
    pub outgoing: bool,
    pub sender: String,
    pub text: String,
    pub time: String,
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
    },
    MarkRead(ChatInfo),
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
    let params = grammers_mtsender::ConnectionParams { proxy_url: proxy, ..Default::default() };
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

pub fn to_chat_msg(m: &Message, fallback_name: &str) -> ChatMsg {
    let text = crate::util::clean_or(m.text(), "[эмодзи]");
    let has_media = m.media().is_some();
    let text = match (text.is_empty(), has_media) {
        (true, true) => "[вложение]".to_string(),
        (true, false) => "[служебное сообщение]".to_string(),
        (false, true) => format!("[вложение] {text}"),
        (false, false) => text,
    };
    let sender = if m.outgoing() {
        "Я".to_string()
    } else {
        m.sender().and_then(|p| p.name().map(crate::util::clean)).unwrap_or_else(|| fallback_name.to_string())
    };
    ChatMsg { id: m.id(), ts: m.date().as_second(), outgoing: m.outgoing(), sender, text, time: format_time(m.date()) }
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
) -> Result<ChatMsg> {
    let mut msg = if markdown { InputMessage::new().markdown(text) } else { InputMessage::new().text(text) };
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
                Cmd::Send { chat, text, markdown, delay_minutes } => {
                    match send(&client, &chat, &text, markdown, delay_minutes).await {
                        Ok(msg) => {
                            let _ = events.send(Event::Sent { chat: chat.id, msg, scheduled: delay_minutes.is_some() });
                        }
                        Err(e) => {
                            log(&format!("send: {e}"));
                            let _ = events.send(Event::Error(format!("не отправлено: {e}")));
                            let _ = events.send(Event::SendFailed { text });
                        }
                    }
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
}
