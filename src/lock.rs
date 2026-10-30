//! Экран блокировки и вход по паролю с анимированным фоном.
//!
//! Фон считается чистыми функциями от времени: без состояния и без выделения памяти,
//! поэтому на слабом процессоре он почти ничего не стоит. Цвета квантуются, и ratatui
//! перерисовывает только изменившиеся клетки.

use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::vault::{Secrets, VaultFile};

/// Названия фонов для настроек.
pub const BG_NAMES: [&str; 4] = ["Аврора", "Матрица", "Звёзды", "Выключен"];
/// Сколько фонов (последний — «выключен»).
pub const BG_COUNT: u8 = BG_NAMES.len() as u8;

const VOID: (u8, u8, u8) = (5, 7, 16);
const PANEL: Color = Color::Rgb(9, 12, 26);

// ------------------------------------------------------------------ мелкие помощники

fn hash(a: u32, b: u32) -> u32 {
    let mut x = a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA6B).rotate_left(13);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297A_2D39);
    x ^ (x >> 15)
}

/// Случайное число из диапазона 0..1 по хэшу.
fn unit(h: u32) -> f32 {
    (h & 0xFFFF) as f32 / 65535.0
}

fn rgb_of(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (42, 171, 238),
    }
}

fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color::Rgb(f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

/// Округляет яркость до `steps` ступеней: меньше изменённых клеток между кадрами.
fn quant(v: f32, steps: f32) -> f32 {
    (v.clamp(0.0, 1.0) * steps).round() / steps
}

fn set(buf: &mut Buffer, x: u16, y: u16, symbol: &str, fg: Color, bg: Color) {
    if let Some(c) = buf.cell_mut((x, y)) {
        c.set_symbol(symbol).set_fg(fg).set_bg(bg);
    }
}

// ------------------------------------------------------------------ фоны

/// Рисует выбранный фон на всей области. `style` вне диапазона = выключено (ровная заливка).
pub fn paint_background(buf: &mut Buffer, area: Rect, style: u8, t: f32, accent: Color) {
    let acc = rgb_of(accent);
    match style {
        0 => aurora(buf, area, t, acc),
        1 => matrix(buf, area, t, acc),
        2 => stars(buf, area, t, acc),
        _ => fill(buf, area, Color::Rgb(VOID.0, VOID.1, VOID.2)),
    }
}

fn fill(buf: &mut Buffer, area: Rect, bg: Color) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            set(buf, x, y, " ", Color::Reset, bg);
        }
    }
}

/// Плавные цветные волны, как северное сияние, плюс редкие мерцающие звёзды.
fn aurora(buf: &mut Buffer, area: Rect, t: f32, acc: (u8, u8, u8)) {
    let violet = (120, 70, 190);
    let teal = (40, 200, 170);
    let h = f32::from(area.height.max(1));
    for y in area.top()..area.bottom() {
        let v = f32::from(y - area.top()) / h;
        // Полоса свечения проходит в верхней половине экрана и плавно гаснет к низу.
        let band = (1.0 - ((v - 0.38).abs() * 1.55)).clamp(0.0, 1.0);
        for x in area.left()..area.right() {
            let xf = f32::from(x - area.left());
            let yf = f32::from(y - area.top());
            let a = (xf * 0.075 + t * 0.30).sin();
            let b = (yf * 0.30 - t * 0.22 + xf * 0.035).sin();
            let c = ((xf * 0.5 + yf) * 0.095 + t * 0.16).sin();
            let n = ((a + b + c) / 3.0 + 1.0) / 2.0; // 0..1
            let level = quant(n * n * (0.25 + 0.75 * band), 14.0);
            let hue = quant((c + 1.0) / 2.0, 6.0);
            let tint_rgb = rgb_of(mix(rgb_of(mix(acc, teal, 0.45)), violet, hue));
            let bg = mix(VOID, tint_rgb, level * 0.55);
            set(buf, x, y, " ", Color::Reset, bg);
        }
    }
    sprinkle_stars(buf, area, t, 0.012, acc);
}

/// Падающие столбцы символов с яркой «головой» и затухающим хвостом.
fn matrix(buf: &mut Buffer, area: Rect, t: f32, acc: (u8, u8, u8)) {
    const CHARS: &[u8] = b"01ABCDEFGHJKLMNPRSTUVXYZ#$%&*+=<>?/|:;";
    let bg = Color::Rgb(3, 5, 10);
    fill(buf, area, bg);
    let h = i32::from(area.height);
    for col in 0..area.width {
        let x = area.left() + col;
        // Каждая вторая колонка пустая: так читаемее и дешевле.
        if hash(u32::from(col), 77) % 3 == 0 {
            continue;
        }
        let hc = hash(u32::from(col), 1);
        let speed = 4.0 + unit(hc) * 9.0;
        let len = 7 + (hc >> 16) % 12;
        let period = (h as u32 + len + 10) as f32;
        let head = (t * speed + unit(hash(u32::from(col), 2)) * period) % period - len as f32 * 0.4;
        for row in 0..h {
            let d = head - row as f32;
            if d < 0.0 || d >= len as f32 {
                continue;
            }
            let k = 1.0 - d / len as f32; // 1 у головы, 0 на конце хвоста
            let step = (t * 3.0) as u32;
            let ch = CHARS[(hash(u32::from(col) * 131 + row as u32, step / 3 + (d as u32)) as usize) % CHARS.len()];
            let fg = if d < 1.0 {
                mix(acc, (235, 255, 250), 0.85)
            } else {
                mix((4, 8, 14), acc, quant(k * k, 8.0) * 0.8)
            };
            let sym = (ch as char).to_string();
            set(buf, x, area.top() + row as u16, &sym, fg, bg);
        }
    }
}

/// Звёздное небо: звёзды медленно плывут влево и мерцают.
fn stars(buf: &mut Buffer, area: Rect, t: f32, acc: (u8, u8, u8)) {
    let h = f32::from(area.height.max(1));
    for y in area.top()..area.bottom() {
        let v = f32::from(y - area.top()) / h;
        let bg = mix((4, 6, 14), (14, 16, 38), v);
        for x in area.left()..area.right() {
            set(buf, x, y, " ", Color::Reset, bg);
        }
    }
    sprinkle_stars(buf, area, t, 0.035, acc);
}

/// Рассыпает мерцающие звёзды по уже залитому фону; `density` — доля клеток.
fn sprinkle_stars(buf: &mut Buffer, area: Rect, t: f32, density: f32, acc: (u8, u8, u8)) {
    let w = u32::from(area.width.max(1));
    let h = u32::from(area.height.max(1));
    let count = ((w * h) as f32 * density) as u32;
    for i in 0..count {
        let hx = hash(i, 11);
        let hy = hash(i, 12);
        let speed = 0.25 + unit(hash(i, 13)) * 1.3;
        let drift = (t * speed) as u32 % w;
        let x = (hx % w + w - drift) % w;
        let y = hy % h;
        let phase = unit(hash(i, 14)) * std::f32::consts::TAU;
        let tw = ((t * (0.8 + unit(hash(i, 15)) * 1.6) + phase).sin() + 1.0) / 2.0;
        let tw = quant(tw, 4.0);
        if tw < 0.25 {
            continue;
        }
        let sym = if tw > 0.9 { "*" } else if tw > 0.55 { "+" } else { "·" };
        let base = if hash(i, 16) % 5 == 0 { acc } else { (200, 210, 235) };
        let fg = mix((30, 36, 60), base, tw);
        let (px, py) = (area.left() + x as u16, area.top() + y as u16);
        if let Some(cell) = buf.cell_mut((px, py)) {
            let bg = cell.bg;
            set(buf, px, py, sym, fg, bg);
        }
    }
}

// ------------------------------------------------------------------ окно с запросом пароля

/// Всё, что нужно для отрисовки экрана блокировки.
pub struct LockView<'a> {
    pub accent: Color,
    /// Номер фона (см. [`BG_NAMES`]).
    pub bg: u8,
    /// Секунды анимации.
    pub t: f32,
    pub version: &'a str,
    pub title: &'a str,
    pub prompt: &'a str,
    /// Сколько точек показать вместо введённого пароля.
    pub dots: usize,
    pub error: &'a str,
    pub footer: &'a str,
    /// Подпись вместо подсказки, пока идёт проверка пароля.
    pub busy: bool,
}

/// Разбивает текст на строки не шире `width` (по словам).
fn wrap(text: &str, width: usize) -> Vec<String> {
    textwrap::wrap(text, width.max(8)).into_iter().map(|c| c.into_owned()).collect()
}

pub fn draw(f: &mut Frame, v: &LockView) {
    let area = f.area();
    paint_background(f.buffer_mut(), area, v.bg, v.t, v.accent);

    let inner_w = (area.width.saturating_sub(6) as usize).clamp(10, 50);
    let accent = Style::new().fg(v.accent).add_modifier(Modifier::BOLD);
    let muted = Style::new().fg(Color::Rgb(130, 145, 165));

    let cursor_on = (v.t * 2.0) as u32 % 2 == 0;
    let dots: String = "●".repeat(v.dots.min(inner_w.saturating_sub(4)));
    let field = format!("{dots}{}", if cursor_on && !v.busy { "▌" } else { " " });

    let mut text: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(format!("Tg-Lite {}", v.version), accent)).centered(),
        Line::from(Span::styled(v.title.to_string(), muted)).centered(),
        Line::from(""),
        Line::from(if v.busy { "Проверяю пароль…" } else { v.prompt }.to_string()).centered(),
        Line::from(""),
        Line::from(vec![Span::styled("› ", muted), Span::styled(field, Style::new().fg(v.accent))]).centered(),
    ];
    if !v.error.is_empty() {
        text.push(Line::from(""));
        for l in wrap(v.error, inner_w) {
            text.push(Line::from(Span::styled(l, Style::new().fg(Color::Rgb(255, 110, 110)))).centered());
        }
    }
    text.push(Line::from(""));
    text.push(Line::from(Span::styled(v.footer.to_string(), muted)).centered());

    let w = (inner_w as u16 + 4).min(area.width);
    let h = (text.len() as u16 + 2).min(area.height);
    let rect = Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h };

    // Тень под окном: небольшой сдвиг делает окно «приподнятым» над фоном.
    let shadow = Rect { x: rect.x + 1, y: rect.y + 1, width: rect.width, height: rect.height }.intersection(area);
    for y in shadow.top()..shadow.bottom() {
        for x in shadow.left()..shadow.right() {
            set(f.buffer_mut(), x, y, " ", Color::Reset, Color::Rgb(2, 3, 8));
        }
    }
    f.render_widget(
        Paragraph::new(text).style(Style::new().bg(PANEL)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(v.accent).bg(PANEL))
                .style(Style::new().bg(PANEL)),
        ),
        rect,
    );
}

// ------------------------------------------------------------------ вход при запуске

/// Итог экрана входа.
pub enum Login {
    /// Пароль подошёл: расшифрованные данные и сам пароль.
    Ok(Box<Secrets>, String),
    /// Пользователь закрыл окно.
    Quit,
    /// Слишком много неверных попыток.
    TooMany,
    /// Графический экран недоступен (ввод перенаправлен): нужен обычный запрос в консоли.
    NoTerminal,
}

const MAX_TRIES: u32 = 5;

/// Экран входа с анимированным фоном. Пароль проверяется расшифровкой хранилища.
pub fn login(vault: &VaultFile, version: &str, accent: Color) -> Result<Login> {
    let Ok(mut terminal) = ratatui::try_init() else {
        return Ok(Login::NoTerminal);
    };
    let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle("Tg-Lite"));
    let res = login_loop(&mut terminal, vault, version, accent);
    ratatui::restore();
    res
}

fn login_loop(terminal: &mut DefaultTerminal, vault: &VaultFile, version: &str, accent: Color) -> Result<Login> {
    let start = Instant::now();
    let mut input = String::new();
    let mut error = String::new();
    let mut tries = 0;
    loop {
        let t = start.elapsed().as_secs_f32();
        let view = LockView {
            accent,
            bg: 0,
            t,
            version,
            title: "Вход",
            prompt: "Введите пароль и нажмите Enter",
            dots: input.chars().count(),
            error: &error,
            footer: "Esc — выйти",
            busy: false,
        };
        terminal.draw(|f| draw(f, &view))?;
        if !event::poll(Duration::from_millis(90))? {
            continue;
        }
        let TermEvent::Key(k) = event::read()? else { continue };
        if k.kind != KeyEventKind::Press {
            continue;
        }
        match k.code {
            KeyCode::Esc | KeyCode::F(10) => return Ok(Login::Quit),
            KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return Ok(Login::Quit),
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Enter if !input.is_empty() => {
                // Расшифровка занимает заметное время: сначала показываем «Проверяю…».
                let busy = LockView { busy: true, error: "", ..view };
                terminal.draw(|f| draw(f, &busy))?;
                match vault.load(&input) {
                    Ok(s) => return Ok(Login::Ok(Box::new(s), input)),
                    Err(e) => {
                        tries += 1;
                        if tries >= MAX_TRIES {
                            return Ok(Login::TooMany);
                        }
                        error = format!("{e} (попытка {tries} из {MAX_TRIES})");
                        input.clear();
                    }
                }
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => input.push(c),
            _ => {}
        }
    }
}

/// Нужно ли перерисовывать экран блокировки каждый кадр (есть ли анимация).
pub fn animated(bg: u8) -> bool {
    bg < BG_COUNT - 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn view(bg: u8, t: f32) -> LockView<'static> {
        LockView {
            accent: Color::Rgb(42, 171, 238),
            bg,
            t,
            version: "0.0.7",
            title: "Заблокировано",
            prompt: "Введите пароль и нажмите Enter",
            dots: 5,
            error: "Неверный пароль. Осталось попыток до удаления данных: 2",
            footer: "F10 — выйти",
            busy: false,
        }
    }

    fn dump(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>() + "\n")
            .collect()
    }

    #[test]
    fn every_background_renders_at_every_size_without_panic() {
        for bg in 0..=BG_COUNT {
            for (w, h) in [(120, 40), (80, 25), (40, 10), (20, 6), (10, 3), (1, 1), (3, 2)] {
                let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                for t in [0.0, 0.1, 7.3, 1234.5] {
                    term.draw(|f| draw(f, &view(bg, t))).unwrap();
                }
            }
        }
    }

    #[test]
    fn full_error_text_is_visible_and_wrapped() {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &view(0, 1.0))).unwrap();
        let out = dump(&term);
        assert!(out.contains("Осталось попыток") && out.contains("данных: 2"), "текст ошибки обрезан:\n{out}");
        assert!(out.contains("Tg-Lite 0.0.7"));
        assert!(out.contains("F10"));
    }

    #[test]
    fn password_is_never_drawn() {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut v = view(0, 1.0);
        v.dots = 3;
        term.draw(|f| draw(f, &v)).unwrap();
        assert!(dump(&term).contains("●●●"));
    }

    #[test]
    fn background_is_deterministic_and_actually_animates() {
        let render = |bg: u8, t: f32| {
            let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
            term.draw(|f| {
                let area = f.area();
                paint_background(f.buffer_mut(), area, bg, t, Color::Rgb(42, 171, 238));
            })
            .unwrap();
            term.backend().buffer().clone()
        };
        for bg in 0..3 {
            assert_eq!(render(bg, 3.0), render(bg, 3.0), "фон {bg} должен быть детерминированным");
            assert_ne!(render(bg, 3.0), render(bg, 6.0), "фон {bg} должен меняться со временем");
        }
        assert_eq!(render(3, 1.0), render(3, 9.0), "«Выключен» не анимируется");
    }

    #[test]
    fn background_stays_dark_enough_for_text() {
        // Яркость любой клетки фона не выше порога: текст окна и подсказки остаются читаемыми.
        for bg in 0..3 {
            let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
            for t in [0.0, 5.0, 20.0] {
                term.draw(|f| {
                let area = f.area();
                paint_background(f.buffer_mut(), area, bg, t, Color::Rgb(42, 171, 238));
            })
            .unwrap();
                let buf = term.backend().buffer();
                for y in 0..20 {
                    for x in 0..60 {
                        if let Color::Rgb(r, g, b) = buf[(x, y)].bg {
                            assert!(u32::from(r) + u32::from(g) + u32::from(b) < 330, "слишком ярко: {r},{g},{b}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn animation_flag() {
        assert!(animated(0) && animated(1) && animated(2));
        assert!(!animated(3));
    }

    /// Выгружает кадры в JSON для просмотра: `TGLITE_SHOTS=/путь cargo test dump_frames -- --ignored`.
    #[test]
    #[ignore]
    fn dump_frames() {
        let Some(dir) = std::env::var_os("TGLITE_SHOTS") else { return };
        std::fs::create_dir_all(&dir).unwrap();
        let col = |c: Color| match c {
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            _ => String::new(),
        };
        for bg in 0..3u8 {
            for (i, t) in [2.0f32, 9.0].iter().enumerate() {
                let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
                term.draw(|f| draw(f, &view(bg, *t))).unwrap();
                let buf = term.backend().buffer();
                let mut rows = Vec::new();
                for y in 0..30 {
                    let mut row = Vec::new();
                    for x in 0..100 {
                        let c = &buf[(x, y)];
                        row.push(format!("{}|{}|{}", c.symbol(), col(c.fg), col(c.bg)));
                    }
                    rows.push(row);
                }
                let path = std::path::Path::new(&dir).join(format!("lock_{bg}_{i}.json"));
                std::fs::write(path, serde_json::to_string(&rows).unwrap()).unwrap();
            }
        }
    }
}
