//! Терминальный интерфейс: список чатов слева, переписка справа, поле ввода внизу.
//! Автоблокировка по бездействию и по Ctrl+L: пока заблокировано, переписка не рисуется.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use grammers_client::session::types::PeerId;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use tokio::sync::mpsc;

use crate::tg::{ChatInfo, ChatMsg, Cmd, Event, Kind};
use crate::vault::Saver;

/// Через сколько бездействия приложение блокируется само.
const LOCK_AFTER: Duration = Duration::from_secs(5 * 60);
/// Как часто сохраняем сессию на диск.
const SAVE_EVERY: Duration = Duration::from_secs(120);
/// Пауза после выбора чата перед загрузкой истории (чтобы не грузить при пролистывании).
const LOAD_DELAY: Duration = Duration::from_millis(250);

#[derive(PartialEq)]
enum Mode {
    Chat,
    Locked,
}

pub struct App {
    chats: Vec<ChatInfo>,
    list_state: ListState,
    histories: HashMap<PeerId, Vec<ChatMsg>>,
    input: String,
    status: String,
    markdown: bool,
    /// Сколько строк прокручено вверх от самого низа переписки.
    scroll: usize,
    mode: Mode,
    lock_input: String,
    lock_error: String,
    last_activity: Instant,
    pending_load: Option<Instant>,
    should_quit: bool,
}

impl App {
    fn new() -> Self {
        App {
            chats: Vec::new(),
            list_state: ListState::default(),
            histories: HashMap::new(),
            input: String::new(),
            status: "Загружаю чаты…".into(),
            markdown: false,
            scroll: 0,
            mode: Mode::Chat,
            lock_input: String::new(),
            lock_error: String::new(),
            last_activity: Instant::now(),
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
                if let Some(h) = self.histories.get_mut(&chat) {
                    h.push(msg);
                    if h.len() > 200 {
                        h.remove(0);
                    }
                }
                if let Some(pos) = self.chats.iter().position(|c| c.id == chat) {
                    if !is_open {
                        self.chats[pos].unread += 1;
                    }
                    // Чат с новым сообщением поднимается наверх списка.
                    let selected_id = self.selected().map(|c| c.id);
                    let c = self.chats.remove(pos);
                    self.chats.insert(0, c);
                    if let Some(id) = selected_id {
                        if let Some(i) = self.chats.iter().position(|c| c.id == id) {
                            self.list_state.select(Some(i));
                        }
                    }
                    self.status = format!("Новое сообщение: {chat_name}");
                } else {
                    // Чата ещё нет в списке (новый диалог): перезагрузим список.
                    let _ = cmd.send(Cmd::LoadDialogs);
                }
            }
            Event::Sent { chat, msg, scheduled } => {
                if scheduled {
                    self.status = "Отложенное сообщение запланировано".into();
                } else {
                    self.status = "Отправлено".into();
                    self.histories.entry(chat).or_default().push(msg);
                    self.scroll = 0;
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
        self.last_activity = Instant::now();

        if self.mode == Mode::Locked {
            match key.code {
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
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.lock_input.push(c);
                }
                _ => {}
            }
            return;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('q') if ctrl => self.should_quit = true,
            KeyCode::Char('l') if ctrl => self.lock(),
            KeyCode::Char('r') if ctrl => {
                self.status = "Обновляю список чатов…".into();
                let _ = cmd.send(Cmd::LoadDialogs);
            }
            KeyCode::Char('t') if ctrl => {
                self.markdown = !self.markdown;
                self.status = format!("Разметка Markdown: {}", if self.markdown { "включена" } else { "выключена" });
            }
            KeyCode::Char('j') if ctrl => self.input.push('\n'),
            KeyCode::Enter if alt => self.input.push('\n'),
            KeyCode::Enter => self.submit(cmd),
            KeyCode::Backspace => {
                self.input.pop();
            }
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
            KeyCode::Esc => self.input.clear(),
            KeyCode::Char(c) if !ctrl => self.input.push(c),
            _ => {}
        }
    }

    /// Отправка. Команды: `/later <минуты> <текст>` — отложенная публикация, `/md` — переключить Markdown.
    fn submit(&mut self, cmd: &mpsc::UnboundedSender<Cmd>) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        if text == "/md" {
            self.markdown = !self.markdown;
            self.input.clear();
            self.status = format!("Разметка Markdown: {}", if self.markdown { "включена" } else { "выключена" });
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
        self.status = "Отправляю…".into();
        let _ = cmd.send(Cmd::Send { chat, text: body, markdown: self.markdown, delay_minutes: delay });
        self.input.clear();
    }
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

// ------------------------------------------------------------------ рисование

fn draw(f: &mut Frame, app: &mut App) {
    if app.mode == Mode::Locked {
        draw_lock(f, app);
        return;
    }
    let area = f.area();
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(((area.width / 3).clamp(20, 34)) as u16), Constraint::Min(20)])
        .split(area);

    // --- список чатов
    let items: Vec<ListItem> = app
        .chats
        .iter()
        .map(|c| {
            let tag = match c.kind {
                Kind::User => "",
                Kind::Group => "[Г] ",
                Kind::Channel => "[К] ",
            };
            let unread = if c.unread > 0 { format!(" ({})", c.unread) } else { String::new() };
            let style = if c.unread > 0 { Style::default().add_modifier(Modifier::BOLD) } else { Style::default() };
            ListItem::new(format!("{tag}{}{unread}", c.name)).style(style)
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Чаты "))
        .highlight_style(Style::default().bg(Color::Blue).fg(Color::White));
    f.render_stateful_widget(list, cols[0], &mut app.list_state);

    // --- правая часть
    let input_lines = wrap_lines(&app.input, cols[1].width.saturating_sub(4) as usize).len().clamp(1, 6) as u16;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(input_lines + 2), Constraint::Length(1)])
        .split(cols[1]);

    draw_messages(f, app, rows[0]);

    let mode = if app.markdown { " [MD]" } else { "" };
    let input_title = format!(" Сообщение{mode} ");
    let wrapped = wrap_lines(&app.input, rows[1].width.saturating_sub(4) as usize);
    let skip = wrapped.len().saturating_sub(input_lines as usize);
    let shown: Vec<Line> = wrapped.iter().skip(skip).map(|l| Line::from(l.as_str())).collect();
    f.render_widget(
        Paragraph::new(shown).block(Block::default().borders(Borders::ALL).title(input_title)),
        rows[1],
    );
    let last = wrapped.last().map(|l| textwrap::core::display_width(l)).unwrap_or(0) as u16;
    let cy = rows[1].y + 1 + (wrapped.len().min(input_lines as usize).saturating_sub(1)) as u16;
    f.set_cursor_position((rows[1].x + 1 + last, cy));

    let help = "Enter отправить · Alt+Enter/Ctrl+J перенос · ↑↓ чаты · PgUp/PgDn прокрутка · Ctrl+L замок · Ctrl+Q выход";
    let status = format!(" {} │ {help}", app.status);
    f.render_widget(Paragraph::new(status).style(Style::default().fg(Color::DarkGray)), rows[2]);
}

fn draw_messages(f: &mut Frame, app: &mut App, area: Rect) {
    let title = app.selected().map(|c| format!(" {} ", c.name)).unwrap_or_else(|| " Переписка ".into());
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    let width = inner.width.max(1) as usize;

    let mut lines: Vec<Line> = Vec::new();
    if let Some(chat) = app.selected() {
        match app.histories.get(&chat.id) {
            None => lines.push(Line::from(Span::styled("Загрузка…", Style::default().fg(Color::DarkGray)))),
            Some(msgs) => {
                for m in msgs {
                    let color = if m.outgoing { Color::Green } else { Color::Cyan };
                    lines.push(Line::from(vec![
                        Span::styled(format!("{} ", m.time), Style::default().fg(Color::DarkGray)),
                        Span::styled(m.sender.clone(), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                    ]));
                    for l in wrap_lines(&m.text, width.saturating_sub(2)) {
                        lines.push(Line::from(format!("  {l}")));
                    }
                }
            }
        }
    }

    let height = inner.height as usize;
    let max_scroll = lines.len().saturating_sub(height);
    app.scroll = app.scroll.min(max_scroll);
    let end = lines.len() - app.scroll;
    let start = end.saturating_sub(height);
    let visible: Vec<Line> = lines[start..end].to_vec();
    f.render_widget(Paragraph::new(visible).block(block), area);
}

fn draw_lock(f: &mut Frame, app: &App) {
    let area = f.area();
    let mut text = vec![
        Line::from(""),
        Line::from(Span::styled("Заблокировано", Style::default().add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from("Введите пароль и нажмите Enter:"),
        Line::from("*".repeat(app.lock_input.chars().count())),
    ];
    if !app.lock_error.is_empty() {
        text.push(Line::from(""));
        text.push(Line::from(Span::styled(app.lock_error.clone(), Style::default().fg(Color::Red))));
    }
    let h = text.len() as u16 + 2;
    let w = 44.min(area.width);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h.min(area.height),
    };
    f.render_widget(
        Paragraph::new(text).alignment(ratatui::layout::Alignment::Center).block(Block::default().borders(Borders::ALL)),
        rect,
    );
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

    loop {
        terminal.draw(|f| draw(f, &mut app))?;
        tokio::select! {
            Some(ev) = key_rx.recv() => {
                if let TermEvent::Key(k) = ev {
                    app.handle_key(k, &cmd, &saver);
                }
            }
            Some(ev) = events.recv() => app.handle_event(ev, &cmd),
            _ = tick.tick() => {
                if app.mode == Mode::Chat && app.last_activity.elapsed() > LOCK_AFTER {
                    app.lock();
                }
                if let Some(t) = app.pending_load {
                    if t.elapsed() >= LOAD_DELAY {
                        app.pending_load = None;
                        if let Some(c) = app.selected().cloned() {
                            let _ = cmd.send(Cmd::LoadHistory(c));
                        }
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
    use grammers_client::session::types::{PeerId, PeerRef};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn chat(id: i64, name: &str, kind: Kind, unread: i32) -> ChatInfo {
        let pid = PeerId::user(id).unwrap();
        ChatInfo { id: pid, peer: pid.to_ambient_ref(), name: name.into(), kind, unread }
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
        assert!(l.iter().all(|x| textwrap::core::display_width(x) <= 10), "{l:?}");
        assert_eq!(l.last().unwrap(), "шесть");
    }

    #[test]
    fn renders_chat_and_lock_screens() {
        let mut app = App::new();
        let a = chat(1, "Мама", Kind::User, 2);
        let b = chat(2, "Мой канал", Kind::Channel, 0);
        app.histories.insert(
            a.id,
            vec![
                ChatMsg { outgoing: false, sender: "Мама".into(), text: "Привет, как дела?".into(), time: "10:01".into() },
                ChatMsg { outgoing: true, sender: "Я".into(), text: "Всё хорошо!".into(), time: "10:02".into() },
            ],
        );
        app.chats = vec![a, b];
        app.select(0);
        app.input = "набираю ответ".into();

        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let out = dump(&term);
        assert!(out.contains("Мама (2)"), "{out}");
        assert!(out.contains("[К] Мой канал"), "{out}");
        assert!(out.contains("Привет, как дела?"), "{out}");
        assert!(out.contains("набираю ответ"), "{out}");

        app.lock();
        app.lock_input = "abc".into();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let out = dump(&term);
        assert!(out.contains("Заблокировано"), "{out}");
        assert!(out.contains("***"), "{out}");
        // Переписка при блокировке не должна быть видна.
        assert!(!out.contains("Привет, как дела?"), "{out}");
        assert!(!out.contains("Мама"), "{out}");
    }
}
