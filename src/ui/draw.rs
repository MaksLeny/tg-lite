//! Отрисовка всех экранов (ratatui). Состояние берётся из `App`, ничего не меняется, кроме прокрутки.

use super::*;

// ------------------------------------------------------------------ рисование

pub(super) fn draw(f: &mut Frame, app: &mut App) {
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

pub(super) fn draw_main(f: &mut Frame, app: &mut App) {
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
    app.hit_list = area;
}

pub(super) fn draw_messages(f: &mut Frame, app: &mut App, area: Rect) {
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
    app.hit_msgs = area;
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
                let focus = app.focus_msg.and_then(|fid| msgs.iter().position(|m| m.id == fid));
                let sel = if picking { Some(app.pick.min(msgs.len() - 1)) } else { focus };
                let read_out = app.selected().filter(|c| c.kind == Kind::User && !c.is_self).map(|c| c.read_out);
                let (l, r) = build_message_lines_marks(msgs, width, app.settings.bubble_pct(), sel, app.theme.select_bg, read_out);
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
    if app.focus_pending {
        // Найденное сообщение ставим примерно посередине окна.
        let idx = app.sel.and_then(|id| app.histories.get(&id)).and_then(|h| h.iter().position(|m| Some(m.id) == app.focus_msg));
        if let Some(&(s0, e0)) = idx.and_then(|i| ranges.get(i)) {
            let mid_end = ((s0 + e0) / 2 + height / 2).min(lines.len());
            app.scroll = lines.len() - mid_end;
        }
        app.focus_pending = false;
    }
    app.max_scroll = max_scroll;
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
    if app.sel.is_some() && app.jumped == app.sel && area.height > 3 {
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
    lines.push(Line::from(Span::styled(truncate(hint, (rect.width as usize).saturating_sub(2)), Style::new().fg(MUTED))));
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

pub(super) fn draw_input(f: &mut Frame, app: &App, area: Rect, layout: &InputLayout, rows: usize) {
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

pub(super) fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let mode_hint = match app.mode {
        Mode::Pick => Some(tr!(
            " ↑ ↓ · R ответ · F переслать · E правка · D удалить · C копия · S скачать · O открыть · L ссылки · Esc",
            " ↑ ↓ · R reply · F forward · E edit · D delete · C copy · S save · O open · L links · Esc"
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

pub(super) fn centered_rect(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

pub(super) fn draw_help(f: &mut Frame, app: &App) {
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
        key(tr!("Мышь", "Mouse"), tr!("клик — чат или вкладка, колесо — прокрутка", "click — chat or tab, wheel — scroll")),
        key(
            "F7",
            tr!("выбор сообщения: R ответить, F переслать, E править, D удалить", "pick message: R reply, F forward, E edit, D delete"),
        ),
        key("Esc", tr!("очистить ввод; отменить ответ, правку, поиск", "clear input; cancel reply, edit, search")),
        key(tr!("F2  или /lock", "F2  or /lock"), tr!("заблокировать (запрос пароля)", "lock (asks for password)")),
        key(tr!("F3  или /md", "F3  or /md"), tr!("Markdown вкл/выкл", "Markdown on/off")),
        key(
            tr!("F4  или /settings", "F4  or /settings"),
            tr!("настройки; /proxy, /passwd — прокси и пароль", "settings; /proxy, /passwd — proxy, password"),
        ),
        key(tr!("F5  или /refresh", "F5  or /refresh"), tr!("обновить список чатов", "refresh chat list")),
        key("F6 / Ctrl+F", tr!("поиск по чатам (/find текст; Esc сбрасывает)", "search chats (/find text; Esc resets)")),
        key(tr!("F10 или /quit", "F10 or /quit"), tr!("выход", "quit")),
        key(
            tr!("/later 30 текст", "/later 30 text"),
            tr!("через 30 минут; /note текст — заметка в «Избранное»", "in 30 minutes; /note text — note to Saved Messages"),
        ),
        key(
            tr!("/edit [текст], /del", "/edit [text], /del"),
            tr!("править / удалить своё последнее сообщение", "edit / delete your last message"),
        ),
        key(
            "/pin /readall /export",
            tr!("закрепить чат; всё прочитано; сохранить переписку в export/", "pin chat; mark all read; save the chat to export/"),
        ),
        key(
            tr!("/send путь [текст]", "/send path [text]"),
            tr!("отправить файл (можно перетащить файл в окно)", "send a file (you can drag a file into the window)"),
        ),
        key("/save", tr!("скачать последнее вложение (или S в режиме F7)", "download the last attachment (or S in F7 mode)")),
        key(
            "/open, /copy",
            tr!(
                "открыть вложение программой Windows (O в F7); копировать текст сообщения (C в F7)",
                "open an attachment with the default app (O in F7); copy message text (C in F7)"
            ),
        ),
        key(
            "/findmsg /findall",
            tr!(
                "найти сообщения в чате или во всех чатах; PgUp у верха подгружает старые",
                "search messages in this chat or in all chats; PgUp at the top loads older ones"
            ),
        ),
        key(
            "Alt+↑ / Alt+↓",
            tr!(
                "прошлые сообщения и команды (↑ ↓ тоже, когда в поле есть текст)",
                "previous messages and commands (↑ ↓ too when the input has text)"
            ),
        ),
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
        key("/pinned, /chat", tr!("закреплённые чата; открыть чат: /chat @имя", "pinned messages; open a chat: /chat @name")),
        key("/join, /leave", tr!("вступить по ссылке или @имени; выйти из чата", "join by link or @name; leave a chat")),
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
            " Прокси применяется при следующем запуске. Вручную: /proxy адрес:порт",
            " Proxy applies on next start. Manually: /proxy address:port"
        ),
        Style::new().fg(MUTED),
    )));
    let rect = centered_rect(f.area(), width, lines.len() as u16 + 2);
    // На низком окне список прокручивается так, чтобы выбранная строка была видна.
    let inner_h = rect.height.saturating_sub(2) as usize;
    let sel_line = 1 + app.settings_row.min(SETTINGS_ROWS - 1);
    let scroll = if lines.len() > inner_h { sel_line.saturating_sub(inner_h.saturating_sub(1)).min(lines.len() - inner_h) } else { 0 };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).scroll((scroll as u16, 0)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(accent))
                .title(Span::styled(tr!(" Настройки ", " Settings "), Style::new().fg(accent).add_modifier(Modifier::BOLD))),
        ),
        rect,
    );
}

pub(super) fn draw_lock(f: &mut Frame, app: &App, title: &str, prompt: &str, footer: &str) {
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
