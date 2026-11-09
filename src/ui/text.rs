//! Чистые вспомогательные функции интерфейса: клавиши, разбор команд, перенос строк, ленту сообщений.

use super::*;

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

/// Текст для правки сообщения: исходный из Telegram, иначе подпись или текст.
pub(super) fn edit_source(m: &ChatMsg) -> String {
    if m.raw.is_empty() { m.caption.clone().unwrap_or_else(|| m.text.clone()) } else { m.raw.clone() }
}

/// Разбирает `/later 30 текст` -> (30, "текст").
pub(super) fn parse_later(text: &str) -> Option<(u32, String)> {
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
pub(super) fn parse_send(arg: &str) -> Option<(String, String)> {
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

/// `/album`: пути (в кавычках, если есть пробелы) и необязательная подпись после `--`.
pub(super) fn parse_album(arg: &str) -> (Vec<String>, String) {
    let mut paths = Vec::new();
    let mut rest = arg.trim();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return (paths, String::new());
        }
        if let Some(cap) = rest.strip_prefix("--") {
            return (paths, cap.trim().to_string());
        }
        let quote = ['"', '\''].into_iter().find(|q| rest.starts_with(*q));
        let (token, tail) = match quote {
            Some(q) => match rest[1..].find(q) {
                Some(end) => (&rest[1..1 + end], &rest[end + 2..]),
                None => (&rest[1..], ""),
            },
            None => rest.split_once(char::is_whitespace).unwrap_or((rest, "")),
        };
        paths.push(token.to_string());
        rest = tail;
    }
}

pub(super) fn truncate(s: &str, max: usize) -> String {
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

pub(super) fn name_color(name: &str) -> Color {
    let h = name.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
    palette().names[(h as usize) % 7]
}

/// Перенос по словам с учётом символов `\n` и ширины кириллицы.
pub(super) fn wrap_lines(text: &str, width: usize) -> Vec<String> {
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

pub(super) struct InputLayout {
    pub(super) lines: Vec<String>,
    pub(super) cur_row: usize,
    pub(super) cur_col: usize,
}

/// Жёсткий перенос поля ввода по ширине. Так положение курсора считается точно.
pub(super) fn layout_input(text: &str, cursor: usize, width: usize) -> InputLayout {
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

pub(super) fn months() -> [&'static str; 12] {
    if crate::i18n::en() {
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"]
    } else {
        ["января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря"]
    }
}

pub(super) fn local_date(ts: i64, tz: &jiff::tz::TimeZone) -> Option<(i16, i8, i8)> {
    let t = jiff::Timestamp::from_second(ts).ok()?;
    let d = t.to_zoned(tz.clone()).date();
    Some((d.year(), d.month(), d.day()))
}

fn is_yesterday(date: (i16, i8, i8), today: (i16, i8, i8)) -> bool {
    jiff::civil::Date::new(today.0, today.1, today.2).and_then(|t| t.yesterday()).is_ok_and(|y| (y.year(), y.month(), y.day()) == date)
}

pub(super) fn day_label(date: (i16, i8, i8), today: Option<(i16, i8, i8)>) -> String {
    let (y, m, d) = date;
    let month = months().get((m as usize).wrapping_sub(1)).copied().unwrap_or("");
    match today {
        Some((ty, tm, td)) if (ty, tm, td) == (y, m, d) => tr!("Сегодня", "Today").to_string(),
        Some(t) if is_yesterday(date, t) => tr!("Вчера", "Yesterday").to_string(),
        Some((ty, _, _)) if ty == y => trf!("{d} {month}", "{month} {d}"),
        _ => trf!("{d} {month} {y}", "{month} {d}, {y}"),
    }
}

/// Короткая запись числа просмотров: 950, 1.2K, 15K, 2.4M.
pub(super) fn short_count(n: i32) -> String {
    match n {
        ..1000 => n.to_string(),
        1000..10_000 => format!("{}.{}K", n / 1000, n % 1000 / 100),
        10_000..1_000_000 => format!("{}K", n / 1000),
        _ => format!("{}.{}M", n / 1_000_000, n % 1_000_000 / 100_000),
    }
}

/// Превращает историю в строки для вывода: даты-разделители, свои сообщения справа, ответы и пересылки.
/// Возвращает строки и диапазон строк каждого сообщения (для выделения в режиме выбора).
#[cfg(test)]
pub(super) fn build_message_lines(
    msgs: &[ChatMsg],
    width: usize,
    bubble_pct: usize,
    selected: Option<usize>,
    sel_bg: Color,
) -> (Vec<Line<'static>>, Vec<(usize, usize)>) {
    build_message_lines_marks(msgs, width, bubble_pct, selected, sel_bg, None, &std::collections::BTreeSet::new())
}

/// Отметка у своего сообщения: «√» отправлено, «√√» прочитано собеседником.
pub(super) fn read_mark(id: i32, read_out: i32) -> &'static str {
    if id <= 0 {
        ""
    } else if id <= read_out {
        " √√"
    } else {
        " √"
    }
}

/// То же, но с отметками прочтения: `read_out` задан только в личных чатах.
pub(super) fn build_message_lines_marks(
    msgs: &[ChatMsg],
    width: usize,
    bubble_pct: usize,
    selected: Option<usize>,
    sel_bg: Color,
    read_out: Option<i32>,
    marked: &std::collections::BTreeSet<i32>,
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
            lines.push(Line::from(Span::styled(label, Style::new().fg(muted()))).centered());
            prev_day = day;
        }
        let first = lines.len();
        let time = if m.edited { trf!("{} (изм.)", "{} (edited)", m.time) } else { m.time.clone() };
        let note = |text: String| Span::styled(text, Style::new().fg(muted()).add_modifier(Modifier::ITALIC));
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
                    Span::styled(
                        m.views.map(|v| trf!("{} просм. · ", "{} views · ", short_count(v))).unwrap_or_default(),
                        Style::new().fg(muted()),
                    ),
                    Span::styled(format!("{time} "), Style::new().fg(muted())),
                    Span::styled(tr!("Я", "Me"), Style::new().fg(own()).add_modifier(Modifier::BOLD)),
                    Span::styled(
                        read_out.map(|r| read_mark(m.id, r)).unwrap_or(""),
                        Style::new().fg(if read_out.is_some_and(|r| m.id > 0 && m.id <= r) { own() } else { muted() }),
                    ),
                ])
                .right_aligned(),
            );
            for n in &notes {
                lines.push(Line::from(vec![note(truncate(n, inner)), Span::styled(" │", Style::new().fg(own()))]).right_aligned());
            }
            for l in wrap_lines(&m.text, inner) {
                lines.push(Line::from(vec![Span::raw(l), Span::styled(" │", Style::new().fg(own()))]).right_aligned());
            }
            for spans in extra_rows(m, inner) {
                let mut spans = spans;
                spans.push(Span::styled(" │", Style::new().fg(own())));
                lines.push(Line::from(spans).right_aligned());
            }
        } else {
            let color = name_color(&m.sender);
            let mut head = Vec::new();
            if m.mentioned {
                head.push(Span::styled("@ ", Style::new().fg(mention()).add_modifier(Modifier::BOLD)));
            }
            head.push(Span::styled(m.sender.clone(), Style::new().fg(color).add_modifier(Modifier::BOLD)));
            head.push(Span::styled(format!(" {time}"), Style::new().fg(muted())));
            if let Some(v) = m.views {
                head.push(Span::styled(trf!(" · {} просм.", " · {} views", short_count(v)), Style::new().fg(muted())));
            }
            lines.push(Line::from(head));
            for n in &notes {
                lines.push(Line::from(vec![Span::styled("│ ", Style::new().fg(color)), note(truncate(n, inner))]));
            }
            for l in wrap_lines(&m.text, inner) {
                lines.push(Line::from(vec![Span::styled("│ ", Style::new().fg(color)), Span::raw(l)]));
            }
            for spans in extra_rows(m, inner) {
                let mut row = vec![Span::styled("│ ", Style::new().fg(color))];
                row.extend(spans);
                lines.push(Line::from(row));
            }
        }
        let is_marked = marked.contains(&m.id);
        if selected == Some(idx) || is_marked {
            for l in &mut lines[first..] {
                l.style = l.style.bg(sel_bg);
            }
        }
        // При пакетном выборе: «✓» у отмеченных, «▸» у сообщения под курсором.
        let flag = if is_marked {
            Some("✓ ")
        } else if selected == Some(idx) && !marked.is_empty() {
            Some("▸ ")
        } else {
            None
        };
        if let Some(f) = flag {
            lines[first].spans.insert(0, Span::styled(f, Style::new().fg(own()).add_modifier(Modifier::BOLD)));
        }
        ranges.push((first, lines.len()));
        lines.push(Line::from(""));
    }
    (lines, ranges)
}

/// Строки под текстом сообщения: реакции и кнопки бота (ряд кнопок — одна строка).
pub(super) fn extra_rows(m: &ChatMsg, inner: usize) -> Vec<Vec<Span<'static>>> {
    let mut rows = Vec::new();
    if !m.reactions.is_empty() {
        let mut spans = Vec::new();
        for r in &m.reactions {
            if !spans.is_empty() {
                spans.push(Span::raw("  "));
            }
            let label = format!("{} {}", crate::util::reaction_label(&r.emoji), r.count);
            let style = if r.mine { Style::new().fg(own()).add_modifier(Modifier::BOLD) } else { Style::new().fg(muted()) };
            spans.push(Span::styled(if r.mine { format!("[{label}]") } else { label }, style));
        }
        rows.push(spans);
    }
    if let Some(p) = &m.poll {
        let kind = if p.quiz { tr!("викторина", "quiz") } else { tr!("опрос", "poll") };
        let state = if p.closed { tr!(" · закрыт", " · closed") } else { "" };
        rows.push(vec![Span::styled(
            truncate(&format!("▤ {kind}: {}{state}", p.question), inner),
            Style::new().add_modifier(Modifier::BOLD),
        )]);
        for o in &p.options {
            let pct = if p.total > 0 { (o.voters * 100 / p.total) as usize } else { 0 };
            let filled = pct / 10;
            let bar = format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled));
            let mark = if o.chosen { "●" } else { "○" };
            let right = if p.total > 0 || p.closed { format!(" {bar} {pct}%") } else { String::new() };
            let right_w = display_width(&right);
            let text = truncate(&o.text, inner.saturating_sub(right_w + 4));
            let style = if o.chosen { Style::new().fg(own()).add_modifier(Modifier::BOLD) } else { Style::new() };
            let mut spans = vec![Span::styled(format!("{mark} {text}"), style), Span::styled(right, Style::new().fg(muted()))];
            if o.correct {
                spans.push(Span::styled(" ✓", Style::new().fg(online_color())));
            }
            rows.push(spans);
        }
        rows.push(vec![Span::styled(trf!("  голосов: {} · V — голосовать", "  votes: {} · V to vote", p.total), Style::new().fg(muted()))]);
    }
    for row in &m.buttons {
        let text: String = row
            .iter()
            .map(|b| match b.action {
                crate::tg::BtnAction::Url(_) => format!("[{} ↗]", b.text),
                _ => format!("[{}]", b.text),
            })
            .collect::<Vec<_>>()
            .join(" ");
        rows.push(vec![Span::styled(truncate(&text, inner), Style::new().fg(palette().fg.unwrap_or(Color::Reset)))]);
    }
    rows
}

/// Текст переписки для сохранения в файл: «дата время имя: текст».
pub(super) fn export_text(chat_name: &str, msgs: &[ChatMsg]) -> String {
    let tz = jiff::tz::TimeZone::system();
    let mut out = trf!("Переписка: {chat_name}\nСохранено Tg-Lite {VERSION}\n\n", "Chat: {chat_name}\nSaved by Tg-Lite {VERSION}\n\n");
    for m in msgs {
        let when =
            jiff::Timestamp::from_second(m.ts).map(|t| t.to_zoned(tz.clone()).strftime("%d.%m.%Y %H:%M").to_string()).unwrap_or_default();
        out.push_str(&format!("[{when}] {}: {}\n", m.sender, m.text.replace('\n', "\n    ")));
    }
    out
}

pub(super) fn beep() {
    crate::util::play_alert();
}

pub(super) fn when_label(minutes: u32) -> String {
    let tz = jiff::tz::TimeZone::system();
    let ts = jiff::Timestamp::now().as_second() + i64::from(minutes) * 60;
    match jiff::Timestamp::from_second(ts) {
        Ok(t) => t.to_zoned(tz).strftime(tr!("%d.%m в %H:%M", "%d.%m at %H:%M")).to_string(),
        Err(_) => trf!("через {minutes} мин.", "in {minutes} min"),
    }
}
