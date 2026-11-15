//! Тесты интерфейса: состояние, клавиши, команды, отрисовка на разных размерах.

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
        read_out: 0,
        draft: String::new(),
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
        can_write: true,
        preview: String::new(),
        last_ts: 0,
        muted: false,
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
    let mama = app.chats[0].id;
    app.handle_event(Event::SendFailed { chat: mama, text: "длинный пост".into() }, &tx);
    assert_eq!(app.input, "длинный пост");
    assert_eq!(app.cursor, 12);
    // если пользователь уже начал печатать новое, его текст не затираем: неотправленное дописывается ниже
    app.handle_event(Event::SendFailed { chat: mama, text: "другой".into() }, &tx);
    assert_eq!(app.input, "длинный пост\n\nдругой");
    // тот же текст дважды не дописывается
    app.handle_event(Event::SendFailed { chat: mama, text: "другой".into() }, &tx);
    assert_eq!(app.input, "длинный пост\n\nдругой");
    // пустышка ничего не меняет
    app.handle_event(Event::SendFailed { chat: mama, text: "  ".into() }, &tx);
    assert_eq!(app.input, "длинный пост\n\nдругой");
}

#[test]
fn unsent_text_of_another_chat_never_lands_in_the_open_chats_input() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let (mama, group) = (app.chats[0].id, app.chats[2].id);
    assert_eq!(app.sel, Some(mama));
    // Отправка в «Рабочий чат» не удалась, пока открыта «Мама»: текст уходит в черновик рабочего чата.
    app.handle_event(Event::SendFailed { chat: group, text: "отчёт".into() }, &tx);
    assert!(app.input.is_empty(), "в поле ввода чужого чата текста быть не должно");
    assert_eq!(app.drafts[&group].0, "отчёт");
    assert!(app.status.contains("черновиком чата «Рабочий чат»"), "{}", app.status);
    // Второй неотправленный кусок дописывается в тот же черновик.
    app.handle_event(Event::SendFailed { chat: group, text: "второй кусок".into() }, &tx);
    assert_eq!(app.drafts[&group].0, "отчёт\n\nвторой кусок");
    // При открытии чата черновик возвращается в поле ввода.
    app.set_sel(Some(group));
    assert_eq!(app.input, "отчёт\n\nвторой кусок");
    // Сообщение, не вышедшее из очереди, ведёт себя так же.
    app.handle_event(Event::QueueDropped { chat: mama, text: "из очереди".into(), reason: "ошибка".into() }, &tx);
    assert_eq!(app.input, "отчёт\n\nвторой кусок", "поле ввода открытого чата не тронуто");
    assert_eq!(app.drafts[&mama].0, "из очереди");
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
    app.paste(Some("x".repeat(crate::split::MAX_INPUT_CHARS + 500)));
    assert_eq!(app.input_len(), crate::split::MAX_INPUT_CHARS);
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

    app.settings_row = 1; // прокси
    app.handle_key(key(KeyCode::Right, none), &tx, &s);
    assert_eq!(app.settings.proxy_mode, ProxyMode::Off);
    app.settings_row = 2; // звук
    app.handle_key(key(KeyCode::Enter, none), &tx, &s);
    assert!(app.settings.sound);
    app.settings_row = 3; // статус
    app.handle_key(key(KeyCode::Char(' '), none), &tx, &s);
    assert!(!app.settings.show_status);
    app.settings_row = 4; // история
    app.handle_key(key(KeyCode::Right, none), &tx, &s);
    assert_eq!(app.settings.history_limit, 100);
    app.settings_row = 5; // цвет
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
        for mode in [Mode::Chat, Mode::Pick, Mode::Forward, Mode::Overlay, Mode::Help, Mode::Settings, Mode::NewPassword, Mode::Locked] {
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
                    ..Default::default()
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
fn export_command_writes_the_chosen_format_and_rejects_unknown_ones() {
    let mut app = app_with_chats();
    let dir = std::env::temp_dir().join(format!("tglite-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    app.cmd_export("", &dir);
    assert!(app.status.starts_with("Сохранено: "), "{}", app.status);
    assert!(app.status.contains("(2 сообщ.)"), "{}", app.status);
    let txt = std::fs::read_to_string(dir.join("Мама.txt")).unwrap();
    assert!(txt.contains("Я: Всё хорошо!") || txt.contains("Мама: Привет"), "{txt}");
    for (arg, ext, marker) in [("md", "md", "# Мама"), ("html", "html", "<!doctype html>"), ("json", "json", "\"messages\"")] {
        app.cmd_export(arg, &dir);
        let body = std::fs::read_to_string(dir.join(format!("Мама.{ext}"))).unwrap();
        assert!(body.contains(marker), "{arg}: {body}");
    }
    // Неизвестный формат: ничего не пишем.
    app.cmd_export("pdf", &dir);
    assert!(app.status.contains("txt, md, html или json"), "{}", app.status);
    assert!(!dir.join("Мама.pdf").exists());
    // Нет истории и нет чата.
    let group = app.chats[2].id;
    app.set_sel(Some(group));
    app.histories.remove(&group);
    app.cmd_export("md", &dir);
    assert!(app.status.contains("нечего сохранять"), "{}", app.status);
    app.sel = None;
    app.cmd_export("md", &dir);
    assert!(app.status.contains("Сначала выберите чат"), "{}", app.status);
    // Папка, куда писать нельзя: ошибка в строке состояния, а не паника.
    app.sel = Some(app.chats[0].id);
    let blocked = dir.join("Мама.txt").join("внутри"); // «папка» внутри обычного файла
    app.cmd_export("", &blocked);
    assert!(app.status.starts_with("Ошибка"), "{}", app.status);
    let _ = std::fs::remove_dir_all(&dir);
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
    let own = app.sel.unwrap();
    app.histories.get_mut(&own).unwrap()[1].raw = "Всё хорошо!".into(); // у настоящих сообщений исходный текст всегда есть
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
        Some(Cmd::Forward { src, dst, ids }) => {
            assert_eq!(src.id, mama);
            assert_eq!(dst.name, "Мой канал");
            assert_eq!(ids, vec![2]);
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
fn long_message_goes_out_in_parts_and_a_huge_one_is_refused_and_kept() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    // Чуть длиннее лимита Telegram: одна команда, делить на части будет сетевая задача.
    let long = "а".repeat(MAX_MESSAGE_CHARS + 1);
    app.input = long.clone();
    app.cursor = app.input_len();
    slow(&mut app);
    app.handle_key(key(KeyCode::Enter, KeyModifiers::NONE), &tx, &s);
    match drain(&mut rx).pop() {
        Some(Cmd::Send { text, .. }) => assert_eq!(text, long),
        _ => panic!("ожидалась одна команда Send"),
    }
    assert!(app.input.is_empty());
    assert!(app.status.contains("2 сообщ"), "{}", app.status);
    // Из такого количества текста вышло бы больше двенадцати сообщений: отказ, текст остаётся в поле.
    let huge = "слово ".repeat(12_000);
    app.input = huge.clone();
    app.cursor = app.input_len();
    slow(&mut app);
    app.handle_key(key(KeyCode::Enter, KeyModifiers::NONE), &tx, &s);
    assert!(drain(&mut rx).is_empty());
    assert_eq!(app.input, huge, "длинный пост не пропадает");
    assert!(app.status.contains("/send"), "{}", app.status);
    // Правка — всегда одно сообщение: длиннее лимита её не принимаем.
    app.clear_input();
    app.edit_target = Some(2);
    app.input = long.clone();
    app.cursor = app.input_len();
    slow(&mut app);
    app.handle_key(key(KeyCode::Enter, KeyModifiers::NONE), &tx, &s);
    assert!(drain(&mut rx).is_empty());
    assert_eq!(app.input, long);
    assert!(app.status.contains("4096"), "{}", app.status);
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

// ---- регрессионные тесты по итогам аудита 1.0

#[test]
fn scheduled_reply_cannot_open_over_the_lock_screen() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let chat = app.selected().unwrap().id;
    app.lock();
    let items = vec![crate::tg::ScheduledMsg { id: 1, date: 1_800_000_000, text: "секрет".into() }];
    app.handle_event(Event::Scheduled { chat, items: items.clone() }, &tx);
    assert_eq!(app.mode, Mode::Locked, "ответ сервера не должен снимать блокировку");
    assert!(app.overlay.is_none());
    // то же для настроек
    app.mode = Mode::Settings;
    app.handle_event(Event::Scheduled { chat, items }, &tx);
    assert_eq!(app.mode, Mode::Settings);
}

#[test]
fn later_with_bad_format_is_not_sent_as_text() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    for bad in ["/later", "/later 30m созвон", "/later abc x", "/later 0 x", "/later 999999999 x"] {
        submit_text(&mut app, bad, &tx, &s);
        assert!(drain(&mut rx).is_empty(), "{bad} не должно уходить собеседнику");
        assert!(app.status.contains("/later"), "{bad}: {}", app.status);
    }
    submit_text(&mut app, "/later 30 созвон", &tx, &s);
    assert!(matches!(drain(&mut rx).pop(), Some(Cmd::Send { .. })));
}

#[test]
fn opened_chat_stays_in_unread_tab_and_arrows_do_not_skip() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    app.chats[0].unread = 1;
    app.chats[1].unread = 1;
    app.chats[2].unread = 1;
    app.tab = Tab::Unread;
    app.ensure_selection();
    let first = app.selected().unwrap().id;
    // чат открыт и прочитан: он остаётся в списке и не сдвигает остальные
    app.chats.iter_mut().find(|c| c.id == first).unwrap().unread = 0;
    assert!(app.visible_ids().contains(&first));
    press(&mut app, KeyCode::Down, &tx, &s);
    let second = app.selected().unwrap().id;
    press(&mut app, KeyCode::Down, &tx, &s);
    let third = app.selected().unwrap().id;
    assert_ne!(first, second);
    assert_ne!(second, third);
    assert_ne!(first, third, "стрелки не должны пропускать чаты");
}

#[test]
fn edit_uses_original_text_and_does_not_destroy_a_draft() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let id = app.selected().unwrap().id;
    // своё сообщение с вложением: исходный текст (с эмодзи) важнее очищенного
    let mut m = msg(9, true, "Я", "[фото] привет");
    m.caption = Some("привет".into());
    m.raw = "привет 😀".into();
    app.histories.get_mut(&id).unwrap().push(m.clone());
    assert_eq!(edit_source(&m), "привет 😀");
    let plain = msg(10, true, "Я", "просто");
    assert_eq!(edit_source(&plain), "просто");
    // /edit без текста берёт подпись, а не метку вложения
    submit_text(&mut app, "/edit", &tx, &s);
    assert_eq!(app.input, "/edit привет 😀");
    // черновик не затирается правкой через F7
    app.clear_input();
    type_text(&mut app, "черновик", &tx, &s);
    press(&mut app, KeyCode::F(7), &tx, &s);
    press(&mut app, KeyCode::Char('e'), &tx, &s);
    assert_eq!(app.input, "черновик");
    assert!(app.edit_target.is_none());
}

#[test]
fn empty_enter_on_lock_screen_is_not_an_attempt() {
    let mut app = app_with_chats();
    app.settings.wipe_after = 3;
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    app.lock();
    for _ in 0..10 {
        press(&mut app, KeyCode::Enter, &tx, &s);
    }
    assert_eq!(app.wrong_attempts, 0);
    assert!(!s.is_wiped());
    assert!(!app.should_quit);
}

#[test]
fn history_load_keeps_messages_that_arrived_meanwhile() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let id = app.selected().unwrap().id;
    app.histories.insert(id, vec![msg(5, false, "Мама", "пришло во время загрузки")]);
    app.handle_event(Event::History { chat: id, msgs: vec![msg(1, false, "Мама", "старое"), msg(2, true, "Я", "ещё старое")] }, &tx);
    let ids: Vec<i32> = app.histories[&id].iter().map(|m| m.id).collect();
    assert_eq!(ids, vec![1, 2, 5]);
    // повторная загрузка с тем же последним сообщением не дублирует
    app.handle_event(Event::History { chat: id, msgs: vec![msg(1, false, "Мама", "a"), msg(5, false, "Мама", "b")] }, &tx);
    let ids: Vec<i32> = app.histories[&id].iter().map(|m| m.id).collect();
    assert_eq!(ids, vec![1, 5]);
}

#[test]
fn global_delete_does_not_touch_channel_histories() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let user = app.chats.iter().find(|c| c.kind == Kind::User).unwrap().id;
    let channel = app.chats.iter().find(|c| c.kind == Kind::Channel).unwrap().id;
    app.histories.insert(user, vec![msg(100, false, "Мама", "личное")]);
    app.histories.insert(channel, vec![msg(100, false, "Канал", "пост")]);
    app.handle_event(Event::Deleted { chat: None, ids: vec![100] }, &tx);
    assert!(app.histories[&user].is_empty());
    assert_eq!(app.histories[&channel].len(), 1, "номера в канале свои");
    app.handle_event(Event::Deleted { chat: Some(channel), ids: vec![100] }, &tx);
    assert!(app.histories[&channel].is_empty());
}

#[test]
fn altgr_characters_are_not_ctrl_shortcuts() {
    let c = KeyModifiers::CONTROL | KeyModifiers::ALT;
    assert_eq!(ctrl_letter(&key(KeyCode::Char('@'), c)), None);
    assert_eq!(ctrl_letter(&key(KeyCode::Char('\\'), c)), None);
    assert_eq!(ctrl_letter(&key(KeyCode::Char('€'), c)), None);
}

#[test]
fn settings_scroll_keeps_selected_row_visible_on_short_windows() {
    let mut app = demo_app();
    app.mode = Mode::Settings;
    for row in [0usize, 5, 10, 11, 12, 13, 14] {
        app.settings_row = row;
        let mut term = Terminal::new(TestBackend::new(100, 10)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        let text = dump(&term);
        let label = match row {
            0 => "Автоблокировка",
            5 => "Цвет интерфейса",
            10 => "Язык интерфейса",
            11 => "Мышь",
            12 => "Облачные черновики",
            13 => "Тема",
            _ => "Пароль приложения",
        };
        assert!(text.contains(label), "строка {row} не видна:\n{text}");
    }
}

#[test]
fn tiny_terminals_do_not_panic_with_overlays_and_settings() {
    let mut app = demo_app();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    submit_text(&mut app, "/faq", &tx, &s);
    for (w, h) in [(0u16, 0u16), (1, 1), (2, 2), (5, 3), (20, 8), (40, 12)] {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
    }
    app.mode = Mode::Settings;
    for (w, h) in [(1u16, 1u16), (10, 5), (60, 10), (80, 14)] {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
    }
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
    app.lock_until = None; // пауза после неверных паролей проверяется отдельным тестом
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
        Some(Cmd::SendFile { path: p, caption, compress, .. }) => {
            assert_eq!(p, path);
            assert_eq!(caption, "мой файл");
            assert!(compress, "сжатие фото включено по умолчанию");
        }
        _ => panic!("ожидалась отправка файла"),
    }
    // Выключили сжатие в настройках — команда несёт «не сжимать».
    app.settings.compress = false;
    submit_text(&mut app, &format!("/send \"{}\"", path.display()), &tx, &s);
    assert!(matches!(drain(&mut rx).pop(), Some(Cmd::SendFile { compress: false, .. })));
    let _ = std::fs::remove_file(&path);
    // несуществующий файл: команды нет, текст остаётся для исправления
    submit_text(&mut app, "/send \"/нет/такого/файла.bin\"", &tx, &s);
    assert!(drain(&mut rx).is_empty());
    assert!(app.status.contains("файл не найден"), "{}", app.status);
    assert!(app.input.starts_with("/send"));
}

/// Сообщение с вложением: вид и имя файла берутся из подписи-метки (`[фото]`, `[файл a.pdf, 1 МБ]`).
fn msg_with_attachment(id: i32, label_text: &str, caption: &str) -> ChatMsg {
    let mut m = msg(id, false, "Мама", label_text);
    m.caption = Some(caption.into());
    m.media = Some(if label_text.starts_with("[фото") {
        tg::MediaInfo { kind: tg::MediaKind::Photo, name: format!("photo_{id}.jpg"), size: None }
    } else if let Some(rest) = label_text.strip_prefix("[файл ") {
        let name = rest.split([',', ']']).next().unwrap_or("file");
        tg::MediaInfo { kind: tg::MediaKind::File, name: name.into(), size: None }
    } else {
        tg::MediaInfo { kind: tg::MediaKind::File, name: format!("file_{id}"), size: None }
    });
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
    assert!(app.pick_note.contains("нет файла"), "{}", app.pick_note);
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
    give_demo_file_an_attachment(&mut pick);
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

/// Кадры для GIF-демо: `TGLITE_SHOTS=/путь [TGLITE_LANG=en] cargo test dump_demo_frames -- --ignored`.
#[test]
#[ignore]
fn dump_demo_frames() {
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
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let mut app = demo_app();
    give_demo_file_an_attachment(&mut app);
    app.status = tr!("Готово", "Done").into();
    let mut n = 0;
    let mut shot = |app: &mut App, ms: u32| {
        let mut term = Terminal::new(TestBackend::new(110, 28)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer();
        let rows: Vec<Vec<String>> = (0..28)
            .map(|y| {
                (0..110)
                    .map(|x| {
                        let c = &buf[(x, y)];
                        format!("{}|{}|{}", c.symbol(), col(c.fg), col(c.bg))
                    })
                    .collect()
            })
            .collect();
        std::fs::write(std::path::Path::new(&dir).join(format!("f{n:02}_{ms}.json")), serde_json::to_string(&rows).unwrap()).unwrap();
        n += 1;
    };
    shot(&mut app, 1800);
    // набор сообщения
    let text = tr!("Спасибо, прочитал. Завтра отвечу", "Thanks, read it. I will reply tomorrow");
    let chars: Vec<char> = text.chars().collect();
    for chunk in chars.chunks(6) {
        for c in chunk {
            app.handle_key(key(KeyCode::Char(*c), KeyModifiers::NONE), &tx, &s);
        }
        shot(&mut app, 140);
    }
    shot(&mut app, 900);
    app.clear_input();
    // вкладки и выбор чата с клавиатуры
    press(&mut app, KeyCode::Tab, &tx, &s);
    shot(&mut app, 1200);
    press(&mut app, KeyCode::BackTab, &tx, &s);
    // поиск
    let cmd = tr!("/findall отчёт", "/findall report");
    for c in cmd.chars() {
        app.handle_key(key(KeyCode::Char(c), KeyModifiers::NONE), &tx, &s);
    }
    shot(&mut app, 1000);
    app.clear_input();
    let q = tr!("отчёт", "report");
    let hit = |chat: PeerId, name: &str, id: i32, who: &str, text: &str| tg::SearchHit {
        chat,
        chat_name: name.into(),
        id,
        ts: 1_761_650_000 + i64::from(id) * 60,
        when: "10:01".into(),
        sender: who.into(),
        text: text.into(),
    };
    let anna = app.chats[0].id;
    let team = app.chats[2].id;
    let hits = vec![
        hit(
            anna,
            tr!("Анна", "Anna"),
            1,
            tr!("Анна", "Anna"),
            tr!("Привет! Ты посмотрел отчёт, который я присылала?", "Hi! Did you look at the report I sent?"),
        ),
        hit(
            team,
            tr!("Команда проекта", "Project Team"),
            7,
            tr!("Борис", "Boris"),
            tr!("Отчёт за квартал готов, смотрите в общей папке", "The quarterly report is ready, see the shared folder"),
        ),
    ];
    app.handle_event(Event::SearchResults { query: q.into(), scope: None, hits }, &tx);
    shot(&mut app, 2200);
    press(&mut app, KeyCode::Esc, &tx, &s);
    // выбор сообщения
    press(&mut app, KeyCode::F(7), &tx, &s);
    shot(&mut app, 1200);
    press(&mut app, KeyCode::Up, &tx, &s);
    shot(&mut app, 1500);
    press(&mut app, KeyCode::Esc, &tx, &s);
    // настройки
    app.mode = Mode::Settings;
    shot(&mut app, 1500);
    app.settings_row = 5;
    app.handle_key(key(KeyCode::Right, KeyModifiers::NONE), &tx, &s);
    shot(&mut app, 1000);
    app.handle_key(key(KeyCode::Right, KeyModifiers::NONE), &tx, &s);
    shot(&mut app, 1500);
    app.mode = Mode::Chat;
    shot(&mut app, 1500);
}

/// В `demo_app` вложение — просто текст; для кадров F7 даём файлу настоящее вложение, чтобы были видны действия с файлом.
fn give_demo_file_an_attachment(app: &mut App) {
    let id = app.chats[0].id;
    if let Some(m) = app.histories.get_mut(&id).and_then(|h| h.iter_mut().find(|m| m.id == 4)) {
        m.media =
            Some(tg::MediaInfo { kind: tg::MediaKind::File, name: tr!("итоги.pdf", "summary.pdf").into(), size: Some(2_000_000) });
    }
}

/// Кадр экрана в JSON для `tools`-скриптов: по клетке «знак|цвет|фон».
fn frame_json(app: &mut App, w: u16, h: u16) -> String {
    let col = |c: Color| match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Black => "#000000".to_string(),
        Color::White => "#ffffff".to_string(),
        Color::Gray => "#aaaaaa".to_string(),
        _ => String::new(),
    };
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| draw(f, app)).unwrap();
    let buf = term.backend().buffer();
    let rows: Vec<Vec<String>> = (0..h)
        .map(|y| {
            (0..w)
                .map(|x| {
                    let c = &buf[(x, y)];
                    format!("{}|{}|{}", c.symbol(), col(c.fg), col(c.bg))
                })
                .collect()
        })
        .collect();
    serde_json::to_string(&rows).unwrap()
}

/// `demo_app` с перепиской и последней строкой у каждого чата: для кадров видов «мессенджер» и «два окна».
fn demo_rich() -> App {
    let mut app = demo_app();
    let ids: Vec<PeerId> = app.chats.iter().map(|c| c.id).collect();
    let (tech, team, food, boris) = (ids[1], ids[2], ids[3], ids[4]);
    let anna_last = app.histories[&ids[0]].last().cloned().unwrap();
    app.touch_preview(ids[0], &anna_last);
    let (me, boris_n, vera, anna) = (tr!("Я", "Me"), tr!("Борис", "Boris"), tr!("Вера", "Vera"), tr!("Анна", "Anna"));
    let lists = vec![
        (
            team,
            vec![
                msg(1, false, boris_n, tr!("Макеты готовы, выложил в общую папку", "The mockups are ready, uploaded to the shared folder")),
                msg(2, false, vera, tr!("Посмотрю после обеда", "I will look after lunch")),
                msg(3, false, vera, tr!("Там на третьем экране кнопка уехала", "The button on the third screen is off")),
                msg(4, true, me, tr!("Поправлю сегодня", "I will fix it today")),
                msg(5, false, boris_n, tr!("Тогда в пятницу демо, не забудьте", "Then the demo is on Friday, do not forget")),
                msg(6, false, anna, tr!("Я подготовлю слайды", "I will prepare the slides")),
            ],
        ),
        (
            tech,
            vec![
                msg(
                    1,
                    false,
                    tr!("Новости техники", "Tech News"),
                    tr!("Обзор: как выбрать недорогой ноутбук для работы", "Review: how to pick an inexpensive laptop for work"),
                ),
                msg(
                    2,
                    false,
                    tr!("Новости техники", "Tech News"),
                    tr!(
                        "Вышла новая версия редактора: быстрее запуск и меньше памяти",
                        "A new editor version is out: faster start and less memory"
                    ),
                ),
            ],
        ),
        (
            food,
            vec![msg(
                1,
                false,
                tr!("Рецепты", "Recipes"),
                tr!("Борщ за 40 минут: пошаговый рецепт", "Borscht in 40 minutes: a step-by-step recipe"),
            )],
        ),
        (boris, vec![msg(1, false, boris_n, tr!("Созвонимся завтра?", "Shall we call tomorrow?"))]),
    ];
    for (id, list) in lists {
        app.touch_preview(id, list.last().unwrap());
        app.histories.insert(id, list);
    }
    app.status.clear();
    app.chats[4].draft = tr!("Да, в 11 удобно", "Yes, 11 works for me").into();
    app
}

/// Кадры видов для README: `TGLITE_SHOTS=/путь [TGLITE_LANG=en] cargo test dump_view_frames -- --ignored`.
#[test]
#[ignore]
fn dump_view_frames() {
    let Some(dir) = std::env::var_os("TGLITE_SHOTS") else { return };
    std::fs::create_dir_all(&dir).unwrap();
    crate::i18n::set(u8::from(std::env::var("TGLITE_LANG").is_ok_and(|v| v == "en")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let save = |name: &str, app: &mut App| {
        std::fs::write(std::path::Path::new(&dir).join(format!("ui_{name}.json")), frame_json(app, 110, 32)).unwrap();
    };
    let mut messenger = demo_rich();
    messenger.set_view(1);
    let team = messenger.chats[2].id;
    messenger.open_chat(team);
    messenger.mark_new_from(team, 2);
    save("messenger", &mut messenger);
    let mut panes = demo_rich();
    panes.set_view(2);
    let (anna, team) = (panes.chats[0].id, panes.chats[2].id);
    panes.open_chat(team);
    press_mod(&mut panes, KeyCode::F(8), KeyModifiers::NONE, &tx, &s);
    panes.open_chat(anna);
    press_mod(&mut panes, KeyCode::F(8), KeyModifiers::NONE, &tx, &s);
    panes.chats[0].unread = 0;
    panes.chats[2].unread = 0;
    save("panes", &mut panes);
    let mut quick = demo_rich();
    press_mod(&mut quick, KeyCode::Char('k'), KeyModifiers::CONTROL, &tx, &s);
    type_text(&mut quick, tr!("пр", "pr"), &tx, &s);
    save("quick", &mut quick);
}

/// Кадры GIF про виды: `TGLITE_SHOTS=/путь [TGLITE_LANG=en] cargo test dump_view_demo_frames -- --ignored`.
#[test]
#[ignore]
fn dump_view_demo_frames() {
    let Some(dir) = std::env::var_os("TGLITE_SHOTS") else { return };
    std::fs::create_dir_all(&dir).unwrap();
    crate::i18n::set(u8::from(std::env::var("TGLITE_LANG").is_ok_and(|v| v == "en")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let mut app = demo_rich();
    let mut n = 0;
    let mut shot = |app: &mut App, ms: u32| {
        std::fs::write(std::path::Path::new(&dir).join(format!("f{n:02}_{ms}.json")), frame_json(app, 110, 28)).unwrap();
        n += 1;
    };
    shot(&mut app, 1800);
    submit_text(&mut app, "/view messenger", &tx, &s);
    let (anna, team) = (app.chats[0].id, app.chats[2].id);
    app.open_chat(team);
    app.mark_new_from(team, 2);
    shot(&mut app, 2600);
    submit_text(&mut app, "/view panes", &tx, &s);
    app.open_chat(team);
    press_mod(&mut app, KeyCode::F(8), KeyModifiers::NONE, &tx, &s);
    app.open_chat(anna);
    press_mod(&mut app, KeyCode::F(8), KeyModifiers::NONE, &tx, &s);
    shot(&mut app, 2600);
    press_mod(&mut app, KeyCode::Char('k'), KeyModifiers::CONTROL, &tx, &s);
    shot(&mut app, 900);
    for c in tr!("борис", "boris").chars() {
        app.handle_key(key(KeyCode::Char(c), KeyModifiers::NONE), &tx, &s);
        shot(&mut app, 220);
    }
    shot(&mut app, 1000);
    press(&mut app, KeyCode::Enter, &tx, &s);
    shot(&mut app, 2600);
    submit_text(&mut app, "/view classic", &tx, &s);
    shot(&mut app, 1800);
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

// ---- проверки документации: README не должен расходиться с программой

const README: &str = include_str!("../../README.md");

#[test]
fn readme_mentions_every_command() {
    // /search, /exit и подобные — псевдонимы, в README их нет
    let aliases = ["/search", "/exit", "/delete", "/password"];
    for c in COMMANDS.iter().filter(|c| !aliases.contains(c)) {
        assert!(README.contains(c), "команда {c} не описана в README");
    }
}

#[test]
fn readme_images_exist_and_versions_match() {
    for part in README.split("docs/").skip(1) {
        let name: String = part.chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
        assert!(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs").join(&name).exists(), "нет картинки docs/{name}");
    }
    assert!(README.contains("1.6.0"), "README должен упоминать текущую версию");
    assert_eq!(VERSION, "1.6.0");
}

#[test]
fn in_app_faq_and_help_cover_the_documented_features() {
    crate::i18n::set(0);
    let faq = faq();
    assert_eq!(faq.len(), 7);
    assert!(faq.iter().all(|(q, a)| !q.is_empty() && !a.is_empty()));
    let mut app = demo_app();
    app.mode = Mode::Help;
    let text = draw_text(&mut app, 110, 120);
    for needle in ["/send", "/save", "/links", "/scheduled", "/paste", "/faq", "F7"] {
        assert!(text.contains(needle), "в справке нет {needle}");
    }
}

#[test]
fn workflows_exist_and_release_runs_on_version_tags() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
    let rel = std::fs::read_to_string(root.join(".github/workflows/release.yml")).unwrap();
    assert!(ci.contains("cargo clippy") && ci.contains("cargo test") && ci.contains("cargo fmt --check"));
    assert!(rel.contains("tags:") && rel.contains("\"v*\"") && rel.contains("sha256"));
    assert!(!rel.contains("TGLITE_API"), "релизный exe не должен содержать ключи");
}

// ---- пачка 1.0.1: история ввода, копирование, /open, прокси с логином

fn alt(app: &mut App, code: KeyCode, tx: &mpsc::UnboundedSender<Cmd>, s: &Arc<Saver>) {
    slow(app);
    app.handle_key(key(code, KeyModifiers::ALT), tx, s);
}

#[test]
fn input_history_walks_back_and_restores_the_draft() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    submit_text(&mut app, "один", &tx, &s);
    submit_text(&mut app, "два", &tx, &s);
    submit_text(&mut app, "два", &tx, &s); // повтор подряд не плодит записи
    assert_eq!(app.input_history, vec!["один", "два"]);

    app.clear_input();
    type_text(&mut app, "чер", &tx, &s);
    press(&mut app, KeyCode::Up, &tx, &s);
    assert_eq!(app.input, "два");
    press(&mut app, KeyCode::Up, &tx, &s);
    assert_eq!(app.input, "один");
    press(&mut app, KeyCode::Up, &tx, &s);
    assert_eq!(app.input, "один", "дальше начала не уходит");
    press(&mut app, KeyCode::Down, &tx, &s);
    assert_eq!(app.input, "два");
    press(&mut app, KeyCode::Down, &tx, &s);
    assert_eq!(app.input, "чер", "после конца истории возвращается недописанный текст");
    assert_eq!(app.cursor, 3);
    press(&mut app, KeyCode::Down, &tx, &s);
    assert_eq!(app.input, "чер", "вниз без истории ничего не ломает");
}

#[test]
fn plain_arrows_still_pick_chats_when_the_input_is_empty() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    submit_text(&mut app, "привет", &tx, &s);
    app.clear_input();
    let before = app.sel;
    press(&mut app, KeyCode::Down, &tx, &s);
    assert_ne!(app.sel, before, "стрелка вниз при пустом поле выбирает следующий чат");
    assert!(app.input.is_empty());
}

#[test]
fn alt_arrows_recall_history_from_an_empty_input() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    submit_text(&mut app, "первое", &tx, &s);
    submit_text(&mut app, "второе", &tx, &s);
    app.clear_input();
    let chat_before = app.sel;
    alt(&mut app, KeyCode::Up, &tx, &s);
    assert_eq!(app.input, "второе");
    assert_eq!(app.sel, chat_before, "чат не переключился");
    alt(&mut app, KeyCode::Up, &tx, &s);
    assert_eq!(app.input, "первое");
    alt(&mut app, KeyCode::Down, &tx, &s);
    alt(&mut app, KeyCode::Down, &tx, &s);
    assert_eq!(app.input, "", "пустой черновик возвращается");
}

#[test]
fn typing_after_recall_leaves_history_mode() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    submit_text(&mut app, "abc", &tx, &s);
    app.clear_input();
    alt(&mut app, KeyCode::Up, &tx, &s);
    assert_eq!(app.input, "abc");
    type_text(&mut app, "d", &tx, &s);
    assert_eq!(app.hist_pos, None);
    assert_eq!(app.input, "abcd");
}

#[test]
fn history_is_capped_and_skips_proxy_passwords() {
    let mut app = app_with_chats();
    for i in 0..130 {
        app.remember_input(&format!("m{i}"));
    }
    assert_eq!(app.input_history.len(), 100);
    assert_eq!(app.input_history.first().map(String::as_str), Some("m30"));
    app.remember_input("/proxy user:secret@127.0.0.1:3128");
    assert!(!app.input_history.iter().any(|h| h.contains("secret")));
    app.remember_input("/proxy 127.0.0.1:1080");
    assert_eq!(app.input_history.last().map(String::as_str), Some("/proxy 127.0.0.1:1080"));
    app.remember_input("");
    assert_eq!(app.input_history.last().map(String::as_str), Some("/proxy 127.0.0.1:1080"));
}

#[test]
fn proxy_with_login_is_stored_but_never_shown() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    submit_text(&mut app, "/proxy bob:secret@127.0.0.1:3128", &tx, &s);
    assert_eq!(app.settings.proxy_manual, "bob:secret@127.0.0.1:3128", "в настройках адрес с логином целиком");
    assert!(!app.status.contains("secret") && !app.status.contains("bob"), "на экране пароля нет: {}", app.status);
    assert!(app.status.contains("***@127.0.0.1:3128"));
    assert!(!app.input_history.iter().any(|h| h.contains("secret")));
    let mut term = Terminal::new(TestBackend::new(110, 40)).unwrap();
    app.mode = Mode::Settings;
    term.draw(|f| draw(f, &mut app)).unwrap();
    let text = dump(&term);
    assert!(!text.contains("secret"), "в настройках пароля нет");
}

#[test]
fn pick_c_copies_message_text_in_both_layouts() {
    crate::i18n::set(0);
    for letter in ['c', 'с', 'C'] {
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = saver();
        app.histories.get_mut(&app.sel.unwrap()).unwrap()[1].raw = "Всё хорошо!".into();
        press(&mut app, KeyCode::F(7), &tx, &s);
        press(&mut app, KeyCode::Char(letter), &tx, &s);
        // Программы буфера обмена в тестовой среде может не быть: важно, что действие отработало и сказало результат.
        assert!(
            app.pick_note.starts_with("Скопировано") || app.pick_note.starts_with("Не удалось скопировать"),
            "{letter}: {}",
            app.pick_note
        );
        assert_eq!(app.mode, Mode::Pick, "режим выбора остаётся");
    }
}

#[test]
fn copy_without_text_says_so() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let id = app.sel.unwrap();
    app.histories.get_mut(&id).unwrap()[1].raw = "Всё хорошо!".into();
    app.histories.get_mut(&id).unwrap().push(msg_with_attachment(3, "[фото]", ""));
    press(&mut app, KeyCode::F(7), &tx, &s);
    assert_eq!(app.pick, 2);
    press(&mut app, KeyCode::Char('c'), &tx, &s);
    assert_eq!(app.pick_note, "В этом сообщении нет текста");
    app.mode = Mode::Chat;
    submit_text(&mut app, "/copy", &tx, &s);
    assert!(app.status.starts_with("Скопировано") || app.status.starts_with("Не удалось скопировать"), "{}", app.status);
}

#[test]
fn copy_command_needs_a_message_with_text() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    for m in app.histories.get_mut(&app.sel.unwrap()).unwrap() {
        m.raw.clear();
    }
    submit_text(&mut app, "/copy", &tx, &s);
    assert_eq!(app.status, "Нет сообщения с текстом");
}

#[test]
fn open_command_and_pick_o_request_a_temporary_download() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    let id = app.sel.unwrap();
    app.histories.get_mut(&id).unwrap().push(msg_with_attachment(7, "[файл a.pdf, 1 МБ]", "подпись"));
    submit_text(&mut app, "/open", &tx, &s);
    assert!(matches!(rx.try_recv(), Ok(Cmd::Open { id: 7, .. })));

    for letter in ['o', 'щ'] {
        app.mode = Mode::Chat;
        press(&mut app, KeyCode::F(7), &tx, &s);
        press(&mut app, KeyCode::Char(letter), &tx, &s);
        assert!(matches!(rx.try_recv(), Ok(Cmd::Open { id: 7, .. })), "{letter}");
        assert_eq!(app.mode, Mode::Chat);
    }
}

#[test]
fn open_without_attachment_does_nothing() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    submit_text(&mut app, "/open", &tx, &s);
    assert_eq!(app.status, "В загруженных сообщениях нет вложений");
    press(&mut app, KeyCode::F(7), &tx, &s);
    press(&mut app, KeyCode::Char('o'), &tx, &s);
    assert_eq!(app.pick_note, "В этом сообщении нет файла, который можно открыть");
    assert!(rx.try_recv().is_err(), "ничего не скачивается");
}

#[test]
fn opened_file_event_refuses_unsafe_types() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    app.handle_event(Event::Open(std::path::PathBuf::from("/tmp/virus.exe")), &tx);
    assert!(app.status.starts_with("Не удалось открыть"), "{}", app.status);
    app.handle_event(Event::Open(std::path::PathBuf::from("/tmp/definitely-missing.png")), &tx);
    assert!(app.status.starts_with("Не удалось открыть"), "несуществующий файл: {}", app.status);
}

#[test]
fn new_commands_are_listed_for_tab_completion_and_help() {
    assert!(COMMANDS.contains(&"/open") && COMMANDS.contains(&"/copy"));
    crate::i18n::set(1);
    let mut app = demo_app();
    app.mode = Mode::Help;
    let text = draw_text(&mut app, 110, 120);
    for needle in ["/open", "/copy", "Alt+"] {
        assert!(text.contains(needle), "в справке нет {needle}");
    }
    app.mode = Mode::Pick;
    let mut term = Terminal::new(TestBackend::new(110, 40)).unwrap();
    term.draw(|f| draw(f, &mut app)).unwrap();
    let text = dump(&term);
    assert!(text.contains("R reply") && text.contains("1-6 react") && text.contains("F forward"), "подсказка режима выбора: {text}");
    crate::i18n::set(0);
}

#[test]
fn opened_file_is_deleted_if_the_app_was_locked_meanwhile() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let path = std::env::temp_dir().join(format!("tglite-late-open-{}.png", std::process::id()));
    std::fs::write(&path, b"x").unwrap();
    app.mode = Mode::Locked;
    app.handle_event(Event::Open(path.clone()), &tx);
    assert!(!path.exists(), "файл стёрт, а не открыт");
    assert_eq!(app.mode, Mode::Locked);
}

#[test]
fn arrows_with_text_but_no_history_still_move_the_chat_selection() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    type_text(&mut app, "abc", &tx, &s);
    let before = app.sel;
    press(&mut app, KeyCode::Down, &tx, &s);
    assert_ne!(app.sel, before, "истории нет, поэтому стрелка работает как раньше");
    assert_eq!(app.drafts.get(&before.unwrap()).map(|d| d.0.as_str()), Some("abc"), "набранное осталось черновиком прежнего чата");
}

/// Бюджет памяти: модель данных для 100 чатов по 60 сообщений (настройка по умолчанию) должна занимать мало.
/// Считается куча, выделенная под чаты и переписки; сам процесс на Windows занимает больше (код, стек, консоль).
#[test]
fn memory_budget_for_100_chats_with_history() {
    let before = crate::alloc_count::live();
    let mut app = App::new(Settings::default());
    let mut chats = Vec::new();
    for i in 0..100 {
        let c = chat(1000 + i, &format!("Чат номер {i} с достаточно длинным названием"), Kind::Group, (i % 7) as i32);
        let hist: Vec<ChatMsg> = (0..60)
            .map(|n| {
                msg(
                    n,
                    n % 3 == 0,
                    "Участник чата",
                    "Обычное сообщение средней длины, примерно как в живой переписке: привет, как дела, что нового?",
                )
            })
            .collect();
        app.histories.insert(c.id, hist);
        chats.push(c);
    }
    app.chats = chats;
    app.ensure_selection();
    let used = crate::alloc_count::live() - before;
    eprintln!("100 чатов x 60 сообщений: {:.2} МБ", used as f64 / 1048576.0);
    assert!(used > 0, "счётчик работает");
    assert!(used < 4 * 1024 * 1024, "бюджет 4 МБ превышен: {:.2} МБ. Что-то стало хранить слишком много", used as f64 / 1048576.0);
    drop(app);
}

// ---- пачка 1.0.2: весь список чатов, старая история, поиск сообщений

fn id_of(app: &App, i: usize) -> PeerId {
    app.chats[i].id
}

fn long_history(n: i32) -> Vec<ChatMsg> {
    (1..=n).map(|i| msg(i, i % 2 == 0, "Мама", &format!("сообщение №{i}"))).collect()
}

#[test]
fn dialogs_page_merges_without_losing_chats() {
    let mut app = app_with_chats(); // Мама(1), Мой канал(2), Рабочий чат(3)
    let (tx, _rx) = mpsc::unbounded_channel();
    let selected = app.sel;
    let page = vec![chat(2, "Мой канал", Kind::Channel, 5), chat(4, "Новый", Kind::User, 1)];
    app.handle_event(Event::DialogsPage(page), &tx);
    let names: Vec<&str> = app.chats.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Мой канал", "Новый", "Мама", "Рабочий чат"], "свежие сверху, остальные на месте");
    assert_eq!(app.chats[0].unread, 5, "данные страницы новее");
    assert_eq!(app.sel, selected, "выбранный чат остался");
    assert!(!app.status.starts_with("Чатов"), "слияние не пишет итог загрузки");
    // полный список заменяет всё
    app.handle_event(Event::Dialogs(vec![chat(9, "Единственный", Kind::User, 0)]), &tx);
    assert_eq!(app.chats.len(), 1);
}

#[test]
fn full_dialog_list_beyond_120_is_kept() {
    let mut app = App::new(Settings::default());
    let (tx, _rx) = mpsc::unbounded_channel();
    let list: Vec<ChatInfo> = (1..=500).map(|i| chat(i, &format!("Чат {i}"), Kind::Group, 0)).collect();
    app.handle_event(Event::Dialogs(list), &tx);
    assert_eq!(app.chats.len(), 500);
    assert_eq!(app.visible_ids().len(), 500, "вкладка «Все» показывает все чаты");
    app.handle_event(Event::DialogsPage((1..=100).rev().map(|i| chat(i, &format!("Чат {i}"), Kind::Group, 0)).collect()), &tx);
    assert_eq!(app.chats.len(), 500, "быстрое обновление не теряет чаты за пределами первой страницы");
    assert_eq!(app.chats[0].name, "Чат 100");
}

#[test]
fn page_up_at_the_top_requests_older_messages_once() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    let id = app.sel.unwrap();
    app.histories.insert(id, long_history(10)[5..].to_vec()); // 6..=10
    app.max_scroll = 20;
    app.scroll = 5;
    press(&mut app, KeyCode::PageUp, &tx, &s);
    assert!(rx.try_recv().is_err(), "ещё не наверху: сеть не нужна");
    assert_eq!(app.scroll, 13);
    app.scroll = 20;
    press(&mut app, KeyCode::PageUp, &tx, &s);
    match rx.try_recv() {
        Ok(Cmd::LoadOlder { before, limit, .. }) => {
            assert_eq!(before, 6, "старше самого раннего загруженного");
            assert!(limit >= 30);
        }
        _ => panic!("ждали LoadOlder"),
    }
    press(&mut app, KeyCode::PageUp, &tx, &s);
    assert!(rx.try_recv().is_err(), "пока идёт загрузка, второй запрос не отправляется");
}

#[test]
fn older_messages_are_prepended_without_duplicates_and_mark_the_start() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let id = app.sel.unwrap();
    app.histories.insert(id, long_history(10)[5..].to_vec());
    app.older_busy = true;
    app.handle_event(Event::Older { chat: id, msgs: long_history(8) }, &tx); // 1..=8, из них 6..=8 уже есть
    let ids: Vec<i32> = app.histories[&id].iter().map(|m| m.id).collect();
    assert_eq!(ids, (1..=10).collect::<Vec<_>>());
    assert!(!app.older_busy);
    assert_eq!(app.status, "Загружено ещё сообщений: 5");
    app.handle_event(Event::Older { chat: id, msgs: vec![] }, &tx);
    assert!(app.older_done.contains(&id));
    assert_eq!(app.status, "Это начало переписки");
    // повторный PgUp у верха ничего не просит
    let (tx2, mut rx2) = mpsc::unbounded_channel();
    app.request_older(&tx2);
    assert!(rx2.try_recv().is_err());
}

#[test]
fn older_messages_keep_the_picked_message_selected() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let id = app.sel.unwrap();
    app.histories.insert(id, long_history(10)[5..].to_vec());
    app.mode = Mode::Pick;
    app.pick = 0;
    app.handle_event(Event::Older { chat: id, msgs: long_history(5) }, &tx);
    assert_eq!(app.pick, 5);
    assert_eq!(app.histories[&id][app.pick].id, 6, "всё то же сообщение");
}

#[test]
fn pick_mode_up_at_the_first_message_loads_older() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    press(&mut app, KeyCode::F(7), &tx, &s);
    app.pick = 0;
    press(&mut app, KeyCode::Up, &tx, &s);
    assert!(matches!(rx.try_recv(), Ok(Cmd::LoadOlder { .. })));
}

#[test]
fn history_stops_growing_at_the_memory_cap() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let id = app.sel.unwrap();
    app.histories.insert(id, long_history(MAX_LOADED as i32));
    app.request_older(&tx);
    assert!(rx.try_recv().is_err());
    assert!(app.status.contains("/findmsg"), "{}", app.status);
}

#[test]
fn find_commands_send_search_requests() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    submit_text(&mut app, "/findmsg отчёт за квартал", &tx, &s);
    match rx.try_recv() {
        Ok(Cmd::SearchChat { query, chat }) => {
            assert_eq!(query, "отчёт за квартал");
            assert_eq!(chat.name, "Мама");
        }
        _ => panic!("ждали SearchChat"),
    }
    submit_text(&mut app, "/findall привет", &tx, &s);
    assert!(matches!(rx.try_recv(), Ok(Cmd::SearchAll { query }) if query == "привет"));
    submit_text(&mut app, "/findmsg", &tx, &s);
    assert!(rx.try_recv().is_err());
    assert!(app.status.starts_with("Что искать"));
    app.chats.clear();
    app.ensure_selection();
    submit_text(&mut app, "/findmsg abc", &tx, &s);
    assert_eq!(app.status, "Сначала выберите чат");
    assert!(rx.try_recv().is_err());
}

fn hit(chat: i64, id: i32, text: &str) -> tg::SearchHit {
    let pid = if chat == 3 { PeerId::chat(3).unwrap() } else { PeerId::user(chat).unwrap() };
    tg::SearchHit {
        chat: pid,
        chat_name: format!("Чат {chat}"),
        id,
        ts: 1_761_650_000 + i64::from(id) * 60,
        when: "10:01".into(),
        sender: "Мама".into(),
        text: text.into(),
    }
}

#[test]
fn search_results_open_in_an_overlay_and_enter_loads_the_surroundings() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    app.handle_event(
        Event::SearchResults {
            query: "привет".into(), scope: None, hits: vec![hit(1, 77, "привет, как дела"), hit(3, 5, "всем привет")]
        },
        &tx,
    );
    assert_eq!(app.mode, Mode::Overlay);
    let ov = app.overlay.as_ref().unwrap();
    assert_eq!(ov.kind, OverlayKind::Search);
    assert_eq!(ov.items.len(), 2);
    assert!(ov.items[0].label.starts_with("[Чат 1]"), "при поиске везде видно, из какого чата: {}", ov.items[0].label);
    assert!(ov.title.contains("привет") && ov.title.contains('2'));
    press(&mut app, KeyCode::Down, &tx, &s);
    press(&mut app, KeyCode::Enter, &tx, &s);
    assert_eq!(app.mode, Mode::Chat);
    assert!(app.overlay.is_none());
    match rx.try_recv() {
        Ok(Cmd::LoadAround { id, chat, .. }) => {
            assert_eq!(id, 5);
            assert_eq!(chat.name, "Рабочий чат");
        }
        _ => panic!("ждали LoadAround"),
    }
}

#[test]
fn found_message_that_is_already_loaded_is_just_highlighted() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let id = app.sel.unwrap();
    app.open_found(id, 2, 0, &tx);
    assert!(rx.try_recv().is_err(), "загружать нечего");
    assert_eq!(app.focus_msg, Some(2));
    assert!(app.focus_pending);
    assert_eq!(app.jumped, None, "это не кусок старой переписки");
}

#[test]
fn search_with_no_hits_or_while_locked_does_nothing_visible() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    app.handle_event(Event::SearchResults { query: "zzz".into(), scope: None, hits: vec![] }, &tx);
    assert_eq!(app.mode, Mode::Chat);
    assert_eq!(app.status, "По запросу «zzz» ничего не найдено");
    app.mode = Mode::Locked;
    app.handle_event(Event::SearchResults { query: "a".into(), scope: None, hits: vec![hit(1, 1, "a")] }, &tx);
    assert_eq!(app.mode, Mode::Locked);
    assert!(app.overlay.is_none(), "результаты не всплывают поверх блокировки");
    app.handle_event(Event::Around { chat: id_of(&app, 0), msgs: long_history(3), focus: 2 }, &tx);
    assert_eq!(app.mode, Mode::Locked);
    assert_eq!(app.jumped, None);
}

#[test]
fn jumping_to_an_old_message_and_coming_back() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    let id = app.sel.unwrap();
    app.handle_event(Event::Around { chat: id, msgs: long_history(40), focus: 10 }, &tx);
    assert_eq!(app.jumped, Some(id));
    assert_eq!(app.focus_msg, Some(10));
    assert_eq!(app.histories[&id].len(), 40);

    // запоздавший ответ загрузки последних сообщений не затирает кусок
    app.handle_event(Event::History { chat: id, msgs: long_history(3) }, &tx);
    assert_eq!(app.histories[&id].len(), 40);

    // новое сообщение не вставляется посреди старой переписки, а только отмечается
    let unread = app.chats.iter().find(|c| c.id == id).unwrap().unread;
    app.handle_event(Event::Incoming { chat: id, chat_name: "Мама".into(), msg: msg(900, false, "Мама", "новое") }, &tx);
    assert_eq!(app.histories[&id].len(), 40);
    assert_eq!(app.chats.iter().find(|c| c.id == id).unwrap().unread, unread + 1);

    // PgDn у самого низа возвращает к последним
    app.scroll = 0;
    press(&mut app, KeyCode::PageDown, &tx, &s);
    assert_eq!(app.jumped, None);
    assert!(matches!(rx.try_recv(), Ok(Cmd::LoadHistory(..))));

    // то же по Esc и по /latest
    for how in ["esc", "cmd"] {
        app.handle_event(Event::Around { chat: id, msgs: long_history(40), focus: 10 }, &tx);
        app.focus_msg = None;
        if how == "esc" {
            press(&mut app, KeyCode::Esc, &tx, &s);
        } else {
            submit_text(&mut app, "/latest", &tx, &s);
        }
        assert_eq!(app.jumped, None, "{how}");
        assert!(matches!(rx.try_recv(), Ok(Cmd::LoadHistory(..))), "{how}");
    }
}

#[test]
fn leaving_a_jumped_chat_drops_the_old_slice() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let id = app.sel.unwrap();
    app.handle_event(Event::Around { chat: id, msgs: long_history(40), focus: 10 }, &tx);
    app.move_selection(1);
    assert_ne!(app.sel, Some(id));
    assert_eq!(app.jumped, None);
    assert!(!app.histories.contains_key(&id), "при возвращении история загрузится заново");
}

#[test]
fn found_message_is_centered_and_marked_on_screen() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let id = app.sel.unwrap();
    app.handle_event(Event::Around { chat: id, msgs: long_history(100), focus: 20 }, &tx);
    let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
    term.draw(|f| draw(f, &mut app)).unwrap();
    let text = dump(&term);
    assert!(text.contains("сообщение №20"), "найденное сообщение на экране");
    assert!(!text.contains("сообщение №100"), "а не последнее");
    assert!(text.contains("к последним"), "подсказка, как вернуться: {text}");
    assert!(!app.focus_pending);
    assert!(app.max_scroll > 0);
}

#[test]
fn search_overlay_and_help_render_on_small_screens() {
    for lang in [0, 1] {
        crate::i18n::set(lang);
        let mut app = app_with_chats();
        let (tx, _rx) = mpsc::unbounded_channel();
        let long = "очень длинный текст найденного сообщения ".repeat(8);
        app.handle_event(Event::SearchResults { query: "q".into(), scope: None, hits: vec![hit(1, 1, &long), hit(2, 2, "x")] }, &tx);
        for (w, h) in [(110u16, 40u16), (80, 24), (40, 12)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| draw(f, &mut app)).unwrap();
        }
        let mut term = Terminal::new(TestBackend::new(110, 40)).unwrap();
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(dump(&term).contains("Enter"), "подсказка окна поиска ({lang})");
    }
    crate::i18n::set(0);
}

// ---- пачка 1.1.0: мышь

fn mouse(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
    MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE }
}

fn drawn(app: &mut App) {
    let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
    term.draw(|f| draw(f, app)).unwrap();
}

#[test]
fn mouse_click_selects_chat_and_tab() {
    let mut app = app_with_chats(); // Мама, Мой канал, Рабочий чат
    let (tx, _rx) = mpsc::unbounded_channel();
    drawn(&mut app);
    // чат во второй строке списка (первая строка — рамка)
    assert!(app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, 3), &tx));
    assert_eq!(app.sel, Some(app.chats[1].id));
    assert!(app.pending_load.is_some(), "история загрузится как при выборе клавишей");
    // клик по рамке и по пустому месту ничего не ломает
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, 1), &tx);
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, 25), &tx);
    assert_eq!(app.sel, Some(app.chats[1].id));
    // вкладка «Каналы»
    drawn(&mut app);
    let (x0, _, tab) = app.hit_tabs.iter().find(|(_, _, t)| *t == Tab::Channels).cloned().unwrap();
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x0 + 1, 0), &tx);
    assert_eq!(app.tab, tab);
    assert_eq!(app.visible_ids(), vec![app.chats[1].id]);
    assert_eq!(app.sel, Some(app.chats[1].id));
}

#[test]
fn mouse_wheel_scrolls_messages_or_moves_in_the_list() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let id = app.sel.unwrap();
    app.histories.insert(id, long_history(60));
    drawn(&mut app);
    let m = app.hit_msgs;
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, m.x + 5, m.y + 5), &tx);
    assert_eq!(app.scroll, 3);
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, m.x + 5, m.y + 5), &tx);
    assert_eq!(app.scroll, 0);
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, m.x + 5, m.y + 5), &tx);
    assert_eq!(app.scroll, 0, "ниже последнего не уходит");
    // у верха колесо подгружает старую историю
    app.scroll = app.max_scroll;
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, m.x + 5, m.y + 5), &tx);
    assert!(matches!(rx.try_recv(), Ok(Cmd::LoadOlder { .. })));
    // над списком чатов колесо двигает выбор
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 3, 5), &tx);
    assert_eq!(app.sel, Some(app.chats[1].id));
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, 3, 5), &tx);
    assert_eq!(app.sel, Some(app.chats[0].id));
}

#[test]
fn mouse_is_ignored_when_off_locked_or_in_dialogs() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    drawn(&mut app);
    let click = mouse(MouseEventKind::Down(MouseButton::Left), 3, 3);
    app.settings.mouse = false;
    assert!(!app.handle_mouse(click, &tx));
    assert_eq!(app.sel, Some(app.chats[0].id));
    app.settings.mouse = true;
    for mode in [Mode::Locked, Mode::Settings, Mode::Help, Mode::Overlay] {
        app.mode = mode;
        assert!(!app.handle_mouse(click, &tx));
        assert_eq!(app.sel, Some(app.chats[0].id));
    }
    // простое движение мыши не вызывает перерисовку
    app.mode = Mode::Chat;
    assert!(!app.handle_mouse(mouse(MouseEventKind::Moved, 3, 3), &tx));
}

#[test]
fn mouse_setting_toggles_and_is_saved() {
    let mut app = App::new(Settings::default());
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    assert!(app.settings.mouse, "по умолчанию включена");
    app.mode = Mode::Settings;
    app.settings_row = 11;
    app.handle_key(key(KeyCode::Right, KeyModifiers::NONE), &tx, &s);
    assert!(!app.settings.mouse);
    assert!(!s.settings().mouse);
    app.handle_key(key(KeyCode::Left, KeyModifiers::NONE), &tx, &s);
    assert!(app.settings.mouse);
}

#[test]
fn mouse_tab_hit_areas_match_what_is_drawn() {
    let mut app = demo_app();
    let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
    term.draw(|f| draw(f, &mut app)).unwrap();
    let first_row: String = (0..110).map(|x| term.backend().buffer()[(x, 0)].symbol().to_string()).collect();
    for (a, b, tab) in app.hit_tabs.clone() {
        let title = app.tabs().into_iter().find(|(t, _)| *t == tab).unwrap().1;
        let cells: String = (a..b).map(|x| term.backend().buffer()[(x, 0)].symbol().to_string()).collect();
        assert!(cells.contains(&title), "область вкладки {title:?} не совпала с нарисованным: {cells:?} в {first_row:?}");
    }
}

// ---- пачка 1.2.0: отметки прочтения, /pinned, /chat, /join, /leave

fn private_app() -> App {
    let mut app = app_with_chats();
    let id = app.chats[0].id; // Мама: личный чат
    app.histories.insert(id, vec![msg(1, true, "Я", "первое"), msg(2, true, "Я", "второе"), msg(3, false, "Мама", "ответ")]);
    app.chats[0].read_out = 1;
    app
}

#[test]
fn read_marks_show_sent_and_read_in_private_chats_only() {
    crate::i18n::set(0);
    assert_eq!(read_mark(5, 5), " √√");
    assert_eq!(read_mark(6, 5), " √");
    assert_eq!(read_mark(0, 5), "", "ещё не отправленное сообщение без отметки");
    let mut app = private_app();
    let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
    term.draw(|f| draw(f, &mut app)).unwrap();
    let text = dump(&term);
    assert!(text.contains("Я √√"), "первое сообщение прочитано: {text}");
    assert!(text.contains("Я √"), "второе только отправлено");
    assert_eq!(text.matches("√√").count(), 1);
    // в группе и канале отметок нет
    let gid = app.chats[2].id;
    app.histories.insert(gid, vec![msg(1, true, "Я", "в группе")]);
    app.chats[2].read_out = 5;
    app.set_sel(Some(gid));
    term.draw(|f| draw(f, &mut app)).unwrap();
    assert!(!dump(&term).contains('√'));
    // «Избранное» тоже без отметок
    let mut saved = private_app();
    saved.chats[0].is_self = true;
    term.draw(|f| draw(f, &mut saved)).unwrap();
    assert!(!dump(&term).contains('√'));
}

#[test]
fn read_outbox_update_moves_the_mark_forward_only() {
    let mut app = private_app();
    let (tx, _rx) = mpsc::unbounded_channel();
    let id = app.chats[0].id;
    app.handle_event(Event::ReadOutbox { chat: id, max_id: 2 }, &tx);
    assert_eq!(app.chats[0].read_out, 2);
    app.handle_event(Event::ReadOutbox { chat: id, max_id: 1 }, &tx);
    assert_eq!(app.chats[0].read_out, 2, "запоздавшее старое обновление не откатывает отметку");
    app.handle_event(Event::ReadOutbox { chat: PeerId::user(999).unwrap(), max_id: 9 }, &tx);
}

#[test]
fn pinned_command_lists_pinned_messages_and_opens_one() {
    crate::i18n::set(0);
    let mut app = private_app();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    submit_text(&mut app, "/pinned", &tx, &s);
    assert!(matches!(rx.try_recv(), Ok(Cmd::ListPinned(c)) if c.name == "Мама"));
    let id = app.chats[0].id;
    app.handle_event(Event::PinnedList { chat: id, hits: vec![] }, &tx);
    assert_eq!(app.status, "В этом чате нет закреплённых сообщений");
    assert!(app.overlay.is_none());
    app.handle_event(Event::PinnedList { chat: id, hits: vec![hit(1, 50, "важное"), hit(1, 3, "ответ")] }, &tx);
    let ov = app.overlay.as_ref().unwrap();
    assert!(ov.title.contains("Закреплённые") && ov.title.contains('2') && ov.title.contains("Мама"), "{}", ov.title);
    assert!(!ov.items[0].label.starts_with('['), "имя чата в списке не повторяется");
    press(&mut app, KeyCode::Enter, &tx, &s);
    assert!(matches!(rx.try_recv(), Ok(Cmd::LoadAround { id: 50, .. })), "старое закреплённое открывается среди соседних");
    // закреплённое, которое уже загружено, просто подсвечивается
    app.handle_event(Event::PinnedList { chat: id, hits: vec![hit(1, 2, "второе")] }, &tx);
    press(&mut app, KeyCode::Enter, &tx, &s);
    assert!(rx.try_recv().is_err());
    assert_eq!(app.focus_msg, Some(2));
    // без выбранного чата и под блокировкой
    app.mode = Mode::Locked;
    app.handle_event(Event::PinnedList { chat: id, hits: vec![hit(1, 2, "x")] }, &tx);
    assert!(app.overlay.is_none() || app.mode == Mode::Locked);
    app.mode = Mode::Chat;
    app.chats.clear();
    app.ensure_selection();
    submit_text(&mut app, "/pinned", &tx, &s);
    assert_eq!(app.status, "Сначала выберите чат");
    assert!(rx.try_recv().is_err());
}

#[test]
fn chat_and_join_commands_validate_their_argument() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    submit_text(&mut app, "/chat @durov", &tx, &s);
    assert!(matches!(rx.try_recv(), Ok(Cmd::OpenTarget(tg::Target::Username(n))) if n == "durov"));
    submit_text(&mut app, "/chat t.me/durov", &tx, &s);
    assert!(matches!(rx.try_recv(), Ok(Cmd::OpenTarget(_))));
    submit_text(&mut app, "/chat t.me/+AbCdEfGh12345", &tx, &s);
    assert!(rx.try_recv().is_err(), "приглашение через /chat не принимается");
    assert!(app.status.contains("/join"));
    submit_text(&mut app, "/chat", &tx, &s);
    assert!(rx.try_recv().is_err());
    assert!(app.status.starts_with("Укажите @имя"));
    submit_text(&mut app, "/join https://t.me/+AbCdEfGh12345", &tx, &s);
    assert!(matches!(rx.try_recv(), Ok(Cmd::Join(tg::Target::Invite(h))) if h == "AbCdEfGh12345"));
    submit_text(&mut app, "/join @mychannel", &tx, &s);
    assert!(matches!(rx.try_recv(), Ok(Cmd::Join(tg::Target::Username(n))) if n == "mychannel"));
    submit_text(&mut app, "/join что-то не то", &tx, &s);
    assert!(rx.try_recv().is_err());
    assert!(app.status.starts_with("Укажите ссылку"));
}

#[test]
fn found_chat_is_added_to_the_list_and_opened() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let newcomer = chat(77, "Новый знакомый", Kind::User, 0);
    let id = newcomer.id;
    app.tab = Tab::Channels; // чужая вкладка: чат всё равно должен стать видимым
    app.handle_event(Event::Found(newcomer.clone()), &tx);
    assert_eq!(app.chats.len(), 4);
    assert_eq!(app.chats[0].id, id);
    assert_eq!(app.sel, Some(id));
    assert_eq!(app.tab, Tab::All);
    assert!(app.pending_load.is_some());
    assert_eq!(app.status, "Открыт чат «Новый знакомый»");
    // уже известный чат не дублируется
    app.handle_event(Event::Found(newcomer), &tx);
    assert_eq!(app.chats.len(), 4);
    // под блокировкой не открывается
    app.mode = Mode::Locked;
    let other = chat(78, "Другой", Kind::User, 0);
    app.handle_event(Event::Found(other), &tx);
    assert_eq!(app.chats.len(), 4);
}

#[test]
fn leave_needs_confirmation_and_refuses_private_chats() {
    crate::i18n::set(0);
    let mut app = app_with_chats(); // выбран личный чат «Мама»
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    submit_text(&mut app, "/leave", &tx, &s);
    assert!(rx.try_recv().is_err());
    assert!(app.status.starts_with("Из личного чата выйти нельзя"));
    let group = app.chats[2].id;
    app.set_sel(Some(group));
    submit_text(&mut app, "/leave", &tx, &s);
    assert!(rx.try_recv().is_err(), "первый /leave только спрашивает");
    assert!(app.status.contains("Повторите /leave"));
    submit_text(&mut app, "/leave", &tx, &s);
    assert!(matches!(rx.try_recv(), Ok(Cmd::Leave(c)) if c.id == group));
    // любая другая команда сбрасывает подтверждение
    submit_text(&mut app, "/leave", &tx, &s);
    submit_text(&mut app, "/latest", &tx, &s);
    submit_text(&mut app, "/leave", &tx, &s);
    assert!(rx.try_recv().is_err());
    // смена чата тоже
    app.set_sel(Some(app.chats[1].id));
    submit_text(&mut app, "/leave", &tx, &s);
    app.set_sel(Some(group));
    submit_text(&mut app, "/leave", &tx, &s);
    assert!(rx.try_recv().is_err());
}

#[test]
fn leaving_removes_the_chat_everywhere() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let group = app.chats[2].id;
    app.histories.insert(group, long_history(3));
    app.drafts.insert(group, ("черновик".into(), 3));
    app.set_sel(Some(group));
    app.handle_event(Event::Left(group), &tx);
    assert_eq!(app.chats.len(), 2);
    assert!(!app.histories.contains_key(&group));
    assert!(!app.drafts.contains_key(&group));
    assert!(app.sel.is_some() && app.sel != Some(group), "выбор переходит на другой чат");
}

// ---- пачка E (1.2.0): облачные черновики и просмотры постов

fn with_draft(mut c: ChatInfo, text: &str) -> ChatInfo {
    c.draft = text.into();
    c
}

#[test]
fn view_counts_are_short() {
    for (n, want) in [
        (5, "5"),
        (999, "999"),
        (1000, "1.0K"),
        (1234, "1.2K"),
        (9999, "9.9K"),
        (10_000, "10K"),
        (15_800, "15K"),
        (999_999, "999K"),
        (1_000_000, "1.0M"),
        (2_450_000, "2.4M"),
    ] {
        assert_eq!(short_count(n), want, "{n}");
    }
}

#[test]
fn channel_posts_show_views() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let channel = app.chats[1].id;
    let mut post = msg(1, false, "Мой канал", "новость");
    post.views = Some(12_345);
    let mut mine = msg(2, true, "Я", "мой пост");
    mine.views = Some(87);
    app.histories.insert(channel, vec![post, mine, msg(3, false, "Кто-то", "без просмотров")]);
    app.set_sel(Some(channel));
    let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
    term.draw(|f| draw(f, &mut app)).unwrap();
    let text = dump(&term);
    assert!(text.contains("12K просм."), "{text}");
    assert!(text.contains("87 просм."));
    assert_eq!(text.matches("просм.").count(), 2, "у сообщений без счётчика ничего не пишем");
    crate::i18n::set(1);
    term.draw(|f| draw(f, &mut app)).unwrap();
    assert!(dump(&term).contains("12K views"));
    crate::i18n::set(0);
}

#[test]
fn draft_is_uploaded_when_leaving_the_chat_and_only_once() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    type_text(&mut app, "не дописал", &tx, &s);
    let mama = app.chats[0].id;
    app.move_selection(1);
    app.flush_drafts(&tx);
    match rx.try_recv() {
        Ok(Cmd::SaveDraft { chat, text }) => {
            assert_eq!(chat.id, mama);
            assert_eq!(text, "не дописал");
        }
        _ => panic!("ждали SaveDraft"),
    }
    // вернулись и снова ушли, ничего не меняя: повторно не отправляем
    app.move_selection(-1);
    app.move_selection(1);
    app.flush_drafts(&tx);
    assert!(rx.try_recv().is_err());
    // стёрли текст: на сервере черновик тоже стирается
    app.move_selection(-1);
    app.input.clear();
    app.cursor = 0;
    app.move_selection(1);
    app.flush_drafts(&tx);
    assert!(matches!(rx.try_recv(), Ok(Cmd::SaveDraft { text, .. }) if text.is_empty()));
}

#[test]
fn draft_is_uploaded_after_the_input_rests() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    type_text(&mut app, "набираю", &tx, &s);
    app.flush_drafts(&tx);
    assert!(rx.try_recv().is_err(), "сразу не отправляем");
    app.draft_watch.2 = Instant::now() - Duration::from_secs(10);
    app.flush_drafts(&tx);
    assert!(matches!(rx.try_recv(), Ok(Cmd::SaveDraft { text, .. }) if text == "набираю"));
    app.draft_watch.2 = Instant::now() - Duration::from_secs(10);
    app.flush_drafts(&tx);
    assert!(rx.try_recv().is_err(), "тот же текст второй раз не уходит");
}

#[test]
fn commands_edits_locks_and_the_setting_keep_drafts_local() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    // команда не уходит в Telegram
    type_text(&mut app, "/findmsg abc", &tx, &s);
    app.move_selection(1);
    app.flush_drafts(&tx);
    assert!(rx.try_recv().is_err());
    // правка своего сообщения не превращается в черновик и не стирает чужой
    app.move_selection(-1);
    app.input.clear();
    app.cloud_drafts.insert(app.chats[0].id, "старый".into());
    app.edit_target = Some(1);
    app.input = "правка".into();
    app.move_selection(1);
    app.flush_drafts(&tx);
    assert!(rx.try_recv().is_err());
    assert_eq!(app.cloud_drafts.get(&app.chats[0].id).map(String::as_str), Some("старый"));
    // под блокировкой и с выключенной настройкой не отправляем
    app.edit_target = None;
    app.settings.cloud_drafts = false;
    app.input = "тайна".into();
    app.cursor = 5;
    app.move_selection(-1);
    app.flush_drafts(&tx);
    assert!(rx.try_recv().is_err());
    app.settings.cloud_drafts = true;
    app.mode = Mode::Locked;
    app.queue_draft(app.chats[0].id, "тайна");
    app.flush_drafts(&tx);
    assert!(rx.try_recv().is_err());
}

#[test]
fn sending_forgets_the_uploaded_draft() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    let id = app.chats[0].id;
    type_text(&mut app, "готово", &tx, &s);
    app.flush_drafts(&tx);
    app.draft_watch.2 = Instant::now() - Duration::from_secs(10);
    app.flush_drafts(&tx);
    assert!(matches!(rx.try_recv(), Ok(Cmd::SaveDraft { .. })));
    press(&mut app, KeyCode::Enter, &tx, &s);
    assert!(matches!(rx.try_recv(), Ok(Cmd::Send { .. })));
    assert!(!app.cloud_drafts.contains_key(&id));
    // тот же текст снова набран и оставлен: уходит как новый черновик
    type_text(&mut app, "готово", &tx, &s);
    app.flush_drafts(&tx);
    app.draft_watch.2 = Instant::now() - Duration::from_secs(10);
    app.flush_drafts(&tx);
    assert!(matches!(rx.try_recv(), Ok(Cmd::SaveDraft { text, .. }) if text == "готово"));
}

#[test]
fn drafts_from_the_phone_fill_the_input_and_the_other_chats() {
    let mut app = App::new(Settings::default());
    let (tx, _rx) = mpsc::unbounded_channel();
    let list = vec![
        with_draft(chat(1, "Мама", Kind::User, 0), "с телефона"),
        with_draft(chat(2, "Папа", Kind::User, 0), "про дачу"),
        chat(3, "Без черновика", Kind::User, 0),
    ];
    app.handle_event(Event::Dialogs(list), &tx);
    assert_eq!(app.input, "с телефона", "в открытом чате черновик сразу в поле");
    assert_eq!(app.cursor, "с телефона".chars().count());
    let papa = app.chats[1].id;
    assert_eq!(app.drafts.get(&papa).map(|d| d.0.as_str()), Some("про дачу"));
    app.move_selection(1);
    assert_eq!(app.input, "про дачу");
    // уже набранное здесь не затирается
    app.input = "моё".into();
    app.cursor = 3;
    let mama = app.chats[0].id;
    app.handle_event(Event::Draft { chat: papa, text: "новое с телефона".into() }, &tx);
    assert_eq!(app.input, "моё");
    assert_eq!(app.drafts.get(&mama).map(|d| d.0.as_str()), Some("с телефона"));
    // пустой черновик из списка чатов ничего не стирает, а живое обновление стирает
    app.handle_event(Event::DialogsPage(vec![chat(1, "Мама", Kind::User, 0)]), &tx);
    assert_eq!(app.drafts.get(&mama).map(|d| d.0.as_str()), Some("с телефона"));
    app.handle_event(Event::Draft { chat: mama, text: String::new() }, &tx);
    assert!(!app.drafts.contains_key(&mama));
}

#[test]
fn a_draft_cleared_on_the_phone_clears_an_untouched_input() {
    let mut app = App::new(Settings::default());
    let (tx, _rx) = mpsc::unbounded_channel();
    app.handle_event(Event::Dialogs(vec![with_draft(chat(1, "Мама", Kind::User, 0), "черновик")]), &tx);
    assert_eq!(app.input, "черновик");
    app.handle_event(Event::Draft { chat: app.chats[0].id, text: String::new() }, &tx);
    assert_eq!(app.input, "", "поле не менялось здесь, значит повторяет состояние телефона");
    // эхо собственной загрузки ничего не меняет
    app.input = "новое".into();
    app.cursor = 5;
    app.queue_draft(app.chats[0].id, "новое");
    app.handle_event(Event::Draft { chat: app.chats[0].id, text: "новое".into() }, &tx);
    assert_eq!(app.input, "новое");
}

#[test]
fn drafts_from_the_phone_are_ignored_when_off_or_locked() {
    let mut app = App::new(Settings::default());
    let (tx, _rx) = mpsc::unbounded_channel();
    app.settings.cloud_drafts = false;
    app.handle_event(Event::Dialogs(vec![with_draft(chat(1, "Мама", Kind::User, 0), "x")]), &tx);
    assert_eq!(app.input, "");
    app.settings.cloud_drafts = true;
    app.mode = Mode::Locked;
    app.handle_event(Event::Draft { chat: app.chats[0].id, text: "секрет".into() }, &tx);
    assert!(app.cloud_drafts.is_empty() && app.input.is_empty());
}

#[test]
fn cloud_drafts_setting_toggles() {
    let mut app = App::new(Settings::default());
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    assert!(app.settings.cloud_drafts);
    app.mode = Mode::Settings;
    app.settings_row = 12;
    app.handle_key(key(KeyCode::Right, KeyModifiers::NONE), &tx, &s);
    assert!(!app.settings.cloud_drafts && !s.settings().cloud_drafts);
}

// ───── пачка 1.3.0: пауза, «Вчера», темы, реакции, кнопки ботов, переход к оригиналу ─────

fn text_of(lines: &[Line]) -> String {
    lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>() + "\n").collect()
}

#[test]
fn password_pause_grows_and_blocks_attempts() {
    assert_eq!([0, 1, 2].map(pause_secs), [0, 0, 0]);
    assert_eq!([3, 4, 5, 6].map(pause_secs), [5, 10, 20, 40]);
    assert_eq!(pause_secs(50), 300, "не больше пяти минут");
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    app.lock();
    for _ in 0..3 {
        type_text(&mut app, "нет", &tx, &s);
        press(&mut app, KeyCode::Enter, &tx, &s);
    }
    assert!(app.pause_left().is_some(), "после третьей ошибки включается пауза");
    type_text(&mut app, "secret", &tx, &s);
    press(&mut app, KeyCode::Enter, &tx, &s);
    assert_eq!(app.mode, Mode::Locked, "верный пароль во время паузы не принимается");
    assert_eq!(app.wrong_attempts, 3, "попытка во время паузы не считается");
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| draw(f, &mut app)).unwrap();
    assert!(dump(&term).contains("Подождите"), "{}", dump(&term));
    app.lock_until = Some(Instant::now() - Duration::from_secs(1));
    type_text(&mut app, "secret", &tx, &s);
    press(&mut app, KeyCode::Enter, &tx, &s);
    assert_eq!(app.mode, Mode::Chat, "после паузы пароль принимается");
    assert!(app.lock_until.is_none());
}

#[test]
fn day_separator_says_yesterday() {
    assert_eq!(day_label((2025, 10, 27), Some((2025, 10, 28))), "Вчера");
    assert_eq!(day_label((2025, 9, 30), Some((2025, 10, 1))), "Вчера", "через границу месяца");
    assert_eq!(day_label((2024, 12, 31), Some((2025, 1, 1))), "Вчера", "через границу года");
    assert_eq!(day_label((2025, 10, 26), Some((2025, 10, 28))), "26 октября");
    assert_eq!(day_label((2025, 10, 28), Some((2025, 10, 28))), "Сегодня");
}

#[test]
fn themes_apply_and_every_palette_renders_every_screen() {
    use std::sync::Mutex;
    static GUARD: Mutex<()> = Mutex::new(());
    let _g = GUARD.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(Settings::default().palette, 0);
    let s: Settings = serde_json::from_str(r#"{"lock_minutes":15}"#).unwrap();
    assert_eq!(s.palette, 0, "старое хранилище без темы остаётся тёмным");
    let mut app = demo_app();
    let (tx, _rx) = mpsc::unbounded_channel();
    let sv = saver();
    app.mode = Mode::Settings;
    app.settings_row = 13;
    press(&mut app, KeyCode::Right, &tx, &sv);
    assert_eq!(app.settings.palette, 1);
    assert_eq!(app.settings.palette_name(), "Светлая");
    for p in 0..PALETTE_COUNT {
        set_palette(p);
        for mode in [Mode::Chat, Mode::Pick, Mode::Overlay, Mode::Help, Mode::Settings, Mode::Forward] {
            app.mode = mode;
            app.settings_row = 2; // выбранная строка подсвечена и не должна попадать на пробную ячейку
            let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
            term.draw(|f| draw(f, &mut app)).unwrap();
            let buf = term.backend().buffer();
            if let Some(bg) = palette().bg {
                assert_eq!(buf[(50, 15)].bg, bg, "тема {p}, режим {mode:?}: и окна поверх тоже");
            }
        }
        let c = name_color("Мама");
        assert!(palette().names.contains(&c));
        assert_eq!(c, name_color("Мама"), "цвет имени стабилен");
    }
    set_palette(0);
    assert!(palette().bg.is_none(), "тёмная тема не трогает цвета консоли");
    // Значение из будущего или повреждённого хранилища не приводит к панике.
    set_palette(200);
    assert_eq!(PALETTE.load(std::sync::atomic::Ordering::Relaxed), PALETTE_COUNT - 1);
    set_palette(0);
}

#[test]
fn reaction_labels_work_without_emoji_glyphs() {
    use crate::util::{reaction_key, reaction_label};
    assert_eq!(reaction_label("👍"), "+1");
    assert_eq!(reaction_label("❤"), "<3");
    assert_eq!(reaction_label("❤\u{fe0f}"), "<3", "с селектором вариации то же самое");
    assert_eq!(reaction_key("❤\u{fe0f}"), reaction_key("❤"));
    assert_eq!(reaction_label("\u{1f9ff}"), "?", "незнакомый смайлик не ломает строку");
    assert_eq!(reaction_label(""), "?", "своё эмодзи приходит без символа");
}

#[test]
fn reactions_and_buttons_are_drawn_under_the_message() {
    use crate::tg::{Btn, BtnAction, Reaction};
    let mut m = msg(1, false, "Бот", "Выберите");
    m.reactions = vec![Reaction { emoji: "👍".into(), count: 3, mine: false }, Reaction { emoji: "❤".into(), count: 2, mine: true }];
    m.buttons = vec![
        vec![
            Btn { text: "Да".into(), action: BtnAction::Callback(vec![1]) },
            Btn { text: "Нет".into(), action: BtnAction::Callback(vec![2]) },
        ],
        vec![Btn { text: "Сайт".into(), action: BtnAction::Url("https://example.org".into()) }],
    ];
    let (lines, _) = build_message_lines(&[m.clone()], 60, 72, None, Color::Blue);
    let text = text_of(&lines);
    assert!(text.contains("+1 3") && text.contains("[<3 2]"), "{text}");
    assert!(text.contains("[Да] [Нет]") && text.contains("[Сайт ↗]"), "{text}");
    m.outgoing = true;
    let (lines, _) = build_message_lines(&[m], 60, 72, None, Color::Blue);
    assert!(text_of(&lines).contains("[Да] [Нет]"), "и у своих сообщений");
    let plain = msg(2, false, "Аня", "обычное");
    let (lines, _) = build_message_lines(&[plain], 60, 72, None, Color::Blue);
    assert_eq!(lines.len(), 4, "разделитель дня, заголовок, текст и пустая строка: {}", text_of(&lines));
}

#[test]
fn pick_mode_digits_set_toggle_and_remove_reactions() {
    use crate::tg::Reaction;
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    press(&mut app, KeyCode::F(7), &tx, &s);
    press(&mut app, KeyCode::Char('2'), &tx, &s);
    match drain(&mut rx).pop() {
        Some(Cmd::React { id, emoji, .. }) => assert_eq!((id, emoji.as_deref()), (2, Some("❤"))),
        _ => panic!("ожидалась реакция"),
    }
    assert_eq!(app.mode, Mode::Pick, "режим выбора остаётся");
    // Ваша реакция уже стоит: та же цифра снимает её.
    let chat_id = app.selected().unwrap().id;
    app.histories.get_mut(&chat_id).unwrap()[1].reactions = vec![Reaction { emoji: "❤\u{fe0f}".into(), count: 1, mine: true }];
    press(&mut app, KeyCode::Char('2'), &tx, &s);
    assert!(matches!(drain(&mut rx).pop(), Some(Cmd::React { emoji: None, .. })));
    press(&mut app, KeyCode::Char('0'), &tx, &s);
    assert!(matches!(drain(&mut rx).pop(), Some(Cmd::React { emoji: None, .. })));
    // Ответ сервера обновляет реакции в переписке.
    let mut fresh = msg(2, true, "Я", "Всё хорошо!");
    fresh.reactions = vec![Reaction { emoji: "🔥".into(), count: 4, mine: false }];
    app.handle_event(Event::Reacted { chat: chat_id, msg: fresh }, &tx);
    assert_eq!(app.histories[&chat_id][1].reactions[0].count, 4);
}

#[test]
fn bot_buttons_open_list_and_press_by_digit() {
    use crate::tg::{Btn, BtnAction};
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    let chat_id = app.selected().unwrap().id;
    press(&mut app, KeyCode::F(7), &tx, &s);
    press(&mut app, KeyCode::Char('k'), &tx, &s);
    assert_eq!(app.mode, Mode::Pick, "у сообщения без кнопок список не открывается");
    assert!(!app.pick_note.is_empty());
    app.histories.get_mut(&chat_id).unwrap()[1].buttons = vec![
        vec![Btn { text: "Да".into(), action: BtnAction::Callback(vec![7, 8]) }],
        vec![Btn { text: "Игра".into(), action: BtnAction::Other }],
    ];
    press(&mut app, KeyCode::Char('л'), &tx, &s); // русская раскладка
    assert_eq!(app.mode, Mode::Overlay);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| draw(f, &mut app)).unwrap();
    let text = dump(&term);
    assert!(text.contains("1. Да") && text.contains("2. Игра") && text.contains("не поддерживается"), "{text}");
    press(&mut app, KeyCode::Char('2'), &tx, &s);
    assert!(drain(&mut rx).is_empty(), "кнопка неподдерживаемого вида ничего не отправляет");
    assert!(app.status.contains("не поддерживается"), "{}", app.status);
    press(&mut app, KeyCode::Char('1'), &tx, &s);
    match drain(&mut rx).pop() {
        Some(Cmd::Press { id, data, .. }) => assert_eq!((id, data), (2, vec![7, 8])),
        _ => panic!("ожидалось нажатие кнопки"),
    }
    assert_eq!(app.mode, Mode::Chat);
    app.handle_event(Event::ButtonAnswer("Готово".into()), &tx);
    assert!(app.status.contains("Готово"));
    app.handle_event(Event::ButtonAnswer(String::new()), &tx);
    assert!(app.status.contains("нажата"));
}

#[test]
fn edited_bot_message_updates_its_buttons() {
    use crate::tg::{Btn, BtnAction};
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let chat_id = app.selected().unwrap().id;
    let mut edited = msg(1, false, "Мама", "новый текст");
    edited.buttons = vec![vec![Btn { text: "Ещё".into(), action: BtnAction::Callback(vec![1]) }]];
    app.handle_event(Event::Edited { chat: chat_id, msg: edited }, &tx);
    assert_eq!(app.histories[&chat_id][0].buttons.len(), 1);
}

#[test]
fn jump_to_reply_original_and_back() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    let chat_id = app.selected().unwrap().id;
    app.histories.get_mut(&chat_id).unwrap()[1].reply_to = Some(1);
    press(&mut app, KeyCode::F(7), &tx, &s);
    assert_eq!(app.pick, 1);
    press(&mut app, KeyCode::Char('g'), &tx, &s);
    assert_eq!(app.pick, 0, "выбран оригинал, он уже загружен");
    assert!(drain(&mut rx).is_empty());
    press(&mut app, KeyCode::Char('g'), &tx, &s);
    assert!(app.pick_note.contains("не ответ"), "у оригинала нет ответа: {}", app.pick_note);
    press(&mut app, KeyCode::Char('и'), &tx, &s); // B в русской раскладке
    assert_eq!(app.pick, 1, "вернулись к ответу");
    press(&mut app, KeyCode::Char('b'), &tx, &s);
    assert!(app.pick_note.contains("некуда"));
    // Оригинал не загружен: просим окружение и после загрузки остаёмся в режиме выбора.
    app.histories.get_mut(&chat_id).unwrap()[1].reply_to = Some(999);
    press(&mut app, KeyCode::Char('g'), &tx, &s);
    assert!(matches!(drain(&mut rx).pop(), Some(Cmd::LoadAround { id: 999, .. })));
    let around = vec![msg(998, false, "Мама", "раньше"), msg(999, false, "Мама", "оригинал"), msg(1, true, "Я", "ответ")];
    app.handle_event(Event::Around { chat: chat_id, msgs: around, focus: 999 }, &tx);
    assert_eq!(app.mode, Mode::Pick);
    assert_eq!(app.histories[&chat_id][app.pick].id, 999);
}

#[test]
fn jump_history_is_dropped_when_the_chat_changes() {
    let mut app = app_with_chats();
    app.jump_back.push((app.selected().unwrap().id, 1, 0));
    let other = app.chats[1].id;
    app.set_sel(Some(other));
    assert!(app.jump_back.is_empty());
}

#[test]
fn new_pick_keys_are_in_help_and_hint() {
    let mut app = demo_app();
    app.mode = Mode::Help;
    let text = draw_text(&mut app, 110, 120);
    assert!(text.contains("F7 → 1-6 G K"), "{text}");
    app.mode = Mode::Pick;
    let mut term = Terminal::new(TestBackend::new(110, 40)).unwrap();
    let chat_id = app.selected().unwrap().id;
    let last = app.histories[&chat_id].len() - 1;
    app.histories.get_mut(&chat_id).unwrap()[last].reply_to = Some(1);
    app.pick = last;
    term.draw(|f| draw(f, &mut app)).unwrap();
    assert!(dump(&term).contains("G к оригиналу"), "подсказка показывает G только для ответа");
}

#[test]
fn device_name_shows_the_app_and_its_version() {
    let n = crate::tg::device_name();
    assert!(n.starts_with(&format!("Tg-Lite v{VERSION} (")), "{n}");
    assert!(n.len() < 64, "короткое название для списка сеансов");
}

// ───── 1.4.0: индикатор связи и очередь исходящих ─────

#[test]
fn network_errors_are_told_apart_from_telegram_refusals() {
    use crate::tg::is_network_error;
    assert!(is_network_error("request error: connection reset by peer"));
    assert!(is_network_error("request error: dropped: sender closed"));
    assert!(is_network_error("request error: timed out"));
    assert!(!is_network_error("request error: rpc error 400: CHAT_WRITE_FORBIDDEN"));
    assert!(!is_network_error("request error: rpc error 420: FLOOD_WAIT (value: 30)"), "ожидание FloodWait не очередь");
    assert!(!is_network_error("сообщение слишком длинное"));
}

#[test]
fn status_bar_shows_connection_and_queue() {
    let mut app = demo_app();
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
    term.draw(|f| draw(f, &mut app)).unwrap();
    assert!(!dump(&term).contains('●') || app.net.is_none(), "пока не измеряли, индикатора нет");
    app.handle_event(Event::Latency(Some(84)), &tx);
    term.draw(|f| draw(f, &mut app)).unwrap();
    assert!(dump(&term).contains("● 84 мс"), "{}", dump(&term));
    app.handle_event(Event::Latency(None), &tx);
    term.draw(|f| draw(f, &mut app)).unwrap();
    assert!(dump(&term).contains("нет связи"), "{}", dump(&term));
    let chat_id = app.selected().unwrap().id;
    app.handle_event(Event::Queued { chat: chat_id, text: "привет из метро".into(), count: 1 }, &tx);
    term.draw(|f| draw(f, &mut app)).unwrap();
    let text = dump(&term);
    assert!(text.contains("привет из метро") && text.contains("в очереди"), "{text}");
    assert!(text.contains("1 ○ нет связи"), "счётчик очереди рядом с индикатором: {text}");
}

#[test]
fn queue_events_update_the_screen_and_return_rejected_text() {
    let mut app = demo_app();
    let (tx, _rx) = mpsc::unbounded_channel();
    let chat_id = app.selected().unwrap().id;
    for t in ["раз", "два"] {
        app.handle_event(Event::Queued { chat: chat_id, text: t.into(), count: app.queued.len() + 1 }, &tx);
    }
    assert_eq!(app.queued.len(), 2);
    app.handle_event(Event::QueueSent { chat: chat_id, text: "раз".into(), left: 1 }, &tx);
    assert_eq!(app.queued, vec![(chat_id, "два".to_string())]);
    app.input.clear();
    app.handle_event(Event::QueueDropped { chat: chat_id, text: "два".into(), reason: "в этот чат писать нельзя".into() }, &tx);
    assert!(app.queued.is_empty());
    assert_eq!(app.input, "два", "отклонённый текст возвращается в поле ввода");
    assert!(app.status.contains("писать нельзя"));
}

#[tokio::test]
async fn queue_flush_keeps_order_stops_without_network_and_drops_refused() {
    use crate::tg::{Queued, flush_with};
    use std::collections::VecDeque;
    let chat = chat(1, "Аня", Kind::User, 0);
    let q = |t: &str| Queued { chat: chat.clone(), text: t.into(), markdown: false, reply_to: None };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut queue: VecDeque<Queued> = [q("a"), q("b"), q("c")].into();
    let calls = std::sync::Mutex::new(Vec::<String>::new());
    // Связи нет: первая же попытка останавливает отправку, очередь цела.
    flush_with(&mut queue, &tx, |q| {
        calls.lock().unwrap().push(q.text.clone());
        async { Err(anyhow::anyhow!("request error: connection reset")) }
    })
    .await;
    assert_eq!(queue.len(), 3);
    assert_eq!(*calls.lock().unwrap(), ["a"], "после сетевой ошибки остальные не пробуются");
    assert!(rx.try_recv().is_err());
    // Связь вернулась, но Telegram отклоняет второе сообщение: оно удаляется, третье уходит.
    flush_with(&mut queue, &tx, |q| {
        let text = q.text.clone();
        async move {
            if text == "b" {
                Err(anyhow::anyhow!("request error: rpc error 403: CHAT_WRITE_FORBIDDEN"))
            } else {
                Ok(msg(1, true, "Я", &text))
            }
        }
    })
    .await;
    assert!(queue.is_empty());
    let mut kinds = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        kinds.push(match ev {
            Event::Sent { msg, .. } => format!("sent {}", msg.text),
            Event::QueueSent { text, left, .. } => format!("qsent {text} {left}"),
            Event::QueueDropped { text, .. } => format!("dropped {text}"),
            _ => "other".into(),
        });
    }
    assert_eq!(kinds, ["sent a", "qsent a 2", "dropped b", "sent c", "qsent c 0"]);
}

fn picking_app(n: i32) -> (App, mpsc::UnboundedSender<Cmd>, mpsc::UnboundedReceiver<Cmd>) {
    let mut app = app_with_chats();
    let (tx, rx) = mpsc::unbounded_channel();
    let id = app.sel.unwrap();
    app.histories.insert(id, long_history(n));
    (app, tx, rx)
}

#[test]
fn space_and_shift_arrows_mark_messages_and_esc_clears_them() {
    let (mut app, tx, _rx) = picking_app(10);
    let s = saver();
    press(&mut app, KeyCode::F(7), &tx, &s);
    assert_eq!(app.pick, 9);
    app.handle_key(key(KeyCode::Up, KeyModifiers::SHIFT), &tx, &s);
    app.handle_key(key(KeyCode::Up, KeyModifiers::SHIFT), &tx, &s);
    assert_eq!(app.marked.iter().copied().collect::<Vec<_>>(), vec![8, 9, 10]);
    press(&mut app, KeyCode::Char(' '), &tx, &s); // снимает отметку с сообщения под курсором (8) и идёт дальше
    assert_eq!(app.marked.iter().copied().collect::<Vec<_>>(), vec![9, 10]);
    drawn(&mut app);
    press(&mut app, KeyCode::Esc, &tx, &s);
    assert!(app.marked.is_empty());
    assert_eq!(app.mode, Mode::Pick, "первый Esc только сбрасывает отметки");
    press(&mut app, KeyCode::Esc, &tx, &s);
    assert_eq!(app.mode, Mode::Chat);
}

#[test]
fn marked_messages_forward_delete_and_copy_in_a_batch() {
    let (mut app, tx, mut rx) = picking_app(10);
    let s = saver();
    press(&mut app, KeyCode::F(7), &tx, &s);
    app.marked = [3, 5, 4].into_iter().collect();
    // удаление: чужие сообщения среди отмеченных блокируют
    press(&mut app, KeyCode::Char('d'), &tx, &s);
    assert!(drain(&mut rx).is_empty());
    assert!(app.pick_note.contains("чужие"));
    app.marked = [2, 6, 4].into_iter().collect(); // все чётные: свои
    press(&mut app, KeyCode::Char('d'), &tx, &s);
    assert!(drain(&mut rx).is_empty(), "первое D только спрашивает");
    press(&mut app, KeyCode::Char('d'), &tx, &s);
    match drain(&mut rx).pop() {
        Some(Cmd::DeleteMany { ids, .. }) => assert_eq!(ids, vec![2, 4, 6]),
        _ => panic!("ожидалось удаление нескольких"),
    }
    assert!(app.marked.is_empty());
    // пересылка по порядку истории
    press(&mut app, KeyCode::F(7), &tx, &s);
    app.marked = [7, 3].into_iter().collect();
    press(&mut app, KeyCode::Char('f'), &tx, &s);
    press(&mut app, KeyCode::Down, &tx, &s);
    press(&mut app, KeyCode::Enter, &tx, &s);
    match drain(&mut rx).into_iter().find(|c| matches!(c, Cmd::Forward { .. })) {
        Some(Cmd::Forward { ids, .. }) => assert_eq!(ids, vec![3, 7]),
        _ => panic!("ожидалась пересылка"),
    }
}

#[test]
fn marked_messages_are_shown_with_a_check_mark_and_status_hint() {
    let (mut app, tx, _rx) = picking_app(6);
    let s = saver();
    press(&mut app, KeyCode::F(7), &tx, &s);
    app.marked = [5, 6].into_iter().collect();
    let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
    term.draw(|f| draw(f, &mut app)).unwrap();
    let text = dump(&term);
    assert!(text.contains("✓"));
    assert!(text.contains("Выбрано 2"));
}

#[test]
fn mouse_drag_marks_a_range_and_edge_autoscrolls() {
    let (mut app, tx, mut rx) = picking_app(60);
    drawn(&mut app);
    let g = app.geom.as_ref().unwrap();
    let body = g.body;
    // потянули от нижнего сообщения немного вверх
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), body.x + 5, body.y + body.height - 2), &tx);
    assert_eq!(app.mode, Mode::Chat, "простой щелчок режим не меняет");
    app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), body.x + 5, body.y + body.height - 8), &tx);
    assert_eq!(app.mode, Mode::Pick);
    assert!(app.marked.len() >= 2, "{:?}", app.marked);
    let anchor = app.drag.as_ref().unwrap().anchor;
    assert!(app.marked.contains(&anchor));
    // удержание у верхнего края листает вверх, отметка растёт
    app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), body.x + 5, body.y), &tx);
    let before = app.marked.len();
    for _ in 0..15 {
        assert!(app.drag_tick(&tx));
        drawn(&mut app);
    }
    assert!(app.marked.len() > before + 15, "{} -> {}", before, app.marked.len());
    assert!(app.scroll > 0, "лента ушла вверх");
    // у самого начала подгружается старая история
    app.pick = 1;
    app.drag_tick(&tx);
    assert!(drain(&mut rx).iter().any(|c| matches!(c, Cmd::LoadOlder { .. })));
    // вниз: двигаемся к последнему сообщению и останавливаемся
    app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), body.x + 5, body.y + body.height - 1), &tx);
    for _ in 0..80 {
        app.drag_tick(&tx);
        drawn(&mut app);
    }
    assert_eq!(app.pick, 59);
    assert!(app.marked.contains(&60));
    assert!(!app.drag_tick(&tx), "дальше листать некуда");
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), body.x + 5, body.y + 2), &tx);
    assert!(app.drag.is_none());
    assert!(!app.marked.is_empty(), "после отпускания выделение остаётся");
}

#[test]
fn ctrl_click_toggles_a_mark_and_plain_click_in_pick_moves_cursor() {
    let (mut app, tx, _rx) = picking_app(10);
    drawn(&mut app);
    let body = app.geom.as_ref().unwrap().body;
    let y = body.y + body.height - 2;
    let ev = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: body.x + 5, row: y, modifiers: KeyModifiers::CONTROL };
    app.handle_mouse(ev, &tx);
    assert_eq!(app.mode, Mode::Pick);
    assert_eq!(app.marked.len(), 1);
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), body.x + 5, y), &tx);
    app.handle_mouse(ev, &tx);
    assert!(app.marked.is_empty(), "повторный Ctrl+щелчок снимает отметку");
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), body.x + 5, y), &tx);
    // колесо в режиме выбора двигает курсор
    let before = app.pick;
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, body.x + 5, body.y + 3), &tx);
    assert_eq!(app.pick, before.saturating_sub(3));
}

fn poll_msg() -> ChatMsg {
    use crate::tg::{PollOpt, PollView};
    let mut m = msg(7, false, "Опросник", "[опрос: Чай или кофе?]");
    m.poll = Some(PollView {
        question: "Чай или кофе?".into(),
        total: 4,
        options: vec![
            PollOpt { text: "Чай".into(), data: vec![1], voters: 1, chosen: false, correct: false },
            PollOpt { text: "Кофе".into(), data: vec![2], voters: 3, chosen: true, correct: false },
        ],
        ..Default::default()
    });
    m
}

#[test]
fn poll_is_drawn_with_percentages_and_can_be_voted() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    let id = app.sel.unwrap();
    app.histories.get_mut(&id).unwrap().push(poll_msg());
    let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
    term.draw(|f| draw(f, &mut app)).unwrap();
    let text = dump(&term);
    assert!(text.contains("опрос: Чай или кофе?"), "{text}");
    assert!(text.contains("25%") && text.contains("75%"), "{text}");
    assert!(text.contains("● Кофе"), "свой вариант отмечен");
    // голосование через V
    press(&mut app, KeyCode::F(7), &tx, &s);
    press(&mut app, KeyCode::Char('v'), &tx, &s);
    assert_eq!(app.mode, Mode::Overlay);
    press(&mut app, KeyCode::Char('1'), &tx, &s);
    match drain(&mut rx).pop() {
        Some(Cmd::Vote { id, option, .. }) => assert_eq!((id, option), (7, vec![1])),
        _ => panic!("ожидалась команда Vote"),
    }
    // ответ сервера обновляет результаты
    let mut upd = poll_msg();
    upd.poll.as_mut().unwrap().total = 5;
    app.handle_event(Event::Reacted { chat: id, msg: upd }, &tx);
    assert_eq!(app.histories[&id].last().unwrap().poll.as_ref().unwrap().total, 5);
    assert_eq!(app.status, "Голос учтён");
}

#[test]
fn voting_needs_an_open_poll() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    press(&mut app, KeyCode::F(7), &tx, &s);
    press(&mut app, KeyCode::Char('v'), &tx, &s);
    assert!(app.pick_note.contains("не опрос"));
    assert!(drain(&mut rx).is_empty());
    let id = app.sel.unwrap();
    let mut closed = poll_msg();
    closed.poll.as_mut().unwrap().closed = true;
    app.histories.get_mut(&id).unwrap().push(closed);
    press(&mut app, KeyCode::End, &tx, &s);
    press(&mut app, KeyCode::Char('v'), &tx, &s);
    assert!(app.pick_note.contains("закрыт"));
}

#[test]
fn topics_list_picks_a_topic_and_sends_into_it() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    let chat = app.sel.unwrap();
    submit_text(&mut app, "/topics", &tx, &s);
    assert!(matches!(drain(&mut rx).pop(), Some(Cmd::ListTopics(_))));
    app.handle_event(Event::Topics { chat, items: vec![(10, "Общее".into(), 0), (11, "Релизы".into(), 3)] }, &tx);
    assert_eq!(app.mode, Mode::Overlay);
    press(&mut app, KeyCode::Down, &tx, &s);
    press(&mut app, KeyCode::Enter, &tx, &s);
    assert_eq!(app.mode, Mode::Chat);
    assert_eq!(app.topic.as_ref().map(|t| t.1), Some(11));
    submit_text(&mut app, "привет", &tx, &s);
    match drain(&mut rx).pop() {
        Some(Cmd::Send { reply_to, .. }) => assert_eq!(reply_to, Some(11)),
        _ => panic!("ожидалась отправка"),
    }
    submit_text(&mut app, "/topics off", &tx, &s);
    assert!(app.topic.is_none());
    submit_text(&mut app, "ещё", &tx, &s);
    match drain(&mut rx).pop() {
        Some(Cmd::Send { reply_to, .. }) => assert_eq!(reply_to, None),
        _ => panic!("ожидалась отправка"),
    }
}

#[test]
fn photo_preview_is_requested_drawn_and_closed() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    let id = app.sel.unwrap();
    press(&mut app, KeyCode::F(7), &tx, &s);
    press(&mut app, KeyCode::Char('p'), &tx, &s);
    assert!(app.pick_note.contains("нет фото"));
    assert!(drain(&mut rx).is_empty());
    app.histories.get_mut(&id).unwrap().push(msg_with_attachment(9, "[фото]", ""));
    press(&mut app, KeyCode::End, &tx, &s);
    press(&mut app, KeyCode::Char('p'), &tx, &s);
    assert!(matches!(drain(&mut rx).pop(), Some(Cmd::Preview { id: 9, .. })));
    // испорченные данные не ломают экран
    app.handle_event(Event::Preview { chat: id, jpeg: b"junk".to_vec() }, &tx);
    assert_eq!(app.mode, Mode::Pick);
    assert!(app.status.contains("превью"));
    // готовая картинка: градиент 40×20 рисуется полублоками и закрывается любой клавишей
    let (w, h) = (40usize, 20usize);
    let rgb: Vec<u8> = (0..w * h).flat_map(|i| [(i % w * 6) as u8, (i / w * 12) as u8, 128]).collect();
    app.preview = Some(Preview { w, h, rgb });
    app.mode = Mode::Preview;
    let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
    term.draw(|f| draw(f, &mut app)).unwrap();
    let text = dump(&term);
    assert!(text.contains("▀"), "{text}");
    assert!(text.contains("Фото"));
    press(&mut app, KeyCode::Char('x'), &tx, &s);
    assert_eq!(app.mode, Mode::Pick);
    assert!(app.preview.is_none());
}

#[test]
fn locking_drops_the_photo_and_marks() {
    let mut app = app_with_chats();
    app.preview = Some(Preview { w: 1, h: 2, rgb: vec![0; 6] });
    app.marked.insert(1);
    app.lock();
    assert!(app.preview.is_none() && app.marked.is_empty());
}

#[test]
fn settings_are_grouped_into_sections_and_navigation_follows_the_screen_order() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    app.mode = Mode::Settings;
    app.settings_row = 0;
    let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
    term.draw(|f| draw(f, &mut app)).unwrap();
    let out = dump(&term);
    for title in ["Защита", "Подключение", "Сообщения и звук", "Внешний вид", "Общие"] {
        assert!(out.contains(title), "нет раздела {title}");
    }
    // каждая строка входит ровно в один раздел
    let mut all: Vec<usize> = settings_sections().iter().flat_map(|(_, r)| r.iter().copied()).collect();
    all.sort_unstable();
    assert_eq!(all, (0..SETTINGS_ROWS).collect::<Vec<_>>());
    // ↓ идёт по порядку на экране и замыкается по кругу
    let mut seen = vec![app.settings_row];
    for _ in 1..SETTINGS_ROWS {
        press(&mut app, KeyCode::Down, &tx, &s);
        seen.push(app.settings_row);
    }
    let expected: Vec<usize> = settings_sections().iter().flat_map(|(_, r)| r.iter().copied()).collect();
    assert_eq!(seen, expected);
    press(&mut app, KeyCode::Down, &tx, &s);
    assert_eq!(app.settings_row, 0);
    press(&mut app, KeyCode::Up, &tx, &s);
    assert_eq!(app.settings_row, *expected.last().unwrap());
}

#[test]
fn ctrl_z_and_ctrl_y_step_through_input_changes() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let c = KeyModifiers::CONTROL;
    for ch in "привет".chars() {
        app.handle_key(key(KeyCode::Char(ch), KeyModifiers::NONE), &tx, &s);
    }
    assert_eq!(app.input, "привет");
    app.handle_key(key(KeyCode::Backspace, KeyModifiers::NONE), &tx, &s);
    app.handle_key(key(KeyCode::Backspace, KeyModifiers::NONE), &tx, &s);
    assert_eq!(app.input, "прив");
    app.handle_key(key(KeyCode::Char('z'), c), &tx, &s);
    assert_eq!(app.input, "приве", "каждое удаление отменяется отдельно");
    app.handle_key(key(KeyCode::Char('я'), c), &tx, &s); // русская раскладка
    assert_eq!(app.input, "привет");
    app.handle_key(key(KeyCode::Char('z'), c), &tx, &s);
    assert_eq!(app.input, "", "быстрый набор откатывается одним шагом");
    app.handle_key(key(KeyCode::Char('y'), c), &tx, &s);
    assert_eq!(app.input, "привет");
    for _ in 0..2 {
        app.handle_key(key(KeyCode::Char('н'), c), &tx, &s);
    }
    assert_eq!(app.input, "прив", "повтор доходит до последней правки");
    app.handle_key(key(KeyCode::Char('y'), c), &tx, &s);
    assert!(app.status.contains("Нечего повторять"));
    // новая правка обнуляет «вперёд»
    app.handle_key(key(KeyCode::Char('z'), c), &tx, &s);
    app.handle_key(key(KeyCode::Char('z'), c), &tx, &s);
    app.handle_key(key(KeyCode::Char('z'), c), &tx, &s);
    app.undo_stamp = Instant::now() - Duration::from_secs(5);
    app.handle_key(key(KeyCode::Char('x'), KeyModifiers::NONE), &tx, &s);
    app.handle_key(key(KeyCode::Char('y'), c), &tx, &s);
    assert_eq!(app.input, "x");
    // очищенное отправкой поле тоже можно вернуть
    app.input = "черновик".into();
    app.cursor = 8;
    app.clear_input();
    app.handle_key(key(KeyCode::Char('z'), c), &tx, &s);
    assert_eq!(app.input, "черновик");
}

#[test]
fn notification_only_when_away_and_not_too_often() {
    let mut app = app_with_chats();
    app.settings.notify = false;
    app.last_activity = Instant::now() - Duration::from_secs(60);
    app.maybe_notify("Мама");
    assert!(app.last_notify.is_none(), "выключено в настройках");
    app.settings.notify = true;
    app.last_activity = Instant::now();
    app.maybe_notify("Мама");
    assert!(app.last_notify.is_none(), "вы за клавиатурой");
    app.last_activity = Instant::now() - Duration::from_secs(60);
    app.maybe_notify("Мама");
    let first = app.last_notify.expect("ушло уведомление");
    app.maybe_notify("Мама");
    assert_eq!(app.last_notify, Some(first), "не чаще раза в 5 секунд");
}

#[test]
fn emoji_and_notification_settings_cycle() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    app.mode = Mode::Settings;
    // строку «Эмодзи» не нажимаем: она включает общий для всех тестов переключатель
    assert_eq!(cycle(&[0u8, 1, 2], 2, 1), 0);
    assert_eq!(app.settings.emoji_name(), "авто");
    app.settings_row = 16;
    press(&mut app, KeyCode::Enter, &tx, &s);
    assert!(app.settings.notify);
    assert!(s.settings().notify);
}

#[test]
fn netcheck_command_and_result_window() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    submit_text(&mut app, "/netcheck", &tx, &s);
    assert!(matches!(drain(&mut rx).pop(), Some(Cmd::NetCheck(_))));
    app.handle_event(Event::NetCheck(vec!["Имя telegram.org: найдено".into(), "Итог: путь открыт".into()]), &tx);
    assert_eq!(app.mode, Mode::Overlay);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| draw(f, &mut app)).unwrap();
    let out = dump(&term);
    assert!(out.contains("Проверка связи") && out.contains("Итог: путь открыт"), "{out}");
}

#[test]
fn album_command_parses_paths_checks_files_and_sends() {
    assert_eq!(parse_album("a.jpg b.png"), (vec!["a.jpg".to_string(), "b.png".to_string()], String::new()));
    assert_eq!(
        parse_album(r#""мой снимок.jpg" 'b c.png' d.jpg -- на даче"#),
        (vec!["мой снимок.jpg".to_string(), "b c.png".to_string(), "d.jpg".to_string()], "на даче".to_string())
    );
    assert_eq!(parse_album("").0.len(), 0);
    let dir = std::env::temp_dir().join(format!("tglite-album-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let files: Vec<String> = (0..3)
        .map(|i| {
            let p = dir.join(format!("f{i}.png"));
            std::fs::write(&p, b"x").unwrap();
            p.to_string_lossy().into_owned()
        })
        .collect();
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    submit_text(&mut app, &format!("/album {}", files[0]), &tx, &s);
    assert!(app.status.contains("Использование"), "один файл — не альбом");
    assert!(drain(&mut rx).is_empty());
    submit_text(&mut app, &format!("/album {} {} nope.png", files[0], files[1]), &tx, &s);
    assert!(app.status.contains("не найден"));
    assert!(drain(&mut rx).is_empty());
    app.clear_input();
    submit_text(&mut app, &format!("/album {} {} {} -- три фото", files[0], files[1], files[2]), &tx, &s);
    match drain(&mut rx).pop() {
        Some(Cmd::SendAlbum { paths, caption, compress, .. }) => {
            assert_eq!((paths.len(), caption.as_str()), (3, "три фото"));
            assert!(compress);
        }
        _ => panic!("ожидался альбом"),
    }
    app.settings.compress = false;
    submit_text(&mut app, &format!("/album {} {}", files[0], files[1]), &tx, &s);
    assert!(matches!(drain(&mut rx).pop(), Some(Cmd::SendAlbum { compress: false, .. })));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn text_file_viewer_requests_scrolls_and_closes() {
    assert!(crate::util::is_text_file("Notes.TXT") && crate::util::is_text_file("a.yaml"));
    assert!(!crate::util::is_text_file("a.exe") && !crate::util::is_text_file(".txt") && !crate::util::is_text_file("txt"));
    assert_eq!(crate::util::text_for_view("a\r\nb\n\nc".as_bytes()), vec!["a", "b", "", "c"]);
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    let id = app.sel.unwrap();
    press(&mut app, KeyCode::F(7), &tx, &s);
    press(&mut app, KeyCode::Char('t'), &tx, &s);
    assert!(app.pick_note.contains("нет файла"));
    assert!(drain(&mut rx).is_empty());
    app.histories.get_mut(&id).unwrap().push(msg_with_attachment(9, "[файл заметки.txt, 1 КБ]", ""));
    press(&mut app, KeyCode::End, &tx, &s);
    press(&mut app, KeyCode::Char('t'), &tx, &s);
    assert!(matches!(drain(&mut rx).pop(), Some(Cmd::ViewText { id: 9, .. })));
    let lines: Vec<String> = (0..100).map(|i| format!("строка {i}")).collect();
    app.handle_event(Event::TextFile { chat: id, name: "заметки.txt".into(), lines }, &tx);
    assert_eq!(app.mode, Mode::Viewer);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| draw(f, &mut app)).unwrap();
    let text = dump(&term);
    assert!(text.contains("заметки.txt") && text.contains("строка 0") && !text.contains("строка 90"), "{text}");
    press(&mut app, KeyCode::End, &tx, &s);
    term.draw(|f| draw(f, &mut app)).unwrap();
    assert!(dump(&term).contains("строка 99"));
    press(&mut app, KeyCode::Esc, &tx, &s);
    assert_eq!(app.mode, Mode::Pick);
    assert!(app.viewer.is_none());
    // блокировка стирает прочитанное
    app.viewer = Some(Viewer { name: "x".into(), lines: vec!["секрет".into()], top: 0 });
    app.lock();
    assert!(app.viewer.is_none());
}

#[test]
fn read_only_channel_refuses_sending_and_replying_but_keeps_the_typed_text() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    let channel = app.chats[1].id;
    app.chats[1].can_write = false;
    app.histories.insert(channel, vec![msg(1, false, "Мой канал", "Новость дня")]);
    app.sel = Some(channel);
    type_text(&mut app, "привет", &tx, &s);
    slow(&mut app);
    app.handle_key(key(KeyCode::Enter, KeyModifiers::NONE), &tx, &s);
    assert!(drain(&mut rx).is_empty(), "в канал без права писать ничего не уходит");
    assert!(app.status.contains("администраторы"), "{}", app.status);
    assert_eq!(app.input, "привет", "набранный текст остаётся");
    for command in ["/send /tmp/x.txt", "/album a b"] {
        app.clear_input();
        submit_text(&mut app, command, &tx, &s);
        assert!(drain(&mut rx).is_empty(), "{command}");
        assert!(app.status.contains("администраторы"), "{command}: {}", app.status);
    }
    app.clear_input();
    press(&mut app, KeyCode::F(7), &tx, &s);
    press(&mut app, KeyCode::Char('r'), &tx, &s);
    assert!(app.pick_note.contains("администраторы"), "{}", app.pick_note);
    assert!(app.reply.is_none());
    // а подсказка и вовсе не предлагает «ответить»
    let mut term = Terminal::new(TestBackend::new(110, 30)).unwrap();
    app.pick_note.clear();
    term.draw(|f| draw(f, &mut app)).unwrap();
    let text = dump(&term);
    assert!(!text.contains("R ответ") && text.contains("F переслать"), "{text}");
    // в обычном чате всё по-прежнему
    app.mode = Mode::Chat;
    app.sel = Some(app.chats[0].id);
    type_text(&mut app, "ок", &tx, &s);
    slow(&mut app);
    app.handle_key(key(KeyCode::Enter, KeyModifiers::NONE), &tx, &s);
    assert!(matches!(drain(&mut rx).pop(), Some(Cmd::Send { .. })));
}

#[test]
fn voice_hint_has_one_o_and_open_is_refused_politely_without_a_program() {
    crate::i18n::set(0);
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    let id = app.sel.unwrap();
    let mut voice = msg_with_attachment(9, "[голосовое 0:07]", "");
    voice.media = Some(tg::MediaInfo { kind: tg::MediaKind::Voice, name: "voice_9.ogg".into(), size: Some(20_000) });
    app.histories.get_mut(&id).unwrap().push(voice);
    press(&mut app, KeyCode::F(7), &tx, &s);
    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    term.draw(|f| draw(f, &mut app)).unwrap();
    let text = dump(&term);
    // в песочнице «программа есть для всего», поэтому «O слушать» показывается один раз, без «O открыть»
    assert_eq!(text.matches("O слушать").count(), 1, "{text}");
    assert!(!text.contains("O открыть") && !text.contains("слушать/смотреть"), "{text}");
    assert!(text.contains("S скачать"));
    press(&mut app, KeyCode::Char('o'), &tx, &s);
    assert!(matches!(drain(&mut rx).pop(), Some(Cmd::Open { id: 9, .. })));
    // у текстового сообщения клавиши про файлы только объясняют, что файла нет
    app.mode = Mode::Pick;
    app.pick = 0;
    for letter in ['o', 's', 't', 'p'] {
        app.pick_note.clear();
        press(&mut app, KeyCode::Char(letter), &tx, &s);
        assert!(app.pick_note.contains("нет"), "{letter}: {}", app.pick_note);
    }
    assert!(drain(&mut rx).is_empty());
}

// ------------------------------------------------------------------ виды окна, Ctrl+K, без звука, тихие часы

/// Три чата, как в `app_with_chats`, но с историей в «Рабочем чате» и выбранным видом окна.
fn app_in_view(view: u8) -> App {
    let mut app = app_with_chats();
    let g = app.chats[2].id;
    app.histories.insert(
        g,
        vec![msg(1, false, "Аня", "раз"), msg(2, false, "Аня", "два"), msg(3, false, "Борис", "три"), msg(4, true, "Я", "четыре")],
    );
    app.settings.view = 0;
    app.set_view(view);
    app
}

fn draw_text(app: &mut App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| draw(f, app)).unwrap();
    dump(&term)
}

fn ctrl(c: char) -> KeyEvent {
    key(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// Нажатие «не из вставки» с модификатором: Enter и Tab сразу после другой клавиши считаются частью вставленного текста.
fn press_mod(app: &mut App, code: KeyCode, mods: KeyModifiers, tx: &mpsc::UnboundedSender<Cmd>, s: &Arc<Saver>) {
    slow(app);
    app.handle_key(key(code, mods), tx, s);
}

#[test]
fn messenger_view_names_the_author_once_and_marks_new_messages() {
    let mut app = app_in_view(1);
    let g = app.chats[2].id;
    app.set_sel(Some(g));
    app.new_from.insert(g, 3);
    let text = draw_text(&mut app, 100, 30);
    assert_eq!(
        text.matches("Аня").count(),
        1,
        "имя автора стоит один раз над серией:
{text}"
    );
    assert!(text.contains("Борис"), "{text}");
    let divider = text.lines().position(|l| l.contains("новые сообщения")).expect("разделитель");
    let boris = text.lines().position(|l| l.contains("Борис")).unwrap();
    let two = text.lines().position(|l| l.contains("два")).unwrap();
    assert!(
        two < divider && divider < boris,
        "разделитель стоит перед первым новым сообщением:
{text}"
    );
    // Классический вид рисует имя над каждым сообщением и разделителя не знает.
    app.settings.view = 0;
    app.new_from.clear();
    let text = draw_text(&mut app, 100, 30);
    assert_eq!(text.matches("Аня").count(), 2, "{text}");
    assert!(!text.contains("новые сообщения"));
}

#[test]
fn new_messages_divider_is_set_when_the_chat_opens_and_only_if_something_was_read_before() {
    let mut app = app_in_view(1);
    let (tx, _rx) = mpsc::unbounded_channel();
    let g = app.chats[2].id;
    app.chats[2].unread = 2;
    app.set_sel(Some(g));
    let h = app.histories.remove(&g).unwrap();
    app.handle_event(Event::History { chat: g, msgs: h }, &tx);
    // Непрочитаны два последних чужих сообщения: «два»? нет — «три» и предыдущее «два» (чужие: 1, 2, 3 → последние два: 2, 3).
    assert_eq!(app.new_from.get(&g), Some(&2));
    assert_eq!(app.chats[2].unread, 0, "открытый чат прочитан");
    // Всё непрочитано: выше нечего отделять.
    let mut app = app_in_view(1);
    app.chats[2].unread = 3;
    app.set_sel(Some(g));
    let h = app.histories.remove(&g).unwrap();
    app.handle_event(Event::History { chat: g, msgs: h }, &tx);
    assert!(app.new_from.is_empty());
    // В классическом виде разделителя нет совсем.
    let mut app = app_in_view(0);
    app.chats[2].unread = 1;
    app.set_sel(Some(g));
    let h = app.histories.remove(&g).unwrap();
    app.handle_event(Event::History { chat: g, msgs: h }, &tx);
    assert!(app.new_from.is_empty());
}

#[test]
fn chat_list_in_the_new_views_has_a_preview_line_and_muted_chats_are_quiet() {
    let mut app = app_in_view(1);
    app.chats[0].preview = "Я: Всё хорошо!".into();
    app.chats[0].last_ts = 1_761_650_120;
    app.chats[1].muted = true;
    app.chats[1].unread = 5;
    app.chats[1].preview = "новая запись".into();
    let text = draw_text(&mut app, 100, 24);
    let lines: Vec<&str> = text.lines().collect();
    let mama = lines.iter().position(|l| l.starts_with("│  Мама")).unwrap();
    assert!(
        lines[mama + 1].contains("Я: Всё хорошо!"),
        "под именем последнее сообщение:
{text}"
    );
    let channel = lines.iter().position(|l| l.starts_with("│К Мой канал")).unwrap();
    assert!(lines[channel + 1].contains("новая запись"));
    // В классическом виде список в одну строку и без превью.
    app.settings.view = 0;
    let text = draw_text(&mut app, 100, 24);
    assert!(!text.contains("Я: Всё хорошо!") && !text.contains("новая запись"));
    // Заглушённый канал не входит в число непрочитанных в заголовке.
    assert_eq!(app.total_unread(), 2);
}

#[test]
fn draft_and_typing_replace_the_preview_in_the_list() {
    let mut app = app_in_view(1);
    app.chats[0].preview = "старое сообщение".into();
    app.drafts.insert(app.chats[0].id, ("привет\nмир".into(), 3));
    let text = draw_text(&mut app, 100, 24);
    assert!(text.contains("черновик: привет мир"), "{text}");
    assert!(!text.contains("старое сообщение"));
    app.drafts.clear();
    app.typing.insert(app.chats[0].id, (Instant::now(), "печатает…".into()));
    let text = draw_text(&mut app, 100, 24);
    assert!(text.contains("печатает…") && !text.contains("черновик"), "{text}");
}

#[test]
fn quick_switch_finds_chats_in_any_layout_and_lists_commands_after_a_slash() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    app.handle_key(ctrl('k'), &tx, &s);
    assert_eq!(app.mode, Mode::Quick);
    let names: Vec<String> = app.quick.as_ref().unwrap().items.iter().map(|i| i.label.clone()).collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(names[0].contains("Мама"), "сначала чаты с непрочитанными: {names:?}");
    // По названию.
    type_text(&mut app, "раб", &tx, &s);
    assert!(app.quick.as_ref().unwrap().items[0].label.contains("Рабочий чат"));
    // Запрос в другой раскладке тоже находит.
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Backspace, KeyModifiers::NONE), &tx, &s);
    }
    type_text(&mut app, "vfvf", &tx, &s);
    assert_eq!(app.quick.as_ref().unwrap().items.len(), 1);
    assert!(app.quick.as_ref().unwrap().items[0].label.contains("Мама"));
    // Enter открывает чат.
    for _ in 0..4 {
        app.handle_key(key(KeyCode::Backspace, KeyModifiers::NONE), &tx, &s);
    }
    type_text(&mut app, "раб", &tx, &s);
    press(&mut app, KeyCode::Enter, &tx, &s);
    assert_eq!(app.mode, Mode::Chat);
    assert_eq!(app.selected().unwrap().name, "Рабочий чат");
    // Команды: «/» и начало имени; Enter вставляет команду в поле ввода.
    app.handle_key(ctrl('k'), &tx, &s);
    type_text(&mut app, "/vi", &tx, &s);
    let items = &app.quick.as_ref().unwrap().items;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "/view");
    assert!(!items[0].hint.is_empty());
    press(&mut app, KeyCode::Enter, &tx, &s);
    assert_eq!(app.input, "/view ");
    assert_eq!(app.mode, Mode::Chat);
    // Ничего не нашлось: Enter не делает ничего, Esc закрывает, Ctrl+K тоже.
    app.input.clear();
    app.cursor = 0;
    app.handle_key(ctrl('k'), &tx, &s);
    type_text(&mut app, "яяяя", &tx, &s);
    assert!(app.quick.as_ref().unwrap().items.is_empty());
    press(&mut app, KeyCode::Enter, &tx, &s);
    assert_eq!(app.mode, Mode::Chat);
    assert!(app.input.is_empty());
    app.handle_key(ctrl('k'), &tx, &s);
    app.handle_key(key(KeyCode::Esc, KeyModifiers::NONE), &tx, &s);
    assert_eq!(app.mode, Mode::Chat);
    app.handle_key(ctrl('k'), &tx, &s);
    app.handle_key(ctrl('k'), &tx, &s);
    assert_eq!(app.mode, Mode::Chat);
}

#[test]
fn quick_switch_navigation_keys_stay_in_range_and_draw_on_small_screens() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    app.open_quick();
    for code in [KeyCode::Up, KeyCode::PageUp, KeyCode::Home, KeyCode::BackTab] {
        app.handle_key(key(code, KeyModifiers::NONE), &tx, &s);
        assert_eq!(app.quick.as_ref().unwrap().sel, 0);
    }
    for code in [KeyCode::Down, KeyCode::Tab, KeyCode::PageDown, KeyCode::End] {
        app.handle_key(key(code, KeyModifiers::NONE), &tx, &s);
        assert!(app.quick.as_ref().unwrap().sel <= 2);
    }
    assert_eq!(app.quick.as_ref().unwrap().sel, 2);
    for (w, h) in [(20, 6), (40, 10), (100, 30), (200, 60)] {
        let text = draw_text(&mut app, w, h);
        assert!(!text.is_empty());
    }
    // Блокировка закрывает окно.
    app.lock();
    assert!(app.quick.is_none());
}

#[test]
fn quick_switch_puts_unread_and_pinned_chats_first_and_mentions_before_everything() {
    let mut app = app_with_chats();
    app.chats[1].muted = true;
    app.chats[1].unread = 9;
    app.chats[2].mentions = 1;
    app.chats[2].unread = 1;
    let items = app.quick_items("");
    let order: Vec<&str> =
        items.iter().map(|i| i.label.trim_start_matches(|c: char| !c.is_alphabetic() || c == 'Г' || c == 'К').trim()).collect();
    assert_eq!(order.len(), 3);
    assert!(items[0].label.contains("Рабочий чат"), "упоминание важнее всего: {order:?}");
    assert!(items[1].label.contains("Мама"), "потом обычные непрочитанные: {order:?}");
    assert!(items[2].label.contains("Мой канал"), "заглушённый канал последним: {order:?}");
    assert!(items[2].hint.contains("без звука"), "{}", items[2].hint);
    assert!(items[0].hint.contains("@1"), "{}", items[0].hint);
}

#[test]
fn view_command_cycles_validates_and_persists() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    assert_eq!(app.view(), View::Classic);
    for (typed, want) in [("/view", 1u8), ("/view", 2), ("/view", 0), ("/view messenger", 1), ("/view два окна", 2), ("/view classic", 0)]
    {
        submit_text(&mut app, typed, &tx, &s);
        assert_eq!(app.settings.view, want, "{typed}");
    }
    submit_text(&mut app, "/view ерунда", &tx, &s);
    assert_eq!(app.settings.view, 0, "неизвестное слово ничего не меняет");
    assert!(app.status.contains("/view"), "{}", app.status);
}

#[test]
fn settings_rows_for_view_quiet_hours_and_photo_shrinking_change_and_clamp() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    app.mode = Mode::Settings;
    app.settings_row = 17;
    app.handle_key(key(KeyCode::Right, KeyModifiers::NONE), &tx, &s);
    assert_eq!(app.settings.view, 1);
    app.handle_key(key(KeyCode::Left, KeyModifiers::NONE), &tx, &s);
    app.handle_key(key(KeyCode::Left, KeyModifiers::NONE), &tx, &s);
    assert_eq!(app.settings.view, 2, "назад из первого значения — в последнее");
    app.settings_row = 18;
    app.handle_key(key(KeyCode::Right, KeyModifiers::NONE), &tx, &s);
    assert_eq!(app.settings.quiet, 1);
    assert_eq!(app.settings.quiet_label(), "22:00–07:00");
    app.settings_row = 19;
    assert!(app.settings.compress);
    app.handle_key(key(KeyCode::Right, KeyModifiers::NONE), &tx, &s);
    assert!(!app.settings.compress);
    let text = draw_text(&mut app, 100, 40);
    for want in ["Вид", "два окна", "Тихие часы", "22:00–07:00", "Сжимать фото при отправке"] {
        assert!(text.contains(want), "в настройках нет «{want}»:\n{text}");
    }
}

#[test]
fn mute_command_toggles_locally_and_keeps_the_chat_out_of_alerts_and_the_unread_total() {
    let mut app = app_with_chats();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    let mama = app.chats[0].id;
    assert!(app.may_alert(mama));
    assert_eq!(app.total_unread(), 2);
    submit_text(&mut app, "/mute", &tx, &s);
    let did = mama.bot_api_dialog_id().unwrap();
    assert_eq!(app.settings.muted, vec![did]);
    assert!(app.is_muted(&app.chats[0].clone()));
    assert!(!app.may_alert(mama));
    assert_eq!(app.total_unread(), 0, "заглушённый чат не считается в заголовке");
    assert!(draw_text(&mut app, 100, 24).contains("без звука"));
    // Новые сообщения всё равно приходят и считаются в самом чате.
    let other = app.chats[2].id;
    app.handle_event(Event::Incoming { chat: other, chat_name: "Рабочий чат".into(), msg: msg(9, false, "Аня", "привет") }, &tx);
    assert_eq!(app.chats.iter().find(|c| c.id == other).unwrap().unread, 1);
    // Повторная команда возвращает звук.
    submit_text(&mut app, "/mute", &tx, &s);
    assert!(app.settings.muted.is_empty());
    assert!(app.may_alert(mama));
    // Чат, заглушённый в самом Telegram, остаётся заглушённым и после /mute ещё раз; об этом сказано.
    let id = app.chats[1].id;
    app.chats[1].muted = true;
    app.set_sel(Some(id));
    submit_text(&mut app, "/mute", &tx, &s);
    submit_text(&mut app, "/mute", &tx, &s);
    assert!(app.is_muted(&app.chats[1].clone()), "в Telegram чат всё ещё заглушён");
    assert!(app.status.contains("в Telegram"), "{}", app.status);
    while rx.try_recv().is_ok() {}
}

#[test]
fn muted_list_is_capped() {
    let mut app = app_with_chats();
    let s = saver();
    app.settings.muted = (1..=500).map(|n| -n).collect();
    let id = app.chats[0].id;
    app.set_sel(Some(id));
    app.cmd_mute(&s);
    assert_eq!(app.settings.muted.len(), 500, "список не растёт без предела");
    assert_eq!(*app.settings.muted.last().unwrap(), id.bot_api_dialog_id().unwrap());
    assert!(!app.settings.muted.contains(&-1), "вытесняется самый давний");
}

#[test]
fn dnd_command_sets_clears_and_rejects_nonsense() {
    let mut app = app_with_chats();
    let mama = app.chats[0].id;
    app.cmd_dnd("30");
    assert!(app.dnd_until.is_some() && !app.may_alert(mama));
    assert!(app.status.contains(':'), "{}", app.status);
    app.cmd_dnd("off");
    assert!(app.dnd_until.is_none() && app.may_alert(mama));
    app.cmd_dnd("");
    let hour = app.dnd_until.unwrap();
    assert!(hour > Instant::now() + Duration::from_secs(59 * 60), "по умолчанию час");
    for bad in ["abc", "-5", "5000", "1.5"] {
        app.dnd_until = None;
        app.cmd_dnd(bad);
        assert!(app.dnd_until.is_none(), "«{bad}» не должно включать режим");
        assert!(app.status.contains("/dnd"), "{}", app.status);
    }
    app.cmd_dnd("0");
    assert!(app.dnd_until.is_none());
}

#[test]
fn next_unread_goes_mentions_first_then_unread_then_muted_and_skips_the_open_chat() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let (mama, channel, group) = (app.chats[0].id, app.chats[1].id, app.chats[2].id);
    app.chats[1].unread = 4;
    app.chats[1].muted = true;
    assert_eq!(app.next_unread_chat(), Some(channel), "в «Маме» непрочитанные, но она открыта");
    app.chats[2].unread = 1;
    assert_eq!(app.next_unread_chat(), Some(group), "обычные раньше заглушённых");
    app.chats[1].mentions = 1;
    assert_eq!(app.next_unread_chat(), Some(channel), "упоминание раньше всего");
    app.handle_key(ctrl('n'), &tx, &s);
    assert_eq!(app.sel, Some(channel));
    for c in &mut app.chats {
        c.unread = 0;
        c.mentions = 0;
    }
    app.handle_key(ctrl('n'), &tx, &s);
    assert_eq!(app.sel, Some(channel), "больше нет — остаёмся");
    assert!(app.status.contains("Непрочитанных"), "{}", app.status);
    let _ = mama;
    app.chats.clear();
    assert_eq!(app.next_unread_chat(), None, "пустой список не падает");
}

#[test]
fn alt_digit_opens_pinned_chats_in_list_order() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let (mama, group) = (app.chats[0].id, app.chats[2].id);
    // Закреплены «Рабочий чат» и «Мама», но порядок закрепления в настройках другой, чем в списке.
    app.settings.pinned = vec![group.bot_api_dialog_id().unwrap(), mama.bot_api_dialog_id().unwrap()];
    assert_eq!(app.favorites(), vec![mama, group], "порядок — как в списке чатов");
    app.handle_key(key(KeyCode::Char('2'), KeyModifiers::ALT), &tx, &s);
    assert_eq!(app.sel, Some(group));
    app.handle_key(key(KeyCode::Char('1'), KeyModifiers::ALT), &tx, &s);
    assert_eq!(app.sel, Some(mama));
    app.handle_key(key(KeyCode::Char('5'), KeyModifiers::ALT), &tx, &s);
    assert_eq!(app.sel, Some(mama), "чата №5 нет — остаёмся");
    assert!(app.status.contains("/pin"), "{}", app.status);
    // В списке у закреплённых чатов в новых видах стоят эти номера.
    app.settings.view = 1;
    let text = draw_text(&mut app, 100, 24);
    assert!(text.lines().any(|l| l.contains('1') && l.contains("Мама")), "{text}");
    // Alt+0 и Alt+буква ничего не открывают.
    app.handle_key(key(KeyCode::Char('0'), KeyModifiers::ALT), &tx, &s);
    assert_eq!(app.sel, Some(mama));
}

#[test]
fn drafts_command_lists_chats_with_unsent_text_and_opens_them() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let (mama, channel, group) = (app.chats[0].id, app.chats[1].id, app.chats[2].id);
    app.set_sel(Some(group));
    submit_text(&mut app, "/drafts", &tx, &s);
    assert!(app.overlay.is_none());
    assert!(app.status.contains("Неотправленных сообщений нет"), "{}", app.status);
    // Черновик, оставленный здесь, и черновик из Telegram с другого устройства.
    app.drafts.insert(mama, ("пишу маме\nвторая строка".into(), 3));
    app.chats[1].draft = "с телефона".into();
    app.apply_server_draft(channel, "с телефона", false);
    // Недописанная команда и пустышка не считаются.
    app.drafts.insert(group, ("/find".into(), 5));
    submit_text(&mut app, "/drafts", &tx, &s);
    let ov = app.overlay.as_ref().expect("список черновиков");
    assert_eq!(ov.kind, OverlayKind::Drafts);
    let labels: Vec<&String> = ov.items.iter().map(|i| &i.label).collect();
    assert_eq!(labels.len(), 2, "{labels:?}");
    assert_eq!(labels[0], "Мама: пишу маме вторая строка", "в порядке списка чатов, перевод строки как пробел");
    assert_eq!(labels[1], "Мой канал: с телефона");
    assert!(draw_text(&mut app, 100, 24).contains("открыть чат с черновиком"));
    press(&mut app, KeyCode::Enter, &tx, &s);
    assert!(app.overlay.is_none());
    assert_eq!(app.sel, Some(mama));
    assert_eq!(app.input, "пишу маме\nвторая строка", "черновик вернулся в поле ввода");
    // Открытый чат в список не попадает: его текст и так перед глазами.
    submit_text(&mut app, "/drafts", &tx, &s);
    let ov = app.overlay.as_ref().expect("остался черновик канала");
    assert_eq!(ov.items.len(), 1);
    // Выключенные черновики из Telegram в списке не показываются: открыть их всё равно нечем.
    app.overlay = None;
    app.mode = Mode::Chat;
    app.settings.cloud_drafts = false;
    app.drafts.clear();
    app.chats[1].draft = "с телефона".into();
    submit_text(&mut app, "/drafts", &tx, &s);
    assert!(app.overlay.is_none());
}

#[test]
fn preview_line_follows_incoming_and_sent_messages_and_the_chat_moves_up() {
    let mut app = app_with_chats();
    let (tx, _rx) = mpsc::unbounded_channel();
    let group = app.chats[2].id;
    app.handle_event(
        Event::Incoming {
            chat: group, chat_name: "Рабочий чат".into(), msg: msg(20, false, "Борис Иванов", "завтра в десять")
        },
        &tx,
    );
    assert_eq!(app.chats[0].id, group, "чат с новым сообщением поднялся наверх");
    assert_eq!(app.chats[0].preview, "Борис: завтра в десять");
    assert!(app.chats[0].last_ts > 0);
    app.handle_event(Event::Sent { chat: group, msg: msg(21, true, "Я", "хорошо"), scheduled: false }, &tx);
    assert_eq!(app.chats[0].preview, "Я: хорошо");
    // Запланированное сообщение в превью не попадает.
    app.handle_event(Event::Sent { chat: group, msg: msg(22, true, "Я", "потом"), scheduled: true }, &tx);
    assert_eq!(app.chats[0].preview, "Я: хорошо");
}

// --- вид «Два окна»

#[test]
fn two_panes_open_chats_in_the_active_pane_and_move_between_panes_with_ctrl_arrows() {
    let mut app = app_in_view(2);
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let (mama, channel, group) = (app.chats[0].id, app.chats[1].id, app.chats[2].id);
    assert!(app.two_panes());
    assert_eq!(app.sel, Some(mama));
    assert_eq!(app.panes.open, vec![mama]);
    // Ctrl+→: правое окно пусто и активно; Ctrl+K кладёт в него чат.
    app.handle_key(key(KeyCode::Right, KeyModifiers::CONTROL), &tx, &s);
    assert!(app.panes.right_active);
    assert_eq!((app.sel, app.panes.other), (None, Some(mama)));
    app.handle_key(ctrl('k'), &tx, &s);
    type_text(&mut app, "раб", &tx, &s);
    press(&mut app, KeyCode::Enter, &tx, &s);
    assert_eq!((app.sel, app.panes.other), (Some(group), Some(mama)));
    assert_eq!(app.panes.open, vec![mama, group]);
    let text = draw_text(&mut app, 120, 30);
    assert!(text.lines().next().unwrap().contains("Мама") && text.lines().next().unwrap().contains("Рабочий чат"), "вкладки:\n{text}");
    assert!(text.contains("┌ Мама") && text.contains("┌ Рабочий чат"), "оба окна рядом:\n{text}");
    assert!(text.contains("→ Рабочий чат"), "поле ввода показывает, куда уйдёт текст:\n{text}");
    // Ctrl+←: активным становится левое окно, чаты окон не теряются.
    app.handle_key(key(KeyCode::Left, KeyModifiers::CONTROL), &tx, &s);
    assert_eq!((app.sel, app.panes.other, app.panes.right_active), (Some(mama), Some(group), false));
    // Повторный Ctrl+← ничего не делает.
    app.handle_key(key(KeyCode::Left, KeyModifiers::CONTROL), &tx, &s);
    assert_eq!((app.sel, app.panes.other), (Some(mama), Some(group)));
    // Чат, уже стоящий во втором окне, не дублируется: активным становится то окно.
    app.open_chat(group);
    assert_eq!((app.sel, app.panes.other, app.panes.right_active), (Some(group), Some(mama), true));
    // Открыть третий чат: он заменяет чат активного окна, вкладка прежнего остаётся.
    app.open_chat(channel);
    assert_eq!((app.sel, app.panes.other), (Some(channel), Some(mama)));
    assert_eq!(app.panes.open, vec![mama, group, channel]);
}

#[test]
fn scroll_position_travels_with_the_chat_when_panes_are_switched() {
    let mut app = app_in_view(2);
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    app.scroll = 7;
    app.handle_key(key(KeyCode::Right, KeyModifiers::CONTROL), &tx, &s);
    assert_eq!(app.panes.other_scroll, 7, "у чата в левом окне прокрутка осталась прежней");
    assert_eq!(app.scroll, 0);
    app.handle_key(key(KeyCode::Left, KeyModifiers::CONTROL), &tx, &s);
    assert_eq!(app.scroll, 7);
}

#[test]
fn tab_cycles_open_chats_skipping_the_other_pane_and_ctrl_w_closes_a_tab() {
    let mut app = app_in_view(2);
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let (mama, channel, group) = (app.chats[0].id, app.chats[1].id, app.chats[2].id);
    app.open_chat(channel);
    app.open_chat(group);
    assert_eq!(app.panes.open, vec![mama, channel, group]);
    // Положим «Мою» (channel) во второе окно.
    app.handle_key(key(KeyCode::Right, KeyModifiers::CONTROL), &tx, &s);
    app.open_chat(channel);
    assert_eq!((app.sel, app.panes.other), (Some(channel), Some(group)));
    press(&mut app, KeyCode::Tab, &tx, &s);
    assert_eq!(app.sel, Some(mama), "Tab: следующая вкладка по кругу, чат второго окна пропущен");
    press(&mut app, KeyCode::Tab, &tx, &s);
    assert_eq!(app.sel, Some(channel));
    press_mod(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT, &tx, &s);
    assert_eq!(app.sel, Some(mama));
    // Ctrl+W закрывает вкладку активного окна.
    app.handle_key(ctrl('w'), &tx, &s);
    assert!(!app.panes.open.contains(&mama));
    assert_ne!(app.sel, Some(mama));
    assert_eq!(app.panes.other, Some(group), "второе окно не тронуто");
    // Закрыть всё: окно остаётся пустым и само не заполняется.
    while app.sel.is_some() {
        app.handle_key(ctrl('w'), &tx, &s);
    }
    assert!(app.sel.is_none());
    app.ensure_selection();
    assert!(app.sel.is_none(), "пустое окно не заполняется само");
    let text = draw_text(&mut app, 120, 30);
    assert!(text.contains("Ctrl+K — открыть чат"), "{text}");
}

#[test]
fn open_tabs_are_limited_and_visible_chats_are_never_dropped() {
    let mut app = app_with_chats();
    app.chats = (1..=15).map(|n| chat(n, &format!("Чат {n}"), Kind::User, 0)).collect();
    app.settings.view = 0;
    app.set_view(2);
    let ids: Vec<PeerId> = app.chats.iter().map(|c| c.id).collect();
    for id in &ids {
        app.open_chat(*id);
    }
    assert!(app.panes.open.len() <= 12, "{}", app.panes.open.len());
    assert!(app.panes.open.contains(app.sel.as_ref().unwrap()), "открытый чат остаётся вкладкой");
    assert_eq!(app.sel, Some(ids[14]));
    // Вкладки не дублируются.
    let mut seen = HashSet::new();
    assert!(app.panes.open.iter().all(|id| seen.insert(*id)));
}

#[test]
fn messages_in_the_second_pane_count_as_read() {
    let mut app = app_in_view(2);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let s = saver();
    let (mama, channel, group) = (app.chats[0].id, app.chats[1].id, app.chats[2].id);
    app.open_chat(group);
    app.handle_key(key(KeyCode::Right, KeyModifiers::CONTROL), &tx, &s);
    app.open_chat(mama);
    assert_eq!(app.panes.other, Some(group));
    while rx.try_recv().is_ok() {}
    app.handle_event(Event::Incoming { chat: group, chat_name: "Рабочий чат".into(), msg: msg(30, false, "Аня", "эй") }, &tx);
    assert_eq!(app.chats.iter().find(|c| c.id == group).unwrap().unread, 0, "чат виден во втором окне");
    let mut sent_read = false;
    while let Ok(c) = rx.try_recv() {
        sent_read |= matches!(c, Cmd::MarkRead(ref ch) if ch.id == group);
    }
    assert!(sent_read, "Telegram должен узнать о прочтении");
    // Новое сообщение во втором окне прокручивает к концу именно его, а не активное окно.
    app.panes.other_scroll = 5;
    app.scroll = 4;
    app.handle_event(Event::Incoming { chat: group, chat_name: "Рабочий чат".into(), msg: msg(31, false, "Аня", "ещё") }, &tx);
    assert_eq!((app.panes.other_scroll, app.scroll), (0, 4));
    // Чат, которого на экране нет, остаётся непрочитанным.
    app.handle_event(Event::Incoming { chat: channel, chat_name: "Мой канал".into(), msg: msg(5, false, "Мой канал", "пост") }, &tx);
    assert_eq!(app.chats.iter().find(|c| c.id == channel).unwrap().unread, 1);
}

#[test]
fn narrow_screens_show_only_the_active_pane_and_wide_screens_both() {
    let mut app = app_in_view(2);
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let (mama, group) = (app.chats[0].id, app.chats[2].id);
    app.open_chat(group);
    app.handle_key(key(KeyCode::Left, KeyModifiers::CONTROL), &tx, &s);
    app.handle_key(key(KeyCode::Right, KeyModifiers::CONTROL), &tx, &s);
    app.open_chat(mama);
    let wide = draw_text(&mut app, 120, 30);
    assert!(wide.contains("┌ Мама") && wide.contains("┌ Рабочий чат"), "{wide}");
    let narrow = draw_text(&mut app, 60, 30);
    assert!(narrow.contains("┌ Мама"), "{narrow}");
    assert!(!narrow.contains("┌ Рабочий чат"), "на узком экране второе окно не рисуется:\n{narrow}");
    assert_eq!(app.panes.hit[1].width, 0);
    for (w, h) in [(10, 5), (30, 8), (64, 12), (80, 24), (300, 80)] {
        draw_text(&mut app, w, h); // не падает на любых размерах
    }
}

#[test]
fn mouse_clicks_switch_panes_and_open_tabs_and_the_wheel_scrolls_the_second_pane() {
    let mut app = app_in_view(2);
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let (mama, group) = (app.chats[0].id, app.chats[2].id);
    // Длинная история во втором окне, чтобы было что прокручивать.
    app.histories.insert(group, (1..=80).map(|i| msg(i, false, "Аня", &format!("сообщение {i}"))).collect());
    app.open_chat(group);
    app.handle_key(key(KeyCode::Left, KeyModifiers::CONTROL), &tx, &s);
    app.handle_key(key(KeyCode::Right, KeyModifiers::CONTROL), &tx, &s);
    app.open_chat(mama);
    assert_eq!((app.sel, app.panes.other, app.panes.right_active), (Some(mama), Some(group), true));
    draw_text(&mut app, 120, 30);
    let left = app.panes.hit[0];
    let mouse = |kind, x, y| MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE };
    // Колесо над левым (неактивным) окном прокручивает его и не переключает окно.
    assert!(app.handle_mouse(mouse(MouseEventKind::ScrollUp, left.x + 5, left.y + 5), &tx));
    assert_eq!(app.panes.other_scroll, 3);
    assert_eq!(app.sel, Some(mama));
    assert!(app.handle_mouse(mouse(MouseEventKind::ScrollDown, left.x + 5, left.y + 5), &tx));
    assert_eq!(app.panes.other_scroll, 0);
    // Щелчок по левому окну делает его активным.
    assert!(app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), left.x + 5, left.y + 5), &tx));
    assert_eq!((app.sel, app.panes.right_active), (Some(group), false));
    // Щелчок по вкладке «Мама» открывает её в активном окне.
    draw_text(&mut app, 120, 30);
    let tab = app.panes.hit_tabs.iter().find(|t| t.2 == mama).copied().expect("вкладка Мама");
    assert!(app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), tab.0 + 1, 0), &tx));
    assert!(app.sel == Some(mama) || app.panes.other == Some(mama));
    // Мышь выключена в настройках — события игнорируются.
    app.settings.mouse = false;
    assert!(!app.handle_mouse(mouse(MouseEventKind::ScrollUp, 3, 3), &tx));
}

#[test]
fn leaving_a_chat_removes_it_from_both_panes_and_tabs() {
    let mut app = app_in_view(2);
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let (mama, group) = (app.chats[0].id, app.chats[2].id);
    app.open_chat(group);
    app.handle_key(key(KeyCode::Right, KeyModifiers::CONTROL), &tx, &s);
    app.open_chat(mama);
    app.handle_event(Event::Left(group), &tx);
    assert_eq!(app.panes.other, None);
    assert!(!app.panes.open.contains(&group));
    app.handle_event(Event::Left(mama), &tx);
    assert!(app.sel.is_none());
    assert!(!app.panes.open.contains(&mama));
    draw_text(&mut app, 100, 30);
}

#[test]
fn switching_views_keeps_the_chat_and_resets_the_state_of_the_old_view() {
    let mut app = app_in_view(2);
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let (mama, group) = (app.chats[0].id, app.chats[2].id);
    app.open_chat(group);
    app.handle_key(key(KeyCode::Left, KeyModifiers::CONTROL), &tx, &s);
    app.handle_key(key(KeyCode::Right, KeyModifiers::CONTROL), &tx, &s);
    app.open_chat(mama);
    app.new_from.insert(group, 2);
    app.set_view(0);
    assert_eq!(app.view(), View::Classic);
    assert!(app.panes.open.is_empty() && app.panes.other.is_none());
    assert!(app.new_from.is_empty());
    assert!(app.sel.is_some(), "в списке чат остаётся выбранным");
    app.set_view(9);
    assert_eq!(app.settings.view, 2, "слишком большое число — последний вид");
    app.set_view(2);
    assert_eq!(app.panes.open.len(), 1, "текущий чат — первая вкладка нового вида");
}

#[test]
fn new_views_have_no_russian_leftovers_in_english() {
    crate::i18n::set(1);
    let mut app = App::new(Settings::default());
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let mut a = chat(1, "Mom", Kind::User, 2);
    a.preview = "Me: hello".into();
    a.last_ts = 1_761_650_000;
    let mut b = chat(2, "Work", Kind::Group, 0);
    b.muted = true;
    app.histories.insert(a.id, vec![msg(1, false, "Mom", "Hi"), msg(2, false, "Mom", "How are you?"), msg(3, true, "Me", "Fine")]);
    app.chats = vec![a, b];
    app.ensure_selection();
    let (mom, work) = (app.chats[0].id, app.chats[1].id);
    for view in [1u8, 2] {
        app.settings.view = 0;
        app.set_view(view);
        app.new_from.insert(mom, 2);
        app.settings.pinned = vec![mom.bot_api_dialog_id().unwrap()];
        for mode in [Mode::Chat, Mode::Help, Mode::Settings] {
            app.mode = mode;
            let text = draw_text(&mut app, 120, 40).replace("Русский", "");
            assert!(!has_cyrillic(&text), "view {view}, {mode:?}:\n{text}");
        }
        app.mode = Mode::Chat;
        app.open_quick();
        let text = draw_text(&mut app, 120, 40);
        assert!(!has_cyrillic(&text) && text.contains("Quick switch"), "{text}");
        type_text(&mut app, "/vi", &tx, &s);
        let text = draw_text(&mut app, 120, 40);
        assert!(!has_cyrillic(&text) && text.contains("/view"), "{text}");
        app.handle_key(key(KeyCode::Esc, KeyModifiers::NONE), &tx, &s);
    }
    app.drafts.insert(work, ("later".into(), 5));
    app.cmd_drafts();
    let text = draw_text(&mut app, 120, 40);
    assert!(!has_cyrillic(&text) && text.contains("open the chat with the draft"), "{text}");
    app.overlay = None;
    app.cmd_dnd("x");
    assert!(!has_cyrillic(&app.status), "{}", app.status);
    app.cmd_dnd("15");
    assert!(!has_cyrillic(&app.status), "{}", app.status);
    app.cmd_view("zzz", &s);
    assert!(!has_cyrillic(&app.status), "{}", app.status);
    crate::i18n::set(0);
}

// ---- справка: помещается в консоль, листается, перечисляет все команды

fn help_app() -> App {
    let mut app = demo_app();
    app.open_help();
    app
}

#[test]
fn help_lists_every_command_in_both_languages() {
    for lang in [0, 1] {
        crate::i18n::set(lang);
        let mut app = help_app();
        let text = draw_text(&mut app, 110, 120);
        assert_eq!(app.help_max, 0, "в высоком окне листать нечего");
        for c in COMMANDS.iter().filter(|c| **c != "/search") {
            assert!(text.contains(c), "{c} нет в справке ({lang})");
        }
        for k in ["Ctrl+K", "Ctrl+N", "Alt+1", "F8", "Ctrl+W", "F7", "Ctrl+Z", "PgUp"] {
            assert!(text.contains(k), "{k} нет в справке ({lang})");
        }
        assert!(text.contains(&format!("Tg-Lite {VERSION}")), "версия в заголовке");
    }
    crate::i18n::set(0);
}

#[test]
fn help_fits_small_consoles_and_scrolls_to_the_last_line() {
    for (lang, last) in [(0, "/report"), (1, "/report")] {
        crate::i18n::set(lang);
        for (w, h) in [(120, 30), (80, 25), (60, 20), (40, 12)] {
            let mut app = help_app();
            let first = draw_text(&mut app, w, h);
            assert!(app.help_max > 0, "{w}x{h}: справка длиннее экрана");
            assert!(first.contains("PgUp PgDn"), "{w}x{h}: подсказка про прокрутку видна:\n{first}");
            assert!(first.contains("Tg-Lite"), "{w}x{h}: версия в заголовке видна");
            assert!(!first.contains(last), "{w}x{h}: последняя строка ещё не видна");
            app.handle_help_key(key(KeyCode::End, KeyModifiers::NONE));
            let end = draw_text(&mut app, w, h);
            assert!(end.contains(last), "{w}x{h}: после End видна последняя строка ({lang}):\n{end}");
            assert!(!end.contains("Basics") && !end.contains("Основное"), "{w}x{h}: начало ушло вверх");
        }
    }
    crate::i18n::set(0);
}

#[test]
fn help_scroll_keys_move_and_anything_else_closes() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let s = saver();
    let mut app = help_app();
    let _ = draw_text(&mut app, 100, 20);
    assert!(app.help_max > 5 && app.help_top == 0 && app.mode == Mode::Help);
    app.handle_key(key(KeyCode::Down, KeyModifiers::NONE), &tx, &s);
    assert_eq!(app.help_top, 1);
    app.handle_key(key(KeyCode::Up, KeyModifiers::NONE), &tx, &s);
    app.handle_key(key(KeyCode::Up, KeyModifiers::NONE), &tx, &s);
    assert_eq!(app.help_top, 0, "выше начала не уходит");
    app.handle_key(key(KeyCode::PageDown, KeyModifiers::NONE), &tx, &s);
    assert_eq!(app.help_top, app.help_rows - 1, "страница без одной строки: контекст сохраняется");
    app.handle_key(key(KeyCode::End, KeyModifiers::NONE), &tx, &s);
    assert_eq!(app.help_top, app.help_max);
    app.handle_key(key(KeyCode::PageDown, KeyModifiers::NONE), &tx, &s);
    app.handle_key(key(KeyCode::Down, KeyModifiers::NONE), &tx, &s);
    assert_eq!(app.help_top, app.help_max, "ниже конца не уходит");
    app.handle_key(key(KeyCode::Home, KeyModifiers::NONE), &tx, &s);
    assert_eq!(app.help_top, 0);
    app.handle_key(key(KeyCode::Down, KeyModifiers::NONE), &tx, &s);
    app.handle_key(key(KeyCode::Char('x'), KeyModifiers::NONE), &tx, &s);
    assert_eq!(app.mode, Mode::Chat, "любая другая клавиша закрывает");
    assert_eq!(app.help_top, 0, "и сбрасывает прокрутку");
    assert!(app.input.is_empty(), "закрывающая клавиша не попадает в поле ввода");
    // Повторное открытие начинается с начала, даже если окно сменили.
    app.help_top = 7;
    app.handle_key(key(KeyCode::F(1), KeyModifiers::NONE), &tx, &s);
    assert!(app.mode == Mode::Help && app.help_top == 0);
}

#[test]
fn help_lines_never_exceed_the_window_and_keys_stay_short() {
    use unicode_width_check::line_width;
    mod unicode_width_check {
        use ratatui::text::Line;
        use textwrap::core::display_width;
        pub fn line_width(l: &Line) -> usize {
            l.spans.iter().map(|s| display_width(&s.content)).sum()
        }
    }
    for lang in [0, 1] {
        crate::i18n::set(lang);
        for w in (30..=100).step_by(5) {
            for l in help::lines(Color::Cyan, w) {
                assert!(line_width(&l) <= w, "строка шире окна {w} ({lang}): {l:?}");
            }
        }
        for k in help::key_labels() {
            assert!(textwrap::core::display_width(k) <= 18, "подпись клавиши длиннее 18 знаков: {k}");
        }
    }
    crate::i18n::set(0);
}

// ------------------------------------------------------------------ прогон отрисовки: любой вид × режим × размер

/// Приложение с «трудными» данными: все виды вложений, широкие символы, очень длинные слова и пустые поля.
fn stress_app(view: u8) -> App {
    use crate::tg::{Btn, BtnAction, Fwd, LinkCard, MediaInfo, MediaKind, PollOpt, PollView, Reaction};
    let mut app = app_in_view(view);
    let long_word = "Ъ".repeat(300);
    let wide = "日本語のテキスト 你好世界 😀👍🏽 e\u{301}e\u{301} ａｂｃ ".repeat(8);
    let mut rich = msg(
        10,
        false,
        "Очень-очень длинное имя отправителя, которое не помещается нигде",
        "Текст с ссылкой https://example.com/very/long/path?x=1",
    );
    rich.edited = true;
    rich.mentioned = true;
    rich.views = Some(1_234_567);
    rich.reply_to = Some(9999);
    rich.fwd = Some(Fwd { name: Some("Имя пересланного автора".into()), from: None });
    rich.reactions = vec![
        Reaction { emoji: "👍".into(), count: 3, mine: true },
        Reaction { emoji: "".into(), count: 0, mine: false },
        Reaction { emoji: "❤".into(), count: 1_000_000, mine: false },
    ];
    rich.buttons = vec![
        vec![
            Btn { text: "Да".into(), action: BtnAction::Callback(vec![1]) },
            Btn { text: "Ссылка на очень длинный адрес".into(), action: BtnAction::Url("https://a.b".into()) },
            Btn { text: "".into(), action: BtnAction::Other },
        ],
        vec![],
    ];
    rich.poll = Some(PollView {
        question: "Вопрос ".repeat(40),
        quiz: true,
        closed: false,
        multiple: true,
        total: 5,
        options: vec![
            PollOpt { text: "Первый вариант".into(), data: vec![0], voters: 3, chosen: true, correct: true },
            PollOpt { text: wide.clone(), data: vec![1], voters: 0, chosen: false, correct: false },
            PollOpt { text: "".into(), data: vec![2], voters: -1, chosen: false, correct: false },
        ],
    });
    rich.media = Some(MediaInfo { kind: MediaKind::Photo, name: "фото.jpg".into(), size: Some(u64::MAX) });
    rich.link = Some(LinkCard { site: "сайт".into(), title: wide.clone(), desc: long_word.clone(), url: "https://example.com".into() });
    let mut empty = msg(11, true, "", "");
    empty.caption = Some(String::new());
    let mut ids: Vec<ChatMsg> = vec![
        rich,
        empty,
        msg(12, false, "Аня", &long_word),
        msg(13, true, "Я", &wide),
        msg(14, false, "Борис", &"слово ".repeat(2000)),
        msg(15, false, "Борис", "строка 1\nстрока 2\n\n\nстрока 5"),
    ];
    ids.extend((20..60).map(|i| msg(i, i % 3 == 0, "Аня", &format!("сообщение номер {i}"))));
    let g = app.chats[2].id;
    app.histories.insert(g, ids);
    // Чаты с неудобными названиями и счётчиками.
    let mut weird = chat(40, "", Kind::User, 99_999);
    weird.mentions = 1000;
    weird.muted = true;
    weird.preview = long_word.clone();
    weird.presence = Some(crate::tg::Presence::Online(i64::MAX / 2));
    let mut cjk = chat(41, &wide, Kind::Channel, 0);
    cjk.can_write = false;
    cjk.preview = wide.clone();
    let mut named = chat(42, &long_word, Kind::Group, 7);
    named.draft = wide.clone();
    app.chats.extend([weird, cjk, named]);
    app
}

#[test]
fn every_view_and_mode_draws_at_every_size_without_panic() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let sizes: [(u16, u16); 22] = [
        (0, 0),
        (1, 1),
        (2, 1),
        (1, 2),
        (3, 3),
        (5, 3),
        (8, 4),
        (10, 5),
        (16, 6),
        (20, 6),
        (24, 8),
        (30, 8),
        (40, 12),
        (50, 15),
        (63, 20),
        (64, 20),
        (65, 20),
        (80, 24),
        (120, 30),
        (200, 50),
        (300, 80),
        (97, 3),
    ];
    type Setup = (&'static str, fn(&mut App));
    let setups: Vec<Setup> = vec![
        ("чат", |_| {}),
        ("длинное поле ввода", |a| {
            a.input = "日本語 😀 длинный текст\nвторая строка ".repeat(40);
            a.cursor = 17;
        }),
        ("курсор за концом", |a| {
            a.input = "abc".into();
            a.cursor = 99;
        }),
        ("ответ и правка", |a| {
            a.reply = Some((10, "Очень длинная подпись ответа ".repeat(20)));
            a.edit_target = Some(12);
        }),
        ("статус", |a| a.status = "очень длинное сообщение в строке состояния ".repeat(30)),
        ("огромная прокрутка", |a| a.scroll = 1_000_000),
        ("прокрутка и найденное", |a| {
            a.scroll = 3;
            a.focus_msg = Some(14);
            a.focus_pending = true;
        }),
        ("выбор сообщения", |a| {
            a.mode = Mode::Pick;
            a.pick = 2;
        }),
        ("выбор вне диапазона", |a| {
            a.mode = Mode::Pick;
            a.pick = 99_999;
        }),
        ("отметки", |a| {
            a.marked.extend([10, 11, 12, 777]);
        }),
        ("пересылка", |a| {
            let src = a.chats[0].clone();
            a.fwd_src = Some((src, vec![1, 2]));
            a.mode = Mode::Forward;
        }),
        ("пересылка без источника", |a| a.mode = Mode::Forward),
        ("первое непрочитанное", |a| {
            let g = a.chats[2].id;
            a.new_from.insert(g, 14);
            a.new_from.insert(a.chats[0].id, 99_999);
        }),
        ("фильтр списка", |a| a.filter = "несуществующее".into()),
        ("закреплённое", |a| {
            let g = a.chats[2].id;
            a.pinned_msg.insert(g, "Закреплённое ".repeat(60));
        }),
        ("тема форума", |a| {
            let g = a.chats[2].id;
            a.topic = Some((g, 5, "Тема ".repeat(50)));
        }),
        ("помощь", |a| a.open_help()),
        ("помощь, листание далеко", |a| {
            a.open_help();
            a.help_top = 1_000_000;
        }),
        ("быстрый переход", |a| a.open_quick()),
        ("быстрый переход, запрос", |a| {
            a.open_quick();
            if let Some(q) = a.quick.as_mut() {
                q.query = "日本語 запрос ".repeat(20);
                q.sel = 99_999;
            }
        }),
        ("блокировка", |a| {
            a.lock_error = "очень длинная ошибка ".repeat(20);
            a.lock_input = "пароль".repeat(100);
            a.lock_until = Some(Instant::now() + Duration::from_secs(60));
            a.mode = Mode::Locked;
        }),
        ("новый пароль 1", |a| {
            a.pw_stage = 0;
            a.mode = Mode::NewPassword;
        }),
        ("новый пароль 2", |a| {
            a.pw_stage = 1;
            a.lock_input = "x".repeat(500);
            a.mode = Mode::NewPassword;
        }),
        ("фото", |a| {
            a.preview = Some(Preview { w: 4, h: 4, rgb: vec![128; 48] });
            a.mode = Mode::Preview;
        }),
        ("фото 1x1", |a| {
            a.preview = Some(Preview { w: 1, h: 1, rgb: vec![255, 0, 0] });
            a.mode = Mode::Preview;
        }),
        ("фото пустое", |a| {
            a.preview = Some(Preview { w: 0, h: 0, rgb: Vec::new() });
            a.mode = Mode::Preview;
        }),
        ("фото большое", |a| {
            a.preview = Some(Preview { w: 640, h: 480, rgb: vec![90; 640 * 480 * 3] });
            a.mode = Mode::Preview;
        }),
        ("фото без точек", |a| {
            a.preview = Some(Preview { w: 50, h: 50, rgb: Vec::new() });
            a.mode = Mode::Preview;
        }),
        ("фото нет вовсе", |a| a.mode = Mode::Preview),
        ("файл", |a| {
            a.viewer = Some(Viewer {
                name: "очень-длинное-имя-файла.txt".repeat(8),
                lines: (0..300).map(|i| format!("строка {i} 日本語 😀")).collect(),
                top: 5,
            });
            a.mode = Mode::Viewer;
        }),
        ("файл пустой", |a| {
            a.viewer = Some(Viewer { name: String::new(), lines: Vec::new(), top: 0 });
            a.mode = Mode::Viewer;
        }),
        ("файл, строка за концом", |a| {
            a.viewer = Some(Viewer { name: "a.txt".into(), lines: vec!["x".into()], top: 1_000_000 });
            a.mode = Mode::Viewer;
        }),
        ("файла нет вовсе", |a| a.mode = Mode::Viewer),
    ];
    let mut failures: Vec<String> = Vec::new();
    let mut drawn = 0usize;
    for view in 0..=2u8 {
        for (name, setup) in &setups {
            let mut app = stress_app(view);
            setup(&mut app);
            let result = catch_unwind(AssertUnwindSafe(|| {
                for &(w, h) in &sizes {
                    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                    term.draw(|f| draw(f, &mut app)).unwrap();
                }
            }));
            drawn += sizes.len();
            if result.is_err() {
                failures.push(format!("вид {view}, «{name}»"));
            }
        }
        // Настройки: каждая строка и каждый вид перечислений.
        for row in 0..SETTINGS_ROWS + 3 {
            let mut app = stress_app(view);
            app.settings_row = row;
            app.mode = Mode::Settings;
            let result = catch_unwind(AssertUnwindSafe(|| {
                for &(w, h) in &sizes {
                    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                    term.draw(|f| draw(f, &mut app)).unwrap();
                }
            }));
            drawn += sizes.len();
            if result.is_err() {
                failures.push(format!("вид {view}, настройки, строка {row}"));
            }
        }
        // Списки поверх чата, у каждого — длинные подписи и выбор за пределами списка.
        for kind in [
            OverlayKind::Links,
            OverlayKind::Scheduled,
            OverlayKind::Faq,
            OverlayKind::Search,
            OverlayKind::Buttons,
            OverlayKind::Poll,
            OverlayKind::Topics,
            OverlayKind::Drafts,
        ] {
            for (count, sel) in [(0usize, 0usize), (1, 0), (5, 3), (80, 79), (3, 999)] {
                let mut app = stress_app(view);
                let items: Vec<OverlayItem> = (0..count)
                    .map(|i| OverlayItem {
                        label: format!("пункт {i} {}", "日本語 длинная подпись ".repeat(6)),
                        data: "https://example.com".into(),
                        id: i as i32,
                        target: None,
                    })
                    .collect();
                app.open_overlay(kind, &"Заголовок ".repeat(12), items);
                if let Some(o) = app.overlay.as_mut() {
                    o.sel = sel;
                    o.confirm = Some(sel);
                }
                let result = catch_unwind(AssertUnwindSafe(|| {
                    for &(w, h) in &sizes {
                        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                        term.draw(|f| draw(f, &mut app)).unwrap();
                    }
                }));
                drawn += sizes.len();
                if result.is_err() {
                    failures.push(format!("вид {view}, список {kind:?}, {count} пунктов, выбран {sel}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "отрисовка упала ({} отрисовок прошло):\n{}", drawn, failures.join("\n"));
    assert!(drawn > 5000, "{drawn}");
}

// ------------------------------------------------------------------ случайный «обстрел»: клавиши, мышь и события Telegram

/// Простой детерминированный генератор: один и тот же прогон повторяется, и найденное падение можно воспроизвести.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

fn fuzz_msg(rng: &mut Rng) -> ChatMsg {
    use crate::tg::{Btn, BtnAction, MediaInfo, MediaKind, PollOpt, PollView, Reaction};
    let id = rng.below(80) as i32 + 1;
    let sender = *rng.pick(&["Аня", "Борис", "", "日本語"]);
    let text =
        *rng.pick(&["", "текст", "длинный текст со ссылкой https://example.com/x и ещё слова", "строка 1\nстрока 2", "😀👍🏽 e\u{301}"]);
    let mut m = msg(id, rng.below(2) == 0, sender, text);
    if rng.below(4) == 0 {
        m.reply_to = Some(rng.below(80) as i32 + 1);
    }
    if rng.below(5) == 0 {
        m.media = Some(MediaInfo { kind: MediaKind::Photo, name: "a.jpg".into(), size: Some(5) });
    }
    if rng.below(6) == 0 {
        m.buttons = vec![vec![Btn { text: "Да".into(), action: BtnAction::Callback(vec![1]) }]];
    }
    if rng.below(8) == 0 {
        m.poll = Some(PollView {
            question: "Q".into(),
            options: vec![PollOpt { text: "A".into(), data: vec![0], voters: 1, chosen: false, correct: false }],
            ..Default::default()
        });
    }
    if rng.below(6) == 0 {
        m.reactions = vec![Reaction { emoji: "👍".into(), count: 2, mine: false }];
    }
    m
}

fn fuzz_msgs(rng: &mut Rng) -> Vec<ChatMsg> {
    let n = rng.below(7);
    let mut v: Vec<ChatMsg> = (0..n).map(|_| fuzz_msg(rng)).collect();
    v.sort_by_key(|m| m.id);
    v.dedup_by_key(|m| m.id);
    v
}

/// Событие от Telegram (кроме открытия файла программой): чаще осмысленное, иногда о чате, которого нет в списке.
fn fuzz_event(rng: &mut Rng, app: &App) -> (u8, Event) {
    use crate::tg::{ScheduledMsg, SearchHit, TypingKind};
    let mut ids: Vec<PeerId> = app.chats.iter().map(|c| c.id).collect();
    ids.push(PeerId::user(999).unwrap());
    let chat = *rng.pick(&ids);
    let n = rng.below(33) as u8;
    let ev = match n {
        0 => Event::Dialogs(app.chats.iter().filter(|_| rng.below(4) != 0).cloned().collect()),
        1 => Event::DialogsPage(app.chats.iter().filter(|_| rng.below(3) != 0).cloned().collect()),
        2 => Event::History { chat, msgs: fuzz_msgs(rng) },
        3 | 4 => {
            let m = fuzz_msg(rng);
            Event::Incoming { chat, chat_name: "Имя".into(), msg: m }
        }
        5 => Event::Sent { chat, msg: fuzz_msg(rng), scheduled: rng.below(3) == 0 },
        6 => Event::SendFailed { chat, text: "не ушло\nтекст".into() },
        7 => Event::Edited { chat, msg: fuzz_msg(rng) },
        8 => {
            let ids: Vec<i32> = (0..rng.below(8)).map(|_| rng.below(80) as i32 + 1).collect();
            Event::Deleted { chat: if rng.below(2) == 0 { Some(chat) } else { None }, ids }
        }
        9 => Event::Presence { user: chat, presence: crate::tg::Presence::Recently },
        10 => {
            Event::Typing { chat, from: Some(*rng.pick(&ids)), kind: *rng.pick(&[TypingKind::Text, TypingKind::Voice, TypingKind::Stop]) }
        }
        11 => Event::Pinned { chat, text: if rng.below(2) == 0 { Some("закреплено".into()) } else { None } },
        12 => Event::Scheduled {
            chat,
            items: (0..rng.below(4))
                .map(|i| ScheduledMsg { id: i as i32 + 1, date: 1_800_000_000 + i as i64, text: "позже".into() })
                .collect(),
        },
        13 => Event::Older { chat, msgs: fuzz_msgs(rng) },
        14 => {
            let msgs = fuzz_msgs(rng);
            let focus = msgs.first().map_or(1, |m| m.id);
            Event::Around { chat, msgs, focus }
        }
        15 => Event::SearchResults {
            query: "запрос".into(),
            scope: if rng.below(2) == 0 { Some(chat) } else { None },
            hits: (0..rng.below(4))
                .map(|i| SearchHit {
                    chat,
                    chat_name: "Чат".into(),
                    id: i as i32 + 1,
                    ts: 1_800_000_000,
                    when: "10:00".into(),
                    sender: "Аня".into(),
                    text: "найдено\nздесь".into(),
                })
                .collect(),
        },
        16 => Event::TextFile { chat, name: "a.txt".into(), lines: (0..rng.below(5)).map(|i| format!("строка {i}")).collect() },
        17 => {
            let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0];
            jpeg.extend((0..rng.below(300)).map(|_| rng.below(256) as u8));
            Event::Preview { chat, jpeg }
        }
        18 => Event::Latency(if rng.below(2) == 0 { Some(rng.below(900) as u32) } else { None }),
        19 => Event::Queued { chat, text: "в очереди".into(), count: rng.below(3) },
        20 => Event::QueueSent { chat, text: "в очереди".into(), left: rng.below(3) },
        21 => Event::QueueDropped { chat, text: "в очереди".into(), reason: "причина".into() },
        22 => Event::Reacted { chat, msg: fuzz_msg(rng) },
        23 => Event::ButtonAnswer(if rng.below(2) == 0 { String::new() } else { "ответ бота".into() }),
        24 => Event::ReadOutbox { chat, max_id: rng.below(80) as i32 },
        25 => Event::Topics { chat, items: (0..rng.below(4)).map(|i| (i as i32 + 1, "Тема".to_string(), i as i32)).collect() },
        26 => Event::PinnedList {
            chat,
            hits: (0..rng.below(3))
                .map(|i| SearchHit {
                    chat,
                    chat_name: "Чат".into(),
                    id: i as i32 + 1,
                    ts: 1_800_000_000,
                    when: "10:00".into(),
                    sender: "Аня".into(),
                    text: "закреп".into(),
                })
                .collect(),
        },
        27 => Event::Found(chat_for_fuzz(rng)),
        28 => Event::Left(chat),
        29 => Event::Draft { chat, text: if rng.below(2) == 0 { "черновик".into() } else { String::new() } },
        30 => Event::Status("статус".into()),
        31 => Event::Error("ошибка".into()),
        _ => Event::NetCheck(vec!["DNS: ok".into(), "DC2: 120 мс".into()]),
    };
    (n, ev)
}

fn chat_for_fuzz(rng: &mut Rng) -> ChatInfo {
    let n = rng.below(4) as i64;
    chat(60 + n, "Найденный чат", if n % 2 == 0 { Kind::Channel } else { Kind::User }, n as i32)
}

fn fuzz_key(rng: &mut Rng) -> KeyEvent {
    let chars: Vec<char> =
        "абвгдеёжзийклмнопрстуфхцчшщъыьэюя ABCDEFGHIJKLMNOPQRSTUVWXYZ abcdefghijklmnopqrstuvwxyz 0123456789 /?!.,日本😀".chars().collect();
    let codes = [
        KeyCode::Enter,
        KeyCode::Esc,
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::Insert,
        KeyCode::F(1),
        KeyCode::F(2),
        KeyCode::F(3),
        KeyCode::F(4),
        KeyCode::F(5),
        KeyCode::F(6),
        KeyCode::F(7),
        KeyCode::F(8),
        KeyCode::F(9),
        KeyCode::F(11),
        KeyCode::F(12),
    ];
    let mods = *rng.pick(&[
        KeyModifiers::NONE,
        KeyModifiers::NONE,
        KeyModifiers::NONE,
        KeyModifiers::SHIFT,
        KeyModifiers::CONTROL,
        KeyModifiers::ALT,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ]);
    if rng.below(100) < 45 {
        let c = *rng.pick(&chars);
        // Управляющие сочетания: буква с Ctrl или Alt; без них — обычный набор.
        KeyEvent::new(KeyCode::Char(c), if mods == KeyModifiers::SHIFT { KeyModifiers::NONE } else { mods })
    } else {
        KeyEvent::new(*rng.pick(&codes), mods)
    }
}

fn fuzz_mouse(rng: &mut Rng) -> MouseEvent {
    let kind = *rng.pick(&[
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Down(MouseButton::Right),
        MouseEventKind::Up(MouseButton::Left),
        MouseEventKind::Drag(MouseButton::Left),
        MouseEventKind::ScrollUp,
        MouseEventKind::ScrollDown,
        MouseEventKind::Moved,
    ]);
    MouseEvent { kind, column: rng.below(130) as u16, row: rng.below(45) as u16, modifiers: KeyModifiers::NONE }
}

/// Что должно быть верным в любой момент, как бы ни нажимали клавиши и какие бы события ни приходили.
fn fuzz_invariants(app: &App) -> Result<(), String> {
    macro_rules! ensure {
        ($cond:expr, $($msg:tt)+) => {
            if !$cond {
                return Err(format!($($msg)+));
            }
        };
    }
    let chars = app.input.chars().count();
    ensure!(app.cursor <= chars, "курсор {} за концом поля ввода ({chars} символов)", app.cursor);
    if let Some(id) = app.sel {
        ensure!(app.chats.iter().any(|c| c.id == id), "выбран чат, которого нет в списке");
    }
    ensure!(app.settings_row < SETTINGS_ROWS, "строка настроек {} за пределами", app.settings_row);
    ensure!(app.panes.open.len() <= 14, "открыто вкладок: {}", app.panes.open.len());
    match app.mode {
        Mode::Overlay => ensure!(app.overlay.is_some(), "режим списка без списка"),
        Mode::Quick => ensure!(app.quick.is_some(), "быстрый переход без окна"),
        Mode::Viewer => ensure!(app.viewer.is_some(), "режим чтения без файла"),
        Mode::Preview => ensure!(app.preview.is_some(), "режим фото без фото"),
        Mode::Forward => ensure!(app.fwd_src.is_some(), "режим пересылки без сообщения"),
        Mode::Pick => {
            ensure!(app.sel.is_some(), "выбор сообщения без чата");
            let len = app.sel.and_then(|id| app.histories.get(&id)).map_or(0, Vec::len);
            ensure!(len == 0 || app.pick < len, "выбрано сообщение {} из {len}", app.pick);
        }
        _ => {}
    }
    if app.mode == Mode::Overlay
        && let Some(o) = &app.overlay
    {
        ensure!(o.items.is_empty() || o.sel < o.items.len(), "выбран пункт списка {} из {}", o.sel, o.items.len());
    }
    if app.mode == Mode::Quick
        && let Some(q) = &app.quick
    {
        ensure!(q.items.is_empty() || q.sel < q.items.len(), "выбран пункт быстрого перехода {} из {}", q.sel, q.items.len());
    }
    Ok(())
}

#[test]
fn random_keys_mouse_and_events_never_panic_or_break_the_state() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    // Команды без побочных действий на диске и в сети; со случайными хвостами.
    let commands = [
        "/find",
        "/findmsg",
        "/findall",
        "/latest",
        "/pinned",
        "/topics",
        "/pin",
        "/chat",
        "/join",
        "/leave",
        "/readall",
        "/note",
        "/edit",
        "/del",
        "/later 1",
        "/md",
        "/lock",
        "/settings",
        "/refresh",
        "/help",
        "/search",
        "/links",
        "/scheduled",
        "/faq",
        "/view",
        "/view panes",
        "/view messenger",
        "/mute",
        "/dnd",
        "/dnd 30",
        "/dnd off",
        "/next",
        "/drafts",
        "/send",
        "/album",
        "/netcheck",
    ];
    let tails = ["", " ", " а", " 1", " 日本語", " @x", " https://example.com", " 999999999999", " -5", " /"];
    let sizes: [(u16, u16); 8] = [(80, 24), (120, 30), (40, 12), (20, 6), (100, 40), (64, 16), (3, 3), (200, 50)];
    // По умолчанию прогон короткий; для долгого поиска редких ошибок: TGL_FUZZ_SEEDS=300 TGL_FUZZ_STEPS=2000 cargo test.
    let env_num = |name: &str, default: u64| std::env::var(name).ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(default);
    let (seeds, steps) = (env_num("TGL_FUZZ_SEEDS", 8), env_num("TGL_FUZZ_STEPS", 600));
    let trace = std::env::var_os("TGL_FUZZ_TRACE").is_some();
    let mut failures: Vec<String> = Vec::new();
    // TGL_FUZZ_ONLY=вид/зерно — повторить только один прогон (вместе с TGL_FUZZ_TRACE видно все шаги).
    let only = std::env::var("TGL_FUZZ_ONLY").ok();
    for view in 0..=2u8 {
        for seed in 1..=seeds {
            if only.as_ref().is_some_and(|o| *o != format!("{view}/{seed}")) {
                continue;
            }
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ (seed * 0x1000_0001 + u64::from(view) * 7919));
            let mut app = stress_app(view);
            let (tx, mut rx) = mpsc::unbounded_channel();
            let s = saver();
            let mut last = String::from("начало");
            let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
                for step in 0..steps {
                    let action = rng.below(20);
                    last = match action {
                        0..=9 => {
                            if rng.below(2) == 0 {
                                slow(&mut app);
                            }
                            let k = fuzz_key(&mut rng);
                            // Значения настроек не перебираем: каждое сохранение — отдельный Argon2 в фоне.
                            if app.mode == Mode::Settings && matches!(k.code, KeyCode::Left | KeyCode::Right | KeyCode::Enter) {
                                continue;
                            }
                            // Смена пароля тоже сохраняет хранилище; её отдельно проверяют другие тесты.
                            if app.mode == Mode::NewPassword && k.code == KeyCode::Enter {
                                continue;
                            }
                            if trace {
                                eprintln!("{view}/{seed}/{step}: клавиша {k:?} в режиме {:?}", app.mode);
                            }
                            app.handle_key(k, &tx, &s);
                            format!("клавиша {k:?}")
                        }
                        10..=11 => {
                            let m = fuzz_mouse(&mut rng);
                            if trace {
                                eprintln!("{view}/{seed}/{step}: мышь {m:?} в режиме {:?}", app.mode);
                            }
                            app.handle_mouse(m, &tx);
                            format!("мышь {m:?}")
                        }
                        12..=15 => {
                            let (n, ev) = fuzz_event(&mut rng, &app);
                            if trace {
                                eprintln!("{view}/{seed}/{step}: событие №{n} в режиме {:?}", app.mode);
                            }
                            app.handle_event(ev, &tx);
                            format!("событие №{n}")
                        }
                        16 | 17 => {
                            let line = format!("{}{}", rng.pick(&commands), rng.pick(&tails));
                            if trace {
                                eprintln!("{view}/{seed}/{step}: команда {line:?} в режиме {:?}", app.mode);
                            }
                            if app.mode == Mode::Chat {
                                submit_text(&mut app, &line, &tx, &s);
                            }
                            format!("команда {line:?}")
                        }
                        18 => {
                            // Выйти из блокировки, чтобы не застрять на экране пароля до конца прогона.
                            if app.mode == Mode::Locked {
                                app.lock_until = None;
                                let pw = s.password();
                                type_text(&mut app, &pw, &tx, &s);
                                slow(&mut app);
                                app.handle_key(key(KeyCode::Enter, KeyModifiers::NONE), &tx, &s);
                            }
                            "разблокировка".to_string()
                        }
                        _ => {
                            let (w, h) = *rng.pick(&sizes);
                            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                            term.draw(|f| draw(f, &mut app)).unwrap();
                            format!("отрисовка {w}x{h}")
                        }
                    };
                    while rx.try_recv().is_ok() {}
                    fuzz_invariants(&app).map_err(|e| format!("шаг {step}, после «{last}»: {e}"))?;
                }
                Ok(())
            }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => failures.push(format!("вид {view}, зерно {seed}: {e}")),
                Err(_) => failures.push(format!("вид {view}, зерно {seed}: паника после «{last}»")),
            }
        }
    }
    assert!(failures.is_empty(), "обстрел нашёл:\n{}", failures.join("\n"));
}

#[test]
fn poll_percent_is_always_between_0_and_100_whatever_the_server_sends() {
    use super::text::poll_percent;
    assert_eq!((poll_percent(1, 4), poll_percent(4, 4), poll_percent(0, 4)), (25, 100, 0));
    assert_eq!(poll_percent(3, 0), 0, "общее число неизвестно");
    assert_eq!(poll_percent(3, -7), 0);
    assert_eq!(poll_percent(-1, 5), 0, "голосов меньше нуля");
    assert_eq!(poll_percent(500, 5), 100, "вариант больше общего числа");
    // Раньше от умножения на 100 переполнялся i32, и полоска «растягивалась» на миллиарды знаков (программа падала).
    assert_eq!(poll_percent(30_000_000, 40_000_000), 75);
    assert_eq!(poll_percent(i32::MAX, i32::MAX), 100);
    assert_eq!(poll_percent(i32::MAX, 1), 100);
}

#[test]
fn a_poll_with_absurd_numbers_still_draws() {
    use crate::tg::{PollOpt, PollView};
    let mut app = app_in_view(0);
    let g = app.chats[2].id;
    let mut m = msg(500, false, "Аня", "опрос");
    m.poll = Some(PollView {
        question: "Вопрос?".into(),
        total: 5,
        options: vec![
            PollOpt { text: "минус".into(), data: vec![0], voters: -1, chosen: false, correct: false },
            PollOpt { text: "много".into(), data: vec![1], voters: i32::MAX, chosen: true, correct: false },
            PollOpt { text: "обычный".into(), data: vec![2], voters: 3, chosen: false, correct: true },
        ],
        ..Default::default()
    });
    app.histories.insert(g, vec![m]);
    app.sel = Some(g);
    let text = draw_text(&mut app, 100, 30);
    assert!(text.contains("минус") && text.contains("много") && text.contains("100%"), "{text}");
}
