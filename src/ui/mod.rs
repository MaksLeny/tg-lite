//! Терминальный интерфейс: вкладки папок сверху, список чатов слева, переписка справа,
//! поле ввода внизу. Автоблокировка по бездействию: пока заблокировано, переписка не рисуется.
//!
//! Клавиши сделаны так, чтобы работать в обычной консоли Windows:
//! F-клавиши и /команды не зависят от раскладки, а Ctrl-комбинации — запасной вариант.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
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
    "/findmsg",
    "/findall",
    "/latest",
    "/pinned",
    "/pin",
    "/chat",
    "/join",
    "/leave",
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
    "/open",
    "/copy",
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
                "F4 → «Звук при новом сообщении»: включить; сигнал идёт через звуковую карту, проверьте громкость",
                "F4 → \"Sound on new message\": on; the beep goes through the sound card, check the volume"
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
    /// Результаты поиска сообщений.
    Search,
}

#[derive(Default)]
struct OverlayItem {
    label: String,
    /// Для ссылок — адрес.
    data: String,
    /// Для отложенных — номер сообщения.
    id: i32,
    /// Для результатов поиска: чат, номер и время найденного сообщения.
    target: Option<(PeerId, i32, i64)>,
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

/// Больше этого числа сообщений одного чата в памяти не копим: дальше помогает поиск.
const MAX_LOADED: usize = 1500;

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

const SETTINGS_ROWS: usize = 14;

pub struct App {
    chats: Vec<ChatInfo>,
    /// Выбранный чат хранится по id, а не по номеру: так выбор не «съезжает» при обновлении списка.
    sel: Option<PeerId>,
    list_state: ListState,
    /// Области для мыши, заполняются при отрисовке: вкладки (x от, x до, вкладка), список чатов, лента сообщений.
    hit_tabs: Vec<(u16, u16, Tab)>,
    hit_list: Rect,
    hit_msgs: Rect,
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
    /// Наибольшая прокрутка переписки по последней отрисовке (ставит `draw`).
    max_scroll: usize,
    /// Идёт подгрузка старых сообщений.
    older_busy: bool,
    /// Чаты, у которых загружена вся история.
    older_done: HashSet<PeerId>,
    /// Чат, в котором показан кусок старой переписки после перехода из поиска (новые сообщения в него не добавляются).
    jumped: Option<PeerId>,
    /// Найденное сообщение, которое выделено и показано посередине.
    focus_msg: Option<i32>,
    /// Надо ли при следующей отрисовке поставить `focus_msg` посередине.
    focus_pending: bool,
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
    /// Отправленные сообщения и команды этого сеанса (только в памяти) для перелистывания стрелками.
    input_history: Vec<String>,
    /// Позиция в истории: `None`, пока человек пишет своё.
    hist_pos: Option<usize>,
    /// Недописанный текст, который вернётся, если дойти стрелкой вниз до конца истории.
    hist_draft: String,
    pending_load: Option<Instant>,
    reload_pending: bool,
    should_quit: bool,
    /// Поиск по названиям чатов (`/find`); пусто — без фильтра.
    filter: String,
    /// Черновики по чатам: переключение чата не теряет набранный текст (только в памяти).
    drafts: HashMap<PeerId, (String, usize)>,
    /// Сообщение, ожидающее подтверждения удаления (второй `/del`).
    pending_delete: Option<(PeerId, i32)>,
    /// Чат, для которого уже набрано первое /leave и ждём повторного.
    pending_leave: Option<PeerId>,
    /// Что сейчас лежит в Telegram как черновик каждого чата (чтобы не отправлять одно и то же).
    cloud_drafts: HashMap<PeerId, String>,
    /// Черновики, которые пора отправить в Telegram (разбирается на каждом такте).
    draft_out: Vec<(PeerId, String)>,
    /// Что набрано в поле и с какого момента не менялось: через пару секунд черновик уходит в Telegram.
    draft_watch: (Option<PeerId>, String, Instant),
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

// ------------------------------------------------------------------ состояние

impl App {
    fn new(settings: Settings) -> Self {
        App {
            chats: Vec::new(),
            sel: None,
            list_state: ListState::default(),
            hit_tabs: Vec::new(),
            hit_list: Rect::default(),
            hit_msgs: Rect::default(),
            tab: Tab::All,
            folders: Vec::new(),
            histories: HashMap::new(),
            input: String::new(),
            cursor: 0,
            status: tr!("Загружаю чаты…", "Loading chats…").into(),
            markdown: false,
            scroll: 0,
            max_scroll: 0,
            older_busy: false,
            older_done: HashSet::new(),
            jumped: None,
            focus_msg: None,
            focus_pending: false,
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
            input_history: Vec::new(),
            hist_pos: None,
            hist_draft: String::new(),
            pending_load: None,
            reload_pending: false,
            should_quit: false,
            filter: String::new(),
            drafts: HashMap::new(),
            pending_delete: None,
            pending_leave: None,
            cloud_drafts: HashMap::new(),
            draft_out: Vec::new(),
            draft_watch: (None, String::new(), Instant::now()),
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

    /// Делает чат выбранным и видимым, даже если он не входит в открытую вкладку.
    fn reveal_chat(&mut self, id: PeerId) {
        if !self.visible_ids().contains(&id) {
            self.tab = Tab::All;
            self.filter.clear();
        }
        self.set_sel(Some(id));
    }

    /// Вливает свежую первую страницу чатов в список: порядок сервера сверху, остальные чаты без изменений ниже.
    fn merge_dialogs(&mut self, page: Vec<ChatInfo>) {
        let ids: HashSet<PeerId> = page.iter().map(|c| c.id).collect();
        let mut merged = page;
        merged.extend(std::mem::take(&mut self.chats).into_iter().filter(|c| !ids.contains(&c.id)));
        self.chats = merged;
    }

    /// Просит старые сообщения выбранного чата (PgUp у самого верха переписки).
    fn request_older(&mut self, cmd: &mpsc::UnboundedSender<Cmd>) {
        let (Some(chat), Some(id)) = (self.selected().cloned(), self.sel) else { return };
        let Some(h) = self.histories.get(&id) else { return };
        let Some(first) = h.first() else { return };
        if self.older_busy {
            return;
        }
        if self.older_done.contains(&id) {
            self.status = tr!("Это начало переписки", "This is the start of the conversation").into();
            return;
        }
        if h.len() >= MAX_LOADED {
            self.status = tr!(
                "Загружено максимум сообщений: нужное найдите через /findmsg",
                "Loaded the maximum number of messages: use /findmsg to find the one you need"
            )
            .into();
            return;
        }
        self.older_busy = true;
        self.status = tr!("Загружаю старые сообщения…", "Loading older messages…").into();
        let _ = cmd.send(Cmd::LoadOlder { chat, before: first.id, limit: (self.settings.history_limit as usize).max(30) });
    }

    /// Возвращает из куска старой переписки к последним сообщениям.
    fn go_latest(&mut self, cmd: &mpsc::UnboundedSender<Cmd>) {
        if self.jumped.take().is_none() {
            return;
        }
        self.focus_msg = None;
        self.scroll = 0;
        if let Some(c) = self.selected().cloned() {
            let _ = cmd.send(Cmd::LoadHistory(c, self.settings.history_limit as usize));
            self.status = tr!("Возвращаюсь к последним сообщениям…", "Going back to the latest messages…").into();
        }
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
                // Открытый чат остаётся в списке «Новые», даже когда он прочитан: иначе выбор «прыгает».
                (self.in_tab(&self.tab, c) || (self.tab == Tab::Unread && Some(c.id) == self.sel))
                    && (q.is_empty() || c.name.to_lowercase().contains(&q))
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
        let was_editing = self.edit_target.take().is_some();
        if was_editing {
            self.input.clear(); // текст правки не должен стать черновиком нового сообщения
            self.cursor = 0;
        }
        self.reply = None;
        if let Some(old) = self.sel {
            if !was_editing {
                let text = self.input.clone();
                self.queue_draft(old, &text);
            }
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
        if let Some(j) = self.jumped.take_if(|j| Some(*j) != new) {
            // Ушли из чата с куском старой переписки: при возвращении загрузим свежую.
            self.histories.remove(&j);
            self.focus_msg = None;
        }
        self.older_busy = false;
        self.sel = new;
        self.pending_delete = None;
        self.pending_leave = None;
    }

    /// Ставит черновик в очередь на отправку в Telegram, если он изменился. Команды («/…») не отправляем.
    fn queue_draft(&mut self, id: PeerId, text: &str) {
        if !self.settings.cloud_drafts || self.mode == Mode::Locked {
            return;
        }
        let text = if text.trim().is_empty() || text.starts_with('/') { "" } else { text };
        if self.cloud_drafts.get(&id).map_or("", String::as_str) == text {
            return;
        }
        if text.is_empty() {
            self.cloud_drafts.remove(&id);
        } else {
            self.cloud_drafts.insert(id, text.to_string());
        }
        self.draft_out.push((id, text.to_string()));
    }

    /// Раз в такт: замечает, что поле ввода давно не менялось, и отправляет черновик; выдаёт накопленную очередь.
    fn flush_drafts(&mut self, cmd: &mpsc::UnboundedSender<Cmd>) {
        const IDLE: Duration = Duration::from_secs(4);
        if self.mode == Mode::Chat && self.edit_target.is_none() {
            if self.draft_watch.0 != self.sel || self.draft_watch.1 != self.input {
                self.draft_watch = (self.sel, self.input.clone(), Instant::now());
            } else if self.draft_watch.2.elapsed() >= IDLE
                && let Some(id) = self.sel
            {
                let text = self.input.clone();
                self.queue_draft(id, &text);
            }
        }
        for (id, text) in std::mem::take(&mut self.draft_out) {
            if let Some(chat) = self.chats.iter().find(|c| c.id == id).cloned() {
                let _ = cmd.send(Cmd::SaveDraft { chat, text });
            }
        }
    }

    /// Подхватывает черновики из списка чатов (набранные на телефоне).
    fn sync_server_drafts(&mut self) {
        let found: Vec<(PeerId, String)> = self.chats.iter().filter(|c| !c.draft.is_empty()).map(|c| (c.id, c.draft.clone())).collect();
        for (id, text) in found {
            self.apply_server_draft(id, &text, false);
        }
    }

    /// Принимает черновик, сохранённый в Telegram на другом устройстве. Набранное здесь не затирается.
    fn apply_server_draft(&mut self, id: PeerId, text: &str, from_update: bool) {
        if !self.settings.cloud_drafts {
            return;
        }
        let prev = self.cloud_drafts.get(&id).cloned().unwrap_or_default();
        if text == prev || (text.is_empty() && !from_update) {
            return; // совпадает с известным; пустой черновик из списка чатов мог устареть и ничего не стирает
        }
        if text.is_empty() {
            self.cloud_drafts.remove(&id);
        } else {
            self.cloud_drafts.insert(id, text.to_string());
        }
        if self.sel == Some(id) {
            if self.edit_target.is_none() && (self.input.is_empty() || self.input == prev) {
                self.input = text.to_string();
                self.cursor = self.input.chars().count();
            }
        } else {
            match self.drafts.get(&id) {
                Some((t, _)) if *t != prev => {} // здесь набрано другое: не трогаем
                _ if text.is_empty() => {
                    self.drafts.remove(&id);
                }
                _ => {
                    self.drafts.insert(id, (text.to_string(), text.chars().count()));
                }
            }
        }
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
        util::clear_view_dir(); // файлы, открытые через /open, не должны лежать на диске при блокировке
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
        self.hist_pos = None;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            let b = self.byte_idx(self.cursor - 1);
            self.input.remove(b);
            self.cursor -= 1;
            self.hist_pos = None;
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.input_len() {
            let b = self.byte_idx(self.cursor);
            self.input.remove(b);
            self.hist_pos = None;
        }
    }

    fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
        self.edit_target = None;
        self.hist_pos = None;
    }

    /// Запоминает отправленный текст для стрелки вверх. Строки с адресом прокси и паролем не сохраняются.
    fn remember_input(&mut self, text: &str) {
        if text.is_empty() || (text.starts_with("/proxy") && text.contains('@')) {
            return;
        }
        if self.input_history.last().map(String::as_str) != Some(text) {
            self.input_history.push(text.to_string());
            if self.input_history.len() > 100 {
                self.input_history.remove(0);
            }
        }
        self.hist_pos = None;
    }

    /// Стрелка вверх (`dir = -1`) и вниз (`dir = 1`) по истории ввода.
    fn history_step(&mut self, dir: i32) {
        if self.input_history.is_empty() {
            return;
        }
        let last = self.input_history.len() - 1;
        let next = match (self.hist_pos, dir) {
            (None, d) if d < 0 => {
                self.hist_draft = self.input.clone();
                Some(last)
            }
            (None, _) => return,
            (Some(i), d) if d < 0 => Some(i.saturating_sub(1)),
            (Some(i), _) if i < last => Some(i + 1),
            (Some(_), _) => None,
        };
        match next {
            Some(i) => {
                self.input = self.input_history[i].clone();
                self.cursor = self.input_len();
                self.hist_pos = Some(i);
            }
            None => {
                self.input = std::mem::take(&mut self.hist_draft);
                self.cursor = self.input_len();
                self.hist_pos = None;
            }
        }
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
            Event::DialogsPage(page) => {
                self.merge_dialogs(page);
                self.sync_server_drafts();
                let keys: Vec<PeerId> = self.histories.keys().copied().collect();
                for k in keys {
                    self.resolve_fwd(k);
                }
                self.reload_pending = false;
                self.ensure_selection();
                if let Some(id) = self.sel
                    && !self.histories.contains_key(&id)
                    && self.pending_load.is_none()
                {
                    self.pending_load = Some(Instant::now());
                }
            }
            Event::Older { chat, msgs } => {
                self.older_busy = false;
                let Some(h) = self.histories.get_mut(&chat) else { return };
                let first = h.first().map_or(i32::MAX, |m| m.id);
                let new: Vec<ChatMsg> = msgs.into_iter().filter(|m| m.id < first).collect();
                if new.is_empty() {
                    self.older_done.insert(chat);
                    self.status = tr!("Это начало переписки", "This is the start of the conversation").into();
                    return;
                }
                let added = new.len();
                self.status = trf!("Загружено ещё сообщений: {added}", "Older messages loaded: {added}");
                h.splice(0..0, new);
                self.resolve_fwd(chat);
                if self.mode == Mode::Pick && self.sel == Some(chat) {
                    self.pick += added; // выбранное сообщение остаётся тем же
                }
            }
            Event::Around { chat, msgs, focus } => {
                if self.mode == Mode::Locked {
                    return;
                }
                if !msgs.iter().any(|m| m.id == focus) {
                    self.status = tr!("Не удалось открыть найденное сообщение", "Could not open the found message").into();
                    return;
                }
                self.reveal_chat(chat);
                self.histories.insert(chat, msgs);
                self.older_done.remove(&chat);
                self.resolve_fwd(chat);
                self.jumped = Some(chat);
                self.focus_msg = Some(focus);
                self.focus_pending = true;
                self.mode = Mode::Chat;
                self.status = tr!(
                    "Найденное сообщение выделено. PgDn или Esc — к последним",
                    "Found message highlighted. PgDn or Esc: back to the latest"
                )
                .into();
            }
            Event::SearchResults { query, scope, hits } => {
                if self.mode == Mode::Locked {
                    return;
                }
                if hits.is_empty() {
                    self.status = trf!("По запросу «{query}» ничего не найдено", "Nothing found for “{query}”");
                    return;
                }
                let items: Vec<OverlayItem> = hits
                    .iter()
                    .map(|h| {
                        let text = h.text.replace('\n', " ");
                        let label = if scope.is_some() {
                            format!("{}  {}: {text}", h.when, h.sender)
                        } else {
                            format!("[{}] {}  {}: {text}", h.chat_name, h.when, h.sender)
                        };
                        OverlayItem { label, target: Some((h.chat, h.id, h.ts)), ..Default::default() }
                    })
                    .collect();
                let title = trf!(" Найдено: {} · «{query}» ", " Found: {} · “{query}” ", items.len());
                self.open_overlay(OverlayKind::Search, &title, items);
            }
            Event::ReadOutbox { chat, max_id } => {
                if let Some(c) = self.chats.iter_mut().find(|c| c.id == chat) {
                    c.read_out = c.read_out.max(max_id);
                }
            }
            Event::PinnedList { chat, hits } => {
                if self.mode == Mode::Locked {
                    return;
                }
                if hits.is_empty() {
                    self.status = tr!("В этом чате нет закреплённых сообщений", "This chat has no pinned messages").into();
                    return;
                }
                let name = self.chats.iter().find(|c| c.id == chat).map(|c| c.name.clone()).unwrap_or_default();
                let items: Vec<OverlayItem> = hits
                    .iter()
                    .map(|h| {
                        let text = h.text.replace('\n', " ");
                        OverlayItem {
                            label: format!("{}  {}: {text}", h.when, h.sender),
                            target: Some((h.chat, h.id, h.ts)),
                            ..Default::default()
                        }
                    })
                    .collect();
                let title = trf!(" Закреплённые: {} · {name} ", " Pinned: {} · {name} ", items.len());
                self.open_overlay(OverlayKind::Search, &title, items);
            }
            Event::Found(info) => {
                if self.mode == Mode::Locked {
                    return;
                }
                let id = info.id;
                let name = info.name.clone();
                if !self.chats.iter().any(|c| c.id == id) {
                    self.chats.insert(0, info);
                }
                self.reveal_chat(id);
                self.scroll = 0;
                self.pending_load = Some(Instant::now());
                self.status = trf!("Открыт чат «{name}»", "Opened chat “{name}”");
            }
            Event::Draft { chat, text } => {
                if self.mode != Mode::Locked {
                    self.apply_server_draft(chat, &text, true);
                }
            }
            Event::Left(id) => {
                self.chats.retain(|c| c.id != id);
                self.histories.remove(&id);
                self.drafts.remove(&id);
                if self.sel == Some(id) {
                    self.sel = None;
                    self.ensure_selection();
                }
                self.status = tr!("Вы вышли из чата", "You left the chat").into();
            }
            Event::Dialogs(list) => {
                self.chats = list;
                self.sync_server_drafts();
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
            Event::History { chat, mut msgs } => {
                if self.jumped == Some(chat) {
                    return; // запоздавший ответ не должен затереть кусок старой переписки
                }
                // Сообщения, пришедшие, пока грузилась история, не теряем.
                if let Some(old) = self.histories.get(&chat) {
                    let last = msgs.last().map_or(0, |m| m.id);
                    msgs.extend(old.iter().filter(|m| m.id > last).cloned());
                }
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
                if self.jumped == Some(chat) {
                    // Показан кусок старой переписки: новое сообщение не вставляем посреди, только отмечаем.
                    if !msg.outgoing {
                        if let Some(c) = self.chats.iter_mut().find(|c| c.id == chat) {
                            c.unread += 1;
                        }
                        self.status = trf!("Новое сообщение: {chat_name} (PgDn — к последним)", "New message: {chat_name} (PgDn: latest)");
                    }
                    return;
                }
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
                    m.raw = if msg.raw.is_empty() { msg.caption.clone().unwrap_or_else(|| msg.text.clone()) } else { msg.raw.clone() };
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
                    // Номера сообщений каналов свои у каждого канала: «общее» удаление их не касается.
                    let applies = match chat {
                        Some(c) => c == *id,
                        None => id.kind() != grammers_client::session::types::PeerKind::Channel,
                    };
                    if applies {
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
                // Ответ мог прийти, пока открыта блокировка или другой экран: ничего не открываем поверх.
                if self.sel != Some(chat) || self.mode != Mode::Chat {
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
                        OverlayItem {
                            label: format!("{when}  {}", m.text.replace('\n', " ")),
                            data: String::new(),
                            id: m.id,
                            ..Default::default()
                        }
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
            Event::Open(path) => {
                if self.mode == Mode::Locked {
                    // Блокировка наступила, пока файл скачивался: ничего не открываем и не оставляем на диске.
                    let _ = std::fs::remove_file(&path);
                    return;
                }
                self.status = if util::open_path(&path) {
                    trf!("Открыто: {}", "Opened: {}", path.file_name().and_then(|n| n.to_str()).unwrap_or(""))
                } else {
                    tr!("Не удалось открыть файл. Сохраните его командой /save", "Could not open the file. Save it with /save").into()
                };
            }
            Event::Status(s) => self.status = s,
            Event::Error(e) => {
                self.older_busy = false;
                self.status = trf!("Ошибка: {e}", "Error: {e}");
            }
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
            11 => s.mouse = !s.mouse,
            12 => s.cloud_drafts = !s.cloud_drafts,
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

    /// Мышь: клик по вкладке или чату, колесо прокручивает ленту под указателем или список чатов.
    /// Работает только в обычном режиме и если включено в настройках.
    fn handle_mouse(&mut self, ev: MouseEvent, cmd: &mpsc::UnboundedSender<Cmd>) -> bool {
        if !self.settings.mouse || self.mode != Mode::Chat {
            return false;
        }
        let (x, y) = (ev.column, ev.row);
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.last_activity = Instant::now();
                if y == 0 {
                    if let Some((_, _, t)) = self.hit_tabs.iter().find(|(a, b, _)| x >= *a && x < *b) {
                        self.tab = t.clone();
                        self.ensure_selection();
                    }
                } else if inside(self.hit_list) && y > self.hit_list.y && y + 1 < self.hit_list.y + self.hit_list.height {
                    let row = (y - self.hit_list.y - 1) as usize + self.list_state.offset();
                    if let Some(&id) = self.visible_ids().get(row)
                        && self.sel != Some(id)
                    {
                        self.set_sel(Some(id));
                        self.scroll = 0;
                        self.pending_load = Some(Instant::now());
                    }
                }
                true
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                self.last_activity = Instant::now();
                let up = ev.kind == MouseEventKind::ScrollUp;
                if inside(self.hit_msgs) {
                    if up {
                        if self.scroll >= self.max_scroll {
                            self.request_older(cmd);
                        }
                        self.scroll += 3;
                    } else {
                        if self.scroll == 0 && self.jumped.is_some() && self.jumped == self.sel {
                            self.go_latest(cmd);
                        }
                        self.scroll = self.scroll.saturating_sub(3);
                    }
                } else if inside(self.hit_list) {
                    self.move_selection(if up { -1 } else { 1 });
                }
                true
            }
            _ => false,
        }
    }

    fn handle_key(&mut self, key: KeyEvent, cmd: &mpsc::UnboundedSender<Cmd>, saver: &Arc<Saver>) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        let since_prev = self.last_key_at.elapsed();
        self.last_key_at = Instant::now();
        self.last_activity = Instant::now();
        self.focus_msg = None; // подсветка найденного сообщения живёт до первого нажатия

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
                        } else if self.lock_input.is_empty() {
                            // Пустой Enter не считается попыткой (иначе случайные нажатия стирали бы данные).
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
            // Alt+↑/↓ всегда листают историю; просто ↑/↓ — когда в поле что-то есть (иначе выбирают чат).
            KeyCode::Up if alt || ((!self.input.is_empty() || self.hist_pos.is_some()) && !self.input_history.is_empty()) => {
                self.history_step(-1)
            }
            KeyCode::Down if alt || ((!self.input.is_empty() || self.hist_pos.is_some()) && !self.input_history.is_empty()) => {
                self.history_step(1)
            }
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => {
                if self.scroll >= self.max_scroll {
                    self.request_older(cmd);
                }
                self.scroll += 8;
            }
            KeyCode::PageDown => {
                if self.scroll == 0 && self.jumped.is_some() && self.jumped == self.sel {
                    self.go_latest(cmd);
                }
                self.scroll = self.scroll.saturating_sub(8);
            }
            KeyCode::Esc => {
                if !self.input.is_empty() {
                    self.clear_input();
                } else if self.edit_target.take().is_some() {
                    self.status = tr!("Правка отменена", "Edit cancelled").into();
                } else if self.reply.take().is_some() {
                    self.status = tr!("Ответ отменён", "Reply cancelled").into();
                } else if self.jumped.is_some() && self.jumped == self.sel {
                    self.go_latest(cmd);
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
        let _ = cmd.send(Cmd::LoadAllDialogs);
    }

    /// Отправка и команды: `/later <минуты> <текст>`, `/md`, `/lock`, `/settings`, `/passwd`,
    /// `/proxy <адрес|auto|off>`, `/refresh`, `/help`, `/quit`.
    fn submit(&mut self, cmd: &mpsc::UnboundedSender<Cmd>, saver: &Arc<Saver>) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.remember_input(&text);
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
        self.pending_leave = None;
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
        if (text == "/later" || text.starts_with("/later ")) && parse_later(&text).is_none() {
            self.status =
                tr!("Формат: /later минуты текст (например /later 30 созвон)", "Usage: /later minutes text (e.g. /later 30 call)").into();
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
        self.cloud_drafts.remove(&chat.id); // после отправки Telegram сам стирает черновик чата
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
                self.hist_pos = None;
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

    /// Открывает найденное сообщение: если оно уже загружено, выделяет его, иначе грузит окружение.
    fn open_found(&mut self, chat_id: PeerId, id: i32, ts: i64, cmd: &mpsc::UnboundedSender<Cmd>) {
        let Some(chat) = self.chats.iter().find(|c| c.id == chat_id).cloned() else {
            self.status = tr!("Чат найденного сообщения не в списке", "The chat of the found message is not in the list").into();
            return;
        };
        let loaded = self.histories.get(&chat_id).is_some_and(|h| h.iter().any(|m| m.id == id));
        if loaded {
            self.reveal_chat(chat_id);
            self.focus_msg = Some(id);
            self.focus_pending = true;
            self.status = tr!("Найденное сообщение выделено", "Found message highlighted").into();
        } else {
            self.status = tr!("Открываю найденное сообщение…", "Opening the found message…").into();
            let _ = cmd.send(Cmd::LoadAround { chat, id, ts });
        }
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
                OverlayKind::Search => {
                    let target = ov.items[ov.sel].target;
                    self.overlay = None;
                    self.mode = Mode::Chat;
                    if let Some((chat_id, id, ts)) = target {
                        self.open_found(chat_id, id, ts, cmd);
                    }
                }
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
                self.pending_leave = None;
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
                'c' | 'с' => Some('c'),
                'o' | 'щ' => Some('o'),
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
            (KeyCode::Up, _) => {
                if self.pick == 0 {
                    self.request_older(cmd);
                }
                self.pick = self.pick.saturating_sub(1);
            }
            (KeyCode::Down, _) => self.pick = (self.pick + 1).min(len - 1),
            (KeyCode::PageUp, _) => {
                if self.pick < 5 {
                    self.request_older(cmd);
                }
                self.pick = self.pick.saturating_sub(5);
            }
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
                    } else if !self.input.is_empty() {
                        self.pick_note = tr!(
                            "В поле ввода уже есть текст: отправьте или очистите его (Esc), потом правьте",
                            "The input box has text: send or clear it (Esc) before editing"
                        )
                        .into();
                    } else {
                        self.input = edit_source(&m);
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
            (_, Some('c')) => {
                if let Some(m) = self.picked().cloned() {
                    self.pick_note = self.copy_message(&m);
                }
            }
            (_, Some('o')) => {
                if let (Some(chat), Some(m)) = (self.selected().cloned(), self.picked().cloned()) {
                    if m.caption.is_none() {
                        self.pick_note = tr!("В этом сообщении нет вложения", "This message has no attachment").into();
                    } else {
                        let _ = cmd.send(Cmd::Open { chat, id: m.id });
                        self.status = tr!("Скачиваю для просмотра…", "Downloading to view…").into();
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
                        let items =
                            links.into_iter().map(|u| OverlayItem { label: u.clone(), data: u, id: 0, ..Default::default() }).collect();
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

    /// Кладёт текст сообщения в буфер обмена и возвращает сообщение для строки состояния.
    fn copy_message(&mut self, m: &ChatMsg) -> String {
        let text = m.raw.trim_end();
        let note = if text.is_empty() {
            tr!("В этом сообщении нет текста", "This message has no text").to_string()
        } else if util::clipboard_set(text) {
            trf!("Скопировано: {} симв.", "Copied: {} chars", text.chars().count())
        } else {
            tr!("Не удалось скопировать: буфер обмена недоступен", "Could not copy: clipboard is not available").to_string()
        };
        self.status = note.clone();
        note
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
        if name != "/leave" {
            self.pending_leave = None; // подтверждение выхода действует только на сразу следующий /leave
        }
        match name {
            "/find" | "/search" => {
                self.clear_input();
                self.set_filter(arg);
            }
            "/findmsg" | "/findall" => {
                self.clear_input();
                if arg.is_empty() {
                    self.status = tr!("Что искать? Например: /findmsg отчёт", "What to look for? For example: /findmsg report").into();
                    return true;
                }
                if name == "/findmsg" {
                    let Some(chat) = self.selected().cloned() else {
                        self.status = tr!("Сначала выберите чат", "Select a chat first").into();
                        return true;
                    };
                    let _ = cmd.send(Cmd::SearchChat { chat, query: arg.to_string() });
                } else {
                    let _ = cmd.send(Cmd::SearchAll { query: arg.to_string() });
                }
                self.status = tr!("Ищу…", "Searching…").into();
            }
            "/pinned" => {
                self.clear_input();
                let Some(chat) = self.selected().cloned() else {
                    self.status = tr!("Сначала выберите чат", "Select a chat first").into();
                    return true;
                };
                let _ = cmd.send(Cmd::ListPinned(chat));
                self.status = tr!("Загружаю закреплённые…", "Loading pinned messages…").into();
            }
            "/chat" | "/join" => {
                self.clear_input();
                let join = name == "/join";
                let Some(target) = tg::parse_target(arg) else {
                    self.status = if join {
                        tr!(
                            "Укажите ссылку или имя: /join t.me/+код или /join @имя",
                            "Give a link or a name: /join t.me/+code or /join @name"
                        )
                        .into()
                    } else {
                        tr!("Укажите @имя: /chat @имя", "Give an @name: /chat @name").into()
                    };
                    return true;
                };
                if !join && !matches!(target, tg::Target::Username(_)) {
                    self.status = tr!("Для ссылки-приглашения используйте /join", "Use /join for an invite link").into();
                    return true;
                }
                let _ = cmd.send(if join { Cmd::Join(target) } else { Cmd::OpenTarget(target) });
                self.status = if join { tr!("Вступаю…", "Joining…").into() } else { tr!("Ищу…", "Looking…").into() };
            }
            "/leave" => {
                self.clear_input();
                let Some(chat) = self.selected().cloned() else {
                    self.status = tr!("Сначала выберите чат", "Select a chat first").into();
                    return true;
                };
                if chat.kind == Kind::User {
                    self.status = tr!(
                        "Из личного чата выйти нельзя: /leave работает для групп и каналов",
                        "You cannot leave a private chat: /leave works for groups and channels"
                    )
                    .into();
                    return true;
                }
                if self.pending_leave == Some(chat.id) {
                    self.pending_leave = None;
                    let _ = cmd.send(Cmd::Leave(chat));
                    self.status = tr!("Выхожу…", "Leaving…").into();
                } else {
                    self.status =
                        trf!("Выйти из «{}»? Повторите /leave для подтверждения", "Leave “{}”? Repeat /leave to confirm", chat.name);
                    self.pending_leave = Some(chat.id);
                }
                return true; // pending_leave сохраняется до следующей другой команды
            }
            "/latest" => {
                self.clear_input();
                if self.jumped.is_some() && self.jumped == self.sel {
                    self.go_latest(cmd);
                } else {
                    self.scroll = 0;
                }
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
                    self.input = format!("/edit {}", edit_source(&last));
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
            "/open" => {
                self.clear_input();
                let (Some(chat), Some(m)) = (
                    self.selected().cloned(),
                    self.sel.and_then(|id| self.histories.get(&id)).and_then(|h| h.iter().rev().find(|m| m.caption.is_some())).cloned(),
                ) else {
                    self.status = tr!("В загруженных сообщениях нет вложений", "No attachments in the loaded messages").into();
                    return true;
                };
                let _ = cmd.send(Cmd::Open { chat, id: m.id });
                self.status = tr!("Скачиваю последнее вложение для просмотра…", "Downloading the latest attachment to view…").into();
            }
            "/copy" => {
                self.clear_input();
                let last = self
                    .sel
                    .and_then(|id| self.histories.get(&id))
                    .and_then(|h| h.iter().rev().find(|m| !m.raw.trim().is_empty()))
                    .cloned();
                match last {
                    Some(m) => {
                        self.copy_message(&m);
                    }
                    None => self.status = tr!("Нет сообщения с текстом", "No message with text").into(),
                }
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
                            items.push(OverlayItem { label: format!("{}: {u}", m.sender), data: u, id: 0, ..Default::default() });
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
                let items = faq()
                    .iter()
                    .map(|(q, a)| OverlayItem { label: format!("{q} — {a}"), data: String::new(), id: 0, ..Default::default() })
                    .collect();
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

mod draw;
mod text;
use draw::*;
use text::*;
pub(crate) use text::{ctrl_letter, is_paste_key};

// ------------------------------------------------------------------ главный цикл

pub async fn run(
    mut terminal: DefaultTerminal,
    cmd: mpsc::UnboundedSender<Cmd>,
    mut events: mpsc::UnboundedReceiver<Event>,
    saver: Arc<Saver>,
) -> Result<()> {
    util::clear_view_dir(); // остатки прошлого сеанса (например, файл был открыт в просмотрщике при выходе)
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
    let mut mouse_on = false;
    let _ = cmd.send(Cmd::LoadAllDialogs);
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut last_save = Instant::now();
    let mut last_refresh = Instant::now();
    let mut last_title = String::new();
    // Перерисовываем только при изменениях: это заметно разгружает слабый процессор.
    let mut dirty = true;
    let mut flood_shown = false;

    loop {
        if app.settings.mouse != mouse_on {
            mouse_on = app.settings.mouse;
            let _ = if mouse_on {
                crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)
            } else {
                crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)
            };
        }
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
                TermEvent::Mouse(m) => {
                    if app.handle_mouse(m, &cmd) {
                        dirty = true;
                    }
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
                app.flush_drafts(&cmd);
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
    if mouse_on {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    }
    util::clear_view_dir();
    Ok(())
}

#[cfg(test)]
mod tests;
