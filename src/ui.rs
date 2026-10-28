//! Терминальный интерфейс: список чатов слева, переписка справа, поле ввода внизу.
//! Автоблокировка по бездействию: пока заблокировано, переписка не рисуется.
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

use crate::tg::{ChatInfo, ChatMsg, Cmd, Event, Kind};
use crate::vault::Saver;

/// Через сколько бездействия приложение блокируется само.
const LOCK_AFTER: Duration = Duration::from_secs(5 * 60);
/// Как часто сохраняем сессию на диск.
const SAVE_EVERY: Duration = Duration::from_secs(120);
/// Пауза после выбора чата перед загрузкой истории (чтобы не грузить при пролистывании).
const LOAD_DELAY: Duration = Duration::from_millis(250);
/// Enter, пришедший быстрее этого после предыдущей клавиши, считаем частью вставленного текста.
const PASTE_GAP: Duration = Duration::from_millis(25);
/// Максимум сообщений на чат в памяти.
const KEEP_MESSAGES: usize = 200;

const ACCENT: Color = Color::Rgb(42, 171, 238);
const SELECT_BG: Color = Color::Rgb(28, 92, 140);
const MUTED: Color = Color::Rgb(120, 135, 150);
const OWN: Color = Color::Rgb(110, 214, 130);
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

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Chat,
    Locked,
    Help,
}

pub struct App {
    chats: Vec<ChatInfo>,
    list_state: ListState,
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
    last_activity: Instant,
    last_key_at: Instant,
    pending_load: Option<Instant>,
    should_quit: bool,
}

// ------------------------------------------------------------------ вспомогательное

/// Буква Ctrl-комбинации независимо от раскладки (русские буквы приводятся к латинским).
fn ctrl_letter(key: &KeyEvent) -> Option<char> {
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

const MONTHS: [&str; 12] = [
    "января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября",
    "декабря",
];

fn local_date(ts: i64, tz: &jiff::tz::TimeZone) -> Option<(i16, i8, i8)> {
    let t = jiff::Timestamp::from_second(ts).ok()?;
    let d = t.to_zoned(tz.clone()).date();
    Some((d.year(), d.month(), d.day()))
}

fn day_label(date: (i16, i8, i8), today: Option<(i16, i8, i8)>) -> String {
    let (y, m, d) = date;
    let month = MONTHS.get((m as usize).wrapping_sub(1)).copied().unwrap_or("");
    match today {
        Some((ty, tm, td)) if (ty, tm, td) == (y, m, d) => "Сегодня".to_string(),
        Some((ty, _, _)) if ty == y => format!("{d} {month}"),
        _ => format!("{d} {month} {y}"),
    }
}

/// Превращает историю в строки для вывода: даты-разделители, свои сообщения справа.
fn build_message_lines(msgs: &[ChatMsg], width: usize) -> Vec<Line<'static>> {
    let tz = jiff::tz::TimeZone::system();
    let today = local_date(jiff::Timestamp::now().as_second(), &tz);
    let bubble = (width * 72 / 100).clamp(16, width.saturating_sub(4).max(16));
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut prev_day: Option<(i16, i8, i8)> = None;

    for m in msgs {
        let day = local_date(m.ts, &tz);
        if day.is_some() && day != prev_day {
            let label = format!("── {} ──", day_label(day.unwrap(), today));
            lines.push(Line::from(Span::styled(label, Style::new().fg(MUTED))).centered());
            prev_day = day;
        }

        if m.outgoing {
            lines.push(
                Line::from(vec![
                    Span::styled(format!("{} ", m.time), Style::new().fg(MUTED)),
                    Span::styled("Я", Style::new().fg(OWN).add_modifier(Modifier::BOLD)),
                ])
                .right_aligned(),
            );
            for l in wrap_lines(&m.text, bubble.saturating_sub(2)) {
                lines.push(Line::from(vec![Span::raw(l), Span::styled(" │", Style::new().fg(OWN))]).right_aligned());
            }
        } else {
            let color = name_color(&m.sender);
            lines.push(Line::from(vec![
                Span::styled(m.sender.clone(), Style::new().fg(color).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" {}", m.time), Style::new().fg(MUTED)),
            ]));
            for l in wrap_lines(&m.text, bubble.saturating_sub(2)) {
                lines.push(Line::from(vec![Span::styled("│ ", Style::new().fg(color)), Span::raw(l)]));
            }
        }
        lines.push(Line::from(""));
    }
    lines
}

// ------------------------------------------------------------------ состояние

impl App {
    fn new() -> Self {
        App {
            chats: Vec::new(),
            list_state: ListState::default(),
            histories: HashMap::new(),
            input: String::new(),
            cursor: 0,
            status: "Загружаю чаты…".into(),
            markdown: false,
            scroll: 0,
            mode: Mode::Chat,
            lock_input: String::new(),
            lock_error: String::new(),
            last_activity: Instant::now(),
            last_key_at: Instant::now() - Duration::from_secs(10),
            pending_load: None,
            should_quit: false,
        }
    }

    fn selected(&self) -> Option<&ChatInfo> {
        self.list_state.selected().and_then(|i| self.chats.get(i))
    }

    fn select(&mut self, idx: usize) {
        if self.chats.is_empty() {
            return;
        }
        let idx = idx.min(self.chats.len() - 1);
        self.list_state.select(Some(idx));
        self.scroll = 0;
        self.pending_load = Some(Instant::now());
    }

    fn lock(&mut self) {
        self.mode = Mode::Locked;
        self.lock_input.clear();
        self.lock_error.clear();
    }

    // --- поле ввода

    fn byte_idx(&self, char_idx: usize) -> usize {
        self.input.char_indices().nth(char_idx).map(|(b, _)| b).unwrap_or(self.input.len())
    }

    fn input_len(&self) -> usize {
        self.input.chars().count()
    }

    fn insert_char(&mut self, c: char) {
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
    }

    fn toggle_markdown(&mut self) {
        self.markdown = !self.markdown;
        self.status = format!("Markdown: {}", if self.markdown { "включён" } else { "выключен" });
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
                let keep = self.selected().map(|c| c.id);
                self.chats = list;
                let idx = keep.and_then(|id| self.chats.iter().position(|c| c.id == id)).unwrap_or(0);
                self.status = format!("Чатов: {}", self.chats.len());
                self.select(idx);
            }
            Event::History { chat, msgs } => {
                self.histories.insert(chat, msgs);
                if self.selected().map(|c| c.id) == Some(chat) {
                    self.scroll = 0;
                    if let Some(c) = self.selected().cloned() {
                        if c.unread > 0 {
                            let _ = cmd.send(Cmd::MarkRead(c));
                            if let Some(i) = self.list_state.selected() {
                                self.chats[i].unread = 0;
                            }
                        }
                    }
                }
            }
            Event::Incoming { chat, chat_name, msg } => {
                let is_open = self.selected().map(|c| c.id) == Some(chat);
                let outgoing = msg.outgoing;
                let added = self.push_unique(chat, msg, false);
                if let Some(pos) = self.chats.iter().position(|c| c.id == chat) {
                    if added && !outgoing {
                        if is_open {
                            if self.mode == Mode::Chat {
                                let _ = cmd.send(Cmd::MarkRead(self.chats[pos].clone()));
                            } else {
                                self.chats[pos].unread += 1;
                            }
                        } else {
                            self.chats[pos].unread += 1;
                        }
                    }
                    if added || !self.histories.contains_key(&chat) {
                        // Чат с новым сообщением поднимается наверх списка.
                        let selected_id = self.selected().map(|c| c.id);
                        let c = self.chats.remove(pos);
                        self.chats.insert(0, c);
                        if let Some(id) = selected_id {
                            if let Some(i) = self.chats.iter().position(|c| c.id == id) {
                                self.list_state.select(Some(i));
                            }
                        }
                        if !outgoing {
                            self.status = if self.mode == Mode::Locked {
                                "Новое сообщение".into()
                            } else {
                                format!("Новое сообщение: {chat_name}")
                            };
                        }
                    }
                } else {
                    // Чата ещё нет в списке (новый диалог): перезагрузим список.
                    let _ = cmd.send(Cmd::LoadDialogs);
                }
                if added && is_open {
                    self.scroll = 0;
                }
            }
            Event::Sent { chat, msg, scheduled } => {
                if scheduled {
                    self.status = "Отложенное сообщение запланировано".into();
                } else {
                    self.status = "Отправлено".into();
                    self.push_unique(chat, msg, true);
                    self.scroll = 0;
                }
            }
            Event::Edited { chat, msg } => {
                if let Some(h) = self.histories.get_mut(&chat) {
                    if let Some(m) = h.iter_mut().find(|m| m.id == msg.id) {
                        m.text = msg.text;
                    }
                }
            }
            Event::Deleted { chat, ids } => {
                for (id, h) in self.histories.iter_mut() {
                    if chat.is_none_or(|c| c == *id) {
                        h.retain(|m| !ids.contains(&m.id));
                    }
                }
            }
            Event::Status(s) => self.status = s,
            Event::Error(e) => self.status = format!("Ошибка: {e}"),
        }
    }

    fn handle_key(&mut self, key: KeyEvent, cmd: &mpsc::UnboundedSender<Cmd>, saver: &Saver) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        let since_prev = self.last_key_at.elapsed();
        self.last_key_at = Instant::now();
        self.last_activity = Instant::now();

        match self.mode {
            Mode::Locked => {
                match key.code {
                    KeyCode::F(10) => self.should_quit = true,
                    KeyCode::Enter => {
                        if self.lock_input == saver.password {
                            self.mode = Mode::Chat;
                            self.lock_input.clear();
                        } else {
                            self.lock_error = "Неверный пароль".into();
                            self.lock_input.clear();
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
            Mode::Chat => {}
        }

        // Ctrl-комбинации (запасной вариант; работают и в русской раскладке).
        if let Some(c) = ctrl_letter(&key) {
            match c {
                'q' => self.should_quit = true,
                'l' => self.lock(),
                'r' => self.refresh(cmd),
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
            KeyCode::F(5) => self.refresh(cmd),
            KeyCode::F(10) => self.should_quit = true,
            // Shift+Enter / Alt+Enter — новая строка. Enter внутри вставленного текста — тоже.
            KeyCode::Enter if shift || alt || since_prev < PASTE_GAP => self.insert_char('\n'),
            KeyCode::Enter => self.submit(cmd),
            KeyCode::Tab => {
                self.insert_char(' ');
                self.insert_char(' ');
            }
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input_len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input_len(),
            KeyCode::Up => {
                if let Some(i) = self.list_state.selected() {
                    self.select(i.saturating_sub(1));
                }
            }
            KeyCode::Down => {
                if let Some(i) = self.list_state.selected() {
                    self.select(i + 1);
                }
            }
            KeyCode::PageUp => self.scroll += 8,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(8),
            KeyCode::Esc => self.clear_input(),
            KeyCode::Char(c) => self.insert_char(c),
            _ => {}
        }
    }

    fn refresh(&mut self, cmd: &mpsc::UnboundedSender<Cmd>) {
        self.status = "Обновляю список чатов…".into();
        let _ = cmd.send(Cmd::LoadDialogs);
    }

    /// Отправка. Команды: `/later <минуты> <текст>`, `/md`, `/lock`, `/quit`, `/refresh`, `/help`.
    fn submit(&mut self, cmd: &mpsc::UnboundedSender<Cmd>) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        let simple = match text.as_str() {
            "/md" => Some(0),
            "/lock" => Some(1),
            "/quit" | "/exit" => Some(2),
            "/refresh" => Some(3),
            "/help" | "/?" => Some(4),
            _ => None,
        };
        if let Some(c) = simple {
            self.clear_input();
            match c {
                0 => self.toggle_markdown(),
                1 => self.lock(),
                2 => self.should_quit = true,
                3 => self.refresh(cmd),
                _ => self.mode = Mode::Help,
            }
            return;
        }
        let Some(chat) = self.selected().cloned() else {
            self.status = "Сначала выберите чат".into();
            return;
        };
        let (body, delay) = match parse_later(&text) {
            Some((min, body)) => (body, Some(min)),
            None => (text, None),
        };
        if body.is_empty() {
            self.status = "Пустое сообщение".into();
            return;
        }
        self.status = if delay.is_some() { "Планирую…".into() } else { "Отправляю…".into() };
        let _ = cmd.send(Cmd::Send { chat, text: body, markdown: self.markdown, delay_minutes: delay });
        self.clear_input();
    }
}

// ------------------------------------------------------------------ рисование

fn draw(f: &mut Frame, app: &mut App) {
    match app.mode {
        Mode::Locked => draw_lock(f, app),
        Mode::Help => {
            draw_main(f, app);
            draw_help(f);
        }
        Mode::Chat => draw_main(f, app),
    }
}

fn draw_main(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let left_w = (area.width * 28 / 100).clamp(16, 34);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(left_w), Constraint::Min(10)])
        .split(area);

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

fn draw_chat_list(f: &mut Frame, app: &mut App, area: Rect) {
    let inner_w = area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = app
        .chats
        .iter()
        .map(|c| {
            let (tag, tag_color) = match c.kind {
                Kind::User => (" ", MUTED),
                Kind::Group => ("Г", Color::Rgb(255, 196, 90)),
                Kind::Channel => ("К", ACCENT),
            };
            let badge = if c.unread > 0 { format!(" {} ", c.unread.min(999)) } else { String::new() };
            let name_w = inner_w.saturating_sub(2 + display_width(&badge));
            let name = truncate(&c.name, name_w);
            let pad = name_w.saturating_sub(display_width(&name));
            let mut spans = vec![
                Span::styled(tag.to_string(), Style::new().fg(tag_color).add_modifier(Modifier::BOLD)),
                Span::raw(" "),
                Span::styled(
                    format!("{name}{}", " ".repeat(pad)),
                    if c.unread > 0 { Style::new().add_modifier(Modifier::BOLD) } else { Style::new() },
                ),
            ];
            if !badge.is_empty() {
                spans.push(Span::styled(badge, Style::new().bg(ACCENT).fg(Color::Black).add_modifier(Modifier::BOLD)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(MUTED))
                .title(Span::styled(" Tg-Lite ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))),
        )
        .highlight_style(Style::new().bg(SELECT_BG).fg(Color::White).add_modifier(Modifier::BOLD));
    f.render_stateful_widget(list, area, &mut app.list_state);
}

fn draw_messages(f: &mut Frame, app: &mut App, area: Rect) {
    let title = match app.selected() {
        Some(c) => {
            let kind = match c.kind {
                Kind::User => "",
                Kind::Group => " · группа",
                Kind::Channel => " · канал",
            };
            Line::from(vec![
                Span::styled(format!(" {}", c.name), Style::new().add_modifier(Modifier::BOLD)),
                Span::styled(format!("{kind} "), Style::new().fg(MUTED)),
            ])
        }
        None => Line::from(" Переписка "),
    };
    let block = Block::default().borders(Borders::ALL).border_style(Style::new().fg(MUTED)).title(title);
    let inner = block.inner(area);
    let width = inner.width.max(1) as usize;

    let lines: Vec<Line> = match app.selected().map(|c| c.id) {
        None => vec![Line::from(Span::styled("Выберите чат слева (↑ ↓)", Style::new().fg(MUTED)))],
        Some(id) => match app.histories.get(&id) {
            None => vec![Line::from(Span::styled("Загрузка…", Style::new().fg(MUTED)))],
            Some(msgs) if msgs.is_empty() => {
                vec![Line::from(Span::styled("Сообщений пока нет", Style::new().fg(MUTED)))]
            }
            Some(msgs) => build_message_lines(msgs, width),
        },
    };

    let height = inner.height as usize;
    let max_scroll = lines.len().saturating_sub(height);
    app.scroll = app.scroll.min(max_scroll);
    let end = lines.len() - app.scroll;
    let start = end.saturating_sub(height);
    let visible: Vec<Line> = lines[start..end].to_vec();
    f.render_widget(Paragraph::new(visible).block(block), area);

    if app.scroll > 0 && area.width > 12 {
        let tag = " ↑ выше · PgDn вниз ".to_string();
        let w = display_width(&tag) as u16;
        if area.width > w + 2 {
            let r = Rect { x: area.x + area.width - w - 2, y: area.y, width: w, height: 1 };
            f.render_widget(Paragraph::new(tag).style(Style::new().fg(Color::Black).bg(ACCENT)), r);
        }
    }
}

fn draw_input(f: &mut Frame, app: &App, area: Rect, layout: &InputLayout, rows: usize) {
    let mut title = vec![Span::styled(" Сообщение ", Style::new().fg(ACCENT))];
    if app.markdown {
        title.push(Span::styled("· Markdown ", Style::new().fg(OWN)));
    }
    let focus = Style::new().fg(ACCENT);
    let skip = if layout.cur_row >= rows { layout.cur_row + 1 - rows } else { 0 };
    let shown: Vec<Line> = if app.input.is_empty() {
        vec![Line::from(Span::styled(
            "Enter — отправить · Shift+Enter — новая строка · /later 30 текст — отложить",
            Style::new().fg(MUTED),
        ))]
    } else {
        layout.lines.iter().skip(skip).take(rows).map(|l| Line::from(l.as_str())).collect()
    };
    f.render_widget(
        Paragraph::new(shown).block(Block::default().borders(Borders::ALL).border_style(focus).title(Line::from(title))),
        area,
    );
    let cy = area.y + 1 + (layout.cur_row - skip) as u16;
    let cx = area.x + 1 + layout.cur_col as u16;
    if cx < area.x + area.width.saturating_sub(1) {
        f.set_cursor_position((cx, cy.min(area.y + area.height.saturating_sub(2))));
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let hint = " F1 справка · F2 замок · F10 выход ";
    let hint_w = display_width(hint) as u16;
    let (left, right) = if area.width > hint_w + 20 {
        let parts = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(10), Constraint::Length(hint_w)])
            .split(area);
        (parts[0], Some(parts[1]))
    } else {
        (area, None)
    };
    let color = if app.status.starts_with("Ошибка") { ERROR } else { MUTED };
    let status = truncate(&format!(" {}", app.status), left.width as usize);
    f.render_widget(Paragraph::new(status).style(Style::new().fg(color)), left);
    if let Some(r) = right {
        f.render_widget(Paragraph::new(hint).style(Style::new().fg(MUTED)), r);
    }
}

fn centered_rect(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

fn draw_help(f: &mut Frame) {
    let key = |k: &str, d: &str| {
        Line::from(vec![
            Span::styled(format!(" {k:<16}"), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::raw(d.to_string()),
        ])
    };
    let lines = vec![
        Line::from(""),
        key("Enter", "отправить"),
        key("Shift+Enter", "новая строка (или вставьте текст целиком)"),
        key("↑ ↓", "выбрать чат"),
        key("PgUp / PgDn", "прокрутка переписки"),
        key("← → Home End", "двигаться по тексту"),
        key("Esc", "очистить поле ввода"),
        Line::from(""),
        key("F2  или /lock", "заблокировать (запрос пароля)"),
        key("F3  или /md", "Markdown вкл/выкл"),
        key("F5  или /refresh", "обновить список чатов"),
        key("F10 или /quit", "выход"),
        key("/later 30 текст", "отложенная публикация через 30 минут"),
        Line::from(""),
        Line::from(Span::styled(" Ctrl+L, Ctrl+Q, Ctrl+T, Ctrl+R тоже работают.", Style::new().fg(MUTED))),
        Line::from(Span::styled(" Нажмите любую клавишу, чтобы закрыть.", Style::new().fg(MUTED))),
    ];
    let rect = centered_rect(f.area(), 66, lines.len() as u16 + 2);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(ACCENT))
                .title(Span::styled(" Справка ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))),
        ),
        rect,
    );
}

fn draw_lock(f: &mut Frame, app: &App) {
    let area = f.area();
    let dots = "●".repeat(app.lock_input.chars().count().min(30));
    let mut text = vec![
        Line::from(""),
        Line::from(Span::styled("Tg-Lite", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))).centered(),
        Line::from(Span::styled("Заблокировано", Style::new().fg(MUTED))).centered(),
        Line::from(""),
        Line::from("Введите пароль и нажмите Enter").centered(),
        Line::from(""),
        Line::from(Span::styled(dots, Style::new().fg(ACCENT))).centered(),
    ];
    if !app.lock_error.is_empty() {
        text.push(Line::from(""));
        text.push(Line::from(Span::styled(app.lock_error.clone(), Style::new().fg(ERROR))).centered());
    }
    text.push(Line::from(""));
    text.push(Line::from(Span::styled("F10 — выйти", Style::new().fg(MUTED))).centered());
    let rect = centered_rect(area, 46, text.len() as u16 + 2);
    f.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(MUTED))),
        rect,
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

    let mut app = App::new();
    let _ = cmd.send(Cmd::LoadDialogs);
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut last_save = Instant::now();
    // Перерисовываем только при изменениях: это заметно разгружает слабый процессор.
    let mut dirty = true;

    loop {
        if dirty {
            terminal.draw(|f| draw(f, &mut app))?;
            dirty = false;
        }
        tokio::select! {
            Some(ev) = key_rx.recv() => match ev {
                TermEvent::Key(k) if k.kind == KeyEventKind::Press => {
                    app.handle_key(k, &cmd, &saver);
                    dirty = true;
                }
                TermEvent::Resize(..) => dirty = true,
                _ => {}
            },
            Some(ev) = events.recv() => {
                app.handle_event(ev, &cmd);
                dirty = true;
            }
            _ = tick.tick() => {
                if app.mode == Mode::Chat && app.last_activity.elapsed() > LOCK_AFTER {
                    app.lock();
                    dirty = true;
                }
                if let Some(t) = app.pending_load {
                    if t.elapsed() >= LOAD_DELAY {
                        app.pending_load = None;
                        if let Some(c) = app.selected().cloned() {
                            let _ = cmd.send(Cmd::LoadHistory(c));
                        }
                        dirty = true;
                    }
                }
                if last_save.elapsed() >= SAVE_EVERY {
                    last_save = Instant::now();
                    let s = Arc::clone(&saver);
                    tokio::task::spawn_blocking(move || { let _ = s.save(); });
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
    use crate::tg::{ChatInfo, ChatMsg, Kind};
    use grammers_client::session::types::PeerId;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn chat(id: i64, name: &str, kind: Kind, unread: i32) -> ChatInfo {
        let pid = PeerId::user(id).unwrap();
        ChatInfo { id: pid, peer: pid.to_ambient_ref(), name: name.into(), kind, unread }
    }

    fn msg(id: i32, outgoing: bool, sender: &str, text: &str) -> ChatMsg {
        ChatMsg { id, ts: 1_761_650_000 + i64::from(id) * 60, outgoing, sender: sender.into(), text: text.into(), time: "10:01".into() }
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

    fn saver() -> Saver {
        use crate::vault::{SessionBlob, VaultFile, VaultSession};
        Saver {
            vault: VaultFile::new(std::env::temp_dir().join("tglt-ui-test.vault")),
            api_id: 1,
            api_hash: "x".into(),
            session: Arc::new(VaultSession::from_blob(SessionBlob::default())),
            password: "secret".into(),
        }
    }

    fn app_with_chats() -> App {
        let mut app = App::new();
        let a = chat(1, "Мама", Kind::User, 2);
        let b = chat(2, "Мой канал", Kind::Channel, 0);
        app.histories.insert(
            a.id,
            vec![msg(1, false, "Мама", "Привет, как дела?"), msg(2, true, "Я", "Всё хорошо!")],
        );
        app.chats = vec![a, b];
        app.select(0);
        app
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
        // AltGr (Ctrl+Alt) вводит обычный символ
        assert_eq!(ctrl_letter(&key(KeyCode::Char('@'), c | KeyModifiers::ALT)), None);
    }

    #[test]
    fn echo_of_own_message_is_not_duplicated() {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let id = app.chats[0].id;
        // Сначала приходит «эхо» из обновлений, потом результат отправки (и наоборот).
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
        for c in "wrong".chars() {
            app.handle_key(key(KeyCode::Char(c), none), &tx, &s);
        }
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert!(app.mode == Mode::Locked && !app.lock_error.is_empty());
        for c in "secret".chars() {
            app.handle_key(key(KeyCode::Char(c), none), &tx, &s);
        }
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert!(app.mode == Mode::Chat);

        // Ctrl+L в русской раскладке
        app.handle_key(key(KeyCode::Char('д'), KeyModifiers::CONTROL), &tx, &s);
        assert!(app.mode == Mode::Locked);
        app.handle_key(key(KeyCode::F(10), none), &tx, &s);
        assert!(app.should_quit);

        // команды в поле ввода
        let mut app = app_with_chats();
        app.last_key_at = Instant::now() - Duration::from_secs(5);
        for c in "/lock".chars() {
            app.handle_key(key(KeyCode::Char(c), none), &tx, &s);
        }
        app.last_key_at = Instant::now() - Duration::from_secs(5);
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert!(app.mode == Mode::Locked);
    }

    #[test]
    fn newline_and_paste_handling() {
        let mut app = app_with_chats();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let s = saver();
        let none = KeyModifiers::NONE;
        for c in "раз".chars() {
            app.handle_key(key(KeyCode::Char(c), none), &tx, &s);
        }
        app.handle_key(key(KeyCode::Enter, KeyModifiers::SHIFT), &tx, &s);
        for c in "два".chars() {
            app.handle_key(key(KeyCode::Char(c), none), &tx, &s);
        }
        // Enter сразу за символом (как при вставке) — тоже перенос, а не отправка
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert_eq!(app.input, "раз\nдва\n");
        assert!(rx.try_recv().is_err(), "ничего не должно уйти в сеть");
        // «настоящий» Enter после паузы отправляет
        app.last_key_at = Instant::now() - Duration::from_secs(1);
        app.handle_key(key(KeyCode::Enter, none), &tx, &s);
        assert!(matches!(rx.try_recv(), Ok(Cmd::Send { .. })));
        assert!(app.input.is_empty());
    }

    #[test]
    fn cursor_editing() {
        let mut app = App::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        let none = KeyModifiers::NONE;
        for c in "привет".chars() {
            app.handle_key(key(KeyCode::Char(c), none), &tx, &s);
        }
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
        let l = layout_input("12345678", 8, 8); // курсор в конце полной строки уходит на следующую
        assert_eq!((l.cur_row, l.cur_col), (1, 0));
    }

    #[test]
    fn renders_chat_help_and_lock_screens() {
        let mut app = app_with_chats();
        app.input = "набираю ответ".into();
        app.cursor = app.input_len();

        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let out = dump(&term);
        assert!(out.contains("Tg-Lite"), "{out}");
        assert!(out.contains("Мама"), "{out}");
        assert!(out.contains("Мой канал"), "{out}");
        assert!(out.contains("Привет, как дела?"), "{out}");
        assert!(out.contains("Всё хорошо!"), "{out}");
        assert!(out.contains("набираю ответ"), "{out}");
        assert!(out.contains("F1 справка"), "{out}");

        // маленькое окно (стандартные 80x25 и меньше) не должно паниковать
        let mut small = Terminal::new(TestBackend::new(40, 10)).unwrap();
        small.draw(|f| draw(f, &mut app)).unwrap();

        app.mode = Mode::Help;
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(dump(&term).contains("Справка"));

        app.lock();
        app.lock_input = "abc".into();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let out = dump(&term);
        assert!(out.contains("Заблокировано"), "{out}");
        assert!(out.contains("●●●"), "{out}");
        assert!(!out.contains("Привет, как дела?"), "{out}");
        assert!(!out.contains("Мама"), "{out}");
        assert!(!out.contains("Мой канал"), "{out}");
    }
}
