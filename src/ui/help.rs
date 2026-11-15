//! Справка (`F1`): клавиши и команды по разделам. Строки переносятся по ширине окна; если справка не помещается
//! на экран по высоте, её можно листать (`↑ ↓ PgUp PgDn Home End`), любая другая клавиша закрывает.

use super::*;

/// Строка справки: заголовок раздела или «клавиша — что делает».
enum Row {
    Section(&'static str),
    Item(&'static str, &'static str),
}

/// Подпись клавиши не длиннее этого (иначе она съела бы место у описания).
const KEY_MAX: usize = 18;
/// Окно справки не шире этого (на широких экранах строки не растягиваются).
const WIDTH: u16 = 100;

fn rows() -> Vec<Row> {
    use Row::{Item, Section};
    vec![
        Section(tr!("Основное", "Basics")),
        Item(
            "Enter",
            tr!(
                "отправить; длинный текст сам делится на сообщения по 4096 знаков",
                "send; long text is split into messages of 4096 characters"
            ),
        ),
        Item("Shift+Enter", tr!("новая строка (вставка текста целиком тоже работает)", "new line (pasting whole text works too)")),
        Item(
            "↑ ↓",
            tr!("выбрать чат; в виде «Два окна» — листать переписку", "select a chat; in the \"Two panes\" layout — scroll the chat"),
        ),
        Item(
            "Tab / Shift+Tab",
            tr!(
                "вкладки; после «/» — дополнить команду; в «Два окна» — открытые чаты",
                "tabs; after \"/\" — complete the command; in \"Two panes\" — open chats"
            ),
        ),
        Item(
            "PgUp / PgDn",
            tr!("прокрутка переписки (PgUp у верха подгружает старые сообщения)", "scroll the chat (PgUp at the top loads older messages)"),
        ),
        Item(
            tr!("Мышь", "Mouse"),
            tr!(
                "клик — чат или вкладка, колесо — прокрутка, протянуть по сообщениям — отметить",
                "click — chat or tab, wheel — scroll, drag over messages — mark them"
            ),
        ),
        Item("Esc", tr!("очистить ввод; затем отменить ответ или правку", "clear the input; then cancel a reply or an edit")),
        Item("Ctrl+Z / Ctrl+Y", tr!("отмена и повтор набора", "undo and redo typing")),
        Item(
            "Alt+↑ / Alt+↓",
            tr!(
                "прошлые сообщения и команды (↑ ↓ тоже, когда в поле есть текст)",
                "previous messages and commands (↑ ↓ too when the input has text)"
            ),
        ),
        Item(
            "Ctrl+V",
            tr!(
                "вставить из буфера (также Shift+Insert и /paste); скриншот в буфере уйдёт фото",
                "paste from the clipboard (also Shift+Insert and /paste); a screenshot goes as a photo"
            ),
        ),
        Item("F1", tr!("эта справка (/help); /faq — если что-то не работает", "this help (/help); /faq — if something does not work")),
        Item("F2", tr!("заблокировать (/lock), спросит пароль", "lock (/lock), asks for the password")),
        Item("F3", tr!("Markdown вкл/выкл (/md)", "Markdown on/off (/md)")),
        Item(
            "F4",
            tr!(
                "настройки (/settings); /proxy и /passwd — прокси и пароль",
                "settings (/settings); /proxy and /passwd — proxy and password"
            ),
        ),
        Item("F5", tr!("обновить список чатов (/refresh)", "refresh the chat list (/refresh)")),
        Item("F6 / Ctrl+F", tr!("поиск чатов по названию (/find текст; Esc сбрасывает)", "find chats by name (/find text; Esc resets)")),
        Item("F10", tr!("выход (/quit)", "quit (/quit)")),
        Item(
            "Ctrl+L Q T R F",
            tr!(
                "то же, что F2, F10, F3, F5, F6; Ctrl+J — новая строка; в любой раскладке",
                "same as F2, F10, F3, F5, F6; Ctrl+J — new line; in any keyboard layout"
            ),
        ),
        Section(tr!("Переход между чатами и вид окна", "Switching chats and the window layout")),
        Item(
            "Ctrl+K",
            tr!(
                "быстрый переход: чаты и команды по первым буквам (раскладка не важна; «/» — команды)",
                "quick switch: chats and commands by the first letters (any layout; \"/\" lists commands)"
            ),
        ),
        Item(
            "Ctrl+N, /next",
            tr!("следующий чат с непрочитанными (сначала с упоминаниями)", "next chat with unread messages (mentions first)"),
        ),
        Item("Alt+1 … Alt+9", tr!("закреплённый чат по номеру (закрепить: /pin)", "the pinned chat with that number (pin one with /pin)")),
        Item(
            tr!("/view [вид]", "/view [layout]"),
            tr!(
                "классический, мессенджер или два окна; без слова — по кругу (есть и в настройках)",
                "classic, messenger or two panes; with no word — the next one (also in the settings)"
            ),
        ),
        Item(
            "F8, Ctrl+← / →",
            tr!(
                "«Два окна»: перейти в другое окно; Ctrl+W закрывает вкладку",
                "\"Two panes\": go to the other pane; Ctrl+W closes the tab"
            ),
        ),
        Item(
            "/mute",
            tr!("этот чат без звука и уведомлений (ещё раз — вернуть)", "this chat without sound and notifications (again — to undo)"),
        ),
        Item(
            tr!("/dnd [минуты]", "/dnd [minutes]"),
            tr!(
                "ничего не звучит и не всплывает на время (/dnd off — выключить)",
                "no sound or notifications for a while (/dnd off — turn off)"
            ),
        ),
        Item("/drafts", tr!("чаты с неотправленным текстом, Enter открывает", "chats with unsent text, Enter opens one")),
        Section(tr!("Сообщения", "Messages")),
        Item(
            "F7",
            tr!(
                "выбрать сообщение: R ответить, F переслать, E править, D удалить, C копировать, S скачать, O открыть, L ссылки",
                "pick a message: R reply, F forward, E edit, D delete, C copy, S save, O open, L links"
            ),
        ),
        Item(
            "F7 → 1-6 G K V P T",
            tr!(
                "реакция (0 — снять); G к оригиналу ответа, B назад; K кнопки бота; V голос в опросе; P фото; T читать текстовый файл",
                "reaction (0 removes); G to the original of a reply, B back; K bot buttons; V vote in a poll; P photo; T read a text file"
            ),
        ),
        Item(
            tr!("F7 → Пробел", "F7 → Space"),
            tr!(
                "отметить сообщения (Shift+↑↓ или мышью); F, D, C действуют на все отмеченные",
                "mark messages (Shift+↑↓ or the mouse); F, D, C act on all marked ones"
            ),
        ),
        Item(
            "/edit, /del",
            tr!(
                "править (/edit новый текст) или удалить своё последнее сообщение (/del просит повторить)",
                "edit (/edit new text) or delete your last message (/del asks to repeat it)"
            ),
        ),
        Item(
            tr!("/later 30 текст", "/later 30 text"),
            tr!("отправить через 30 минут; /scheduled — список и отмена", "send in 30 minutes; /scheduled — the list, cancel"),
        ),
        Item(tr!("/note текст", "/note text"), tr!("быстрая заметка в «Избранное»", "a quick note to Saved Messages")),
        Section(tr!("Файлы и ссылки", "Files and links")),
        Item(
            tr!("/send путь [текст]", "/send path [text]"),
            tr!(
                "отправить файл (путь можно перетащить в окно); фото сжимаются (настройка)",
                "send a file (you can drag a path into the window); photos are shrunk (a setting)"
            ),
        ),
        Item(
            "/album",
            tr!(
                "/album путь1 путь2 … [-- текст]: 2–10 фото одним сообщением-альбомом",
                "/album path1 path2 … [-- text]: 2–10 photos as one album message"
            ),
        ),
        Item(
            "/save, /open",
            tr!("скачать вложение; открыть программой Windows (O в F7)", "download an attachment; open it with the default app (O in F7)"),
        ),
        Item(
            "/copy, /links",
            tr!(
                "копировать текст сообщения (C в F7); ссылки чата (L для сообщения)",
                "copy a message's text (C in F7); links of the chat (L for a message)"
            ),
        ),
        Item(
            tr!("/export [формат]", "/export [format]"),
            tr!("сохранить переписку в export/: txt, md, html или json", "save the conversation to export/: txt, md, html or json"),
        ),
        Section(tr!("Поиск и чаты", "Search and chats")),
        Item(
            "/findmsg, /findall",
            tr!(
                "искать сообщения в этом чате или во всех; /latest — к последним",
                "search messages in this chat or in all of them; /latest — back to the newest"
            ),
        ),
        Item(
            "/pinned, /chat",
            tr!(
                "закреплённые сообщения; /chat @имя — открыть чат по публичному имени",
                "pinned messages; /chat @name — open a chat by its public name"
            ),
        ),
        Item(
            "/join, /leave",
            tr!("вступить по ссылке или @имени; выйти из группы или канала", "join by a link or @name; leave a group or channel"),
        ),
        Item(
            "/topics",
            tr!("темы форума: выбрать, куда писать (/topics off — назад)", "forum topics: pick where to write (/topics off — back)"),
        ),
        Item(
            "/pin, /readall",
            tr!("закрепить чат вверху списка; пометить всё прочитанным", "pin the chat to the top; mark everything as read"),
        ),
        Section(tr!("Если что-то не так", "If something is wrong")),
        Item("/netcheck", tr!("проверить, какой путь до Telegram работает", "check which route to Telegram works")),
        Item(
            "/report",
            tr!(
                "сохранить отчёт tglite-report.txt для автора (без переписки, паролей и номеров)",
                "save tglite-report.txt for the author (no messages, passwords or phone numbers)"
            ),
        ),
    ]
}

/// Строки справки для окна шириной `width` знаков: описания переносятся.
pub(super) fn lines(accent: Color, width: usize) -> Vec<Line<'static>> {
    let all = rows();
    let key_w =
        all.iter().filter_map(|r| if let Row::Item(k, _) = r { Some(display_width(k)) } else { None }).max().unwrap_or(8).min(KEY_MAX) + 2;
    // В очень узком окне колонка клавиш уже: длинные подписи обрезаются, но описание остаётся читаемым.
    let key_w = key_w.min((width * 2 / 5).max(8));
    let desc_w = width.saturating_sub(key_w + 1).max(8);
    let mut out = Vec::new();
    for row in all {
        match row {
            Row::Section(title) => {
                out.push(Line::from(Span::styled(
                    format!(" {}", truncate(title, width.saturating_sub(2))),
                    Style::new().fg(muted()).add_modifier(Modifier::BOLD),
                )));
            }
            Row::Item(key, desc) => {
                for (i, part) in wrap_lines(desc, desc_w).into_iter().enumerate() {
                    let cell = if i == 0 {
                        let key = truncate(key, key_w - 2);
                        format!(" {key}{}", " ".repeat(key_w.saturating_sub(display_width(&key) + 1)))
                    } else {
                        " ".repeat(key_w)
                    };
                    out.push(Line::from(vec![Span::styled(cell, Style::new().fg(accent).add_modifier(Modifier::BOLD)), Span::raw(part)]));
                }
            }
        }
    }
    out
}

pub(super) fn draw_help(f: &mut Frame, app: &mut App) {
    let accent = app.theme.accent;
    let area = f.area();
    let width = area.width.min(WIDTH);
    let lines = lines(accent, width.saturating_sub(2) as usize);
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX).min(area.height);
    let visible = height.saturating_sub(2) as usize;
    app.help_rows = visible;
    app.help_max = lines.len().saturating_sub(visible);
    app.help_top = app.help_top.min(app.help_max);
    let hint = if app.help_max > 0 {
        tr!(" ↑ ↓ PgUp PgDn — листать · другая клавиша закрывает ", " ↑ ↓ PgUp PgDn — scroll · any other key closes ")
    } else {
        tr!(" любая клавиша закрывает ", " any key closes ")
    };
    let rect = centered_rect(area, width, height);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).scroll((u16::try_from(app.help_top).unwrap_or(0), 0)).block(
            Block::default()
                .style(base_style())
                .borders(Borders::ALL)
                .border_style(Style::new().fg(accent))
                .title(Span::styled(
                    trf!(" Справка · Tg-Lite {VERSION} ", " Help · Tg-Lite {VERSION} "),
                    Style::new().fg(accent).add_modifier(Modifier::BOLD),
                ))
                .title_bottom(Span::styled(hint, Style::new().fg(muted()))),
        ),
        rect,
    );
}

impl App {
    pub(super) fn open_help(&mut self) {
        self.help_top = 0;
        self.mode = Mode::Help;
    }

    /// Клавиши справки: стрелки листают, всё остальное закрывает.
    pub(super) fn handle_help_key(&mut self, key: KeyEvent) {
        let page = self.help_rows.saturating_sub(1).max(1);
        match key.code {
            KeyCode::Up => self.help_top = self.help_top.saturating_sub(1),
            KeyCode::Down => self.help_top = (self.help_top + 1).min(self.help_max),
            KeyCode::PageUp => self.help_top = self.help_top.saturating_sub(page),
            KeyCode::PageDown => self.help_top = (self.help_top + page).min(self.help_max),
            KeyCode::Home => self.help_top = 0,
            KeyCode::End => self.help_top = self.help_max,
            _ => {
                self.help_top = 0;
                self.mode = Mode::Chat;
            }
        }
    }
}

#[cfg(test)]
pub(super) fn key_labels() -> Vec<&'static str> {
    rows().into_iter().filter_map(|r| if let Row::Item(k, _) = r { Some(k) } else { None }).collect()
}
