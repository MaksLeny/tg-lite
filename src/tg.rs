//! Работа с Telegram: вход в аккаунт и фоновый обработчик команд интерфейса.

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

use crate::vault::VaultSession;

/// Сколько диалогов и сообщений держим в памяти. Небольшие числа = мало ОЗУ.
const MAX_DIALOGS: usize = 120;
const HISTORY_LIMIT: usize = 60;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    User,
    Group,
    Channel,
}

#[derive(Clone, Debug)]
pub struct ChatInfo {
    pub id: PeerId,
    pub peer: PeerRef,
    pub name: String,
    pub kind: Kind,
    pub unread: i32,
}

#[derive(Clone, Debug)]
pub struct ChatMsg {
    pub outgoing: bool,
    pub sender: String,
    pub text: String,
    pub time: String,
}

/// Команды от интерфейса к фоновому обработчику.
pub enum Cmd {
    LoadDialogs,
    LoadHistory(ChatInfo),
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
    History { chat: PeerId, msgs: Vec<ChatMsg> },
    Incoming { chat: PeerId, chat_name: String, msg: ChatMsg },
    Sent { chat: PeerId, msg: ChatMsg, scheduled: bool },
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
    let SenderPool { runner, updates, handle } =
        SenderPool::with_configuration(Arc::clone(&session), api_id, params);
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
    let text = m.text().to_string();
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
        m.sender()
            .and_then(|p| p.name().map(str::to_string))
            .unwrap_or_else(|| fallback_name.to_string())
    };
    ChatMsg { outgoing: m.outgoing(), sender, text, time: format_time(m.date()) }
}

fn dialog_unread(d: &grammers_client::peer::Dialog) -> i32 {
    match &d.raw {
        tl::enums::Dialog::Dialog(x) => x.unread_count,
        _ => 0,
    }
}

// ------------------------------------------------------------ операции

pub async fn load_dialogs(client: &Client) -> Result<Vec<ChatInfo>> {
    let mut it = client.iter_dialogs();
    let mut out = Vec::new();
    while let Some(d) = it.next().await? {
        out.push(ChatInfo {
            id: d.peer_id(),
            peer: d.peer_ref(),
            name: d.peer().name().unwrap_or("(без имени)").to_string(),
            kind: kind_of(d.peer()),
            unread: dialog_unread(&d),
        });
        if out.len() >= MAX_DIALOGS {
            break;
        }
    }
    Ok(out)
}

pub async fn load_history(client: &Client, chat: &ChatInfo) -> Result<Vec<ChatMsg>> {
    let mut it = client.iter_messages(chat.peer).limit(HISTORY_LIMIT);
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
pub fn spawn_worker(
    conn: Connection,
    events: mpsc::UnboundedSender<Event>,
) -> mpsc::UnboundedSender<Cmd> {
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Cmd>();
    let client = conn.client.clone();

    // 1) входящие сообщения
    {
        let client = client.clone();
        let events = events.clone();
        let updates = conn.updates;
        tokio::spawn(async move {
            let mut stream = match client.stream_updates(updates, UpdatesConfiguration { catch_up: false }).await {
                Ok(s) => s,
                Err(e) => {
                    let _ = events.send(Event::Error(format!("не удалось получать обновления: {e}")));
                    return;
                }
            };
            loop {
                match stream.next().await {
                    Ok(Update::NewMessage(m)) => {
                        let name = m.peer().and_then(|p| p.name()).unwrap_or("?").to_string();
                        let msg = to_chat_msg(&m, &name);
                        let _ = events.send(Event::Incoming { chat: m.peer_id(), chat_name: name, msg });
                    }
                    Ok(_) => {}
                    Err(e) => {
                        let _ = events.send(Event::Status(format!("связь потеряна: {e}; переподключаюсь…")));
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });
    }

    // 2) команды интерфейса
    tokio::spawn(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                Cmd::LoadDialogs => match load_dialogs(&client).await {
                    Ok(d) => {
                        let _ = events.send(Event::Dialogs(d));
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error(format!("список чатов: {e}")));
                    }
                },
                Cmd::LoadHistory(chat) => match load_history(&client, &chat).await {
                    Ok(msgs) => {
                        let _ = events.send(Event::History { chat: chat.id, msgs });
                    }
                    Err(e) => {
                        let _ = events.send(Event::Error(format!("история: {e}")));
                    }
                },
                Cmd::Send { chat, text, markdown, delay_minutes } => {
                    match send(&client, &chat, &text, markdown, delay_minutes).await {
                        Ok(msg) => {
                            let _ = events.send(Event::Sent {
                                chat: chat.id,
                                msg,
                                scheduled: delay_minutes.is_some(),
                            });
                        }
                        Err(e) => {
                            let _ = events.send(Event::Error(format!("отправка: {e}")));
                        }
                    }
                }
                Cmd::MarkRead(chat) => {
                    let _ = client.mark_as_read(chat.peer).await;
                }
            }
        }
    });

    cmd_tx
}
