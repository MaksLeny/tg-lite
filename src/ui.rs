//! Терминальный интерфейс: вкладки папок сверху, список чатов слева, переписка справа,
//! поле ввода внизу. Автоблокировка по бездействию: пока заблокировано, переписка не рисуется.
//!
//! Клавиши сделаны так, чтобы работать в обычной консоли Windows:
//! F-клавиши и /команды не зависят от раскладки, а Ctrl-комбинации — запасной вариант.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use grammers_client::session::types::PeerId;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use textwrap::core::display_width;
use tokio::sync::mpsc;

use crate::settings::{HISTORY_CHOICES, LOCK_CHOICES, ProxyMode, Settings, WIPE_CHOICES, cycle};
use crate::tg::{self, ChatInfo, ChatMsg, Cmd, Event, Folder, Kind};
use crate::util;
use crate::vault::Saver;
use crate::{tr, trf};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Как часто сохраняем сессию на диск.
const SAVE_EVERY: Duration = Duration::from_secs(120);
/// Как часто обновляем список чатов (статусы «в сети», непрочитанные, папки).
const REFRESH_EVERY: Duration = Duration::from_secs(120);
/// Пауза после выбора чата перед загрузкой истории (чтобы не грузить при пролистывании).
const LOAD_DELAY: Duration = Duration::from_millis(250);
/// Enter или Tab, пришедшие быстрее этого после предыдущей клавиши, считаем частью вставленного текста.
const PASTE_GAP: Duration = Duration::from_millis(25);
/// Максимум сообщений на чат в памяти.
const KEEP_MESSAGES: usize = 300;
/// Предел длины сообщения в Telegram.
const MAX_MESSAGE_CHARS: usize = 4096;

/// Команды для автодополнения по Tab.
const COMMANDS: &[&str] = &[
    "/find",
    "/pin",
    "/readall",
    "/export",
    "/note",
    "/edit",
    "/del",
    "/later",
    "/md",
    "/lock",
    "/settings",
    "/passwd",
    "/proxy",
    "/refresh",
    "/help",
    "/quit",
    "/search",
    "/save",
    "/send",
    "/links",
    "/scheduled",
    "/paste",
    "/faq",
];

/// Частые вопросы: показываются командой /faq.
fn faq() -> [(&'static str, &'static str); 7] {
    [
        (
            tr!("Не вставляется текст", "Text will not paste"),
            tr!(
                "Ctrl+V, Shift+Insert или команда /paste; в старой консоли Windows можно ещё правой кнопкой мыши",
                "Ctrl+V, Shift+Insert or the /paste command; in the old Windows console you can also right-click"
            ),
        ),
        (
            tr!("Вместо букв квадраты", "Boxes instead of letters"),
            tr!(
                "включите в консоли шрифт Consolas или запустите в Windows Terminal; эмодзи программа скрывает сама",
                "set the console font to Consolas or run in Windows Terminal; the program hides emoji itself"
            ),
        ),
        (
            tr!("Нет подключения", "No connection"),
            tr!(
                "Telegram может быть заблокирован: включите VPN или задайте /proxy 127.0.0.1:порт, затем перезапустите",
                "Telegram may be blocked: turn on a VPN or set /proxy 127.0.0.1:port, then restart"
            ),
        ),
        (
            tr!("Антивирус ругается", "Antivirus complains"),
            tr!(
                "exe без подписи, собран из открытого кода; сверьте SHA256 с релизом или соберите сами",
                "the exe is unsigned and built from open source; compare the SHA256 with the release or build it yourself"
            ),
        ),
        (
            tr!("Забыл пароль", "Forgot the password"),
            tr!(
                "восстановить нельзя: удалите tglite.vault рядом с exe и войдите заново",
                "it cannot be recovered: delete tglite.vault next to the exe and sign in again"
            ),
        ),
        (
            tr!("Нет звука", "No sound"),
            tr!(
                "F4 → Звук: включить; сигнал идёт через звуковую карту, проверьте громкость",
                "F4 → Sound: on; the beep goes through the sound card, check the volume"
            ),
        ),
        (
            tr!("Что-то сломалось", "Something broke"),
            tr!(
                "запустите tg-lite --debug и пришлите tglite.log (в нём нет текстов переписки)",
                "run tg-lite --debug and send tglite.log (it contains no message text)"
            ),
        ),
    ]
}

const MUTED: Color = Color::Rgb(120, 135, 150);
const OWN: Color = Color::Rgb(110, 214, 130);
const ONLINE: Color = Color::Rgb(110, 214, 130);
const ERROR: Color = Color::Rgb(255, 110, 110);
const NAME_COLORS: [Color; 7] = [
    Color::Rgb(255, 140, 120),
    Color::Rgb(255, 196, 90),
    Color::Rgb(160, 220, 110),
    Color::Rgb(90, 210, 200),
    Color::Rgb(120, 170, 255),
    Color::Rgb(200, 140, 255),
    Color::Rgb(255, 130, 190),
];

#[derive(Clone, Copy)]
struct Theme {
    accent: Color,
    select_bg: Color,
}

fn theme(idx: u8) -> Theme {
    let (accent, select_bg) = match idx {
        1 => (Color::Rgb(80, 205, 125), Color::Rgb(30, 100, 60)),
        2 => (Color::Rgb(255, 176, 60), Color::Rgb(120, 82, 20)),
        3 => (Color::Rgb(240, 100, 170), Color::Rgb(120, 40, 85)),
        4 => (Color::Rgb(170, 135, 255), Color::Rgb(75, 55, 135)),
        _ => (Color::Rgb(42, 171, 238), Color::Rgb(28, 92, 140)),
    };
    Theme { accent, select_bg }
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum Mode {
    Chat,
    /// Выбор сообщения стрелками: ответить, переслать, править, удалить.
    Pick,
    /// Выбор чата, куда переслать сообщение.
    Forward,
    Locked,
    Help,
    Settings,
    NewPassword,
    /// Список поверх чата: ссылки или отложенные сообщения.
    Overlay,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum OverlayKind {
    Links,
    Scheduled,
    /// Подсказки «Если не работает».
    Faq,
}

struct OverlayItem {
    label: String,
    /// Для ссылок — адрес.
    data: String,
    /// Для отложенных — номер сообщения.
    id: i32,
}

struct Overlay {
    kind: OverlayKind,
    chat: Option<ChatInfo>,
    title: String,
    items: Vec<OverlayItem>,
    sel: usize,
    /// Номер элемента, для которого ждём подтверждения отмены.
    confirm: Option<usize>,
}

/// Сколько секунд показывать «печатает…» без новых обновлений.
const TYPING_TTL: Duration = Duration::from_secs(6);
const MENTION: Color = Color::Rgb(255, 196, 90);

/// Вкладка списка чатов: встроенные фильтры и папки из аккаунта.
#[derive(Clone, PartialEq, Debug)]
enum Tab {
    All,
    Unread,
    Users,
    Groups,
    Channels,
    Folder(i32),
}

const SETTINGS_ROWS: usize = 12;

pub struct App {
    chats: Vec<ChatInfo>,
    /// Выбранный чат хранится по id, а не по номеру: так выбор не «съезжает» при обновлении списка.
    sel: Option<PeerId>,
    list_state: ListState,
    tab: Tab,
    folders: Vec<Folder>,
    histories: HashMap<PeerId, Vec<ChatMsg>>,
    input: String,
    /// Позиция курсора в поле ввода (в символах, не в байтах).
    cursor: usize,
    status: String,
    markdown: bool,
    /// Сколько строк прокручено вверх от самого низа переписки.
    scroll: usize,
    mode: Mode,
    lock_input: String,
    lock_error: String,
    settings: Settings,
    theme: Theme,
    settings_row: usize,
    /// 0 — вводим новый пароль, 1 — повторяем.
    pw_stage: u8,
    pw_first: String,
    last_activity: Instant,
    last_key_at: Instant,
    pending_load: Option<Instant>,
    reload_pending: bool,
    should_quit: bool,
    /// Поиск по названиям чатов (`/find`); пусто — без фильтра.
    filter: String,
    /// Черновики по чатам: переключение чата не теряет набранный текст (только в памяти).
    drafts: HashMap<PeerId, (String, usize)>,
    /// Сообщение, ожидающее подтверждения удаления (второй `/del`).
    pending_delete: Option<(PeerId, i32)>,
    /// Индекс выбранного сообщения в режиме выбора.
    pick: usize,
    /// Ответ на сообщение: (номер, подпись для поля ввода).
    reply: Option<(i32, String)>,
    /// Номер сообщения, которое сейчас правится.
    edit_target: Option<i32>,
    /// Пересылаемое сообщение: откуда и номер.
    fwd_src: Option<(ChatInfo, i32)>,
    /// Неверные пароли подряд на экране блокировки.
    wrong_attempts: u32,
    /// Момент запуска: от него считается время анимации фона.
    started: Instant,
    /// Подсказка в режиме выбора сообщения (почему действие недоступно, просьба подтвердить).
    pick_note: String,
    /// Кто сейчас печатает: чат -> (когда обновили, подпись).
    typing: HashMap<PeerId, (Instant, String)>,
    /// Закреплённые сообщения чатов (текст).
    pinned_msg: HashMap<PeerId, String>,
    overlay: Option<Overlay>,
}

// ------------------------------------------------------------------ вспомогательное

/// Буква Ctrl-комбинации независимо от раскладки (русские буквы приводятся к латинским).
pub(crate) fn ctrl_letter(key: &KeyEvent) -> Option<char> {
    if key.modifiers.contains(KeyModifiers::ALT) {
        return None; // AltGr (Ctrl+Alt) вводит обычные символы
    }
    let KeyCode::Char(c) = key.code else { return None };
    // Управляющие коды 0x01..0x1A = Ctrl+A..Ctrl+Z.
    let c = match c {
        '\u{1}'..='\u{1a}' => (b'a' + (c as u8 - 1)) as char,
        _ if key.modifiers.contains(KeyModifiers::CONTROL) => c.to_lowercase().next()?,
        _ => return None,
    };
    Some(match c {
        'й' => 'q',
        'ц' => 'w',
        'у' => 'e',
        'к' => 'r',
        'е' => 't',
        'н' => 'y',
        'г' => 'u',
        'ш' => 'i',
        'щ' => 'o',
        'з' => 'p',
        'ф' => 'a',
        'ы' => 's',
        'в' => 'd',
        'а' => 'f',
        'п' => 'g',
        'р' => 'h',
        'о' => 'j',
        'л' => 'k',
        'д' => 'l',
        'я' => 'z',
        'ч' => 'x',
        'с' => 'c',
        'м' => 'v',
        'и' => 'b',
        'т' => 'n',
        'ь' => 'm',
        other => other,
    })
}

/// Ctrl+V (в любой раскладке), Ctrl+Shift+V или Shift+Insert — вставка из буфера обмена.
pub(crate) fn is_paste_key(key: &KeyEvent) -> bool {
    ctrl_letter(key) == Some('v') || (key.code == KeyCode::Insert && key.modifiers.contains(KeyModifiers::SHIFT))
}

/// Разбирает `/later 30 текст` -> (30, "текст").
fn parse_later(text: &str) -> Option<(u32, String)> {
    let rest = text.strip_prefix("/later ")?;
    let mut parts = rest.trim_start().splitn(2, char::is_whitespace);
    let minutes: u32 = parts.next()?.parse().ok()?;
    if minutes == 0 || minutes > 60 * 24 * 365 {
        return None;
    }
    let body = parts.next().unwrap_or("").trim().to_string();
    Some((minutes, body))
}

/// Разбирает `/send "C:\мои файлы\a.pdf" подпись` -> (путь, подпись).
/// Путь можно взять в кавычки (так его вставляет перетаскивание файла в окно).
fn parse_send(arg: &str) -> Option<(String, String)> {
    let a = arg.trim();
    if a.is_empty() {
        return None;
    }
    for q in ['"', '\''] {
        if let Some(rest) = a.strip_prefix(q) {
            let end = rest.find(q)?;
            return Some((rest[..end].to_string(), rest[end + 1..].trim().to_string()));
        }
    }
    // Путь без кавычек: весь текст может быть именем файла с пробелами.
    if std::path::Path::new(a).is_file() {
        return Some((a.to_string(), String::new()));
    }
    match a.split_once(char::is_whitespace) {
        Some((p, c)) => Some((p.to_string(), c.trim().to_string())),
        None => Some((a.to_string(), String::new())),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = display_width(&ch.to_string());
        if w + cw + 1 > max {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

fn name_color(name: &str) -> Color {
    let h = name.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
    NAME_COLORS[(h as usize) % NAME_COLORS.len()]
}

/// Перенос по словам с учётом символов `\n` и ширины кириллицы.
fn wrap_lines(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        if raw.is_empty() {
            out.push(String::new());
            continue;
        }
        for l in textwrap::wrap(raw, width) {
            out.push(l.into_owned());
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

struct InputLayout {
    lines: Vec<String>,
    cur_row: usize,
    cur_col: usize,
}

/// Жёсткий перенос поля ввода по ширине. Так положение курсора считается точно.
fn layout_input(text: &str, cursor: usize, width: usize) -> InputLayout {
    let width = width.max(2);
    let mut lines = vec![String::new()];
    let mut col = 0usize;
    let (mut cur_row, mut cur_col) = (0usize, 0usize);
    let total = text.chars().count();
    for (i, ch) in text.chars().enumerate() {
        if ch == '\n' {
            if i == cursor {
                cur_row = lines.len() - 1;
                cur_col = col;
            }
            lines.push(String::new());
            col = 0;
            continue;
        }
        let w = display_width(&ch.to_string());
        if col + w > width {
            lines.push(String::new());
            col = 0;
        }
        if i == cursor {
            cur_row = lines.len() - 1;
            cur_col = col;
        }
        lines.last_mut().unwrap().push(ch);
        col += w;
    }
    if cursor >= total {
        if col >= width {
            lines.push(String::new());
            col = 0;
        }
        cur_row = lines.len() - 1;
        cur_col = col;
    }
    InputLayout { lines, cur_row, cur_col }
}

fn months() -> [&'static str; 12] {
    if crate::i18n::en() {
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"]
    } else {
        ["января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря"]
    }
}

fn local_date(ts: i64, tz: &jiff::tz::TimeZone) -> Option<(i16, i8, i8)> {
    let t = jiff::Timestamp::from_second(ts).ok()?;
    let d = t.to_zoned(tz.clone()).date();
    Some((d.year(), d.month(), d.day()))
}

fn day_label(date: (i16, i8, i8), today: Option<(i16, i8, i8)>) -> String {
    let (y, m, d) = date;
    let month = months().get((m as usize).wrapping_sub(1)).copied().unwrap_or("");
    match today {
        Some((ty, tm, td)) if (ty, tm, td) == (y, m, d) => tr!("Сегодня", "Today").to_string(),
        Some((ty, _, _)) if ty == y => trf!("{d} {month}", "{month} {d}"),
        _ => trf!("{d} {month} {y}", "{month} {d}, {y}"),
    }
}

/// Превращает историю в строки для вывода: даты-разделители, свои сообщения справа, ответы и пересылки.
/// Возвращает строки и диапазон строк каждого сообщения (для выделения в режиме выбора).
fn build_message_lines(
    msgs: &[ChatMsg],
    width: usize,
    bubble_pct: usize,
    selected: Option<usize>,
    sel_bg: Color,
) -> (Vec<Line<'static>>, Vec<(usize, usize)>) {
    let tz = jiff::tz::TimeZone::system();
    let today = local_date(jiff::Timestamp::now().as_second(), &tz);
    let bubble = (width * bubble_pct / 100).clamp(16, width.saturating_sub(4).max(16));
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut ranges: Vec<(usize, usize)> = Vec::with_capacity(msgs.len());
    let mut prev_day: Option<(i16, i8, i8)> = None;

    for (idx, m) in msgs.iter().enumerate() {
        let day = local_date(m.ts, &tz);
        if let Some(d) = day
            && Some(d) != prev_day
        {
            let label = format!("── {} ──", day_label(d, today));
            lines.push(Line::from(Span::styled(label, Style::new().fg(MUTED))).centered());
            prev_day = day;
        }
        let first = lines.len();
        let time = if m.edited { trf!("{} (изм.)", "{} (edited)", m.time) } else { m.time.clone() };
        let note = |text: String| Span::styled(text, Style::new().fg(MUTED).add_modifier(Modifier::ITALIC));
        let inner = bubble.saturating_sub(2);

        // Пометки: «переслано» и «в ответ на …».
        let mut notes: Vec<String> = Vec::new();
        if let Some(f) = &m.fwd {
            notes.push(match &f.name {
                Some(n) if !n.is_empty() => trf!("↪ переслано от {n}", "↪ forwarded from {n}"),
                _ => tr!("↪ переслано", "↪ forwarded").to_string(),
            });
        }
        if let Some(rid) = m.reply_to {
            let quoted = msgs.iter().find(|x| x.id == rid);
            notes.push(match quoted {
                Some(q) => {
                    let who = if q.outgoing { tr!("Я", "Me") } else { q.sender.as_str() };
                    format!("┌ {who}: {}", truncate(&q.text.replace('\n', " "), inner.saturating_sub(display_width(who) + 4)))
                }
                None => tr!("┌ ответ на сообщение", "┌ reply to a message").to_string(),
            });
        }

        if m.outgoing {
            lines.push(
                Line::from(vec![
                    Span::styled(format!("{time} "), Style::new().fg(MUTED)),
                    Span::styled(tr!("Я", "Me"), Style::new().fg(OWN).add_modifier(Modifier::BOLD)),
                ])
                .right_aligned(),
            );
            for n in &notes {
                lines.push(Line::from(vec![note(truncate(n, inner)), Span::styled(" │", Style::new().fg(OWN))]).right_aligned());
            }
            for l in wrap_lines(&m.text, inner) {
                lines.push(Line::from(vec![Span::raw(l), Span::styled(" │", Style::new().fg(OWN))]).right_aligned());
            }
        } else {
            let color = name_color(&m.sender);
            let mut head = Vec::new();
            if m.mentioned {
                head.push(Span::styled("@ ", Style::new().fg(MENTION).add_modifier(Modifier::BOLD)));
            }
            head.push(Span::styled(m.sender.clone(), Style::new().fg(color).add_modifier(Modifier::BOLD)));
            head.push(Span::styled(format!(" {time}"), Style::new().fg(MUTED)));
            lines.push(Line::from(head));
            for n in &notes {
                lines.push(Line::from(vec![Span::styled("│ ", Style::new().fg(color)), note(truncate(n, inner))]));
            }
            for l in wrap_lines(&m.text, inner) {
                lines.push(Line::from(vec![Span::styled("│ ", Style::new().fg(color)), Span::raw(l)]));
            }
        }
        if selected == Some(idx) {
            for l in &mut lines[first..] {
                l.style = l.style.bg(sel_bg);
            }
        }
        ranges.push((first, lines.len()));
        lines.push(Line::from(""));
    }
    (lines, ranges)
}

/// Текст переписки для сохранения в файл: «дата время имя: текст».
fn export_text(chat_name: &str, msgs: &[ChatMsg]) -> String {
    let tz = jiff::tz::TimeZone::system();
    let mut out = trf!("Переписка: {chat_name}\nСохранено Tg-Lite {VERSION}\n\n", "Chat: {chat_name}\nSaved by Tg-Lite {VERSION}\n\n");
    for m in msgs {
        let when =
            jiff::Timestamp::from_second(m.ts).map(|t| t.to_zoned(tz.clone()).strftime("%d.%m.%Y %H:%M").to_string()).unwrap_or_default();
        out.push_str(&format!("[{when}] {}: {}\n", m.sender, m.text.replace('\n', "\n    ")));
    }
    out
}

fn beep() {
    crate::util::play_alert();
}

fn when_label(minutes: u32) -> String {
    let tz = jiff::tz::TimeZone::system();
    let ts = jiff::Timestamp::now().as_second() + i64::from(minutes) * 60;
    match jiff::Timestamp::from_second(ts) {
        Ok(t) => t.to_zoned(tz).strftime(tr!("%d.%m в %H:%M", "%d.%m at %H:%M")).to_string(),
        Err(_) => trf!("через {minutes} мин.", "in {minutes} min"),
    }
}

// ------------------------------------------------------------------ состояние

impl App {
    fn new(settings: Settings) -> Self {
        App {
            chats: Vec::new(),
            sel: None,
            list_state: ListState::default(),
            tab: Tab::All,
            folders: Vec::new(),
            histories: HashMap::new(),
            input: String::new(),
            cursor: 0,
            status: tr!("Загружаю чаты…", "Loading chats…").into(),
            markdown: false,
            scroll: 0,
            mode: Mode::Chat,
            lock_input: String::new(),
            lock_error: String::new(),
            theme: theme(settings.accent),
            settings,
            settings_row: 0,
            pw_stage: 0,
            pw_first: String::new(),
            last_activity: Instant::now(),
            last_key_at: Instant::now() - Duration::from_secs(10),
            pending_load: None,
            reload_pending: false,
            should_quit: false,
            filter: String::new(),
            drafts: HashMap::new(),
            pending_delete: None,
            pick: 0,
            reply: None,
            edit_target: None,
            fwd_src: None,
            wrong_attempts: 0,
            started: Instant::now(),
            pick_note: String::new(),
            typing: HashMap::new(),
            pinned_msg: HashMap::new(),
            overlay: None,
        }
    }

    fn selected(&self) -> Option<&ChatInfo> {
        let id = self.sel?;
        self.chats.iter().find(|c| c.id == id)
    }

    // --- вкладки и список

    fn in_tab(&self, tab: &Tab, c: &ChatInfo) -> bool {
        match tab {
            Tab::All => true,
            Tab::Unread => c.unread > 0,
            Tab::Users => c.kind == Kind::User,
            Tab::Groups => c.kind == Kind::Group,
            Tab::Channels => c.kind == Kind::Channel,
            Tab::Folder(id) => self.folders.iter().find(|f| f.id == *id).is_some_and(|f| f.matches(c)),
        }
    }

    fn tabs(&self) -> Vec<(Tab, String)> {
        let mut v = vec![
            (Tab::All, tr!("Все", "All").to_string()),
            (Tab::Unread, tr!("Новые", "New").to_string()),
            (Tab::Users, tr!("Личные", "Users").to_string()),
            (Tab::Groups, tr!("Группы", "Groups").to_string()),
            (Tab::Channels, tr!("Каналы", "Channels").to_string()),
        ];
        for f in &self.folders {
            v.push((Tab::Folder(f.id), f.title.clone()));
        }
        v
    }

    fn unread_in(&self, tab: &Tab) -> usize {
        self.chats.iter().filter(|c| c.unread > 0 && self.in_tab(tab, c)).count()
    }

    fn is_pinned(&self, c: &ChatInfo) -> bool {
        c.id.bot_api_dialog_id().is_some_and(|d| self.settings.pinned.contains(&d))
    }

    /// Индексы чатов, видимых во вкладке: с учётом поиска, закреплённые сверху.
    fn visible_idx(&self) -> Vec<usize> {
        let q = self.filter.to_lowercase();
        let mut v: Vec<usize> = (0..self.chats.len())
            .filter(|&i| {
                let c = &self.chats[i];
                self.in_tab(&self.tab, c) && (q.is_empty() || c.name.to_lowercase().contains(&q))
            })
            .collect();
        v.sort_by_key(|&i| !self.is_pinned(&self.chats[i])); // сортировка устойчивая
        v
    }

    fn visible_ids(&self) -> Vec<PeerId> {
        self.visible_idx().into_iter().map(|i| self.chats[i].id).collect()
    }

    /// Подставляет имена отправителей пересланных сообщений из списка чатов.
    fn resolve_fwd(&mut self, chat: PeerId) {
        let Some(h) = self.histories.get_mut(&chat) else { return };
        for m in h.iter_mut() {
            if let Some(f) = m.fwd.as_mut()
                && f.name.is_none()
                && let Some(c) = f.from.and_then(|from| self.chats.iter().find(|c| c.id == from))
            {
                f.name = Some(c.name.clone());
            }
        }
    }

    /// Меняет выбранный чат; набранный текст остаётся черновиком старого чата.
    fn set_sel(&mut self, new: Option<PeerId>) {
        if new == self.sel {
            return;
        }
        let had_old = self.sel.is_some();
        if self.edit_target.take().is_some() {
            self.input.clear(); // текст правки не должен стать черновиком нового сообщения
            self.cursor = 0;
        }
        self.reply = None;
        if let Some(old) = self.sel {
            if self.input.is_empty() {
                self.drafts.remove(&old);
            } else {
                self.drafts.insert(old, (std::mem::take(&mut self.input), self.cursor));
            }
        }
        match new.and_then(|n| self.drafts.remove(&n)) {
            Some((text, cur)) => {
                self.cursor = cur.min(text.chars().count());
                self.input = text;
            }
            None if had_old => self.cursor = 0, // текст ушёл в черновик, поле пустое
            None => {}                          // выбора не было: набранный текст не трогаем
        }
        self.sel = new;
        self.pending_delete = None;
    }

    /// Следит, чтобы выбранный чат был виден в текущей вкладке. Возвращает true, если выбор сменился.
    fn ensure_selection(&mut self) -> bool {
        let vis = self.visible_ids();
        if let Some(s) = self.sel
            && vis.contains(&s)
        {
            return false;
        }
        self.set_sel(vis.first().copied());
        self.scroll = 0;
        self.pending_load = self.sel.map(|_| Instant::now());
        true
    }

    fn move_selection(&mut self, delta: i32) {
        let vis = self.visible_ids();
        if vis.is_empty() {
            return;
        }
        let pos = self.sel.and_then(|s| vis.iter().position(|x| *x == s)).unwrap_or(0) as i32;
        let np = (pos + delta).clamp(0, vis.len() as i32 - 1) as usize;
        if self.sel != Some(vis[np]) {
            self.set_sel(Some(vis[np]));
            self.scroll = 0;
            self.pending_load = Some(Instant::now());
        }
    }

    fn switch_tab(&mut self, delta: i32) {
        let tabs = self.tabs();
        let pos = tabs.iter().position(|(t, _)| *t == self.tab).unwrap_or(0) as i32;
        let np = (pos + delta).rem_euclid(tabs.len() as i32) as usize;
        self.tab = tabs[np].0.clone();
        self.ensure_selection();
    }

    fn total_unread(&self) -> i32 {
        self.chats.iter().map(|c| c.unread.max(0)).sum()
    }

    fn title(&self) -> String {
        let n = self.total_unread();
        if self.mode == Mode::Locked || n == 0 { "Tg-Lite".to_string() } else { format!("Tg-Lite ({n})") }
    }

    fn lock(&mut self) {
        self.mode = Mode::Locked;
        self.lock_input.clear();
        self.lock_error.clear();
        self.pw_first.clear();
        self.pw_stage = 0;
        self.fwd_src = None;
        self.overlay = None;
    }

    fn should_autolock(&self) -> bool {
        self.mode != Mode::Locked
            && self.settings.lock_minutes > 0
            && self.last_activity.elapsed() > Duration::from_secs(u64::from(self.settings.lock_minutes) * 60)
    }

    // --- поле ввода

    fn byte_idx(&self, char_idx: usize) -> usize {
        self.input.char_indices().nth(char_idx).map(|(b, _)| b).unwrap_or(self.input.len())
    }

    /// Вставка текста (из буфера обмена или из события терминала) в зависимости от экрана.
    fn paste(&mut self, text: Option<String>) {
        let Some(text) = text.or_else(crate::util::clipboard_text) else {
            self.status = tr!("Буфер обмена пуст или недоступен", "Clipboard is empty or unavailable").into();
            return;
        };
        match self.mode {
            Mode::Chat => {
                let room = MAX_MESSAGE_CHARS.saturating_sub(self.input_len());
                let clean = crate::util::paste_clean(&text, room);
                if clean.is_empty() {
                    self.status = tr!("Нечего вставлять", "Nothing to paste").into();
                    return;
                }
                for c in clean.chars() {
                    self.insert_char(c);
                }
            }
            Mode::Locked | Mode::NewPassword => self.lock_input.push_str(&crate::util::paste_line(&text)),
            _ => {}
        }
    }

    fn input_len(&self) -> usize {
        self.input.chars().count()
    }

    fn insert_char(&mut self, c: char) {
        if !crate::util::is_safe_char(c) && c != '\n' {
            return; // эмодзи и невидимые символы консоль не рисует; в сообщение они не попадут
        }
        let b = self.byte_idx(self.cursor);
        self.input.insert(b, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            let b = self.byte_idx(self.cursor - 1);
            self.input.remove(b);
            self.cursor -= 1;
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.input_len() {
            let b = self.byte_idx(self.cursor);
            self.input.remove(b);
        }
    }

    fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
        self.edit_target = None;
    }

    fn toggle_markdown(&mut self) {
        self.markdown = !self.markdown;
        self.status = format!("Markdown: {}", if self.markdown { tr!("включён", "on") } else { tr!("выключен", "off") });
    }

    // --- сообщения

    /// Добавляет сообщение в историю, если его там ещё нет (Telegram присылает «эхо» своих сообщений).
    fn push_unique(&mut self, chat: PeerId, msg: ChatMsg, create: bool) -> bool {
        let h = if create {
            self.histories.entry(chat).or_default()
        } else {
            match self.histories.get_mut(&chat) {
                Some(h) => h,
                None => return false,
            }
        };
        if h.iter().any(|m| m.id == msg.id) {
            return false;
        }
        h.push(msg);
        if h.len() > KEEP_MESSAGES {
            h.remove(0);
        }
        true
    }

    fn handle_event(&mut self, ev: Event, cmd: &mpsc::UnboundedSender<Cmd>) {
        match ev {
            Event::Dialogs(list) => {
                self.chats = list;
                let keys: Vec<PeerId> = self.histories.keys().copied().collect();
                for k in keys {
                    self.resolve_fwd(k);
                }
                self.reload_pending = false;
                self.ensure_selection();
                self.status = trf!("Чатов: {}", "Chats: {}", self.chats.len());
                // Историю подгружаем, только если её ещё нет: иначе каждое обновление списка было бы запросом.
                if let Some(id) = self.sel
                    && !self.histories.contains_key(&id)
                    && self.pending_load.is_none()
                {
                    self.pending_load = Some(Instant::now());
                }
            }
            Event::Folders(list) => {
                self.folders = list;
                if let Tab::Folder(id) = self.tab
                    && !self.folders.iter().any(|f| f.id == id)
                {
                    self.tab = Tab::All;
                }
                self.ensure_selection();
            }
            Event::History { chat, msgs } => {
                self.histories.insert(chat, msgs);
                self.resolve_fwd(chat);
                if self.sel == Some(chat) {
                    self.scroll = 0;
                    if let Some(pos) = self.chats.iter().position(|c| c.id == chat)
                        && self.chats[pos].unread > 0
                        && self.mode != Mode::Locked
                    {
                        let _ = cmd.send(Cmd::MarkRead(self.chats[pos].clone()));
                        self.chats[pos].unread = 0;
                    }
                }
            }
            Event::Incoming { chat, chat_name, msg } => {
                let is_open = self.sel == Some(chat);
                let outgoing = msg.outgoing;
                if !outgoing {
                    self.typing.remove(&chat);
                }
                // «Новое» = такого сообщения ещё не было. Если история чата не загружена, оно новое тоже:
                // иначе счётчик непрочитанных не рос бы для всех чатов, кроме открытых.
                let known = self.histories.get(&chat).is_some_and(|h| h.iter().any(|m| m.id == msg.id));
                self.push_unique(chat, msg, false);
                self.resolve_fwd(chat);
                let added = !known;
                if let Some(pos) = self.chats.iter().position(|c| c.id == chat) {
                    if added && !outgoing {
                        if is_open && self.mode != Mode::Locked {
                            let _ = cmd.send(Cmd::MarkRead(self.chats[pos].clone()));
                        } else {
                            self.chats[pos].unread += 1;
                        }
                        if self.settings.sound && !(is_open && self.mode == Mode::Chat) {
                            beep();
                        }
                        self.status = if self.mode == Mode::Locked {
                            tr!("Новое сообщение", "New message").into()
                        } else {
                            trf!("Новое сообщение: {chat_name}", "New message: {chat_name}")
                        };
                    }
                    if added {
                        // Чат с новым сообщением поднимается наверх списка.
                        let c = self.chats.remove(pos);
                        self.chats.insert(0, c);
                    }
                } else if !self.reload_pending {
                    // Чата ещё нет в списке (новый диалог): перезагрузим список.
                    self.reload_pending = true;
                    let _ = cmd.send(Cmd::LoadDialogs);
                }
                if added && is_open {
                    self.scroll = 0;
                }
            }
            Event::Sent { chat, msg, scheduled } => {
                if scheduled {
                    self.status = tr!("Отложенное сообщение запланировано", "Scheduled message set").into();
                } else {
                    self.status = tr!("Отправлено", "Sent").into();
                    // Если история чата ещё не загружена, сообщение подтянется вместе с ней.
                    self.push_unique(chat, msg, false);
                    if self.sel == Some(chat) {
                        self.scroll = 0;
                    }
                }
            }
            Event::SendFailed { text } => {
                // Возвращаем текст в поле ввода, чтобы длинный пост не пропал.
                if self.input.is_empty() {
                    self.input = text;
                    self.cursor = self.input_len();
                }
            }
            Event::Edited { chat, msg } => {
                if let Some(h) = self.histories.get_mut(&chat)
                    && let Some(m) = h.iter_mut().find(|m| m.id == msg.id)
                {
                    match (&m.caption, &msg.caption) {
                        // Правка нашего текста у сообщения с вложением: подпись вложения остаётся.
                        (Some(old), None) => {
                            let label = m.text.strip_suffix(old.as_str()).unwrap_or("").trim_end().to_string();
                            m.text = tg::compose_text(&label, &msg.text);
                            m.caption = Some(msg.text.clone());
                        }
                        _ => {
                            m.text = msg.text;
                            m.caption = msg.caption;
                        }
                    }
                    m.edited = true;
                }
            }
            Event::Deleted { chat, ids } => {
                for (id, h) in self.histories.iter_mut() {
                    if chat.is_none_or(|c| c == *id) {
                        h.retain(|m| !ids.contains(&m.id));
                    }
                }
            }
            Event::Presence { user, presence } => {
                if let Some(c) = self.chats.iter_mut().find(|c| c.id == user) {
                    c.presence = Some(presence);
                }
            }
            Event::Typing { chat, from, kind } => {
                if kind == tg::TypingKind::Stop {
                    self.typing.remove(&chat);
                } else {
                    let who = from.and_then(|f| self.chats.iter().find(|c| c.id == f)).map(|c| c.name.clone());
                    let label = match (chat.kind(), who) {
                        (grammers_client::session::types::PeerKind::User, _) => kind.label().to_string(),
                        (_, Some(n)) => format!("{n} {}", kind.label()),
                        (_, None) => trf!("кто-то {}", "someone {}", kind.label()),
                    };
                    self.typing.insert(chat, (Instant::now(), label));
                }
            }
            Event::Pinned { chat, text } => match text {
                Some(t) => {
                    self.pinned_msg.insert(chat, t);
                }
                None => {
                    self.pinned_msg.remove(&chat);
                }
            },
            Event::Scheduled { chat, items } => {
                if self.sel != Some(chat) {
                    return;
                }
                if items.is_empty() {
                    self.status = tr!("Отложенных сообщений в этом чате нет", "No scheduled messages in this chat").into();
                    return;
                }
                let tz = jiff::tz::TimeZone::system();
                let list: Vec<OverlayItem> = items
                    .iter()
                    .map(|m| {
                        let when = jiff::Timestamp::from_second(m.date)
                            .map(|t| t.to_zoned(tz.clone()).strftime("%d.%m %H:%M").to_string())
                            .unwrap_or_default();
                        OverlayItem { label: format!("{when}  {}", m.text.replace('\n', " ")), data: String::new(), id: m.id }
                    })
                    .collect();
                self.overlay = Some(Overlay {
                    kind: OverlayKind::Scheduled,
                    chat: self.selected().cloned(),
                    title: tr!(" Отложенные сообщения ", " Scheduled messages ").into(),
                    items: list,
                    sel: 0,
                    confirm: None,
                });
                self.mode = Mode::Overlay;
            }
            Event::Status(s) => self.status = s,
            Event::Error(e) => self.status = trf!("Ошибка: {e}", "Error: {e}"),
        }
    }

    // --- настройки

    fn apply_settings(&mut self, saver: &Arc<Saver>) {
        self.theme = theme(self.settings.accent);
        saver.set_settings(self.settings.clone());
        saver.save_in_background();
    }

    fn change_setting(&mut self, dir: i32, activate: bool, saver: &Arc<Saver>) {
        let s = &mut self.settings;
        match self.settings_row {
            0 => s.lock_minutes = cycle(&LOCK_CHOICES, s.lock_minutes, dir),
            1 => s.proxy_mode = cycle(&[ProxyMode::Auto, ProxyMode::Off, ProxyMode::Manual], s.proxy_mode, dir),
            2 => {
                s.sound = !s.sound;
                if s.sound {
                    beep(); // сразу слышно, работает ли звук
                }
            }
            3 => s.show_status = !s.show_status,
            4 => s.history_limit = cycle(&HISTORY_CHOICES, s.history_limit, dir),
            5 => s.accent = cycle(&[0u8, 1, 2, 3, 4], s.accent, dir),
            6 => s.wipe_after = cycle(&WIPE_CHOICES, s.wipe_after, dir),
            7 => s.lock_bg = cycle(&[0u8, 1, 2, 3], s.lock_bg, dir),
            8 => s.list_width = cycle(&[0u8, 1, 2], s.list_width, dir),
            9 => s.bubble_width = cycle(&[0u8, 1, 2], s.bubble_width, dir),
            10 => {
                s.lang = cycle(&[0u8, 1], s.lang, dir);
                crate::i18n::set(s.lang);
                crate::i18n::save();
                self.status = tr!("Язык: русский", "Language: English").into();
            }
            _ => {
                if activate {
                    self.mode = Mode::NewPassword;
                    self.pw_stage = 0;
                    self.pw_first.clear();
                    self.lock_input.clear();
                    self.lock_error.clear();
                }
                return;
            }
        }
        self.apply_settings(saver);
    }

    fn handle_settings_key(&mut self, key: KeyEvent, saver: &Arc<Saver>) {
        match key.code {
            KeyCode::Esc | KeyCode::F(4) => self.mode = Mode::Chat,
            KeyCode::F(10) => self.should_quit = true,
            KeyCode::Up => self.settings_row = (self.settings_row + SETTINGS_ROWS - 1) % SETTINGS_ROWS,
            KeyCode::Down | KeyCode::Tab => self.settings_row = (self.settings_row + 1) % SETTINGS_ROWS,
            KeyCode::Left => self.change_setting(-1, false, saver),
            KeyCode::Right => self.change_setting(1, false, saver),
            KeyCode::Enter | KeyCode::Char(' ') => self.change_setting(1, true, saver),
            _ => {}
        }
    }

    fn handle_new_password_key(&mut self, key: KeyEvent, saver: &Arc<Saver>) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Settings;
                self.lock_input.clear();
            }
            KeyCode::Backspace => {
                self.lock_input.pop();
            }
            KeyCode::Enter => {
                if self.lock_input.is_empty() {
                    self.lock_error = tr!("Пароль не может быть пустым", "Password cannot be empty").into();
                } else if self.pw_stage == 0 {
                    self.pw_first = std::mem::take(&mut self.lock_input);
                    self.pw_stage = 1;
                    self.lock_error.clear();
                } else if self.lock_input == self.pw_first {
                    saver.set_password(std::mem::take(&mut self.pw_first));
                    saver.save_in_background();
                    self.lock_input.clear();
                    self.mode = Mode::Settings;
                    self.status = tr!("Пароль изменён", "Password changed").into();
                } else {
                    self.lock_error = tr!("Пароли не совпали, введите заново", "Passwords do not match, try again").into();
                    self.lock_input.clear();
                    self.pw_first.clear();
                    self.pw_stage = 0;
                }
            }
            KeyCode::Char(c) if ctrl_letter(&key).is_none() => self.lock_input.push(c),
            _ => {}
        }
    }

    // --- клавиши

    fn handle_key(&mut self, key: KeyEvent, cmd: &mpsc::UnboundedSender<Cmd>, saver: &Arc<Saver>) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        let since_prev = self.last_key_at.elapsed();
        self.last_key_at = Instant::now();
        self.last_activity = Instant::now();

        if is_paste_key(&key) && matches!(self.mode, Mode::Chat | Mode::Locked | Mode::NewPassword) {
            self.paste(None);
            return;
        }

        match self.mode {
            Mode::Locked => {
                match key.code {
                    KeyCode::F(10) => self.should_quit = true,
                    KeyCode::Enter => {
                        if self.lock_input == saver.password() {
                            self.mode = Mode::Chat;
                            self.wrong_attempts = 0;
                            self.lock_input.clear();
                            // Всё, что пришло за время блокировки в открытый чат, теперь прочитано.
                            if let Some(c) = self.selected().cloned()
                                && c.unread > 0
                            {
                                let _ = cmd.send(Cmd::MarkRead(c));
                                if let Some(c) = self.chats.iter_mut().find(|c| Some(c.id) == self.sel) {
                                    c.unread = 0;
                                }
                            }
                        } else {
                            self.wrong_attempts += 1;
                            self.lock_input.clear();
                            let limit = self.settings.wipe_after;
                            if limit > 0 && self.wrong_attempts >= limit {
                                saver.wipe();
                                self.should_quit = true;
                            } else if limit > 0 {
                                self.lock_error = trf!(
                                    "Неверный пароль. Осталось попыток до удаления данных: {}",
                                    "Wrong password. Attempts left before data is wiped: {}",
                                    limit - self.wrong_attempts
                                );
                            } else {
                                self.lock_error = tr!("Неверный пароль", "Wrong password").into();
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        self.lock_input.pop();
                    }
                    KeyCode::Esc => self.lock_input.clear(),
                    KeyCode::Char(c) if ctrl_letter(&key).is_none() => self.lock_input.push(c),
                    _ => {}
                }
                return;
            }
            Mode::Help => {
                self.mode = Mode::Chat;
                return;
            }
            Mode::Settings => {
                self.handle_settings_key(key, saver);
                return;
            }
            Mode::NewPassword => {
                self.handle_new_password_key(key, saver);
                return;
            }
            Mode::Overlay => {
                self.handle_overlay_key(key, cmd);
                return;
            }
            Mode::Pick => {
                self.handle_pick_key(key, cmd);
                return;
            }
            Mode::Forward => {
                self.handle_forward_key(key, cmd);
                return;
            }
            Mode::Chat => {}
        }

        // Ctrl-комбинации (запасной вариант; работают и в русской раскладке).
        if let Some(c) = ctrl_letter(&key) {
            match c {
                'q' => self.should_quit = true,
                'l' => self.lock(),
                'r' => self.refresh(cmd),
                'f' => self.start_find(),
                't' => self.toggle_markdown(),
                'j' => self.insert_char('\n'),
                _ => {}
            }
            return;
        }

        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::F(1) => self.mode = Mode::Help,
            KeyCode::F(2) => self.lock(),
            KeyCode::F(3) => self.toggle_markdown(),
            KeyCode::F(4) => self.mode = Mode::Settings,
            KeyCode::F(5) => self.refresh(cmd),
            KeyCode::F(6) => self.start_find(),
            KeyCode::F(7) => self.start_pick(),
            KeyCode::F(10) => self.should_quit = true,
            // Shift+Enter / Alt+Enter — новая строка. Enter внутри вставленного текста — тоже.
            KeyCode::Enter if shift || alt || since_prev < PASTE_GAP => self.insert_char('\n'),
            KeyCode::Enter => self.submit(cmd, saver),
            // Tab переключает вкладки; Tab внутри вставленного текста — отступ.
            KeyCode::Tab if since_prev < PASTE_GAP => {
                self.insert_char(' ');
                self.insert_char(' ');
            }
            KeyCode::Tab if self.input.starts_with('/') && !self.input.contains(' ') => self.complete_command(),
            KeyCode::Tab => self.switch_tab(1),
            KeyCode::BackTab => self.switch_tab(-1),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input_len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input_len(),
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => self.scroll += 8,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(8),
            KeyCode::Esc => {
                if !self.input.is_empty() {
                    self.clear_input();
                } else if self.edit_target.take().is_some() {
                    self.status = tr!("Правка отменена", "Edit cancelled").into();
                } else if self.reply.take().is_some() {
                    self.status = tr!("Ответ отменён", "Reply cancelled").into();
                } else if !self.filter.is_empty() {
                    self.set_filter("");
                }
            }
            KeyCode::Char(c) => self.insert_char(c),
            _ => {}
        }
    }

    fn refresh(&mut self, cmd: &mpsc::UnboundedSender<Cmd>) {
        self.status = tr!("Обновляю список чатов…", "Refreshing chat list…").into();
        let _ = cmd.send(Cmd::LoadDialogs);
    }

    /// Отправка и команды: `/later <минуты> <текст>`, `/md`, `/lock`, `/settings`, `/passwd`,
    /// `/proxy <адрес|auto|off>`, `/refresh`, `/help`, `/quit`.
    fn submit(&mut self, cmd: &mpsc::UnboundedSender<Cmd>, saver: &Arc<Saver>) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(rest) = text.strip_prefix("/proxy")
            && (rest.is_empty() || rest.starts_with(' '))
        {
            self.clear_input();
            self.proxy_command(rest.trim(), saver);
            return;
        }
        if self.try_command(&text, cmd, saver) {
            return;
        }
        self.pending_delete = None;
        let simple = match text.as_str() {
            "/md" => Some(0),
            "/lock" => Some(1),
            "/quit" | "/exit" => Some(2),
            "/refresh" => Some(3),
            "/help" | "/?" => Some(4),
            "/settings" => Some(5),
            "/passwd" | "/password" => Some(6),
            _ => None,
        };
        if let Some(c) = simple {
            self.clear_input();
            match c {
                0 => self.toggle_markdown(),
                1 => self.lock(),
                2 => self.should_quit = true,
                3 => self.refresh(cmd),
                4 => self.mode = Mode::Help,
                5 => self.mode = Mode::Settings,
                _ => {
                    self.settings_row = SETTINGS_ROWS - 1;
                    self.mode = Mode::Settings;
                    self.change_setting(1, true, saver);
                }
            }
            return;
        }
        let Some(chat) = self.selected().cloned() else {
            self.status = tr!("Сначала выберите чат", "Select a chat first").into();
            return;
        };
        if let Some(id) = self.edit_target {
            let _ = cmd.send(Cmd::Edit { chat, id, text, markdown: self.markdown });
            self.status = tr!("Исправляю сообщение…", "Editing message…").into();
            self.clear_input();
            return;
        }
        let (body, delay) = match parse_later(&text) {
            Some((min, body)) => (body, Some(min)),
            None => (text, None),
        };
        if body.is_empty() {
            self.status = tr!("Пустое сообщение", "Empty message").into();
            return;
        }
        if !self.markdown && body.chars().count() > MAX_MESSAGE_CHARS {
            self.status = trf!(
                "Ошибка: сообщение длиннее {MAX_MESSAGE_CHARS} символов ({}), Telegram его не примет. Сократите текст",
                "Error: message is longer than {MAX_MESSAGE_CHARS} characters ({}), Telegram will not accept it. Shorten the text",
                body.chars().count()
            );
            return;
        }
        self.status = match delay {
            Some(min) => trf!("Планирую на {}…", "Scheduling for {}…", when_label(min)),
            None => tr!("Отправляю…", "Sending…").into(),
        };
        let reply_to = self.reply.take().map(|(id, _)| id);
        let _ = cmd.send(Cmd::Send { chat, text: body, markdown: self.markdown, delay_minutes: delay, reply_to });
        self.clear_input();
    }

    /// Автодополнение команды по Tab: единственная подходящая дописывается целиком, иначе общее начало.
    fn complete_command(&mut self) {
        let part = self.input.clone();
        let matches: Vec<&str> = COMMANDS.iter().copied().filter(|c| c.starts_with(part.as_str())).collect();
        match matches.as_slice() {
            [] => self.status = tr!("Такой команды нет. F1 — справка", "No such command. F1 for help").into(),
            [one] => {
                self.input = format!("{one} ");
                self.cursor = self.input_len();
            }
            many => {
                let first = many[0];
                let mut common = first.len();
                for m in many {
                    common = common.min(first.bytes().zip(m.bytes()).take_while(|(a, b)| a == b).count());
                }
                self.input = first[..common].to_string();
                self.cursor = self.input_len();
                self.status = trf!("Варианты: {}", "Options: {}", many.join(" "));
            }
        }
    }

    // --- список поверх чата (ссылки, отложенные)

    fn open_overlay(&mut self, kind: OverlayKind, title: &str, items: Vec<OverlayItem>) {
        self.overlay = Some(Overlay { kind, chat: self.selected().cloned(), title: title.into(), items, sel: 0, confirm: None });
        self.mode = Mode::Overlay;
    }

    fn handle_overlay_key(&mut self, key: KeyEvent, cmd: &mpsc::UnboundedSender<Cmd>) {
        let Some(ov) = self.overlay.as_mut() else {
            self.mode = Mode::Chat;
            return;
        };
        let len = ov.items.len();
        if len == 0 {
            self.overlay = None;
            self.mode = Mode::Chat;
            return;
        }
        if key.code != KeyCode::Enter {
            ov.confirm = None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::F(10) => {
                if key.code == KeyCode::F(10) {
                    self.should_quit = true;
                }
                self.overlay = None;
                self.mode = Mode::Chat;
            }
            KeyCode::Up => ov.sel = ov.sel.saturating_sub(1),
            KeyCode::Down => ov.sel = (ov.sel + 1).min(len - 1),
            KeyCode::Home => ov.sel = 0,
            KeyCode::End => ov.sel = len - 1,
            KeyCode::Enter => match ov.kind {
                OverlayKind::Links => {
                    let url = ov.items[ov.sel].data.clone();
                    self.status = if util::open_url(&url) {
                        trf!("Открываю в браузере: {url}", "Opening in browser: {url}")
                    } else {
                        tr!("Не удалось открыть ссылку", "Could not open the link").into()
                    };
                }
                OverlayKind::Faq => {}
                OverlayKind::Scheduled => {
                    if ov.confirm == Some(ov.sel) {
                        let item = ov.items.remove(ov.sel);
                        ov.confirm = None;
                        if let Some(chat) = ov.chat.clone() {
                            let _ = cmd.send(Cmd::DeleteScheduled { chat, id: item.id });
                        }
                        self.status = tr!("Отменяю отложенное сообщение…", "Cancelling scheduled message…").into();
                        if ov.items.is_empty() {
                            self.overlay = None;
                            self.mode = Mode::Chat;
                        } else {
                            ov.sel = ov.sel.min(ov.items.len() - 1);
                        }
                    } else {
                        ov.confirm = Some(ov.sel);
                    }
                }
            },
            _ => {}
        }
    }

    /// Закрывает «печатает…» у чатов, от которых давно не было обновлений. Возвращает true, если что-то исчезло.
    fn expire_typing(&mut self) -> bool {
        let before = self.typing.len();
        self.typing.retain(|_, (t, _)| t.elapsed() < TYPING_TTL);
        self.typing.len() != before
    }

    // --- режим выбора сообщения

    fn start_pick(&mut self) {
        match self.sel.and_then(|id| self.histories.get(&id)) {
            Some(h) if !h.is_empty() => {
                self.pick = h.len() - 1;
                self.pick_note.clear();
                self.mode = Mode::Pick;
                self.pending_delete = None;
            }
            _ => self.status = tr!("В этом чате пока нет сообщений", "No messages in this chat yet").into(),
        }
    }

    fn picked(&self) -> Option<&ChatMsg> {
        self.histories.get(&self.sel?)?.get(self.pick)
    }

    fn handle_pick_key(&mut self, key: KeyEvent, cmd: &mpsc::UnboundedSender<Cmd>) {
        let len = self.sel.and_then(|id| self.histories.get(&id)).map_or(0, Vec::len);
        if len == 0 {
            self.mode = Mode::Chat;
            return;
        }
        self.pick = self.pick.min(len - 1);
        self.pick_note.clear();
        // Буквы работают и в русской раскладке (клавиши R, F, E, D = К, А, У, В).
        let letter = match key.code {
            KeyCode::Char(c) => match c.to_lowercase().next().unwrap_or(c) {
                'r' | 'к' => Some('r'),
                'f' | 'а' => Some('f'),
                'e' | 'у' => Some('e'),
                'd' | 'в' => Some('d'),
                's' | 'ы' => Some('s'),
                'l' | 'д' => Some('l'),
                _ => None,
            },
            _ => None,
        };
        if letter != Some('d') {
            self.pending_delete = None;
        }
        match (key.code, letter) {
            (KeyCode::Esc | KeyCode::F(7) | KeyCode::Enter, _) => self.mode = Mode::Chat,
            (KeyCode::F(10), _) => self.should_quit = true,
            (KeyCode::Up, _) => self.pick = self.pick.saturating_sub(1),
            (KeyCode::Down, _) => self.pick = (self.pick + 1).min(len - 1),
            (KeyCode::PageUp, _) => self.pick = self.pick.saturating_sub(5),
            (KeyCode::PageDown, _) => self.pick = (self.pick + 5).min(len - 1),
            (KeyCode::Home, _) => self.pick = 0,
            (KeyCode::End, _) => self.pick = len - 1,
            (_, Some('r')) => {
                if let Some(m) = self.picked().cloned() {
                    let who = if m.outgoing { tr!("Я", "Me").to_string() } else { m.sender.clone() };
                    self.reply = Some((m.id, format!("{who}: {}", truncate(&m.text.replace('\n', " "), 40))));
                    self.mode = Mode::Chat;
                    self.scroll = 0;
                    self.status =
                        tr!("Пишите ответ и нажмите Enter (Esc — отмена)", "Type your reply and press Enter (Esc to cancel)").into();
                }
            }
            (_, Some('e')) => {
                if let Some(m) = self.picked().cloned() {
                    if !m.outgoing {
                        self.status = tr!("Можно править только свои сообщения", "You can only edit your own messages").into();
                        self.pick_note =
                            tr!("Это чужое сообщение: править можно только свои", "Not your message: you can only edit your own").into();
                    } else {
                        self.input = m.caption.clone().unwrap_or_else(|| m.text.clone());
                        self.cursor = self.input_len();
                        self.edit_target = Some(m.id);
                        self.mode = Mode::Chat;
                        self.status = tr!(
                            "Правка: измените текст и нажмите Enter (Esc — отмена)",
                            "Edit: change the text and press Enter (Esc to cancel)"
                        )
                        .into();
                    }
                }
            }
            (_, Some('d')) => {
                let (Some(chat), Some(m)) = (self.selected().cloned(), self.picked().cloned()) else { return };
                if !m.outgoing {
                    self.status = tr!("Можно удалять только свои сообщения", "You can only delete your own messages").into();
                    self.pick_note =
                        tr!("Это чужое сообщение: удалять можно только свои", "Not your message: you can only delete your own").into();
                } else if self.pending_delete == Some((chat.id, m.id)) {
                    self.pending_delete = None;
                    let _ = cmd.send(Cmd::Delete { chat, id: m.id });
                    self.status = tr!("Удаляю…", "Deleting…").into();
                    self.mode = Mode::Chat;
                } else {
                    self.pending_delete = Some((chat.id, m.id));
                    self.status =
                        trf!("Удалить «{}»? Нажмите D ещё раз", "Delete \"{}\"? Press D again", truncate(&m.text.replace('\n', " "), 30));
                    self.pick_note = self.status.clone();
                }
            }
            (_, Some('s')) => {
                if let (Some(chat), Some(m)) = (self.selected().cloned(), self.picked().cloned()) {
                    if m.caption.is_none() {
                        self.pick_note = tr!("В этом сообщении нет вложения", "This message has no attachment").into();
                    } else {
                        let _ = cmd.send(Cmd::Save { chat, id: m.id });
                        self.status = tr!("Скачиваю…", "Downloading…").into();
                        self.mode = Mode::Chat;
                    }
                }
            }
            (_, Some('l')) => {
                if let Some(m) = self.picked().cloned() {
                    let links = util::extract_links(&m.text);
                    if links.is_empty() {
                        self.pick_note = tr!("В этом сообщении нет ссылок", "This message has no links").into();
                    } else {
                        let items = links.into_iter().map(|u| OverlayItem { label: u.clone(), data: u, id: 0 }).collect();
                        self.open_overlay(OverlayKind::Links, tr!(" Ссылки сообщения ", " Message links "), items);
                    }
                }
            }
            (_, Some('f')) => {
                if let (Some(chat), Some(m)) = (self.selected().cloned(), self.picked().cloned()) {
                    self.fwd_src = Some((chat, m.id));
                    self.mode = Mode::Forward;
                    self.status = tr!(
                        "Выберите чат (↑ ↓, Tab) и нажмите Enter, Esc — отмена",
                        "Select a chat (↑ ↓, Tab) and press Enter, Esc to cancel"
                    )
                    .into();
                }
            }
            _ => {}
        }
    }

    fn handle_forward_key(&mut self, key: KeyEvent, cmd: &mpsc::UnboundedSender<Cmd>) {
        let Some((src, id)) = self.fwd_src.clone() else {
            self.mode = Mode::Chat;
            return;
        };
        match key.code {
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::Tab => self.switch_tab(1),
            KeyCode::BackTab => self.switch_tab(-1),
            KeyCode::F(10) => self.should_quit = true,
            KeyCode::Enter => {
                if let Some(dst) = self.selected().cloned() {
                    let _ = cmd.send(Cmd::Forward { src: src.clone(), dst, id });
                    self.status = tr!("Пересылаю…", "Forwarding…").into();
                    self.finish_forward(&src);
                }
            }
            KeyCode::Esc => {
                self.status = tr!("Пересылка отменена", "Forwarding cancelled").into();
                self.finish_forward(&src);
            }
            _ => {}
        }
    }

    /// Выходит из режима пересылки и возвращается в исходный чат.
    fn finish_forward(&mut self, src: &ChatInfo) {
        self.fwd_src = None;
        self.mode = Mode::Chat;
        if !self.visible_ids().contains(&src.id) {
            self.tab = Tab::All;
            self.filter.clear();
        }
        self.set_sel(Some(src.id));
        self.pending_load = Some(Instant::now());
    }

    fn start_find(&mut self) {
        self.input = "/find ".to_string();
        self.cursor = self.input_len();
    }

    fn set_filter(&mut self, q: &str) {
        self.filter = q.trim().to_string();
        self.ensure_selection();
        self.status = if self.filter.is_empty() {
            tr!("Поиск сброшен", "Search cleared").to_string()
        } else {
            trf!("Найдено чатов: {}", "Chats found: {}", self.visible_idx().len())
        };
    }

    /// Последнее своё сообщение в открытом чате.
    fn last_own(&self) -> Option<&ChatMsg> {
        self.histories.get(&self.sel?)?.iter().rev().find(|m| m.outgoing && m.id > 0)
    }

    /// Команды, зависящие от списка чатов и истории. Возвращает true, если `text` — команда.
    fn try_command(&mut self, text: &str, cmd: &mpsc::UnboundedSender<Cmd>, saver: &Arc<Saver>) -> bool {
        let (name, arg) = match text.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a.trim()),
            None => (text, ""),
        };
        match name {
            "/find" | "/search" => {
                self.clear_input();
                self.set_filter(arg);
            }
            "/pin" => {
                self.clear_input();
                let Some(id) = self.selected().and_then(|c| c.id.bot_api_dialog_id()) else {
                    self.status = tr!("Сначала выберите чат", "Select a chat first").into();
                    return true;
                };
                let pinned = &mut self.settings.pinned;
                if let Some(pos) = pinned.iter().position(|x| *x == id) {
                    pinned.remove(pos);
                    self.status = tr!("Чат откреплён", "Chat unpinned").into();
                } else {
                    pinned.push(id);
                    self.status = tr!("Чат закреплён наверху списка", "Chat pinned to the top of the list").into();
                }
                self.apply_settings(saver);
            }
            "/readall" => {
                self.clear_input();
                let mut n = 0;
                for c in self.chats.iter_mut().filter(|c| c.unread > 0) {
                    c.unread = 0;
                    n += 1;
                    let _ = cmd.send(Cmd::MarkRead(c.clone()));
                }
                self.status = trf!("Прочитано чатов: {n}", "Chats marked read: {n}");
            }
            "/export" => {
                self.clear_input();
                let Some(c) = self.selected().cloned() else {
                    self.status = tr!("Сначала выберите чат", "Select a chat first").into();
                    return true;
                };
                match self.histories.get(&c.id) {
                    Some(h) if !h.is_empty() => {
                        self.status = match crate::util::write_export(&c.name, &export_text(&c.name, h)) {
                            Ok(p) => trf!("Сохранено: {}", "Saved: {}", p.display()),
                            Err(e) => trf!("Ошибка: не удалось сохранить файл: {e}", "Error: could not save the file: {e}"),
                        };
                    }
                    _ => self.status = tr!("В этом чате пока нечего сохранять", "Nothing to save in this chat yet").into(),
                }
            }
            "/note" => {
                if arg.is_empty() {
                    self.clear_input();
                    self.status = tr!(
                        "Использование: /note текст — быстрая заметка в «Избранное»",
                        "Usage: /note text — quick note to Saved Messages"
                    )
                    .into();
                    return true;
                }
                let Some(me) = self.chats.iter().find(|c| c.is_self).cloned() else {
                    self.clear_input();
                    self.status = tr!("Чат «Избранное» ещё не загружен", "Saved Messages chat is not loaded yet").into();
                    return true;
                };
                let _ =
                    cmd.send(Cmd::Send { chat: me, text: arg.to_string(), markdown: self.markdown, delay_minutes: None, reply_to: None });
                self.status = tr!("Сохраняю в «Избранное»…", "Saving to Saved Messages…").into();
                self.clear_input();
            }
            "/edit" => {
                let Some(chat) = self.selected().cloned() else {
                    self.clear_input();
                    self.status = tr!("Сначала выберите чат", "Select a chat first").into();
                    return true;
                };
                let Some(last) = self.last_own().cloned() else {
                    self.clear_input();
                    self.status = tr!("Нет вашего сообщения, которое можно исправить", "You have no message that can be edited").into();
                    return true;
                };
                if arg.is_empty() {
                    // Без текста: подставляем старый текст, чтобы его можно было поправить.
                    self.input = format!("/edit {}", last.text);
                    self.cursor = self.input_len();
                } else {
                    let _ = cmd.send(Cmd::Edit { chat, id: last.id, text: arg.to_string(), markdown: self.markdown });
                    self.status = tr!("Исправляю сообщение…", "Editing message…").into();
                    self.clear_input();
                }
            }
            "/del" | "/delete" => {
                self.clear_input();
                let (Some(chat), Some(last)) = (self.selected().cloned(), self.last_own().cloned()) else {
                    self.status = tr!("Нет вашего сообщения, которое можно удалить", "You have no message that can be deleted").into();
                    return true;
                };
                if self.pending_delete == Some((chat.id, last.id)) {
                    self.pending_delete = None;
                    let _ = cmd.send(Cmd::Delete { chat, id: last.id });
                    self.status = tr!("Удаляю…", "Deleting…").into();
                } else {
                    self.pending_delete = Some((chat.id, last.id));
                    self.status = trf!(
                        "Удалить «{}»? Повторите /del для подтверждения",
                        "Delete \"{}\"? Repeat /del to confirm",
                        truncate(&last.text.replace('\n', " "), 30)
                    );
                }
                return true; // pending_delete сохраняется до следующей другой команды
            }
            "/save" => {
                self.clear_input();
                let (Some(chat), Some(m)) = (
                    self.selected().cloned(),
                    self.sel.and_then(|id| self.histories.get(&id)).and_then(|h| h.iter().rev().find(|m| m.caption.is_some())).cloned(),
                ) else {
                    self.status = tr!("В загруженных сообщениях нет вложений", "No attachments in the loaded messages").into();
                    return true;
                };
                let _ = cmd.send(Cmd::Save { chat, id: m.id });
                self.status = tr!("Скачиваю последнее вложение…", "Downloading the latest attachment…").into();
            }
            "/send" => {
                let Some(chat) = self.selected().cloned() else {
                    self.clear_input();
                    self.status = tr!("Сначала выберите чат", "Select a chat first").into();
                    return true;
                };
                match parse_send(arg) {
                    Some((path, caption)) if std::path::Path::new(&path).is_file() => {
                        let _ = cmd.send(Cmd::SendFile { chat, path: path.into(), caption });
                        self.status = tr!("Отправляю файл…", "Sending file…").into();
                        self.clear_input();
                    }
                    Some((path, _)) => {
                        self.status = trf!("Ошибка: файл не найден: {path}", "Error: file not found: {path}");
                    }
                    None => {
                        self.status = tr!(
                            "Использование: /send путь [подпись]. Можно перетащить файл в окно",
                            "Usage: /send path [caption]. You can also drag a file into the window"
                        )
                        .into();
                    }
                }
                return true; // при ошибке текст остаётся в поле ввода
            }
            "/links" => {
                self.clear_input();
                let Some(h) = self.sel.and_then(|id| self.histories.get(&id)) else {
                    self.status = tr!("Сначала откройте чат", "Open a chat first").into();
                    return true;
                };
                let mut items: Vec<OverlayItem> = Vec::new();
                for m in h.iter().rev().take(80) {
                    for u in util::extract_links(&m.text) {
                        if !items.iter().any(|i| i.data == u) {
                            items.push(OverlayItem { label: format!("{}: {u}", m.sender), data: u, id: 0 });
                        }
                    }
                }
                if items.is_empty() {
                    self.status = tr!("В загруженных сообщениях нет ссылок", "No links in the loaded messages").into();
                } else {
                    self.open_overlay(OverlayKind::Links, tr!(" Ссылки чата ", " Chat links "), items);
                }
            }
            "/paste" => {
                self.clear_input();
                self.paste(None);
            }
            "/faq" => {
                self.clear_input();
                let items = faq().iter().map(|(q, a)| OverlayItem { label: format!("{q} — {a}"), data: String::new(), id: 0 }).collect();
                self.open_overlay(OverlayKind::Faq, tr!(" Если не работает ", " If something doesn't work "), items);
            }
            "/scheduled" => {
                self.clear_input();
                match self.selected().cloned() {
                    Some(chat) => {
                        let _ = cmd.send(Cmd::ListScheduled(chat));
                        self.status = tr!("Загружаю отложенные…", "Loading scheduled messages…").into();
                    }
                    None => self.status = tr!("Сначала выберите чат", "Select a chat first").into(),
                }
            }
            _ => return false,
        }
        self.pending_delete = None;
        true
    }

    fn proxy_command(&mut self, arg: &str, saver: &Arc<Saver>) {
        match arg {
            "" => {
                self.status = trf!(
                    "Прокси: {}. Задать: /proxy 127.0.0.1:10808, /proxy auto, /proxy off",
                    "Proxy: {}. Set with: /proxy 127.0.0.1:10808, /proxy auto, /proxy off",
                    self.settings.proxy_label()
                );
                return;
            }
            "auto" => self.settings.proxy_mode = ProxyMode::Auto,
            "off" | "0" => self.settings.proxy_mode = ProxyMode::Off,
            addr => {
                self.settings.proxy_manual = addr.to_string();
                self.settings.proxy_mode = ProxyMode::Manual;
            }
        }
        self.apply_settings(saver);
        self.status =
            trf!("Прокси: {} (применится при следующем запуске)", "Proxy: {} (applies on next start)", self.settings.proxy_label());
    }
}

// ------------------------------------------------------------------ рисование

fn draw(f: &mut Frame, app: &mut App) {
    match app.mode {
        Mode::Locked => draw_lock(
            f,
            app,
            tr!("Заблокировано", "Locked"),
            tr!("Введите пароль и нажмите Enter", "Enter the password and press Enter"),
            tr!("F10 — выйти", "F10 — quit"),
        ),
        Mode::NewPassword => {
            let prompt = if app.pw_stage == 0 {
                tr!("Введите новый пароль (любой длины)", "Enter a new password (any length)")
            } else {
                tr!("Повторите новый пароль", "Repeat the new password")
            };
            draw_lock(f, app, tr!("Смена пароля", "Change password"), prompt, tr!("Esc — отмена", "Esc — cancel"));
        }
        Mode::Help => {
            draw_main(f, app);
            draw_help(f, app);
        }
        Mode::Settings => {
            draw_main(f, app);
            draw_settings(f, app);
        }
        Mode::Chat | Mode::Pick | Mode::Forward => draw_main(f, app),
        Mode::Overlay => {
            draw_main(f, app);
            draw_overlay(f, app);
        }
    }
}

fn draw_main(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let outer = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(1), Constraint::Min(3)]).split(area);
    draw_tabs(f, app, outer[0]);

    let area = outer[1];
    let (pct, max_w) = app.settings.list_dims();
    let left_w = (area.width * pct / 100).clamp(16, max_w);
    let cols =
        Layout::default().direction(Direction::Horizontal).constraints([Constraint::Length(left_w), Constraint::Min(10)]).split(area);

    draw_chat_list(f, app, cols[0]);

    let input_width = cols[1].width.saturating_sub(2) as usize;
    let layout = layout_input(&app.input, app.cursor, input_width);
    let input_rows = layout.lines.len().clamp(1, 6) as u16;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(input_rows + 2), Constraint::Length(1)])
        .split(cols[1]);

    draw_messages(f, app, rows[0]);
    draw_input(f, app, rows[1], &layout, input_rows as usize);
    draw_status(f, app, rows[2]);
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let tabs = app.tabs();
    let active = tabs.iter().position(|(t, _)| *t == app.tab).unwrap_or(0);
    // Заготовки подписей вместе с шириной.
    let items: Vec<(String, Option<String>, usize)> = tabs
        .iter()
        .map(|(t, title)| {
            let title = truncate(title, 16);
            let n = app.unread_in(t);
            let badge = if n > 0 { Some(format!(" {} ", n.min(999))) } else { None };
            let w = display_width(&title) + 2 + badge.as_ref().map(|b| display_width(b)).unwrap_or(0);
            (title, badge, w)
        })
        .collect();

    let avail = area.width as usize;
    // Первая показываемая вкладка: сдвигаем окно так, чтобы активная поместилась.
    let mut start = 0;
    while start < active && items[start..=active].iter().map(|i| i.2 + 1).sum::<usize>() + 2 > avail {
        start += 1;
    }

    let mut spans: Vec<Span> = Vec::new();
    let mut used = 0;
    if start > 0 {
        spans.push(Span::styled("‹", Style::new().fg(MUTED)));
        used += 1;
    }
    for (i, (title, badge, w)) in items.iter().enumerate().skip(start) {
        if used + w + 2 > avail && i != active {
            spans.push(Span::styled("›", Style::new().fg(MUTED)));
            break;
        }
        let style = if i == active {
            Style::new().bg(app.theme.accent).fg(Color::Black).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::Gray)
        };
        spans.push(Span::styled(format!(" {title} "), style));
        if let Some(b) = badge {
            let bs = if i == active {
                Style::new().bg(Color::Black).fg(app.theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::new().bg(app.theme.select_bg).fg(Color::White).add_modifier(Modifier::BOLD)
            };
            spans.push(Span::styled(b.clone(), bs));
        }
        spans.push(Span::raw(" "));
        used += w + 1;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_chat_list(f: &mut Frame, app: &mut App, area: Rect) {
    let inner_w = area.width.saturating_sub(2) as usize;
    let now = jiff::Timestamp::now().as_second();
    let show_status = app.settings.show_status;
    let idx = app.visible_idx();
    let visible: Vec<&ChatInfo> = idx.iter().map(|&i| &app.chats[i]).collect();
    let pinned: Vec<bool> = visible.iter().map(|c| app.is_pinned(c)).collect();
    let items: Vec<ListItem> = visible
        .iter()
        .zip(pinned.iter())
        .map(|(c, &pin)| {
            let online = show_status && c.presence.is_some_and(|p| tg::is_online(p, now));
            let (tag, tag_color) = match c.kind {
                Kind::User if online => ("●", ONLINE),
                Kind::User => (" ", MUTED),
                Kind::Group => (tr!("Г", "G"), Color::Rgb(255, 196, 90)),
                Kind::Channel => (tr!("К", "C"), app.theme.accent),
            };
            let badge = if c.unread > 0 {
                format!(" {}{} ", if c.mentions > 0 { "@" } else { "" }, c.unread.min(999))
            } else if c.mentions > 0 {
                " @ ".to_string()
            } else {
                String::new()
            };
            let name_w = inner_w.saturating_sub(2 + display_width(&badge));
            let name = truncate(&c.name, name_w);
            let pad = name_w.saturating_sub(display_width(&name));
            let mut spans = vec![
                Span::styled(tag.to_string(), Style::new().fg(tag_color).add_modifier(Modifier::BOLD)),
                if pin { Span::styled("*", Style::new().fg(app.theme.accent)) } else { Span::raw(" ") },
                Span::styled(
                    format!("{name}{}", " ".repeat(pad)),
                    if c.unread > 0 { Style::new().add_modifier(Modifier::BOLD) } else { Style::new() },
                ),
            ];
            if !badge.is_empty() {
                spans.push(Span::styled(badge, Style::new().bg(app.theme.accent).fg(Color::Black).add_modifier(Modifier::BOLD)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let sel_pos = app.sel.and_then(|s| visible.iter().position(|c| c.id == s));
    let count = visible.len();
    let list_title =
        if app.filter.is_empty() { " Tg-Lite ".to_string() } else { trf!(" поиск: {} ", " search: {} ", truncate(&app.filter, 14)) };
    app.list_state.select(sel_pos);

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(MUTED))
                .title(Span::styled(list_title, Style::new().fg(app.theme.accent).add_modifier(Modifier::BOLD)))
                .title_bottom(Span::styled(format!(" {count} "), Style::new().fg(MUTED))),
        )
        .highlight_style(Style::new().bg(app.theme.select_bg).fg(Color::White).add_modifier(Modifier::BOLD));
    f.render_stateful_widget(list, area, &mut app.list_state);
}

fn draw_messages(f: &mut Frame, app: &mut App, area: Rect) {
    let tz = jiff::tz::TimeZone::system();
    let now = jiff::Timestamp::now().as_second();
    let title = match app.selected() {
        Some(c) => {
            let kind = match c.kind {
                Kind::User => "",
                Kind::Group => tr!(" · группа", " · group"),
                Kind::Channel => tr!(" · канал", " · channel"),
            };
            let mut spans = vec![
                Span::styled(format!(" {}", c.name), Style::new().add_modifier(Modifier::BOLD)),
                Span::styled(kind.to_string(), Style::new().fg(MUTED)),
            ];
            let typing = app.typing.get(&c.id).filter(|(t, _)| t.elapsed() < TYPING_TTL).map(|(_, l)| l.clone());
            if let Some(t) = typing {
                spans.push(Span::styled(" · ", Style::new().fg(MUTED)));
                spans.push(Span::styled(t, Style::new().fg(app.theme.accent).add_modifier(Modifier::ITALIC)));
            } else if app.settings.show_status
                && let Some(p) = c.presence
            {
                let label = tg::presence_label(p, now, &tz);
                if !label.is_empty() {
                    let color = if tg::is_online(p, now) { ONLINE } else { MUTED };
                    spans.push(Span::styled(" · ", Style::new().fg(MUTED)));
                    spans.push(Span::styled(label, Style::new().fg(color)));
                }
            }
            spans.push(Span::raw(" "));
            Line::from(spans)
        }
        None => Line::from(tr!(" Переписка ", " Chat ")),
    };
    let block = Block::default().borders(Borders::ALL).border_style(Style::new().fg(MUTED)).title(title);
    let inner = block.inner(area);
    let width = inner.width.max(1) as usize;

    let picking = app.mode == Mode::Pick;
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let lines: Vec<Line> = match app.sel {
        None => vec![Line::from(Span::styled(
            tr!("Выберите чат слева (↑ ↓), вкладки папок: Tab", "Pick a chat on the left (↑ ↓), folder tabs: Tab"),
            Style::new().fg(MUTED),
        ))],
        Some(id) => match app.histories.get(&id) {
            None => vec![Line::from(Span::styled(tr!("Загрузка…", "Loading…"), Style::new().fg(MUTED)))],
            Some(msgs) if msgs.is_empty() => {
                vec![Line::from(Span::styled(tr!("Сообщений пока нет", "No messages yet"), Style::new().fg(MUTED)))]
            }
            Some(msgs) => {
                let sel = if picking { Some(app.pick.min(msgs.len() - 1)) } else { None };
                let (l, r) = build_message_lines(msgs, width, app.settings.bubble_pct(), sel, app.theme.select_bg);
                ranges = r;
                l
            }
        },
    };

    // Закреплённое сообщение занимает верхнюю строку, если места достаточно.
    let banner = app.sel.and_then(|id| app.pinned_msg.get(&id)).filter(|_| inner.height > 6).cloned();
    let body = match banner {
        Some(_) => Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: inner.height - 1 },
        None => inner,
    };
    let height = body.height as usize;
    if picking {
        // Прокрутка так, чтобы выбранное сообщение было на экране.
        if let Some(&(s0, e0)) = ranges.get(app.pick) {
            let total = lines.len();
            let end = total - app.scroll.min(total.saturating_sub(height));
            let start = end.saturating_sub(height);
            if e0 > end {
                app.scroll = total - e0;
            } else if s0 < start {
                app.scroll = total.saturating_sub(s0 + height);
            }
        }
    }
    let max_scroll = lines.len().saturating_sub(height);
    app.scroll = app.scroll.min(max_scroll);
    let end = lines.len() - app.scroll;
    let start = end.saturating_sub(height);
    let visible: Vec<Line> = lines[start..end].to_vec();
    f.render_widget(block, area);
    if let Some(text) = banner {
        let line = Line::from(vec![
            Span::styled(tr!("▌ закреплено: ", "▌ pinned: "), Style::new().fg(app.theme.accent)),
            Span::styled(truncate(&text, (inner.width as usize).saturating_sub(15)), Style::new().fg(MUTED)),
        ]);
        f.render_widget(Paragraph::new(line), Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 });
    }
    f.render_widget(Paragraph::new(visible), body);

    if app.scroll > 0 && area.width > 12 {
        let tag = tr!(" ↑ выше · PgDn вниз ", " ↑ older · PgDn down ").to_string();
        let w = display_width(&tag) as u16;
        if area.width > w + 2 {
            let r = Rect { x: area.x + area.width - w - 2, y: area.y, width: w, height: 1 };
            f.render_widget(Paragraph::new(tag).style(Style::new().fg(Color::Black).bg(app.theme.accent)), r);
        }
    }
}

/// «Если не работает»: вопрос выделен, ответ переносится по ширине окна целиком.
fn draw_faq(f: &mut Frame, app: &App, ov: &Overlay) {
    let accent = app.theme.accent;
    let area = f.area();
    let w = area.width.saturating_sub(4).clamp(20, 96);
    let inner_w = (w as usize).saturating_sub(4).max(8);
    let mut lines: Vec<Line> = Vec::new();
    let mut starts = Vec::new();
    for (i, it) in ov.items.iter().enumerate() {
        let (q, a) = it.label.split_once(" — ").unwrap_or((it.label.as_str(), ""));
        starts.push(lines.len());
        let qstyle = if i == ov.sel {
            Style::new().bg(app.theme.select_bg).fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(accent).add_modifier(Modifier::BOLD)
        };
        lines.push(Line::from(Span::styled(format!(" {:<width$} ", truncate(q, inner_w), width = inner_w), qstyle)));
        for part in textwrap::wrap(a, inner_w.saturating_sub(2)) {
            lines.push(Line::from(format!("   {part}")));
        }
        lines.push(Line::from(""));
    }
    let total = lines.len();
    let height = (area.height.saturating_sub(6) as usize).clamp(4, 40).min(total.max(1));
    let first = starts.get(ov.sel).copied().unwrap_or(0).min(total.saturating_sub(height));
    let mut shown: Vec<Line> = lines.into_iter().skip(first).take(height).collect();
    shown.push(Line::from(Span::styled(
        tr!(" ↑ ↓ — читать · Esc — закрыть · автор: @maks1m_salov", " ↑ ↓ — read · Esc — close · author: @maks1m_salov"),
        Style::new().fg(MUTED),
    )));
    let rect = centered_rect(area, w, shown.len() as u16 + 2);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(shown).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(accent))
                .title(Span::styled(ov.title.clone(), Style::new().fg(accent).add_modifier(Modifier::BOLD)))
                .title_bottom(Span::styled(format!(" {}/{} ", ov.sel + 1, ov.items.len()), Style::new().fg(MUTED))),
        ),
        rect,
    );
}

fn draw_overlay(f: &mut Frame, app: &App) {
    let Some(ov) = &app.overlay else { return };
    if ov.kind == OverlayKind::Faq {
        draw_faq(f, app, ov);
        return;
    }
    let accent = app.theme.accent;
    let area = f.area();
    let w = area.width.saturating_sub(6).clamp(20, 90);
    let rows = ov.items.len().min(14) as u16;
    let rect = centered_rect(area, w, rows + 4);
    let inner_w = rect.width.saturating_sub(4) as usize;
    // Окно списка прокручивается вместе с выбором.
    let visible = rows as usize;
    let first = ov.sel.saturating_sub(visible.saturating_sub(1));
    let mut lines: Vec<Line> = Vec::new();
    for (i, it) in ov.items.iter().enumerate().skip(first).take(visible) {
        let selected = i == ov.sel;
        let mark = if ov.confirm == Some(i) { "✖ " } else { "  " };
        let text = truncate(&format!("{mark}{}", it.label), inner_w);
        let style =
            if selected { Style::new().bg(app.theme.select_bg).fg(Color::White).add_modifier(Modifier::BOLD) } else { Style::new() };
        lines.push(Line::from(Span::styled(format!(" {text:<inner_w$} "), style)));
    }
    lines.push(Line::from(""));
    let hint = match ov.kind {
        OverlayKind::Links if ov.confirm.is_none() => {
            tr!(" ↑ ↓ выбрать · Enter — открыть в браузере · Esc — закрыть", " ↑ ↓ select · Enter — open in browser · Esc — close")
        }
        OverlayKind::Links => "",
        OverlayKind::Faq => tr!(
            " ↑ ↓ — читать · Esc — закрыть · связь с автором: @maks1m_salov",
            " ↑ ↓ — read · Esc — close · contact the author: @maks1m_salov"
        ),
        OverlayKind::Scheduled if ov.confirm.is_some() => {
            tr!(" Enter ещё раз — отменить отправку · любая другая клавиша — нет", " Enter again — cancel sending · any other key — no")
        }
        OverlayKind::Scheduled => {
            tr!(" ↑ ↓ выбрать · Enter — отменить отправку · Esc — закрыть", " ↑ ↓ select · Enter — cancel sending · Esc — close")
        }
    };
    lines.push(Line::from(Span::styled(truncate(hint, rect.width as usize - 2), Style::new().fg(MUTED))));
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(accent))
                .title(Span::styled(ov.title.clone(), Style::new().fg(accent).add_modifier(Modifier::BOLD)))
                .title_bottom(Span::styled(format!(" {}/{} ", ov.sel + 1, ov.items.len()), Style::new().fg(MUTED))),
        ),
        rect,
    );
}

fn draw_input(f: &mut Frame, app: &App, area: Rect, layout: &InputLayout, rows: usize) {
    let mut title = vec![Span::styled(tr!(" Сообщение ", " Message "), Style::new().fg(app.theme.accent))];
    if app.markdown {
        title.push(Span::styled("· Markdown ", Style::new().fg(OWN)));
    }
    if let Some((_, label)) = &app.reply {
        title.push(Span::styled(trf!("· ответ: {label} ", "· reply: {label} "), Style::new().fg(OWN)));
    }
    if app.edit_target.is_some() {
        title.push(Span::styled(tr!("· правка сообщения ", "· editing message "), Style::new().fg(OWN)));
    }
    let focus = Style::new().fg(app.theme.accent);
    let skip = if layout.cur_row >= rows { layout.cur_row + 1 - rows } else { 0 };
    let shown: Vec<Line> = if app.input.is_empty() {
        vec![Line::from(Span::styled(
            tr!(
                "Enter — отправить · Shift+Enter — новая строка · F1 — все команды",
                "Enter — send · Shift+Enter — new line · F1 — all commands"
            ),
            Style::new().fg(MUTED),
        ))]
    } else {
        layout.lines.iter().skip(skip).take(rows).map(|l| Line::from(l.as_str())).collect()
    };
    f.render_widget(Paragraph::new(shown).block(Block::default().borders(Borders::ALL).border_style(focus).title(Line::from(title))), area);
    let cy = area.y + 1 + (layout.cur_row - skip) as u16;
    let cx = area.x + 1 + layout.cur_col as u16;
    if cx < area.x + area.width.saturating_sub(1) {
        f.set_cursor_position((cx, cy.min(area.y + area.height.saturating_sub(2))));
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let mode_hint = match app.mode {
        Mode::Pick => Some(tr!(
            " ↑ ↓ · R ответ · F переслать · E правка · D удалить · S скачать · L ссылки · Esc",
            " ↑ ↓ · R reply · F forward · E edit · D delete · S save · L links · Esc"
        )),
        Mode::Forward => Some(tr!(
            " Переслать: выберите чат ↑ ↓ / Tab · Enter — переслать · Esc — отмена",
            " Forward: pick a chat ↑ ↓ / Tab · Enter — forward · Esc — cancel"
        )),
        _ => None,
    };
    if let Some(h) = mode_hint {
        // Сообщение о недоступном действии важнее подсказки по клавишам.
        let (text, color) = if app.mode == Mode::Pick && !app.pick_note.is_empty() {
            (format!(" {}", app.pick_note), Color::Rgb(255, 196, 90))
        } else {
            (h.to_string(), app.theme.accent)
        };
        f.render_widget(Paragraph::new(truncate(&text, area.width as usize)).style(Style::new().fg(color)), area);
        return;
    }
    // Подсказка подбирается под ширину окна, чтобы статус слева не вытеснялся.
    let hints = [
        tr!(" Tab папки · F1 справка · F4 настройки · F2 замок · F10 выход ", " Tab folders · F1 help · F4 settings · F2 lock · F10 quit "),
        tr!(" F1 справка · F10 выход ", " F1 help · F10 quit "),
        tr!(" F1 справка ", " F1 help "),
    ];
    let hint = hints.iter().find(|h| display_width(h) + 20 <= area.width as usize).copied();
    let (left, right) = match hint {
        Some(h) => {
            let parts = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(10), Constraint::Length(display_width(h) as u16)])
                .split(area);
            (parts[0], Some((parts[1], h)))
        }
        None => (area, None),
    };
    let color = if app.status.starts_with("Ошибка") || app.status.starts_with("Error") { ERROR } else { MUTED };
    let status = truncate(&format!(" {}", app.status), left.width as usize);
    f.render_widget(Paragraph::new(status).style(Style::new().fg(color)), left);
    if let Some((r, h)) = right {
        f.render_widget(Paragraph::new(h).style(Style::new().fg(MUTED)), r);
    }
}

fn centered_rect(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

fn draw_help(f: &mut Frame, app: &App) {
    let accent = app.theme.accent;
    let key = |k: &str, d: &str| {
        Line::from(vec![Span::styled(format!(" {k:<17}"), Style::new().fg(accent).add_modifier(Modifier::BOLD)), Span::raw(d.to_string())])
    };
    let lines = vec![
        key("Enter", tr!("отправить", "send")),
        key("Shift+Enter", tr!("новая строка (вставка текста целиком тоже работает)", "new line (pasting whole text works too)")),
        key("↑ ↓", tr!("выбрать чат", "select chat")),
        key("Tab / Shift+Tab", tr!("вкладки; после «/» — дополнить команду", "tabs; after \"/\" — complete command")),
        key("PgUp / PgDn", tr!("прокрутка переписки", "scroll the chat")),
        key(
            "F7",
            tr!("выбор сообщения: R ответить, F переслать, E править, D удалить", "pick message: R reply, F forward, E edit, D delete"),
        ),
        key("Esc", tr!("очистить ввод; отменить ответ, правку, поиск", "clear input; cancel reply, edit, search")),
        key(tr!("F2  или /lock", "F2  or /lock"), tr!("заблокировать (запрос пароля)", "lock (asks for password)")),
        key(tr!("F3  или /md", "F3  or /md"), tr!("Markdown вкл/выкл", "Markdown on/off")),
        key(tr!("F4  или /settings", "F4  or /settings"), tr!("настройки", "settings")),
        key(tr!("F5  или /refresh", "F5  or /refresh"), tr!("обновить список чатов", "refresh chat list")),
        key("F6 / Ctrl+F", tr!("поиск по чатам (/find текст; Esc сбрасывает)", "search chats (/find text; Esc resets)")),
        key(tr!("F10 или /quit", "F10 or /quit"), tr!("выход", "quit")),
        key(tr!("/later 30 текст", "/later 30 text"), tr!("отложенная публикация через 30 минут", "send in 30 minutes")),
        key(
            tr!("/note текст", "/note text"),
            tr!("быстрая заметка в «Избранное» из любого чата", "quick note to Saved Messages from any chat"),
        ),
        key(
            tr!("/edit [текст], /del", "/edit [text], /del"),
            tr!("править / удалить своё последнее сообщение", "edit / delete your last message"),
        ),
        key("/pin", tr!("закрепить / открепить чат наверху списка", "pin / unpin chat at the top of the list")),
        key("/readall", tr!("отметить все чаты прочитанными", "mark all chats as read")),
        key("/export", tr!("сохранить переписку в файл export/…txt", "save the chat to a file export/…txt")),
        key(
            tr!("/send путь [текст]", "/send path [text]"),
            tr!("отправить файл (можно перетащить файл в окно)", "send a file (you can drag a file into the window)"),
        ),
        key("/save", tr!("скачать последнее вложение (или S в режиме F7)", "download the last attachment (or S in F7 mode)")),
        key(
            "Ctrl+V / /paste",
            tr!(
                "вставить из буфера обмена (также Shift+Insert); /faq — если не работает",
                "paste from clipboard (also Shift+Insert); /faq — if it fails"
            ),
        ),
        key(
            "/links, /scheduled",
            tr!("ссылки чата (L для сообщения); отложенные, отмена", "chat links (L for a message); scheduled, cancel"),
        ),
        key("/proxy, /passwd", tr!("прокси вручную (auto, off); смена пароля", "set proxy (auto, off); change password")),
        Line::from(Span::styled(
            trf!(
                " Ctrl+L/Q/T/R/F тоже работают · Tg-Lite {VERSION} · любая клавиша закрывает",
                " Ctrl+L/Q/T/R/F work too · Tg-Lite {VERSION} · any key closes"
            ),
            Style::new().fg(MUTED),
        )),
    ];
    let rect = centered_rect(f.area(), 72, lines.len() as u16 + 2);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(accent))
                .title(Span::styled(tr!(" Справка ", " Help "), Style::new().fg(accent).add_modifier(Modifier::BOLD))),
        ),
        rect,
    );
}

fn on_off(v: bool) -> &'static str {
    if v { tr!("включено", "on") } else { tr!("выключено", "off") }
}

fn draw_settings(f: &mut Frame, app: &App) {
    let accent = app.theme.accent;
    let s = &app.settings;
    let rows: [(&str, String); SETTINGS_ROWS] = [
        (tr!("Автоблокировка через", "Auto-lock after"), s.lock_label()),
        (tr!("Прокси", "Proxy"), s.proxy_label()),
        (tr!("Звук при новом сообщении", "Sound on new message"), on_off(s.sound).to_string()),
        (tr!("Статус «в сети / был(а)»", "Online / last seen status"), on_off(s.show_status).to_string()),
        (tr!("Сообщений при открытии", "Messages on open"), s.history_limit.to_string()),
        (tr!("Цвет интерфейса", "Interface color"), s.accent_name().to_string()),
        (tr!("Самоуничтожение данных", "Data self-destruct"), s.wipe_label()),
        (tr!("Фон экрана блокировки", "Lock screen background"), s.lock_bg_name().to_string()),
        (
            tr!("Ширина списка чатов", "Chat list width"),
            Settings::width_name(s.list_width, [tr!("узкий", "narrow"), tr!("обычный", "normal"), tr!("широкий", "wide")]).to_string(),
        ),
        (
            tr!("Ширина сообщений", "Message width"),
            Settings::width_name(s.bubble_width, [tr!("узкие", "narrow"), tr!("обычные", "normal"), tr!("широкие", "wide")]).to_string(),
        ),
        (tr!("Язык интерфейса", "Interface language"), if s.lang == 0 { "Русский".to_string() } else { "English".to_string() }),
        (tr!("Пароль приложения", "App password"), tr!("сменить…", "change…").to_string()),
    ];
    let width = 78u16;
    let label_w = 28usize;
    let mut lines = vec![Line::from("")];
    for (i, (label, value)) in rows.iter().enumerate() {
        let selected = i == app.settings_row;
        let shown = if i == SETTINGS_ROWS - 1 { value.clone() } else { format!("‹ {value} ›") };
        let shown = truncate(&shown, (width as usize).saturating_sub(label_w + 6));
        let row_style = if selected { Style::new().bg(app.theme.select_bg).fg(Color::White) } else { Style::new() };
        lines.push(Line::from(vec![
            Span::styled(format!(" {label:<label_w$}"), row_style),
            Span::styled(
                format!("{shown:<w$} ", w = (width as usize).saturating_sub(label_w + 4)),
                if selected { row_style.add_modifier(Modifier::BOLD) } else { Style::new().fg(accent) },
            ),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        tr!(" ↑ ↓ выбрать · ← → изменить · Enter — действие · Esc — закрыть", " ↑ ↓ select · ← → change · Enter — action · Esc — close"),
        Style::new().fg(MUTED),
    )));
    lines.push(Line::from(Span::styled(
        tr!(
            " Прокси применяется при следующем запуске. Задать вручную: /proxy 127.0.0.1:10808",
            " Proxy applies on next start. Set manually: /proxy 127.0.0.1:10808"
        ),
        Style::new().fg(MUTED),
    )));
    let rect = centered_rect(f.area(), width, lines.len() as u16 + 2);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(accent))
                .title(Span::styled(tr!(" Настройки ", " Settings "), Style::new().fg(accent).add_modifier(Modifier::BOLD))),
        ),
        rect,
    );
}

fn draw_lock(f: &mut Frame, app: &App, title: &str, prompt: &str, footer: &str) {
    crate::lock::draw(
        f,
        &crate::lock::LockView {
            accent: app.theme.accent,
            bg: app.settings.lock_bg,
            t: app.started.elapsed().as_secs_f32(),
            version: VERSION,
            title,
            prompt,
            dots: app.lock_input.chars().count(),
            error: &app.lock_error,
            footer,
            busy: false,
        },
    );
}

// ------------------------------------------------------------------ главный цикл

pub async fn run(
    mut terminal: DefaultTerminal,
    cmd: mpsc::UnboundedSender<Cmd>,
    mut events: mpsc::UnboundedReceiver<Event>,
    saver: Arc<Saver>,
) -> Result<()> {
    // Клавиатура читается в отдельном потоке и пересылается в async-цикл.
    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<TermEvent>();
    std::thread::spawn(move || {
        while let Ok(ev) = crossterm::event::read() {
            if key_tx.send(ev).is_err() {
                break;
            }
        }
    });

    // Терминалы с «bracketed paste» присылают вставку одним событием (на старой консоли Windows
    // это не поддерживается, там работает чтение буфера по Ctrl+V).
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);
    let mut app = App::new(saver.settings());
    let _ = cmd.send(Cmd::LoadDialogs);
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut last_save = Instant::now();
    let mut last_refresh = Instant::now();
    let mut last_title = String::new();
    // Перерисовываем только при изменениях: это заметно разгружает слабый процессор.
    let mut dirty = true;
    let mut flood_shown = false;

    loop {
        if dirty {
            terminal.draw(|f| draw(f, &mut app))?;
            dirty = false;
            let title = app.title();
            if title != last_title {
                let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle(&title));
                last_title = title;
            }
        }
        tokio::select! {
            Some(ev) = key_rx.recv() => match ev {
                TermEvent::Key(k) if k.kind == KeyEventKind::Press => {
                    let before = std::mem::discriminant(&app.mode);
                    let refresh = k.code == KeyCode::F(5);
                    app.handle_key(k, &cmd, &saver);
                    // Смена экрана и F5: полная перерисовка, чтобы не оставалось «мусора».
                    if refresh || before != std::mem::discriminant(&app.mode) {
                        let _ = terminal.clear();
                    }
                    dirty = true;
                }
                TermEvent::Paste(text) => {
                    app.last_activity = Instant::now();
                    app.paste(Some(text));
                    dirty = true;
                }
                TermEvent::Resize(..) => {
                    let _ = terminal.clear();
                    dirty = true;
                }
                _ => {}
            },
            Some(ev) = events.recv() => {
                app.handle_event(ev, &cmd);
                dirty = true;
            }
            _ = tick.tick() => {
                // Фон экрана блокировки анимирован: пока он на экране, перерисовываем каждый кадр.
                if matches!(app.mode, Mode::Locked | Mode::NewPassword) && crate::lock::animated(app.settings.lock_bg) {
                    dirty = true;
                }
                if app.expire_typing() {
                    dirty = true;
                }
                let left = tg::flood_left();
                if left > 0 {
                    let text = trf!("Telegram просит подождать {left} с, сообщение отправится само…", "Telegram asks to wait {left} s, the message will be sent automatically…");
                    if app.status != text {
                        app.status = text;
                        dirty = true;
                    }
                    flood_shown = true;
                } else if flood_shown {
                    flood_shown = false;
                    app.status = tr!("Готово", "Done").into();
                    dirty = true;
                }
                if app.should_autolock() {
                    app.lock();
                    let _ = terminal.clear();
                    dirty = true;
                }
                if let Some(t) = app.pending_load
                    && t.elapsed() >= LOAD_DELAY {
                        app.pending_load = None;
                        if let Some(c) = app.selected().cloned() {
                            let _ = cmd.send(Cmd::LoadHistory(c, app.settings.history_limit as usize));
                        }
                        dirty = true;
                    }
                if last_refresh.elapsed() >= REFRESH_EVERY {
                    last_refresh = Instant::now();
                    let _ = cmd.send(Cmd::LoadDialogs);
                }
                if last_save.elapsed() >= SAVE_EVERY {
                    last_save = Instant::now();
                    saver.save_in_background();
                }
            }
        }
        if app.should_quit {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tg::{ChatInfo, ChatMsg, Folder, Kind, Presence};
    use grammers_client::session::types::PeerId;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn chat(id: i64, name: &str, kind: Kind, unread: i32) -> ChatInfo {
        let pid = match kind {
            Kind::User => PeerId::user(id).unwrap(),
            Kind::Group => PeerId::chat(id).unwrap(),
            Kind::Channel => PeerId::channel(id).unwrap(),
        };
        ChatInfo {
            id: pid,
            peer: pid.to_ambient_ref(),
            name: name.into(),
            kind,
            unread,
            presence: None,
            is_contact: false,
            is_bot: false,
            is_self: false,
            mentions: 0,
        }
    }

    fn msg(id: i32, outgoing: bool, sender: &str, text: &str) -> ChatMsg {
        ChatMsg {
            id,
            ts: 1_761_650_000 + i64::from(id) * 60,
            outgoing,
            sender: sender.into(),
            text: text.into(),
            time: "10:01".into(),
            ..Default::default()
        }
    }

    fn dump(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        let mut s = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        s
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn saver() -> Arc<Saver> {
        use crate::vault::{SessionBlob, VaultFile, VaultSession};
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!("tglt-ui-test-{}-{}.vault", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        Arc::new(Saver::new(
            VaultFile::new(path),
            1,
            "x".into(),
            Arc::new(VaultSession::from_blob(SessionBlob::default())),
            "secret".into(),
            Settings::default(),
        ))
    }

    fn app_with_chats() -> App {
        let mut app = App::new(Settings::default());
        let a = chat(1, "Мама", Kind::User, 2);
        let b = chat(2, "Мой канал", Kind::Channel, 0);
        let g = chat(3, "Рабочий чат", Kind::Group, 0);
        app.histories.insert(a.id, vec![msg(1, false, "Мама", "Привет, как дела?"), msg(2, true, "Я", "Всё хорошо!")]);
        app.chats = vec![a, b, g];
        app.ensure_selection();
        app
    }

    fn type_text(app: &mut App, text: &str, tx: &mpsc::UnboundedSender<Cmd>, s: &Arc<Saver>) {
        for c in text.chars() {
            app.handle_key(key(KeyCode::Char(c), KeyModifiers::NONE), tx, s);
        }
    }

    fn slow(app: &mut App) {
        app.last_key_at = Instant::now() - Duration::from_secs(5);
    }

    #[test]
    fn later_command_parsing() {
        assert_eq!(parse_later("/later 30 привет"), Some((30, "привет".into())));
        assert_eq!(parse_later("/later 5 много  слов тут"), Some((5, "много  слов тут".into())));
        assert_eq!(parse_later("/later 0 x"), None);
        assert_eq!(parse_later("/later abc x"), None);
        assert_eq!(parse_later("привет"), None);
    }

    #[test]
    fn wraps_cyrillic_and_newlines() {
        let l = wrap_lines("раз два три четыре пять\nшесть", 10);
        assert!(l.iter().all(|x| display_width(x) <= 10), "{l:?}");
        assert_eq!(l.last().unwrap(), "шесть");
    }

    #[test]
    fn ctrl_letters_work_in_any_layout() {
        let c = KeyModifiers::CONTROL;
        assert_eq!(ctrl_letter(&key(KeyCode::Char('l'), c)), Some('l'));
        assert_eq!(ctrl_letter(&key(KeyCode::Char('Q'), c)), Some('q'));
        assert_eq!(ctrl_letter(&key(KeyCode::Char('д'), c)), Some('l')); // русская раскладка
        assert_eq!(ctrl_letter(&key(KeyCode::Char('\u{c}'), KeyModifiers::NONE)), Some('l')); // «сырой» код
        assert_eq!(ctrl_letter(&key(KeyCode::Char('l'), KeyModifiers::NONE)), None);
        assert_eq!(ctrl_letter(&key(KeyCode::Char('@'), c | KeyModifiers::ALT)), None); // AltGr
    }

    #[test]
    fn echo_of_own_message_is_not_duplicated() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let id = app.chats[0].id;
        app.handle_event(Event::Incoming { chat: id, chat_name: "Мама".into(), msg: msg(3, true, "Я", "тест") }, &tx);
        app.handle_event(Event::Sent { chat: id, msg: msg(3, true, "Я", "тест"), scheduled: false }, &tx);
        app.handle_event(Event::Sent { chat: id, msg: msg(4, true, "Я", "ещё"), scheduled: false }, &tx);
        app.handle_event(Event::Incoming { chat: id, chat_name: "Мама".into(), msg: msg(4, true, "Я", "ещё") }, &tx);
        let ids: Vec<i32> = app.histories[&id].iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4]);
    }

    #[test]
    fn edits_and_deletions_apply() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let id = app.chats[0].id;
        app.handle_event(Event::Edited { chat: id, msg: msg(1, false, "Мама", "исправлено") }, &tx);
        assert_eq!(app.histories[&id][0].text, "исправлено");
        app.handle_event(Event::Deleted { chat: None, ids: vec![2] }, &tx);
        assert_eq!(app.histories[&id].len(), 1);
    }

    #[test]
    fn failed_send_returns_text_to_input() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        app.handle_event(Event::SendFailed { text: "длинный пост".into() }, &tx);
        assert_eq!(app.input, "длинный пост");
        assert_eq!(app.cursor, 12);
        // если пользователь уже начал печатать новое, его текст не затираем
        app.handle_event(Event::SendFailed { text: "другой".into() }, &tx);
        assert_eq!(app.input, "длинный пост");
    }

    #[test]
    fn keys_lock_help_quit_and_commands() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        let none = KeyModifiers::NONE;

        app.handle_key(key(KeyCode::F(1), none), &tx, &s);
        assert!(app.mode == Mode::Help);
        app.handle_key(key(KeyCode::Char('x'), none), &tx, &s); // любая клавиша закрывает справку
        assert!(app.mode == Mode::Chat && app.input.is_empty());

        app.handle_key(key(KeyCode::F(2), none), &tx, &s);
        assert!(app.mode == Mode::Locked);
        type_text(&mut app, "wrong", &tx, &s);
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert!(app.mode == Mode::Locked && !app.lock_error.is_empty());
        type_text(&mut app, "secret", &tx, &s);
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert!(app.mode == Mode::Chat);

        // Ctrl+L в русской раскладке
        app.handle_key(key(KeyCode::Char('д'), KeyModifiers::CONTROL), &tx, &s);
        assert!(app.mode == Mode::Locked);
        app.handle_key(key(KeyCode::F(10), none), &tx, &s);
        assert!(app.should_quit);

        // команды в поле ввода
        let mut app = app_with_chats();
        slow(&mut app);
        type_text(&mut app, "/lock", &tx, &s);
        slow(&mut app);
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert!(app.mode == Mode::Locked);
    }

    #[test]
    fn newline_and_paste_handling() {
        let mut app = app_with_chats();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        let none = KeyModifiers::NONE;
        type_text(&mut app, "раз", &tx, &s);
        app.handle_key(key(KeyCode::Enter, KeyModifiers::SHIFT), &tx, &s);
        type_text(&mut app, "два", &tx, &s);
        // Enter сразу за символом (как при вставке) — тоже перенос, а не отправка
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert_eq!(app.input, "раз\nдва\n");
        assert!(rx.try_recv().is_err(), "ничего не должно уйти в сеть");
        // Tab внутри вставки — отступ, а не смена вкладки
        app.handle_key(key(KeyCode::Tab, none), &tx, &s);
        assert_eq!(app.input, "раз\nдва\n  ");
        assert!(app.tab == Tab::All);
        // «настоящий» Enter после паузы отправляет
        slow(&mut app);
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert!(matches!(rx.try_recv(), Ok(Cmd::Send { .. })));
        assert!(app.input.is_empty());
    }

    #[test]
    fn paste_event_and_keys() {
        let mut app = app_with_chats();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        // вставка одним событием: переводы строк \r\n нормализуются, эмодзи убираются
        app.paste(Some("C:\\папка\\файл.txt\r\nвторая 😀 строка".into()));
        assert_eq!(app.input, "C:\\папка\\файл.txt\nвторая  строка");
        assert_eq!(app.cursor, app.input_len());
        assert!(rx.try_recv().is_err(), "вставка ничего не отправляет");
        // вставка в середину
        app.clear_input();
        type_text(&mut app, "ac", &tx, &s);
        app.cursor = 1;
        app.paste(Some("b".into()));
        assert_eq!(app.input, "abc");
        // лимит длины
        app.clear_input();
        app.paste(Some("x".repeat(MAX_MESSAGE_CHARS + 500)));
        assert_eq!(app.input_len(), MAX_MESSAGE_CHARS);
        // клавиши вставки распознаются в любой раскладке
        let c = KeyModifiers::CONTROL;
        assert!(is_paste_key(&key(KeyCode::Char('v'), c)));
        assert!(is_paste_key(&key(KeyCode::Char('м'), c)));
        assert!(is_paste_key(&key(KeyCode::Char('\u{16}'), KeyModifiers::NONE)));
        assert!(is_paste_key(&key(KeyCode::Insert, KeyModifiers::SHIFT)));
        assert!(!is_paste_key(&key(KeyCode::Char('v'), KeyModifiers::NONE)));
    }

    fn has_cyrillic(s: &str) -> bool {
        s.chars().any(|c| ('\u{400}'..='\u{4ff}').contains(&c))
    }

    #[test]
    fn english_ui_has_no_russian_leftovers() {
        crate::i18n::set(1);
        let mut app = App::new(Settings::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        let mut term = Terminal::new(TestBackend::new(110, 40)).unwrap();
        // чат без данных, справка, настройки, блокировка, смена пароля, FAQ
        for mode in [Mode::Chat, Mode::Help, Mode::Settings, Mode::Locked, Mode::NewPassword] {
            app.mode = mode;
            term.draw(|f| draw(f, &mut app)).unwrap();
            let text = dump(&term).replace("Русский", ""); // название языка в настройках
            assert!(!has_cyrillic(&text), "русский текст на английском экране {mode:?}:\n{text}");
        }
        app.mode = Mode::Chat;
        assert!(app.try_command("/faq", &tx, &s));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let text = dump(&term);
        assert!(!has_cyrillic(&text), "FAQ:\n{text}");
        assert!(text.contains("If something doesn't work"));
        crate::i18n::set(0);
    }

    #[test]
    fn language_row_switches_and_persists_in_settings() {
        let mut app = App::new(Settings::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        app.mode = Mode::Settings;
        app.settings_row = 10;
        app.handle_key(key(KeyCode::Right, KeyModifiers::NONE), &tx, &s);
        assert_eq!(app.settings.lang, 1);
        assert!(crate::i18n::en());
        assert!(app.status.contains("English"));
        app.handle_key(key(KeyCode::Right, KeyModifiers::NONE), &tx, &s);
        assert_eq!(app.settings.lang, 0);
        assert!(!crate::i18n::en());
    }

    #[test]
    fn faq_and_paste_commands() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        app.input = "/faq".into();
        app.cursor = 4;
        assert!(app.try_command("/faq", &tx, &s));
        assert!(app.mode == Mode::Overlay);
        assert_eq!(app.overlay.as_ref().unwrap().items.len(), faq().len());
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(dump(&term).contains("Если не работает"));
        app.handle_key(key(KeyCode::Down, KeyModifiers::NONE), &tx, &s);
        app.handle_key(key(KeyCode::Enter, KeyModifiers::NONE), &tx, &s); // Enter ничего не ломает
        app.handle_key(key(KeyCode::Esc, KeyModifiers::NONE), &tx, &s);
        assert!(app.mode == Mode::Chat);
        assert!(COMMANDS.contains(&"/paste") && COMMANDS.contains(&"/faq"));
    }

    #[test]
    fn paste_into_password_fields_is_single_line() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        app.lock();
        app.paste(Some("секрет\r\n".into()));
        assert_eq!(app.lock_input, "секрет");
        app.handle_key(key(KeyCode::Esc, KeyModifiers::NONE), &tx, &s);
        assert!(app.lock_input.is_empty());
    }

    #[test]
    fn cursor_editing() {
        let mut app = App::new(Settings::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        let none = KeyModifiers::NONE;
        type_text(&mut app, "привет", &tx, &s);
        app.handle_key(key(KeyCode::Left, none), &tx, &s);
        app.handle_key(key(KeyCode::Left, none), &tx, &s);
        app.handle_key(key(KeyCode::Backspace, none), &tx, &s); // удаляет «в»
        assert_eq!(app.input, "приет");
        app.handle_key(key(KeyCode::Char('в'), none), &tx, &s);
        assert_eq!(app.input, "привет");
        app.handle_key(key(KeyCode::Home, none), &tx, &s);
        app.handle_key(key(KeyCode::Delete, none), &tx, &s);
        assert_eq!(app.input, "ривет");
        app.handle_key(key(KeyCode::End, none), &tx, &s);
        assert_eq!(app.cursor, 5);
    }

    #[test]
    fn input_layout_places_cursor() {
        let l = layout_input("hello world", 5, 8);
        assert_eq!((l.cur_row, l.cur_col), (0, 5));
        let l = layout_input("hello world", 11, 8);
        assert_eq!(l.lines.len(), 2);
        assert_eq!((l.cur_row, l.cur_col), (1, 3));
        let l = layout_input("ab\ncd", 3, 10);
        assert_eq!((l.cur_row, l.cur_col), (1, 0));
        let l = layout_input("12345678", 8, 8);
        assert_eq!((l.cur_row, l.cur_col), (1, 0));
    }

    #[test]
    fn tabs_filter_chats_and_keep_selection_valid() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        assert_eq!(app.visible_ids().len(), 3);
        assert_eq!(app.selected().unwrap().name, "Мама");

        app.handle_key(key(KeyCode::Tab, KeyModifiers::NONE), &tx, &s); // Новые
        assert!(app.tab == Tab::Unread);
        slow(&mut app);
        app.handle_key(key(KeyCode::Tab, KeyModifiers::NONE), &tx, &s); // Личные
        assert!(app.tab == Tab::Users);
        assert_eq!(app.visible_ids().len(), 1);
        slow(&mut app);
        app.handle_key(key(KeyCode::Tab, KeyModifiers::NONE), &tx, &s); // Группы
        assert!(app.tab == Tab::Groups);
        assert_eq!(app.selected().unwrap().name, "Рабочий чат", "выбор переходит на чат из этой вкладки");
        slow(&mut app);
        app.handle_key(key(KeyCode::Tab, KeyModifiers::NONE), &tx, &s); // Каналы
        assert_eq!(app.selected().unwrap().name, "Мой канал");
        app.handle_key(key(KeyCode::BackTab, KeyModifiers::SHIFT), &tx, &s);
        assert!(app.tab == Tab::Groups);

        // навигация стрелками не выходит за пределы вкладки
        app.handle_key(key(KeyCode::Down, KeyModifiers::NONE), &tx, &s);
        assert_eq!(app.selected().unwrap().name, "Рабочий чат");
    }

    #[test]
    fn account_folders_become_tabs() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        let ch = app.chats[1].id;
        app.handle_event(Event::Folders(vec![Folder { id: 5, title: "Новости".into(), include: vec![ch], ..Default::default() }]), &tx);
        let titles: Vec<String> = app.tabs().into_iter().map(|t| t.1).collect();
        assert_eq!(titles, vec!["Все", "Новые", "Личные", "Группы", "Каналы", "Новости"]);
        for _ in 0..5 {
            slow(&mut app);
            app.handle_key(key(KeyCode::Tab, KeyModifiers::NONE), &tx, &s);
        }
        assert!(app.tab == Tab::Folder(5));
        assert_eq!(app.visible_ids(), vec![ch]);
        // папка исчезла из аккаунта — возвращаемся на «Все»
        app.handle_event(Event::Folders(vec![]), &tx);
        assert!(app.tab == Tab::All);
        assert!(app.selected().is_some());
    }

    #[test]
    fn selection_survives_list_refresh_and_reorder() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        app.move_selection(1);
        assert_eq!(app.selected().unwrap().name, "Мой канал");
        // новое сообщение в другом чате поднимает его наверх, выбор остаётся на канале
        let mama = app.chats[0].id;
        app.handle_event(Event::Incoming { chat: mama, chat_name: "Мама".into(), msg: msg(9, false, "Мама", "эй") }, &tx);
        assert_eq!(app.selected().unwrap().name, "Мой канал");
        assert_eq!(app.chats[0].id, mama);
        // обновление списка с сервера тоже не сбрасывает выбор
        let fresh = vec![chat(3, "Рабочий чат", Kind::Group, 0), chat(2, "Мой канал", Kind::Channel, 0), chat(1, "Мама", Kind::User, 0)];
        app.handle_event(Event::Dialogs(fresh), &tx);
        assert_eq!(app.selected().unwrap().name, "Мой канал");
    }

    #[test]
    fn incoming_in_unloaded_chat_counts_as_unread() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let group = app.chats[2].id; // история группы не загружена
        assert!(!app.histories.contains_key(&group));
        app.handle_event(Event::Incoming { chat: group, chat_name: "Рабочий чат".into(), msg: msg(1, false, "Коллега", "ку") }, &tx);
        assert_eq!(app.chats[0].id, group, "чат поднялся наверх");
        assert_eq!(app.chats[0].unread, 1);
        assert!(app.status.contains("Рабочий чат"));
        // своё сообщение с другого устройства счётчик не увеличивает
        app.handle_event(Event::Incoming { chat: group, chat_name: "Рабочий чат".into(), msg: msg(2, true, "Я", "ответ") }, &tx);
        assert_eq!(app.chats[0].unread, 1);
    }

    #[test]
    fn presence_updates_and_shows_in_header() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mama = app.chats[0].id;
        let now = jiff::Timestamp::now().as_second();
        app.handle_event(Event::Presence { user: mama, presence: Presence::Online(now + 300) }, &tx);

        let mut term = Terminal::new(TestBackend::new(110, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let out = dump(&term);
        assert!(out.contains("в сети"), "{out}");
        assert!(out.contains('●'), "точка онлайна в списке: {out}");

        app.settings.show_status = false;
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(!dump(&term).contains("в сети"));

        app.settings.show_status = true;
        app.handle_event(Event::Presence { user: mama, presence: Presence::Recently }, &tx);
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(dump(&term).contains("был(а) недавно"));
    }

    #[test]
    fn settings_screen_changes_and_saves() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        let none = KeyModifiers::NONE;

        app.handle_key(key(KeyCode::F(4), none), &tx, &s);
        assert!(app.mode == Mode::Settings);
        app.handle_key(key(KeyCode::Right, none), &tx, &s); // 5 -> 10 минут
        assert_eq!(app.settings.lock_minutes, 10);
        assert_eq!(s.settings().lock_minutes, 10, "настройка записана в хранилище");
        app.handle_key(key(KeyCode::Left, none), &tx, &s);
        app.handle_key(key(KeyCode::Left, none), &tx, &s);
        assert_eq!(app.settings.lock_minutes, 2);

        app.handle_key(key(KeyCode::Down, none), &tx, &s); // прокси
        app.handle_key(key(KeyCode::Right, none), &tx, &s);
        assert_eq!(app.settings.proxy_mode, ProxyMode::Off);
        app.handle_key(key(KeyCode::Down, none), &tx, &s); // звук
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert!(app.settings.sound);
        app.handle_key(key(KeyCode::Down, none), &tx, &s); // статус
        app.handle_key(key(KeyCode::Char(' '), none), &tx, &s);
        assert!(!app.settings.show_status);
        app.handle_key(key(KeyCode::Down, none), &tx, &s); // история
        app.handle_key(key(KeyCode::Right, none), &tx, &s);
        assert_eq!(app.settings.history_limit, 100);
        app.handle_key(key(KeyCode::Down, none), &tx, &s); // цвет
        app.handle_key(key(KeyCode::Right, none), &tx, &s);
        assert_eq!(app.settings.accent, 1);
        assert_eq!(s.settings(), app.settings);

        // экран настроек рисуется без паники и показывает значения
        let mut term = Terminal::new(TestBackend::new(100, 28)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let out = dump(&term);
        assert!(out.contains("Настройки") && out.contains("Зелёный") && out.contains("2 минуты"), "{out}");

        app.handle_key(key(KeyCode::Esc, none), &tx, &s);
        assert!(app.mode == Mode::Chat);
    }

    #[test]
    fn autolock_follows_setting() {
        let mut app = app_with_chats();
        app.last_activity = Instant::now() - Duration::from_secs(150);
        app.settings.lock_minutes = 5;
        assert!(!app.should_autolock());
        app.settings.lock_minutes = 2;
        assert!(app.should_autolock());
        app.settings.lock_minutes = 0; // никогда
        assert!(!app.should_autolock());
    }

    #[test]
    fn password_of_any_length_can_be_set() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        let none = KeyModifiers::NONE;

        slow(&mut app);
        type_text(&mut app, "/passwd", &tx, &s);
        slow(&mut app);
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert!(app.mode == Mode::NewPassword);

        app.handle_key(key(KeyCode::Enter, none), &tx, &s); // пустой не принимаем
        assert!(!app.lock_error.is_empty() && app.pw_stage == 0);

        type_text(&mut app, "1", &tx, &s); // один символ — можно
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert_eq!(app.pw_stage, 1);
        type_text(&mut app, "2", &tx, &s); // не совпало — заново
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert!(app.pw_stage == 0 && !app.lock_error.is_empty());
        type_text(&mut app, "1", &tx, &s);
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        type_text(&mut app, "1", &tx, &s);
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert_eq!(s.password(), "1");
        assert!(app.mode == Mode::Settings);

        // блокировка теперь открывается новым паролем
        app.handle_key(key(KeyCode::Esc, none), &tx, &s);
        app.handle_key(key(KeyCode::F(2), none), &tx, &s);
        type_text(&mut app, "secret", &tx, &s);
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert!(app.mode == Mode::Locked);
        type_text(&mut app, "1", &tx, &s);
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert!(app.mode == Mode::Chat);
    }

    #[test]
    fn proxy_command_updates_settings() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        slow(&mut app);
        type_text(&mut app, "/proxy 127.0.0.1:10808", &tx, &s);
        slow(&mut app);
        app.handle_key(key(KeyCode::Enter, KeyModifiers::NONE), &tx, &s);
        assert_eq!(app.settings.proxy_mode, ProxyMode::Manual);
        assert_eq!(app.settings.proxy_manual, "127.0.0.1:10808");
        assert_eq!(s.settings().proxy_manual, "127.0.0.1:10808");
        app.input = "/proxy off".into();
        app.cursor = 10;
        app.submit(&tx_dummy(), &s);
        assert_eq!(app.settings.proxy_mode, ProxyMode::Off);
        assert!(app.input.is_empty());
    }

    fn tx_dummy() -> mpsc::UnboundedSender<Cmd> {
        mpsc::unbounded_channel().0
    }

    #[test]
    fn window_title_shows_unread_but_not_when_locked() {
        let mut app = app_with_chats();
        assert_eq!(app.title(), "Tg-Lite (2)");
        app.lock();
        assert_eq!(app.title(), "Tg-Lite");
    }

    #[test]
    fn renders_all_screens_at_various_sizes() {
        let mut app = app_with_chats();
        app.input = "набираю ответ".into();
        app.cursor = app.input_len();

        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let out = dump(&term);
        for needle in [
            "Tg-Lite",
            "Все",
            "Личные",
            "Группы",
            "Каналы",
            "Мама",
            "Мой канал",
            "Привет, как дела?",
            "Всё хорошо!",
            "набираю ответ",
            "F1 справка",
        ] {
            assert!(out.contains(needle), "нет «{needle}»:\n{out}");
        }

        // маленькие окна не должны паниковать (в том числе меньше стандартных 80x25)
        for (w, h) in [(80, 25), (40, 10), (20, 6), (12, 4)] {
            let mut small = Terminal::new(TestBackend::new(w, h)).unwrap();
            for mode in [Mode::Chat, Mode::Pick, Mode::Forward, Mode::Overlay, Mode::Help, Mode::Settings, Mode::NewPassword, Mode::Locked]
            {
                app.mode = mode;
                app.reply = Some((1, "Мама: привет".into()));
                app.overlay = Some(Overlay {
                    kind: OverlayKind::Links,
                    chat: None,
                    title: " Ссылки ".into(),
                    items: vec![OverlayItem {
                        label: "Мама: https://example.com/очень/длинная/ссылка".into(),
                        data: "https://example.com".into(),
                        id: 0,
                    }],
                    sel: 0,
                    confirm: None,
                });
                small.draw(|f| draw(f, &mut app)).unwrap();
            }
        }
        app.mode = Mode::Chat;

        app.mode = Mode::Help;
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(dump(&term).contains("Справка"));

        app.mode = Mode::Chat;
        app.lock();
        app.lock_input = "abc".into();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let out = dump(&term);
        assert!(out.contains("Заблокировано"), "{out}");
        assert!(out.contains("●●●"), "{out}");
        for secret in ["Привет, как дела?", "Мама", "Мой канал", "Рабочий чат"] {
            assert!(!out.contains(secret), "на экране блокировки видно «{secret}»:\n{out}");
        }
    }

    #[test]
    fn many_tabs_scroll_without_panic() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let folders: Vec<Folder> = (0..12)
            .map(|i| Folder {
                id: i + 1, title: format!("Очень длинное название папки {i}"), groups: true, ..Default::default()
            })
            .collect();
        app.handle_event(Event::Folders(folders), &tx);
        app.tab = Tab::Folder(12);
        let mut term = Terminal::new(TestBackend::new(70, 20)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let out = dump(&term);
        let first_line = out.lines().next().unwrap();
        assert!(first_line.starts_with('‹'), "слева есть скрытые вкладки: {first_line}");
        assert!(first_line.contains("Очень длинное н…"), "активная вкладка должна быть видна: {first_line}");
    }

    fn drain(rx: &mut mpsc::UnboundedReceiver<Cmd>) -> Vec<Cmd> {
        let mut v = Vec::new();
        while let Ok(c) = rx.try_recv() {
            v.push(c);
        }
        v
    }

    fn submit_text(app: &mut App, text: &str, tx: &mpsc::UnboundedSender<Cmd>, s: &Arc<Saver>) {
        app.clear_input();
        type_text(app, text, tx, s);
        slow(app);
        app.handle_key(key(KeyCode::Enter, KeyModifiers::NONE), tx, s);
    }

    #[test]
    fn find_filters_chats_and_esc_resets() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        submit_text(&mut app, "/find кана", &tx, &s);
        assert_eq!(app.visible_ids().len(), 1);
        assert_eq!(app.selected().unwrap().name, "Мой канал", "выбор переходит на найденный чат");
        // регистр не важен
        submit_text(&mut app, "/find МАМ", &tx, &s);
        assert_eq!(app.selected().unwrap().name, "Мама");
        // ничего не найдено
        submit_text(&mut app, "/find zzz", &tx, &s);
        assert!(app.visible_ids().is_empty());
        assert!(app.selected().is_none());
        // Esc при пустом поле сбрасывает поиск
        slow(&mut app);
        app.handle_key(key(KeyCode::Esc, KeyModifiers::NONE), &tx, &s);
        assert!(app.filter.is_empty());
        assert_eq!(app.visible_ids().len(), 3);
        assert!(app.selected().is_some());
    }

    #[test]
    fn f6_prepares_find_command() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        app.handle_key(key(KeyCode::F(6), KeyModifiers::NONE), &tx, &s);
        assert_eq!(app.input, "/find ");
        app.clear_input();
        app.handle_key(key(KeyCode::Char('а'), KeyModifiers::CONTROL), &tx, &s); // Ctrl+F в русской раскладке
        assert_eq!(app.input, "/find ");
    }

    #[test]
    fn unread_tab_shows_only_unread() {
        let mut app = app_with_chats(); // у «Мамы» 2 непрочитанных
        app.tab = Tab::Unread;
        app.ensure_selection();
        assert_eq!(app.visible_ids().len(), 1);
        assert_eq!(app.selected().unwrap().name, "Мама");
        assert_eq!(app.unread_in(&Tab::Unread), 1);
    }

    #[test]
    fn pinned_chats_go_first_and_persist_in_settings() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        app.move_selection(2); // «Рабочий чат»
        submit_text(&mut app, "/pin", &tx, &s);
        let names: Vec<String> = app.visible_idx().iter().map(|&i| app.chats[i].name.clone()).collect();
        assert_eq!(names, vec!["Рабочий чат", "Мама", "Мой канал"]);
        assert_eq!(app.settings.pinned.len(), 1);
        assert_eq!(s.settings().pinned, app.settings.pinned, "закрепление сохраняется в хранилище");
        // повторный /pin открепляет
        submit_text(&mut app, "/pin", &tx, &s);
        assert!(app.settings.pinned.is_empty());
        let names: Vec<String> = app.visible_idx().iter().map(|&i| app.chats[i].name.clone()).collect();
        assert_eq!(names, vec!["Мама", "Мой канал", "Рабочий чат"]);
    }

    #[test]
    fn drafts_are_kept_per_chat() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        type_text(&mut app, "черновик для мамы", &tx, &s);
        app.move_selection(1); // канал
        assert_eq!(app.input, "", "в другом чате поле пустое");
        type_text(&mut app, "пост", &tx, &s);
        app.move_selection(-1);
        assert_eq!(app.input, "черновик для мамы");
        assert_eq!(app.cursor, app.input_len());
        app.move_selection(1);
        assert_eq!(app.input, "пост");
    }

    #[test]
    fn typed_text_survives_when_no_chat_selected() {
        let mut app = App::new(Settings::default());
        app.input = "/find".into();
        app.cursor = 5;
        app.chats = vec![chat(1, "Мама", Kind::User, 0)];
        app.ensure_selection();
        assert_eq!(app.input, "/find", "текст не должен пропадать, когда чат появляется");
    }

    #[test]
    fn edit_and_delete_last_own_message() {
        let mut app = app_with_chats(); // у «Мамы» последнее своё сообщение: id 2
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        // /edit без текста подставляет старый текст
        submit_text(&mut app, "/edit", &tx, &s);
        assert_eq!(app.input, "/edit Всё хорошо!");
        // правка
        submit_text(&mut app, "/edit Всё отлично!", &tx, &s);
        match drain(&mut rx).pop() {
            Some(Cmd::Edit { id, text, .. }) => assert_eq!((id, text.as_str()), (2, "Всё отлично!")),
            _ => panic!("ожидалась команда Edit"),
        }
        // удаление требует подтверждения
        submit_text(&mut app, "/del", &tx, &s);
        assert!(drain(&mut rx).is_empty(), "первый /del только спрашивает");
        assert!(app.status.contains("Повторите /del"));
        submit_text(&mut app, "/del", &tx, &s);
        match drain(&mut rx).pop() {
            Some(Cmd::Delete { id, .. }) => assert_eq!(id, 2),
            _ => panic!("ожидалась команда Delete"),
        }
    }

    #[test]
    fn delete_confirmation_resets_after_other_input() {
        let mut app = app_with_chats();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        submit_text(&mut app, "/del", &tx, &s);
        submit_text(&mut app, "обычное сообщение", &tx, &s); // сбрасывает ожидание
        drain(&mut rx);
        submit_text(&mut app, "/del", &tx, &s);
        assert!(drain(&mut rx).is_empty(), "после другого действия снова нужно подтверждение");
    }

    #[test]
    fn del_without_own_messages_is_refused() {
        let mut app = app_with_chats();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        app.move_selection(1); // канал без загруженной истории
        submit_text(&mut app, "/del", &tx, &s);
        assert!(drain(&mut rx).is_empty());
        assert!(app.status.contains("Нет вашего сообщения"));
    }

    #[test]
    fn note_goes_to_saved_messages_from_any_chat() {
        let mut app = app_with_chats();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        let mut me = chat(77, "Избранное", Kind::User, 0);
        me.is_self = true;
        app.chats.push(me.clone());
        submit_text(&mut app, "/note купить хлеб", &tx, &s);
        match drain(&mut rx).pop() {
            Some(Cmd::Send { chat, text, .. }) => {
                assert_eq!(chat.id, me.id);
                assert_eq!(text, "купить хлеб");
            }
            _ => panic!("ожидалась отправка в Избранное"),
        }
    }

    #[test]
    fn readall_marks_everything_read() {
        let mut app = app_with_chats();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        app.chats[1].unread = 5;
        submit_text(&mut app, "/readall", &tx, &s);
        assert_eq!(app.total_unread(), 0);
        let reads = drain(&mut rx).into_iter().filter(|c| matches!(c, Cmd::MarkRead(_))).count();
        assert_eq!(reads, 2);
    }

    #[test]
    fn sent_message_does_not_create_partial_history() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let channel = app.chats[1].id; // история не загружена
        app.handle_event(Event::Sent { chat: channel, msg: msg(5, true, "Я", "пост"), scheduled: false }, &tx);
        assert!(!app.histories.contains_key(&channel), "иначе история канала уже не загрузилась бы целиком");
    }

    #[test]
    fn lock_forgets_half_entered_password() {
        let mut app = app_with_chats();
        app.mode = Mode::NewPassword;
        app.pw_stage = 1;
        app.pw_first = "secret".into();
        app.lock();
        assert!(app.pw_first.is_empty());
        assert_eq!(app.pw_stage, 0);
    }

    #[test]
    fn export_contains_messages() {
        let text = export_text("Мама", &[msg(1, false, "Мама", "Привет\nкак дела"), msg(2, true, "Я", "хорошо")]);
        assert!(text.starts_with("Переписка: Мама"));
        assert!(text.contains("Мама: Привет\n    как дела"));
        assert!(text.contains("Я: хорошо"));
    }

    #[test]
    fn unknown_slash_text_is_still_a_message() {
        let mut app = app_with_chats();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        submit_text(&mut app, "/привет всем", &tx, &s);
        assert!(matches!(drain(&mut rx).pop(), Some(Cmd::Send { .. })));
    }

    fn press(app: &mut App, code: KeyCode, tx: &mpsc::UnboundedSender<Cmd>, s: &Arc<Saver>) {
        slow(app);
        app.handle_key(key(code, KeyModifiers::NONE), tx, s);
    }

    #[test]
    fn pick_mode_reply_sends_reply_to() {
        let mut app = app_with_chats(); // у «Мамы»: сообщения 1 (её) и 2 (моё)
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        press(&mut app, KeyCode::F(7), &tx, &s);
        assert_eq!(app.mode, Mode::Pick);
        assert_eq!(app.pick, 1, "выбор начинается с последнего сообщения");
        press(&mut app, KeyCode::Up, &tx, &s);
        press(&mut app, KeyCode::Char('r'), &tx, &s);
        assert_eq!(app.mode, Mode::Chat);
        assert_eq!(app.reply.as_ref().map(|r| r.0), Some(1));
        submit_text(&mut app, "и тебе привет", &tx, &s);
        match drain(&mut rx).pop() {
            Some(Cmd::Send { text, reply_to, .. }) => assert_eq!((text.as_str(), reply_to), ("и тебе привет", Some(1))),
            _ => panic!("ожидалась отправка"),
        }
        assert!(app.reply.is_none(), "ответ сбрасывается после отправки");
    }

    #[test]
    fn pick_mode_works_in_russian_layout_and_esc_cancels_reply() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        press(&mut app, KeyCode::F(7), &tx, &s);
        press(&mut app, KeyCode::Char('к'), &tx, &s); // R на русской раскладке
        assert!(app.reply.is_some());
        press(&mut app, KeyCode::Esc, &tx, &s);
        assert!(app.reply.is_none());
    }

    #[test]
    fn pick_mode_edit_loads_text_and_sends_edit() {
        let mut app = app_with_chats();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        press(&mut app, KeyCode::F(7), &tx, &s);
        press(&mut app, KeyCode::Up, &tx, &s);
        press(&mut app, KeyCode::Char('e'), &tx, &s);
        assert_eq!(app.status, "Можно править только свои сообщения", "чужое сообщение править нельзя");
        press(&mut app, KeyCode::Down, &tx, &s);
        press(&mut app, KeyCode::Char('e'), &tx, &s);
        assert_eq!(app.input, "Всё хорошо!");
        assert_eq!(app.edit_target, Some(2));
        app.clear_input();
        app.edit_target = Some(2);
        submit_text_keep_edit(&mut app, "Всё отлично", &tx, &s);
        match drain(&mut rx).pop() {
            Some(Cmd::Edit { id, text, .. }) => assert_eq!((id, text.as_str()), (2, "Всё отлично")),
            _ => panic!("ожидалась команда Edit"),
        }
    }

    fn submit_text_keep_edit(app: &mut App, text: &str, tx: &mpsc::UnboundedSender<Cmd>, s: &Arc<Saver>) {
        let target = app.edit_target;
        type_text(app, text, tx, s);
        app.edit_target = target;
        slow(app);
        app.handle_key(key(KeyCode::Enter, KeyModifiers::NONE), tx, s);
    }

    #[test]
    fn pick_mode_delete_needs_second_press() {
        let mut app = app_with_chats();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        press(&mut app, KeyCode::F(7), &tx, &s);
        press(&mut app, KeyCode::Char('d'), &tx, &s);
        assert!(drain(&mut rx).is_empty());
        assert!(app.status.contains("ещё раз"));
        press(&mut app, KeyCode::Char('в'), &tx, &s); // русская раскладка
        assert!(matches!(drain(&mut rx).pop(), Some(Cmd::Delete { id: 2, .. })));
        assert_eq!(app.mode, Mode::Chat);
    }

    #[test]
    fn forward_goes_to_chosen_chat_and_returns() {
        let mut app = app_with_chats();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        let mama = app.selected().unwrap().id;
        press(&mut app, KeyCode::F(7), &tx, &s);
        press(&mut app, KeyCode::Char('f'), &tx, &s);
        assert_eq!(app.mode, Mode::Forward);
        press(&mut app, KeyCode::Down, &tx, &s); // «Мой канал»
        press(&mut app, KeyCode::Enter, &tx, &s);
        match drain(&mut rx).into_iter().find(|c| matches!(c, Cmd::Forward { .. })) {
            Some(Cmd::Forward { src, dst, id }) => {
                assert_eq!(src.id, mama);
                assert_eq!(dst.name, "Мой канал");
                assert_eq!(id, 2);
            }
            _ => panic!("ожидалась пересылка"),
        }
        assert_eq!(app.mode, Mode::Chat);
        assert_eq!(app.selected().unwrap().id, mama, "после пересылки возвращаемся в исходный чат");
    }

    #[test]
    fn forward_can_be_cancelled() {
        let mut app = app_with_chats();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        press(&mut app, KeyCode::F(7), &tx, &s);
        press(&mut app, KeyCode::Char('f'), &tx, &s);
        press(&mut app, KeyCode::Down, &tx, &s);
        press(&mut app, KeyCode::Esc, &tx, &s);
        assert!(drain(&mut rx).iter().all(|c| !matches!(c, Cmd::Forward { .. })));
        assert_eq!(app.selected().unwrap().name, "Мама");
    }

    #[test]
    fn long_message_is_refused_and_kept() {
        let mut app = app_with_chats();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        let long = "а".repeat(MAX_MESSAGE_CHARS + 1);
        app.input = long.clone();
        app.cursor = app.input_len();
        slow(&mut app);
        app.handle_key(key(KeyCode::Enter, KeyModifiers::NONE), &tx, &s);
        assert!(drain(&mut rx).is_empty());
        assert_eq!(app.input, long, "длинный пост не пропадает");
        assert!(app.status.contains("4096"));
    }

    #[test]
    fn tab_completes_commands() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        type_text(&mut app, "/rea", &tx, &s);
        press(&mut app, KeyCode::Tab, &tx, &s);
        assert_eq!(app.input, "/readall ");
        app.clear_input();
        type_text(&mut app, "/p", &tx, &s);
        press(&mut app, KeyCode::Tab, &tx, &s);
        assert!(app.input.starts_with("/p"), "несколько вариантов: дописывается общее начало");
        assert!(app.status.contains("/pin") && app.status.contains("/passwd"));
        assert!(app.tab == Tab::All, "Tab с командой не переключает вкладки");
    }

    #[test]
    fn self_destruct_after_wrong_passwords() {
        let mut app = app_with_chats();
        app.settings.wipe_after = 3;
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        s.save().unwrap();
        app.lock();
        for i in 1..=3 {
            type_text(&mut app, "неверный", &tx, &s);
            press(&mut app, KeyCode::Enter, &tx, &s);
            if i < 3 {
                assert!(!s.is_wiped());
                assert!(app.lock_error.contains("Осталось попыток"), "{}", app.lock_error);
            }
        }
        assert!(s.is_wiped());
        assert!(app.should_quit);
    }

    #[test]
    fn right_password_resets_attempts_and_wipe_is_off_by_default() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        app.lock();
        for _ in 0..5 {
            type_text(&mut app, "нет", &tx, &s);
            press(&mut app, KeyCode::Enter, &tx, &s);
        }
        assert!(!s.is_wiped(), "по умолчанию данные не стираются");
        type_text(&mut app, "secret", &tx, &s);
        press(&mut app, KeyCode::Enter, &tx, &s);
        assert_eq!(app.mode, Mode::Chat);
        assert_eq!(app.wrong_attempts, 0);
    }

    #[test]
    fn messages_show_reply_quote_forward_and_edited_mark() {
        let mut quoted = msg(1, false, "Мама", "Купи хлеб");
        quoted.edited = true;
        let mut reply = msg(2, true, "Я", "Куплю");
        reply.reply_to = Some(1);
        let mut fwd = msg(3, false, "Мама", "новость");
        fwd.fwd = Some(crate::tg::Fwd { name: Some("Канал Новостей".into()), from: None });
        let (lines, ranges) = build_message_lines(&[quoted, reply, fwd], 60, 72, Some(1), Color::Blue);
        let text: String = lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>() + "\n").collect();
        assert!(text.contains("10:01 (изм.)"), "{text}");
        assert!(text.contains("┌ Мама: Купи хлеб"), "{text}");
        assert!(text.contains("↪ переслано от Канал Новостей"), "{text}");
        assert_eq!(ranges.len(), 3);
        assert!(ranges[0].1 <= ranges[1].0 && ranges[1].1 <= ranges[2].0, "диапазоны не пересекаются");
        // выбранное сообщение подсвечено, остальные нет
        assert!(lines[ranges[1].0].style.bg.is_some());
        assert!(lines[ranges[0].0].style.bg.is_none());
    }

    #[test]
    fn forward_sender_name_is_resolved_from_chat_list() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mama = app.chats[0].id;
        let mut m = msg(7, false, "Мама", "переслано");
        m.fwd = Some(crate::tg::Fwd { name: None, from: Some(mama) });
        app.handle_event(Event::History { chat: mama, msgs: vec![m] }, &tx);
        let name = app.histories[&mama][0].fwd.as_ref().unwrap().name.clone();
        assert_eq!(name.as_deref(), Some("Мама"));
    }

    #[test]
    fn local_edit_keeps_attachment_label() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mama = app.chats[0].id;
        let mut photo = msg(5, true, "Я", "[фото] старая подпись");
        photo.caption = Some("старая подпись".into());
        app.histories.get_mut(&mama).unwrap().push(photo);
        let mut edit = msg(5, true, "Я", "новая подпись");
        edit.caption = None;
        app.handle_event(Event::Edited { chat: mama, msg: edit }, &tx);
        let m = app.histories[&mama].iter().find(|m| m.id == 5).unwrap();
        assert_eq!(m.text, "[фото] новая подпись");
        assert_eq!(m.caption.as_deref(), Some("новая подпись"));
        assert!(m.edited);
    }

    #[test]
    fn settings_have_wipe_row() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        app.mode = Mode::Settings;
        app.settings_row = 6;
        press(&mut app, KeyCode::Right, &tx, &s);
        assert_eq!(app.settings.wipe_after, 3);
        assert_eq!(s.settings().wipe_after, 3);
    }

    #[test]
    fn send_command_parsing() {
        assert_eq!(parse_send(r#""C:\мои файлы\a.pdf" смотри"#), Some((r"C:\мои файлы\a.pdf".into(), "смотри".into())));
        assert_eq!(parse_send("'/tmp/a b.txt'"), Some(("/tmp/a b.txt".into(), String::new())));
        assert_eq!(parse_send("/tmp/nonexistent.txt привет всем"), Some(("/tmp/nonexistent.txt".into(), "привет всем".into())));
        assert_eq!(parse_send("  "), None);
        assert_eq!(parse_send("\"без закрывающей"), None);
    }

    #[test]
    fn send_file_command_checks_the_file() {
        let mut app = app_with_chats();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        let path = std::env::temp_dir().join(format!("tglt-send-{}.txt", std::process::id()));
        std::fs::write(&path, b"hello").unwrap();
        submit_text(&mut app, &format!("/send \"{}\" мой файл", path.display()), &tx, &s);
        match drain(&mut rx).pop() {
            Some(Cmd::SendFile { path: p, caption, .. }) => {
                assert_eq!(p, path);
                assert_eq!(caption, "мой файл");
            }
            _ => panic!("ожидалась отправка файла"),
        }
        let _ = std::fs::remove_file(&path);
        // несуществующий файл: команды нет, текст остаётся для исправления
        submit_text(&mut app, "/send \"/нет/такого/файла.bin\"", &tx, &s);
        assert!(drain(&mut rx).is_empty());
        assert!(app.status.contains("файл не найден"), "{}", app.status);
        assert!(app.input.starts_with("/send"));
    }

    fn msg_with_attachment(id: i32, label_text: &str, caption: &str) -> ChatMsg {
        let mut m = msg(id, false, "Мама", label_text);
        m.caption = Some(caption.into());
        m
    }

    #[test]
    fn save_command_and_pick_s_download_attachment() {
        let mut app = app_with_chats();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        submit_text(&mut app, "/save", &tx, &s);
        assert!(drain(&mut rx).is_empty());
        assert!(app.status.contains("нет вложений"));
        let mama = app.chats[0].id;
        app.histories.get_mut(&mama).unwrap().push(msg_with_attachment(9, "[фото] вот", "вот"));
        submit_text(&mut app, "/save", &tx, &s);
        assert!(matches!(drain(&mut rx).pop(), Some(Cmd::Save { id: 9, .. })));
        // в режиме выбора: на сообщении без вложения S только подсказывает
        press(&mut app, KeyCode::F(7), &tx, &s);
        press(&mut app, KeyCode::Up, &tx, &s);
        press(&mut app, KeyCode::Char('s'), &tx, &s);
        assert!(drain(&mut rx).is_empty());
        assert!(app.pick_note.contains("нет вложения"));
        press(&mut app, KeyCode::Down, &tx, &s);
        press(&mut app, KeyCode::Char('ы'), &tx, &s); // русская раскладка
        assert!(matches!(drain(&mut rx).pop(), Some(Cmd::Save { id: 9, .. })));
    }

    #[test]
    fn links_overlay_lists_unique_links_and_closes() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        let mama = app.chats[0].id;
        let h = app.histories.get_mut(&mama).unwrap();
        h.push(msg(3, false, "Мама", "глянь https://example.com/a и https://rust-lang.org."));
        h.push(msg(4, false, "Мама", "ещё раз https://example.com/a"));
        submit_text(&mut app, "/links", &tx, &s);
        assert_eq!(app.mode, Mode::Overlay);
        let ov = app.overlay.as_ref().unwrap();
        let urls: Vec<&str> = ov.items.iter().map(|i| i.data.as_str()).collect();
        assert_eq!(urls, vec!["https://example.com/a", "https://rust-lang.org"]);
        press(&mut app, KeyCode::Down, &tx, &s);
        assert_eq!(app.overlay.as_ref().unwrap().sel, 1);
        press(&mut app, KeyCode::Down, &tx, &s);
        assert_eq!(app.overlay.as_ref().unwrap().sel, 1, "за пределы списка не выходим");
        press(&mut app, KeyCode::Esc, &tx, &s);
        assert_eq!(app.mode, Mode::Chat);
        // без ссылок
        let mut quiet = app_with_chats();
        submit_text(&mut quiet, "/links", &tx, &s);
        assert_eq!(quiet.mode, Mode::Chat);
        assert!(quiet.status.contains("нет ссылок"));
    }

    #[test]
    fn pick_l_opens_links_of_message_only() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        let mama = app.chats[0].id;
        app.histories.get_mut(&mama).unwrap().push(msg(5, false, "Мама", "https://example.com/x"));
        press(&mut app, KeyCode::F(7), &tx, &s);
        press(&mut app, KeyCode::Char('l'), &tx, &s);
        assert_eq!(app.mode, Mode::Overlay);
        assert_eq!(app.overlay.as_ref().unwrap().items.len(), 1);
    }

    #[test]
    fn scheduled_list_and_two_step_cancel() {
        let mut app = app_with_chats();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        let chat = app.selected().unwrap().id;
        submit_text(&mut app, "/scheduled", &tx, &s);
        assert!(matches!(drain(&mut rx).pop(), Some(Cmd::ListScheduled(_))));
        app.handle_event(
            Event::Scheduled {
                chat,
                items: vec![
                    crate::tg::ScheduledMsg { id: 11, date: 1_800_000_000, text: "первый".into() },
                    crate::tg::ScheduledMsg { id: 12, date: 1_800_003_600, text: "второй".into() },
                ],
            },
            &tx,
        );
        assert_eq!(app.mode, Mode::Overlay);
        press(&mut app, KeyCode::Enter, &tx, &s);
        assert!(drain(&mut rx).is_empty(), "первое нажатие только просит подтверждения");
        press(&mut app, KeyCode::Enter, &tx, &s);
        match drain(&mut rx).pop() {
            Some(Cmd::DeleteScheduled { id, .. }) => assert_eq!(id, 11),
            _ => panic!("ожидалась отмена отложенного"),
        }
        assert_eq!(app.overlay.as_ref().unwrap().items.len(), 1);
        // другая клавиша между нажатиями сбрасывает подтверждение
        press(&mut app, KeyCode::Enter, &tx, &s);
        press(&mut app, KeyCode::Up, &tx, &s);
        press(&mut app, KeyCode::Enter, &tx, &s);
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn empty_schedule_does_not_open_overlay() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let chat = app.selected().unwrap().id;
        app.handle_event(Event::Scheduled { chat, items: vec![] }, &tx);
        assert_eq!(app.mode, Mode::Chat);
        assert!(app.status.contains("нет"));
    }

    #[test]
    fn typing_indicator_shows_expires_and_clears_on_message() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mama = app.chats[0].id;
        app.handle_event(Event::Typing { chat: mama, from: None, kind: crate::tg::TypingKind::Text }, &tx);
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(dump(&term).contains("печатает…"));
        // сообщение от собеседника убирает индикатор
        app.handle_event(Event::Incoming { chat: mama, chat_name: "Мама".into(), msg: msg(20, false, "Мама", "привет") }, &tx);
        assert!(app.typing.is_empty());
        // устаревший индикатор исчезает сам
        app.typing.insert(mama, (Instant::now() - Duration::from_secs(10), "печатает…".into()));
        assert!(app.expire_typing());
        assert!(app.typing.is_empty());
        assert!(!app.expire_typing());
        // «остановился»
        app.handle_event(Event::Typing { chat: mama, from: None, kind: crate::tg::TypingKind::Voice }, &tx);
        app.handle_event(Event::Typing { chat: mama, from: None, kind: crate::tg::TypingKind::Stop }, &tx);
        assert!(app.typing.is_empty());
    }

    #[test]
    fn pinned_message_banner_is_drawn() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mama = app.chats[0].id;
        app.handle_event(Event::Pinned { chat: mama, text: Some("Не забудь про субботу".into()) }, &tx);
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let out = dump(&term);
        assert!(out.contains("закреплено: Не забудь про субботу"), "{out}");
        assert!(out.contains("Всё хорошо!"), "сообщения остаются видны");
        app.handle_event(Event::Pinned { chat: mama, text: None }, &tx);
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(!dump(&term).contains("закреплено"));
    }

    #[test]
    fn mentions_are_marked() {
        let mut m = msg(1, false, "Мама", "@я привет");
        m.mentioned = true;
        let (lines, _) = build_message_lines(&[m], 60, 72, None, Color::Blue);
        let text: String = lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>() + "\n").collect();
        assert!(text.contains("@ Мама"), "{text}");
        let mut app = app_with_chats();
        app.chats[1].mentions = 2;
        app.chats[1].unread = 3;
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(dump(&term).contains("@3"), "значок упоминаний в списке");
    }

    #[test]
    fn width_settings_change_layout() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        assert_eq!(app.settings.list_dims(), (28, 34));
        assert_eq!(app.settings.bubble_pct(), 72);
        app.mode = Mode::Settings;
        app.settings_row = 8;
        press(&mut app, KeyCode::Right, &tx, &s);
        assert_eq!(app.settings.list_width, 2);
        assert_eq!(app.settings.list_dims(), (36, 46));
        app.settings_row = 9;
        press(&mut app, KeyCode::Left, &tx, &s);
        assert_eq!(app.settings.bubble_pct(), 60);
        assert_eq!(s.settings().bubble_width, 0);
        // длинное сообщение в узкой ленте переносится на большее число строк, чем в широкой
        let long = msg(1, false, "Мама", &"слово ".repeat(40));
        let narrow = build_message_lines(std::slice::from_ref(&long), 100, 60, None, Color::Blue).0.len();
        let wide = build_message_lines(std::slice::from_ref(&long), 100, 88, None, Color::Blue).0.len();
        assert!(narrow > wide, "{narrow} <= {wide}");
    }

    #[test]
    fn new_commands_are_completed_by_tab() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        type_text(&mut app, "/sch", &tx, &s);
        press(&mut app, KeyCode::Tab, &tx, &s);
        assert_eq!(app.input, "/scheduled ");
        app.clear_input();
        type_text(&mut app, "/li", &tx, &s);
        press(&mut app, KeyCode::Tab, &tx, &s);
        assert_eq!(app.input, "/links ");
    }

    /// Выгружает кадры интерфейса в JSON: `TGLITE_SHOTS=/путь cargo test dump_ui_frames -- --ignored`.
    #[test]
    #[ignore]
    fn dump_ui_frames() {
        let Some(dir) = std::env::var_os("TGLITE_SHOTS") else { return };
        std::fs::create_dir_all(&dir).unwrap();
        crate::i18n::set(u8::from(std::env::var("TGLITE_LANG").is_ok_and(|v| v == "en")));
        let col = |c: Color| match c {
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            Color::Black => "#000000".to_string(),
            Color::White => "#ffffff".to_string(),
            Color::Gray => "#aaaaaa".to_string(),
            _ => String::new(),
        };
        let mut app = demo_app();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        let mut shots: Vec<(&str, App)> = Vec::new();
        shots.push(("chat", demo_app()));
        let mut pick = demo_app();
        press(&mut pick, KeyCode::F(7), &tx, &s);
        press(&mut pick, KeyCode::Up, &tx, &s);
        shots.push(("pick", pick));
        let mut links = demo_app();
        submit_text(&mut links, "/links", &tx, &s);
        shots.push(("links", links));
        app.mode = Mode::Settings;
        shots.push(("settings", app));
        let mut help = demo_app();
        help.mode = Mode::Help;
        shots.push(("help", help));
        let mut faq = demo_app();
        submit_text(&mut faq, "/faq", &tx, &s);
        shots.push(("faq", faq));
        for (name, mut a) in shots {
            let mut term = Terminal::new(TestBackend::new(110, 32)).unwrap();
            term.draw(|f| draw(f, &mut a)).unwrap();
            let buf = term.backend().buffer();
            let rows: Vec<Vec<String>> = (0..32)
                .map(|y| {
                    (0..110)
                        .map(|x| {
                            let c = &buf[(x, y)];
                            format!("{}|{}|{}", c.symbol(), col(c.fg), col(c.bg))
                        })
                        .collect()
                })
                .collect();
            std::fs::write(std::path::Path::new(&dir).join(format!("ui_{name}.json")), serde_json::to_string(&rows).unwrap()).unwrap();
        }
    }

    /// Демонстрационные данные для скриншотов: вымышленные имена и тексты.
    fn demo_app() -> App {
        let mut app = App::new(Settings::default());
        let anna = tr!("Анна", "Anna");
        let me = tr!("Я", "Me");
        let mut a = chat(1, anna, Kind::User, 2);
        a.presence = Some(crate::tg::Presence::Online(i64::MAX / 2));
        let b = chat(2, tr!("Новости техники", "Tech News"), Kind::Channel, 5);
        let g = chat(3, tr!("Команда проекта", "Project Team"), Kind::Group, 0);
        let c = chat(4, tr!("Рецепты", "Recipes"), Kind::Channel, 0);
        let d = chat(5, tr!("Борис", "Boris"), Kind::User, 0);
        let mut m1 = msg(1, false, anna, tr!("Привет! Ты посмотрел отчёт, который я присылала?", "Hi! Did you look at the report I sent?"));
        m1.edited = true;
        let m2 = msg(
            2,
            true,
            me,
            tr!(
                "Да, почти закончил. Вот ссылка на черновик: https://example.com/draft",
                "Yes, almost done. Here is a link to the draft: https://example.com/draft"
            ),
        );
        let mut m3 = msg(3, false, anna, tr!("Отлично, спасибо!", "Great, thanks!"));
        m3.reply_to = Some(2);
        let cap = tr!("вот финальная версия", "here is the final version");
        let mut m4 = msg(4, false, anna, &format!("{} {cap}", tr!("[файл итоги.pdf, 2 МБ]", "[file summary.pdf, 2 MB]")));
        m4.caption = Some(cap.into());
        let m5 = msg(
            5,
            true,
            me,
            tr!("Получил. Перешлю команде, как только прочитаю.", "Got it. I will forward it to the team once I have read it."),
        );
        app.histories.insert(a.id, vec![m1, m2, m3, m4, m5]);
        app.pinned_msg.insert(a.id, tr!("Созвон по проекту в пятницу в 15:00", "Project call on Friday at 3 PM").into());
        app.chats = vec![a, b, g, c, d];
        app.ensure_selection();
        app
    }
}
