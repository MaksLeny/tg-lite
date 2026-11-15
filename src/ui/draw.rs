//! Отрисовка всех экранов (ratatui). Состояние берётся из `App`, ничего не меняется, кроме прокрутки.

use super::*;

// ------------------------------------------------------------------ рисование

pub(super) fn draw(f: &mut Frame, app: &mut App) {
    if !matches!(app.mode, Mode::Locked | Mode::NewPassword) && palette().bg.is_some() {
        f.render_widget(Block::default().style(base_style()), f.area());
    }
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
        Mode::Preview => {
            draw_main(f, app);
            draw_preview(f, app);
        }
        Mode::Viewer => {
            draw_main(f, app);
            draw_viewer(f, app);
        }
        Mode::Quick => {
            draw_main(f, app);
            draw_quick(f, app);
        }
    }
}

/// Окно быстрого перехода (Ctrl+K): строка запроса, под ней чаты или команды.
fn draw_quick(f: &mut Frame, app: &App) {
    let Some(p) = &app.quick else { return };
    let area = f.area();
    let width = area.width.saturating_sub(6).clamp(30, 70);
    let rows = p.items.len().clamp(1, 12) as u16;
    let rect = centered_rect(area, width, rows + 4);
    let inner_w = rect.width.saturating_sub(4) as usize;
    let accent = app.theme.accent;
    let mut lines: Vec<Line> = vec![Line::from(vec![
        Span::styled("› ", Style::new().fg(accent).add_modifier(Modifier::BOLD)),
        Span::raw(p.query.clone()),
        Span::styled("▏", Style::new().fg(accent)),
    ])];
    // Окно из 12 строк: выбранная всегда видна.
    let top = p.sel.saturating_sub(11);
    if p.items.is_empty() {
        lines.push(Line::from(Span::styled(
            tr!(" Ничего не найдено. «/» — команды", " Nothing found. \"/\" lists commands"),
            Style::new().fg(muted()),
        )));
    }
    for (i, item) in p.items.iter().enumerate().skip(top).take(12) {
        let hint_w = display_width(&item.hint);
        let label_w = inner_w.saturating_sub(hint_w + 2);
        let label = truncate(&item.label, label_w);
        let pad = inner_w.saturating_sub(display_width(&label) + hint_w + 1);
        let row = format!(" {label}{}{} ", " ".repeat(pad), item.hint);
        let style =
            if i == p.sel { Style::new().bg(app.theme.select_bg).fg(Color::White).add_modifier(Modifier::BOLD) } else { Style::new() };
        lines.push(Line::from(Span::styled(row, style)));
    }
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .style(base_style())
                .borders(Borders::ALL)
                .border_style(Style::new().fg(accent))
                .title(Span::styled(tr!(" Быстрый переход ", " Quick switch "), Style::new().fg(accent).add_modifier(Modifier::BOLD)))
                .title_bottom(Span::styled(
                    tr!(" ↑ ↓ выбрать · Enter — открыть · «/» — команды · Esc ", " ↑ ↓ pick · Enter — open · \"/\" — commands · Esc "),
                    Style::new().fg(muted()),
                )),
        ),
        rect,
    );
}

/// Текстовый файл: окно почти на весь экран, длинные строки переносятся, листается построчно.
fn draw_viewer(f: &mut Frame, app: &App) {
    let Some(v) = &app.viewer else { return };
    let area = f.area();
    let rect = Rect::new(
        2.min(area.width / 4),
        1.min(area.height / 4),
        area.width.saturating_sub(4).max(10),
        area.height.saturating_sub(2).max(5),
    );
    let inner_w = rect.width.saturating_sub(4) as usize;
    let inner_h = rect.height.saturating_sub(2) as usize;
    let mut rows: Vec<Line> = Vec::new();
    for l in v.lines.iter().skip(v.top) {
        if rows.len() >= inner_h {
            break;
        }
        if l.is_empty() {
            rows.push(Line::from(""));
            continue;
        }
        for part in textwrap::wrap(l, inner_w.max(8)) {
            rows.push(Line::from(format!(" {part}")));
        }
    }
    rows.truncate(inner_h);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(rows).block(
            Block::default()
                .style(base_style())
                .borders(Borders::ALL)
                .border_style(Style::new().fg(app.theme.accent))
                .title(Span::styled(
                    format!(" {} ", truncate(&v.name, inner_w.saturating_sub(4))),
                    Style::new().fg(app.theme.accent).add_modifier(Modifier::BOLD),
                ))
                .title_bottom(Span::styled(
                    format!(
                        "{} {}/{} ",
                        tr!(" ↑ ↓ PgUp PgDn — листать · Esc — закрыть ·", " ↑ ↓ PgUp PgDn — scroll · Esc — close ·"),
                        v.top + 1,
                        v.lines.len()
                    ),
                    Style::new().fg(muted()),
                )),
        ),
        rect,
    );
}

/// Фото из точек RGB: каждая ячейка консоли — две точки («▀»: верх цветом текста, низ цветом фона).
fn draw_preview(f: &mut Frame, app: &App) {
    let Some(p) = &app.preview else { return };
    let area = f.area();
    let (cols, rows) = (area.width.saturating_sub(4) as usize, area.height.saturating_sub(4) as usize);
    let (pw, ph) = util::fit_cells(p.w, p.h, cols, rows);
    if pw == 0 {
        return;
    }
    let px = util::shrink_rgb(p.w, p.h, &p.rgb, pw, ph);
    let lines: Vec<Line> = (0..ph / 2)
        .map(|r| {
            Line::from(
                (0..pw)
                    .map(|c| {
                        let (t, b) = (px[(r * 2) * pw + c], px[(r * 2 + 1) * pw + c]);
                        Span::styled("▀", Style::new().fg(Color::Rgb(t[0], t[1], t[2])).bg(Color::Rgb(b[0], b[1], b[2])))
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let rect = centered_rect(area, pw as u16 + 2, (ph / 2) as u16 + 2);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .style(base_style())
                .borders(Borders::ALL)
                .border_style(Style::new().fg(app.theme.accent))
                .title(tr!(" Фото · любая клавиша — закрыть ", " Photo · any key closes ")),
        ),
        rect,
    );
}

pub(super) fn draw_main(f: &mut Frame, app: &mut App) {
    if app.two_panes() {
        draw_main_panes(f, app);
        return;
    }
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

/// Вид «Два окна»: сверху вкладки открытых чатов, под ними два окна рядом, затем поле ввода и статус.
/// На узком экране показывается только активное окно (второе открывается по Ctrl+← / →).
fn draw_main_panes(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let outer = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(1), Constraint::Min(3)]).split(area);
    draw_open_tabs(f, app, outer[0]);
    let input_width = outer[1].width.saturating_sub(2) as usize;
    let layout = layout_input(&app.input, app.cursor, input_width);
    let input_rows = layout.lines.len().clamp(1, 6) as u16;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(input_rows + 2), Constraint::Length(1)])
        .split(outer[1]);
    app.hit_list = Rect::default();
    if rows[0].width >= PANES_MIN_WIDTH {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(rows[0]);
        app.panes.hit = [cols[0], cols[1]];
        let right = app.panes.right_active;
        // Активное окно — всегда выбранный чат; второе окно рисуется с его собственной прокруткой.
        draw_messages_in(f, app, cols[0], if right { Pane::Other } else { Pane::Active });
        draw_messages_in(f, app, cols[1], if right { Pane::Active } else { Pane::Other });
    } else {
        app.panes.hit = [rows[0], Rect::default()];
        draw_messages_in(f, app, rows[0], Pane::Active);
    }
    draw_input(f, app, rows[1], &layout, input_rows as usize);
    draw_status(f, app, rows[2]);
}

/// Вкладки открытых чатов: окно активного выделено цветом, окно второго — подчёркиванием; у вкладок счётчик новых.
fn draw_open_tabs(f: &mut Frame, app: &mut App, area: Rect) {
    let mut spans: Vec<Span> = Vec::new();
    let mut hits: Vec<(u16, u16, PeerId)> = Vec::new();
    let mut used = 0usize;
    let avail = area.width as usize;
    let hint = tr!(" Ctrl+K — открыть чат ", " Ctrl+K — open a chat ");
    let reserve = if avail > 60 { display_width(hint) } else { 0 };
    let ids: Vec<PeerId> = app.panes.open.clone();
    for id in ids {
        let Some(c) = app.chats.iter().find(|c| c.id == id) else { continue };
        let title = truncate(&c.name, 16);
        let badge = if c.unread > 0 && !app.is_muted(c) { format!(" {} ", c.unread.min(999)) } else { String::new() };
        let w = display_width(&title) + 2 + display_width(&badge);
        if used + w + 1 + reserve > avail {
            spans.push(Span::styled("›", Style::new().fg(muted())));
            break;
        }
        let style = if app.sel == Some(id) {
            Style::new().bg(app.theme.accent).fg(Color::Black).add_modifier(Modifier::BOLD)
        } else if app.panes.other == Some(id) {
            Style::new().fg(palette().fg.unwrap_or(Color::Gray)).add_modifier(Modifier::UNDERLINED)
        } else {
            Style::new().fg(palette().fg.unwrap_or(Color::Gray))
        };
        let x0 = area.x + used as u16;
        hits.push((x0, x0 + w as u16, id));
        spans.push(Span::styled(format!(" {title} "), style));
        if !badge.is_empty() {
            spans.push(Span::styled(badge, Style::new().bg(app.theme.select_bg).fg(Color::White).add_modifier(Modifier::BOLD)));
        }
        spans.push(Span::raw(" "));
        used += w + 1;
    }
    if used == 0 {
        spans.push(Span::styled(tr!(" Ctrl+K — открыть чат", " Ctrl+K — open a chat"), Style::new().fg(muted())));
    } else if reserve > 0 {
        let pad = avail.saturating_sub(used + reserve);
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(hint, Style::new().fg(muted())));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    app.panes.hit_tabs = hits;
}

pub(super) fn draw_tabs(f: &mut Frame, app: &mut App, area: Rect) {
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
    let mut hits: Vec<(u16, u16, Tab)> = Vec::new();
    if start > 0 {
        spans.push(Span::styled("‹", Style::new().fg(muted())));
        used += 1;
    }
    for (i, (title, badge, w)) in items.iter().enumerate().skip(start) {
        if used + w + 2 > avail && i != active {
            spans.push(Span::styled("›", Style::new().fg(muted())));
            break;
        }
        let style = if i == active {
            Style::new().bg(app.theme.accent).fg(Color::Black).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(palette().fg.unwrap_or(Color::Gray))
        };
        let x0 = area.x + used as u16;
        hits.push((x0, x0 + *w as u16, tabs[i].0.clone()));
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
    app.hit_tabs = hits;
}

pub(super) fn draw_chat_list(f: &mut Frame, app: &mut App, area: Rect) {
    let inner_w = area.width.saturating_sub(2) as usize;
    let now = jiff::Timestamp::now().as_second();
    let show_status = app.settings.show_status;
    let idx = app.visible_idx();
    let visible: Vec<&ChatInfo> = idx.iter().map(|&i| &app.chats[i]).collect();
    let pinned: Vec<bool> = visible.iter().map(|c| app.is_pinned(c)).collect();
    let two_lines = app.view() != View::Classic;
    let tz = jiff::tz::TimeZone::system();
    let today = local_date(now, &tz);
    let favorites = app.favorites();
    let items: Vec<ListItem> = visible
        .iter()
        .zip(pinned.iter())
        .map(|(c, &pin)| {
            let online = show_status && c.presence.is_some_and(|p| tg::is_online(p, now));
            let (tag, tag_color) = match c.kind {
                Kind::User if online => ("●", online_color()),
                Kind::User => (" ", muted()),
                Kind::Group => (tr!("Г", "G"), mention()),
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
            // Закреплённый чат: «*»; в новых видах — номер, по которому он открывается на Alt+цифра.
            let pin_mark = if !pin {
                Span::raw(" ")
            } else if two_lines && let Some(n) = favorites.iter().position(|f| *f == c.id) {
                Span::styled((n + 1).to_string(), Style::new().fg(app.theme.accent))
            } else {
                Span::styled("*", Style::new().fg(app.theme.accent))
            };
            let muted_chat = app.is_muted(c);
            let mut spans = vec![
                Span::styled(tag.to_string(), Style::new().fg(tag_color).add_modifier(Modifier::BOLD)),
                pin_mark,
                Span::styled(
                    format!("{name}{}", " ".repeat(pad)),
                    if c.unread > 0 && !muted_chat { Style::new().add_modifier(Modifier::BOLD) } else { Style::new() },
                ),
            ];
            if !badge.is_empty() {
                // У заглушённого чата счётчик серый: он не требует внимания.
                let badge_style = if muted_chat {
                    Style::new().bg(muted()).fg(Color::Black)
                } else {
                    Style::new().bg(app.theme.accent).fg(Color::Black).add_modifier(Modifier::BOLD)
                };
                spans.push(Span::styled(badge, badge_style));
            }
            if !two_lines {
                return ListItem::new(Line::from(spans));
            }
            // Вторая строка: «печатает…», черновик или последнее сообщение, справа время.
            let when = if c.last_ts > 0 {
                match local_date(c.last_ts, &tz) {
                    Some(d) if Some(d) == today => jiff::Timestamp::from_second(c.last_ts)
                        .map(|t| t.to_zoned(tz.clone()).strftime("%H:%M").to_string())
                        .unwrap_or_default(),
                    Some((_, m, d)) => format!("{d:02}.{m:02}"),
                    None => String::new(),
                }
            } else {
                String::new()
            };
            let draft = app.drafts.get(&c.id).map(|(t, _)| t.clone()).filter(|t| !t.trim().is_empty() && !t.starts_with('/'));
            let draft = draft.or_else(|| Some(c.draft.clone()).filter(|t| !t.trim().is_empty()));
            let typing = app.typing.get(&c.id).filter(|(t, _)| t.elapsed() < TYPING_TTL).map(|(_, l)| l.clone());
            let (text, style) = if let Some(t) = typing {
                (t, Style::new().fg(app.theme.accent).add_modifier(Modifier::ITALIC))
            } else if let Some(d) = draft {
                (
                    format!("{}: {}", tr!("черновик", "draft"), d.split_whitespace().collect::<Vec<_>>().join(" ")),
                    Style::new().fg(mention()),
                )
            } else {
                (c.preview.clone(), Style::new().fg(muted()))
            };
            let when_w = display_width(&when);
            let room = inner_w.saturating_sub(2 + when_w + usize::from(when_w > 0));
            let text = truncate(&text, room);
            let fill = inner_w.saturating_sub(2 + display_width(&text) + when_w);
            let second = Line::from(vec![
                Span::raw("  "),
                Span::styled(text, style),
                Span::raw(" ".repeat(fill)),
                Span::styled(when, Style::new().fg(muted())),
            ]);
            ListItem::new(vec![Line::from(spans), second])
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
                .style(base_style())
                .borders(Borders::ALL)
                .border_style(Style::new().fg(muted()))
                .title(Span::styled(list_title, Style::new().fg(app.theme.accent).add_modifier(Modifier::BOLD)))
                .title_bottom(Span::styled(format!(" {count} "), Style::new().fg(muted()))),
        )
        .highlight_style(Style::new().bg(app.theme.select_bg).fg(Color::White).add_modifier(Modifier::BOLD));
    f.render_stateful_widget(list, area, &mut app.list_state);
    app.hit_list = area;
}

/// Какое окно сообщений рисуется: основное (выбранный чат) или второе окно вида «Два окна».
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Pane {
    Active,
    Other,
}

pub(super) fn draw_messages(f: &mut Frame, app: &mut App, area: Rect) {
    draw_messages_in(f, app, area, Pane::Active);
}

/// Окно сообщений. У второго окна своя прокрутка, выбора сообщений и перехода к найденному в нём нет.
pub(super) fn draw_messages_in(f: &mut Frame, app: &mut App, area: Rect, pane: Pane) {
    let tz = jiff::tz::TimeZone::system();
    let now = jiff::Timestamp::now().as_second();
    let active = pane == Pane::Active;
    let two = app.two_panes();
    let chat_id = if active { app.sel } else { app.panes.other };
    let mut scroll = if active { app.scroll } else { app.panes.other_scroll };
    let info = chat_id.and_then(|id| app.chats.iter().find(|c| c.id == id)).cloned();
    let title = match &info {
        Some(c) => {
            let kind = match c.kind {
                Kind::User => "",
                Kind::Group => tr!(" · группа", " · group"),
                Kind::Channel => tr!(" · канал", " · channel"),
            };
            let mut spans = vec![
                Span::styled(format!(" {}", c.name), Style::new().add_modifier(Modifier::BOLD)),
                Span::styled(kind.to_string(), Style::new().fg(muted())),
            ];
            if app.is_muted(c) {
                spans.push(Span::styled(tr!(" · без звука", " · muted"), Style::new().fg(muted())));
            }
            let typing = app.typing.get(&c.id).filter(|(t, _)| t.elapsed() < TYPING_TTL).map(|(_, l)| l.clone());
            if let Some(t) = typing {
                spans.push(Span::styled(" · ", Style::new().fg(muted())));
                spans.push(Span::styled(t, Style::new().fg(app.theme.accent).add_modifier(Modifier::ITALIC)));
            } else if app.settings.show_status
                && let Some(p) = c.presence
            {
                let label = tg::presence_label(p, now, &tz);
                if !label.is_empty() {
                    let color = if tg::is_online(p, now) { online_color() } else { muted() };
                    spans.push(Span::styled(" · ", Style::new().fg(muted())));
                    spans.push(Span::styled(label, Style::new().fg(color)));
                }
            }
            spans.push(Span::raw(" "));
            Line::from(spans)
        }
        None => Line::from(tr!(" Переписка ", " Chat ")),
    };
    // В двух окнах рамка активного окна цветная: сразу видно, куда пойдёт набираемый текст.
    let border = if two && active { app.theme.accent } else { muted() };
    let block = Block::default().borders(Borders::ALL).border_style(Style::new().fg(border)).title(title);
    if active {
        app.hit_msgs = area;
    }
    let inner = block.inner(area);
    let width = inner.width.max(1) as usize;

    let picking = active && app.mode == Mode::Pick;
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut lines: Vec<Line> = match chat_id {
        None => vec![Line::from(Span::styled(
            if two {
                tr!("Ctrl+K — открыть чат", "Ctrl+K — open a chat")
            } else {
                tr!("Выберите чат слева (↑ ↓), вкладки папок: Tab", "Pick a chat on the left (↑ ↓), folder tabs: Tab")
            },
            Style::new().fg(muted()),
        ))],
        Some(id) => match app.histories.get(&id) {
            None => vec![Line::from(Span::styled(tr!("Загрузка…", "Loading…"), Style::new().fg(muted())))],
            Some(msgs) if msgs.is_empty() => {
                vec![Line::from(Span::styled(tr!("Сообщений пока нет", "No messages yet"), Style::new().fg(muted())))]
            }
            Some(msgs) => {
                let focus = if active { app.focus_msg.and_then(|fid| msgs.iter().position(|m| m.id == fid)) } else { None };
                let sel = if picking { Some(app.pick.min(msgs.len() - 1)) } else { focus };
                let read_out = info.as_ref().filter(|c| c.kind == Kind::User && !c.is_self).map(|c| c.read_out);
                let opts = text::LineOpts {
                    bubble_pct: app.settings.bubble_pct(),
                    selected: sel,
                    sel_bg: app.theme.select_bg,
                    read_out,
                    marked: &app.marked,
                    grouped: app.view() != View::Classic,
                    divider: app.new_from.get(&id).copied(),
                };
                let (l, r) = text::build_message_lines_opts(msgs, width, &opts);
                ranges = r;
                l
            }
        },
    };

    // Сообщения, которые ждут связи, показываются внизу серым.
    if let Some(id) = chat_id {
        for (_, text) in app.queued.iter().filter(|(c, _)| *c == id) {
            let one_line = text.replace('\n', " ");
            let note = tr!(" (в очереди)", " (queued)");
            let room = width.saturating_sub(display_width(note) + 2);
            lines.push(Line::from(Span::styled(format!("⏳ {}{note}", truncate(&one_line, room)), Style::new().fg(muted()))));
        }
    }
    // Закреплённое сообщение занимает верхнюю строку, если места достаточно.
    let banner = chat_id.and_then(|id| app.pinned_msg.get(&id)).filter(|_| inner.height > 6).cloned();
    let body = match banner {
        Some(_) => Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: inner.height - 1 },
        None => inner,
    };
    let height = body.height as usize;
    if picking {
        // Прокрутка так, чтобы выбранное сообщение было на экране.
        if let Some(&(s0, e0)) = ranges.get(app.pick) {
            let total = lines.len();
            let end = total - scroll.min(total.saturating_sub(height));
            let start = end.saturating_sub(height);
            if e0 > end {
                scroll = total - e0;
            } else if s0 < start {
                scroll = total.saturating_sub(s0 + height);
            }
        }
    }
    let max_scroll = lines.len().saturating_sub(height);
    if active && app.focus_pending {
        // Найденное сообщение ставим примерно посередине окна.
        let idx = chat_id.and_then(|id| app.histories.get(&id)).and_then(|h| h.iter().position(|m| Some(m.id) == app.focus_msg));
        if let Some(&(s0, e0)) = idx.and_then(|i| ranges.get(i)) {
            let mid_end = ((s0 + e0) / 2 + height / 2).min(lines.len());
            scroll = lines.len() - mid_end;
        }
        app.focus_pending = false;
    }
    scroll = scroll.min(max_scroll);
    if active {
        app.max_scroll = max_scroll;
        app.scroll = scroll;
    } else {
        app.panes.other_max = max_scroll;
        app.panes.other_scroll = scroll;
    }
    let end = lines.len() - scroll;
    let start = end.saturating_sub(height);
    let visible: Vec<Line> = lines[start..end].to_vec();
    f.render_widget(block, area);
    if let Some(text) = banner {
        let line = Line::from(vec![
            Span::styled(tr!("▌ закреплено: ", "▌ pinned: "), Style::new().fg(app.theme.accent)),
            Span::styled(truncate(&text, (inner.width as usize).saturating_sub(15)), Style::new().fg(muted())),
        ]);
        f.render_widget(Paragraph::new(line), Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 });
    }
    f.render_widget(Paragraph::new(visible), body);
    if active {
        app.geom = Some(MsgGeom { ranges, start, body });
    }

    if scroll > 0 && area.width > 12 {
        let tag = tr!(" ↑ выше · PgDn вниз ", " ↑ older · PgDn down ").to_string();
        let w = display_width(&tag) as u16;
        if area.width > w + 2 {
            let r = Rect { x: area.x + area.width - w - 2, y: area.y, width: w, height: 1 };
            f.render_widget(Paragraph::new(tag).style(Style::new().fg(Color::Black).bg(app.theme.accent)), r);
        }
    }
    if active && app.sel.is_some() && app.jumped == app.sel && area.height > 3 {
        let tag = tr!(" старая переписка · PgDn или Esc — к последним ", " older messages · PgDn or Esc: latest ").to_string();
        let w = display_width(&tag) as u16;
        if area.width > w + 2 {
            let r = Rect { x: area.x + area.width - w - 2, y: area.y + area.height - 1, width: w, height: 1 };
            f.render_widget(Paragraph::new(tag).style(Style::new().fg(Color::Black).bg(app.theme.accent)), r);
        }
    }
}

/// «Если не работает»: вопрос выделен, ответ переносится по ширине окна целиком.
pub(super) fn draw_faq(f: &mut Frame, app: &App, ov: &Overlay) {
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
        Style::new().fg(muted()),
    )));
    let rect = centered_rect(area, w, shown.len() as u16 + 2);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(shown).block(
            Block::default()
                .style(base_style())
                .borders(Borders::ALL)
                .border_style(Style::new().fg(accent))
                .title(Span::styled(ov.title.clone(), Style::new().fg(accent).add_modifier(Modifier::BOLD)))
                .title_bottom(Span::styled(format!(" {}/{} ", ov.sel + 1, ov.items.len()), Style::new().fg(muted()))),
        ),
        rect,
    );
}

pub(super) fn draw_overlay(f: &mut Frame, app: &App) {
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
        OverlayKind::Search => {
            tr!(" ↑ ↓ выбрать · Enter — открыть в чате · Esc — закрыть", " ↑ ↓ select · Enter — open in chat · Esc — close")
        }
        OverlayKind::Buttons => {
            tr!(" ↑ ↓ или цифра — выбрать · Enter — нажать · Esc — закрыть", " ↑ ↓ or a digit — pick · Enter — press · Esc — close")
        }
        OverlayKind::Poll => {
            tr!(" ↑ ↓ или цифра — выбрать · Enter — проголосовать · Esc — закрыть", " ↑ ↓ or a digit — pick · Enter — vote · Esc — close")
        }
        OverlayKind::Topics => {
            tr!(" ↑ ↓ выбрать · Enter — писать в эту тему · Esc — закрыть", " ↑ ↓ select · Enter — post to this topic · Esc — close")
        }
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
        OverlayKind::Drafts => {
            tr!(
                " ↑ ↓ выбрать · Enter — открыть чат с черновиком · Esc — закрыть",
                " ↑ ↓ select · Enter — open the chat with the draft · Esc — close"
            )
        }
    };
    lines.push(Line::from(Span::styled(truncate(hint, (rect.width as usize).saturating_sub(2)), Style::new().fg(muted()))));
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .style(base_style())
                .borders(Borders::ALL)
                .border_style(Style::new().fg(accent))
                .title(Span::styled(ov.title.clone(), Style::new().fg(accent).add_modifier(Modifier::BOLD)))
                .title_bottom(Span::styled(format!(" {}/{} ", ov.sel + 1, ov.items.len()), Style::new().fg(muted()))),
        ),
        rect,
    );
}

pub(super) fn draw_input(f: &mut Frame, app: &App, area: Rect, layout: &InputLayout, rows: usize) {
    let mut title = vec![Span::styled(tr!(" Сообщение ", " Message "), Style::new().fg(app.theme.accent))];
    if app.two_panes()
        && let Some(c) = app.selected()
    {
        title.push(Span::styled(format!("→ {} ", truncate(&c.name, 20)), Style::new().fg(own())));
    }
    if app.markdown {
        title.push(Span::styled("· Markdown ", Style::new().fg(own())));
    }
    if let Some((_, _, name)) = app.topic.as_ref().filter(|(c, _, _)| Some(*c) == app.sel) {
        title.push(Span::styled(trf!("· тема: {name} ", "· topic: {name} "), Style::new().fg(own())));
    }
    if let Some((_, label)) = &app.reply {
        title.push(Span::styled(trf!("· ответ: {label} ", "· reply: {label} "), Style::new().fg(own())));
    }
    if app.edit_target.is_some() {
        title.push(Span::styled(tr!("· правка сообщения ", "· editing message "), Style::new().fg(own())));
    }
    let focus = Style::new().fg(app.theme.accent);
    let skip = if layout.cur_row >= rows { layout.cur_row + 1 - rows } else { 0 };
    let shown: Vec<Line> = if app.input.is_empty() {
        vec![Line::from(Span::styled(
            tr!(
                "Enter — отправить · Shift+Enter — новая строка · F1 — все команды",
                "Enter — send · Shift+Enter — new line · F1 — all commands"
            ),
            Style::new().fg(muted()),
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

pub(super) fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let pick_hint;
    let mode_hint = match app.mode {
        Mode::Pick => {
            pick_hint = if !app.marked.is_empty() {
                trf!(
                    " Выбрано {} · F переслать · D удалить · C копия · Пробел — отметка · Esc сбросить",
                    " Marked {} · F forward · D delete · C copy · Space toggles · Esc clears",
                    app.marked.len()
                )
            } else if let Some(m) = app.picked() {
                // Только то, что для этого сообщения работает: список строит та же функция, что решает, что делают клавиши.
                super::actions::pick_hint(&app.actions(m), !app.jump_back.is_empty())
            } else {
                String::from(" Esc")
            };
            Some(pick_hint.as_str())
        }
        Mode::Forward => Some(tr!(
            " Переслать: выберите чат ↑ ↓ / Tab · Enter — переслать · Esc — отмена",
            " Forward: pick a chat ↑ ↓ / Tab · Enter — forward · Esc — cancel"
        )),
        _ => None,
    };
    if let Some(h) = mode_hint {
        // Сообщение о недоступном действии важнее подсказки по клавишам.
        let (text, color) = if app.mode == Mode::Pick && !app.pick_note.is_empty() {
            (format!(" {}", app.pick_note), mention())
        } else {
            (h.to_string(), app.theme.accent)
        };
        f.render_widget(Paragraph::new(truncate(&text, area.width as usize)).style(Style::new().fg(color)), area);
        return;
    }
    // Подсказка подбирается под ширину окна, чтобы статус слева не вытеснялся.
    let panes_hints = [
        tr!(
            " Ctrl+←/→ окно · Ctrl+K чат · Tab вкладки · Ctrl+W закрыть · F1 справка · F10 выход ",
            " Ctrl+←/→ pane · Ctrl+K chat · Tab tabs · Ctrl+W close · F1 help · F10 quit "
        ),
        tr!(" Ctrl+←/→ окно · Ctrl+K чат · F1 справка ", " Ctrl+←/→ pane · Ctrl+K chat · F1 help "),
        tr!(" F1 справка · F10 выход ", " F1 help · F10 quit "),
        tr!(" F1 справка ", " F1 help "),
    ];
    let list_hints = [
        tr!(
            " Tab папки · Ctrl+K переход · F1 справка · F4 настройки · F2 замок · F10 выход ",
            " Tab folders · Ctrl+K switch · F1 help · F4 settings · F2 lock · F10 quit "
        ),
        tr!(" Tab папки · F1 справка · F4 настройки · F2 замок · F10 выход ", " Tab folders · F1 help · F4 settings · F2 lock · F10 quit "),
        tr!(" F1 справка · F10 выход ", " F1 help · F10 quit "),
        tr!(" F1 справка ", " F1 help "),
    ];
    let hints: &[&str] = if app.two_panes() { &panes_hints } else { &list_hints };
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
    let color = if app.status.starts_with("Ошибка") || app.status.starts_with("Error") { error_color() } else { muted() };
    // Индикатор связи у правого края статуса, если хватает места.
    let (net_text, net_color) = match app.net {
        None => (String::new(), muted()),
        Some(Some(ms)) => (format!("● {ms} {} ", tr!("мс", "ms")), if ms > 1500 { mention() } else { own() }),
        Some(None) => (format!("{} ", tr!("○ нет связи", "○ offline")), error_color()),
    };
    let queued = if app.queued.is_empty() { String::new() } else { trf!("⏳{} ", "⏳{} ", app.queued.len()) };
    let net_text = format!("{queued}{net_text}");
    let net_w = display_width(&net_text) as u16;
    let left = if net_w > 0 && left.width > net_w + 24 {
        let parts =
            Layout::default().direction(Direction::Horizontal).constraints([Constraint::Min(10), Constraint::Length(net_w)]).split(left);
        f.render_widget(Paragraph::new(net_text.clone()).style(Style::new().fg(net_color)), parts[1]);
        parts[0]
    } else {
        left
    };
    let status = truncate(&format!(" {}", app.status), left.width as usize);
    f.render_widget(Paragraph::new(status).style(Style::new().fg(color)), left);
    if let Some((r, h)) = right {
        f.render_widget(Paragraph::new(h).style(Style::new().fg(muted())), r);
    }
}

pub(super) fn centered_rect(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

pub(super) fn on_off(v: bool) -> &'static str {
    if v { tr!("включено", "on") } else { tr!("выключено", "off") }
}

pub(super) fn draw_settings(f: &mut Frame, app: &App) {
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
        (tr!("Мышь", "Mouse"), on_off(s.mouse).to_string()),
        (tr!("Облачные черновики", "Cloud drafts"), on_off(s.cloud_drafts).to_string()),
        (tr!("Тема", "Theme"), s.palette_name().to_string()),
        (tr!("Пароль приложения", "App password"), tr!("сменить…", "change…").to_string()),
        (tr!("Эмодзи в тексте", "Emoji in text"), s.emoji_name().to_string()),
        (tr!("Уведомления Windows", "Windows notifications"), on_off(s.notify).to_string()),
        (tr!("Вид", "View"), s.view_name().to_string()),
        (tr!("Тихие часы", "Quiet hours"), s.quiet_label()),
        (tr!("Сжимать фото при отправке", "Shrink photos on send"), on_off(s.compress).to_string()),
    ];
    let width = 78u16;
    let label_w = 28usize;
    let mut lines = vec![];
    let mut sel_line = 0;
    for (title, group) in settings_sections() {
        lines.push(Line::from(Span::styled(format!(" {title}"), Style::new().fg(muted()).add_modifier(Modifier::BOLD))));
        for &i in group {
            let (label, value) = &rows[i];
            let selected = i == app.settings_row;
            if selected {
                sel_line = lines.len();
            }
            let shown = if i == 14 { value.clone() } else { format!("‹ {value} ›") };
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
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        tr!(" ↑ ↓ выбрать · ← → изменить · Enter — действие · Esc — закрыть", " ↑ ↓ select · ← → change · Enter — action · Esc — close"),
        Style::new().fg(muted()),
    )));
    lines.push(Line::from(Span::styled(
        tr!(
            " Прокси применяется при следующем запуске. Вручную: /proxy адрес:порт",
            " Proxy applies on next start. Manually: /proxy address:port"
        ),
        Style::new().fg(muted()),
    )));
    let rect = centered_rect(f.area(), width, lines.len() as u16 + 2);
    // На низком окне список прокручивается так, чтобы выбранная строка была видна.
    let inner_h = rect.height.saturating_sub(2) as usize;
    let scroll = if lines.len() > inner_h { sel_line.saturating_sub(inner_h.saturating_sub(1)).min(lines.len() - inner_h) } else { 0 };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).scroll((scroll as u16, 0)).block(
            Block::default()
                .style(base_style())
                .borders(Borders::ALL)
                .border_style(Style::new().fg(accent))
                .title(Span::styled(tr!(" Настройки ", " Settings "), Style::new().fg(accent).add_modifier(Modifier::BOLD))),
        ),
        rect,
    );
}

pub(super) fn draw_lock(f: &mut Frame, app: &App, title: &str, prompt: &str, footer: &str) {
    let wait = match (app.mode, app.pause_left()) {
        (Mode::Locked, Some(n)) => Some(trf!("Подождите {n} с до следующей попытки", "Wait {n} s before the next attempt")),
        _ => None,
    };
    let error = wait.as_deref().unwrap_or(&app.lock_error);
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
            error,
            footer,
            busy: false,
        },
    );
}
