//! Виды окна («Классический», «Мессенджер», «Два окна»), быстрый переход Ctrl+K и то, что с ними связано:
//! следующий непрочитанный чат, чаты на Alt+1…9, заглушённые чаты и тихие часы.

use super::*;

/// Вид окна.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum View {
    /// Список чатов слева, переписка справа.
    Classic,
    /// В списке под именем видно последнее сообщение; в переписке имя автора стоит один раз над серией сообщений,
    /// а новое отделено строкой «новые сообщения».
    Messenger,
    /// Два чата рядом, открытые чаты вкладками сверху.
    TwoPanes,
}

impl View {
    pub(super) fn from_setting(v: u8) -> View {
        match v {
            1 => View::Messenger,
            2 => View::TwoPanes,
            _ => View::Classic,
        }
    }
}

/// Состояние вида «Два окна». Активное окно — обычный выбранный чат (`App::sel`), здесь только то,
/// что относится ко второму окну и вкладкам.
#[derive(Default)]
pub(super) struct Panes {
    /// Чат во втором (неактивном) окне.
    pub other: Option<PeerId>,
    /// Прокрутка второго окна и наибольшая допустимая (её выставляет отрисовка).
    pub other_scroll: usize,
    pub other_max: usize,
    /// Активно правое окно; иначе левое.
    pub right_active: bool,
    /// Открытые чаты (вкладки сверху) в порядке открытия.
    pub open: Vec<PeerId>,
    /// Первый чат уже выбран: пустое окно после закрытия вкладок само не заполняется.
    pub started: bool,
    /// Области окон для мыши: левое и правое (у правого нулевая ширина, если оно не помещается).
    pub hit: [Rect; 2],
    /// Вкладки на экране: от какого столбца, до какого, какой чат.
    pub hit_tabs: Vec<(u16, u16, PeerId)>,
}

/// Что делает выбранная строка окна быстрого перехода.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum QuickAction {
    Chat(PeerId),
    Command(&'static str),
}

pub(super) struct QuickItem {
    pub label: String,
    /// Пояснение у правого края: сколько непрочитанных или что делает команда.
    pub hint: String,
    pub action: QuickAction,
}

/// Окно быстрого перехода (Ctrl+K): строка запроса и найденные чаты или команды.
pub(super) struct Quick {
    pub query: String,
    pub items: Vec<QuickItem>,
    pub sel: usize,
}

/// Меньше этой ширины два окна рядом не помещаются: показывается только активное.
pub(super) const PANES_MIN_WIDTH: u16 = 64;
/// Сколько вкладок держим открытыми.
const MAX_OPEN_TABS: usize = 12;
/// Сколько строк даёт окно быстрого перехода.
const QUICK_LIMIT: usize = 60;
/// Сколько заглушённых чатов запоминаем.
const MAX_MUTED: usize = 500;

/// Минута суток по местному времени (0..1440).
pub(super) fn local_minute() -> u16 {
    let z = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::system());
    (z.hour() as u16) * 60 + z.minute() as u16
}

/// Буквы русской и английской раскладки по клавишам: запрос, набранный не в той раскладке, всё равно находит чат.
fn swap_layout(s: &str) -> String {
    const LAT: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.`";
    const CYR: &str = "йцукенгшщзхъфывапролджэячсмитьбюё";
    s.chars()
        .map(|c| {
            let lower = c.to_lowercase().next().unwrap_or(c);
            if let Some(i) = LAT.chars().position(|x| x == lower) {
                CYR.chars().nth(i).unwrap_or(c)
            } else if let Some(i) = CYR.chars().position(|x| x == lower) {
                LAT.chars().nth(i).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// Где `needle` встречается в `hay` подряд.
fn find_run(hay: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

/// Если буквы `needle` стоят в `hay` в том же порядке (возможно, с пропусками), то сколько пропущено.
fn scattered_gaps(hay: &[char], needle: &[char]) -> Option<usize> {
    let mut pos = 0;
    let (mut first, mut last) = (None, 0);
    for &n in needle {
        let at = hay[pos..].iter().position(|&h| h == n)? + pos;
        first.get_or_insert(at);
        last = at;
        pos = at + 1;
    }
    Some(last + 1 - first? - needle.len())
}

fn words_score(name: &str, query: &str) -> u32 {
    let hay: Vec<char> = name.to_lowercase().chars().collect();
    let mut total = 0u32;
    for word in query.to_lowercase().split_whitespace() {
        let w: Vec<char> = word.chars().collect();
        total += match find_run(&hay, &w) {
            Some(pos) => {
                let word_start = pos == 0 || !hay[pos - 1].is_alphanumeric();
                let bonus = if pos == 0 {
                    60
                } else if word_start {
                    40
                } else {
                    0
                };
                100 + bonus + (w.len() as u32).min(20) * 2 - (pos as u32).min(30)
            }
            None => match scattered_gaps(&hay, &w) {
                Some(gaps) => 30 + (w.len() as u32).min(20) - (gaps as u32).min(25),
                None => return 0,
            },
        };
    }
    total
}

/// Насколько название подходит запросу: 0 — не подходит, больше — лучше. Пустой запрос подходит всем.
/// Каждое слово запроса должно найтись в названии подряд (лучше с начала слова) или хотя бы буквами по порядку.
pub(super) fn fuzzy_score(name: &str, query: &str) -> u32 {
    let q = query.trim();
    if q.is_empty() {
        return 1;
    }
    words_score(name, q).max(words_score(name, &swap_layout(q)))
}

/// Что делает команда: подпись в окне быстрого перехода.
pub(super) fn command_info(name: &str) -> &'static str {
    match name {
        "/find" | "/search" => tr!("искать чат по названию", "find a chat by name"),
        "/findmsg" => tr!("искать в этом чате", "search in this chat"),
        "/findall" => tr!("искать во всех чатах", "search in all chats"),
        "/latest" => tr!("к последним сообщениям", "jump to the latest messages"),
        "/pinned" => tr!("закреплённые сообщения", "pinned messages"),
        "/topics" => tr!("темы форума", "forum topics"),
        "/pin" => tr!("закрепить чат вверху списка (Alt+цифра)", "pin the chat to the top (Alt+digit)"),
        "/chat" => tr!("открыть чат по @имени", "open a chat by @name"),
        "/join" => tr!("вступить по ссылке", "join by link"),
        "/leave" => tr!("выйти из группы или канала", "leave a group or channel"),
        "/readall" => tr!("прочитать всё", "mark everything as read"),
        "/export" => tr!("сохранить переписку в файл (txt, md, html, json)", "save the chat to a file (txt, md, html, json)"),
        "/note" => tr!("заметка в «Избранное»", "note to Saved Messages"),
        "/edit" => tr!("исправить своё сообщение", "edit your message"),
        "/del" => tr!("удалить своё сообщение", "delete your message"),
        "/later" => tr!("отправить позже", "send later"),
        "/md" => tr!("разметка Markdown", "Markdown formatting"),
        "/lock" => tr!("заблокировать", "lock"),
        "/settings" => tr!("настройки", "settings"),
        "/passwd" => tr!("сменить пароль", "change the password"),
        "/proxy" => tr!("прокси", "proxy"),
        "/refresh" => tr!("обновить список чатов", "refresh the chat list"),
        "/help" => tr!("справка", "help"),
        "/quit" => tr!("выйти", "quit"),
        "/save" => tr!("сохранить файл из сообщения", "save the file from a message"),
        "/open" => tr!("открыть файл из сообщения", "open the file from a message"),
        "/copy" => tr!("скопировать текст сообщения", "copy a message"),
        "/send" => tr!("отправить файл или фото", "send a file or photo"),
        "/links" => tr!("ссылки чата", "links of the chat"),
        "/scheduled" => tr!("отложенные сообщения", "scheduled messages"),
        "/paste" => tr!("вставить из буфера", "paste from the clipboard"),
        "/faq" => tr!("если что-то не работает", "if something does not work"),
        "/album" => tr!("несколько фото одним альбомом", "several photos as one album"),
        "/netcheck" => tr!("проверить связь с Telegram", "check the connection to Telegram"),
        "/report" => tr!("отчёт о неполадке для автора", "a problem report for the author"),
        "/view" => tr!("вид окна: классический, мессенджер, два окна", "window layout: classic, messenger, two panes"),
        "/mute" => tr!("без звука для этого чата", "mute this chat"),
        "/dnd" => tr!("не беспокоить на N минут", "do not disturb for N minutes"),
        "/next" => tr!("следующий непрочитанный чат", "next unread chat"),
        "/drafts" => tr!("чаты с неотправленным текстом", "chats with unsent text"),
        _ => "",
    }
}

impl App {
    pub(super) fn view(&self) -> View {
        View::from_setting(self.settings.view)
    }

    pub(super) fn two_panes(&self) -> bool {
        self.view() == View::TwoPanes
    }

    /// Сколько строк занимает чат в списке: в классическом виде одну, в остальных две (имя и последнее сообщение).
    pub(super) fn list_row_height(&self) -> u16 {
        if self.view() == View::Classic { 1 } else { 2 }
    }

    /// Чат виден на экране: открыт сейчас или стоит во втором окне. Сообщения в нём считаются прочитанными.
    pub(super) fn is_visible_chat(&self, id: PeerId) -> bool {
        self.sel == Some(id) || (self.two_panes() && self.panes.other == Some(id))
    }

    // --- заглушённые чаты и тихие часы

    /// Заглушён ли чат: настройкой в Telegram или командой /mute в Tg-Lite.
    pub(super) fn is_muted(&self, c: &ChatInfo) -> bool {
        c.muted || c.id.bot_api_dialog_id().is_some_and(|d| self.settings.muted.contains(&d))
    }

    /// Сейчас тихо: тихие часы или «не беспокоить».
    pub(super) fn quiet_now(&self) -> bool {
        self.dnd_until.is_some_and(|t| Instant::now() < t) || self.settings.is_quiet_at(local_minute())
    }

    /// Можно ли шуметь (звук, всплывающее окно) из-за нового сообщения в этом чате.
    pub(super) fn may_alert(&self, chat: PeerId) -> bool {
        !self.quiet_now() && !self.chats.iter().find(|c| c.id == chat).is_some_and(|c| self.is_muted(c))
    }

    /// `/mute`: чат без звука и уведомлений (локально, в Telegram ничего не меняется).
    pub(super) fn cmd_mute(&mut self, saver: &Arc<Saver>) {
        let Some(c) = self.selected().cloned() else {
            self.status = tr!("Сначала выберите чат", "Select a chat first").into();
            return;
        };
        let Some(did) = c.id.bot_api_dialog_id() else { return };
        if let Some(pos) = self.settings.muted.iter().position(|d| *d == did) {
            self.settings.muted.remove(pos);
            self.status = if c.muted {
                tr!(
                    "В Tg-Lite звук возвращён, но в Telegram этот чат заглушён: снять можно там",
                    "Sound is back in Tg-Lite, but the chat is muted in Telegram: unmute it there"
                )
                .into()
            } else {
                trf!("Звук чата «{}» снова включён", "Sound of \"{}\" is back on", c.name)
            };
        } else {
            self.settings.muted.push(did);
            if self.settings.muted.len() > MAX_MUTED {
                self.settings.muted.remove(0);
            }
            self.status = trf!(
                "Чат «{}» без звука и уведомлений (/mute ещё раз — вернуть)",
                "\"{}\" is muted: no sound or notifications (/mute again to undo)",
                c.name
            );
        }
        self.apply_settings(saver);
    }

    /// `/dnd [минуты|off]`: ни звука, ни уведомлений на время.
    pub(super) fn cmd_dnd(&mut self, arg: &str) {
        let a = arg.trim().to_lowercase();
        if matches!(a.as_str(), "off" | "0" | "выкл") {
            self.dnd_until = None;
            self.status = tr!("«Не беспокоить» выключено", "Do not disturb is off").into();
            return;
        }
        let minutes = if a.is_empty() { Some(60) } else { a.parse::<u32>().ok().filter(|m| (1..=1440).contains(m)) };
        let Some(minutes) = minutes else {
            self.status =
                tr!("Сколько минут? Например: /dnd 30 (или /dnd off)", "How many minutes? For example: /dnd 30 (or /dnd off)").into();
            return;
        };
        self.dnd_until = Some(Instant::now() + Duration::from_secs(u64::from(minutes) * 60));
        self.status = trf!("Не беспокоить до {}", "Do not disturb until {}", when_label(minutes));
    }

    // --- переход между чатами

    /// Закреплённые чаты в том порядке, в каком они стоят в списке; первые девять открываются по Alt+1…9.
    pub(super) fn favorites(&self) -> Vec<PeerId> {
        self.chats.iter().filter(|c| self.is_pinned(c)).map(|c| c.id).take(9).collect()
    }

    /// Alt+цифра: открыть закреплённый чат с этим номером.
    pub(super) fn open_favorite(&mut self, n: usize) {
        match self.favorites().get(n.wrapping_sub(1)).copied() {
            Some(id) => self.open_chat(id),
            None => {
                self.status =
                    trf!("Нет закреплённого чата №{n}: закрепить текущий — /pin", "No pinned chat №{n}: pin the current one with /pin")
            }
        }
    }

    /// Следующий чат с непрочитанными: сначала с упоминаниями, затем обычные, затем заглушённые.
    /// Ищет по порядку списка, начиная со следующего после открытого.
    pub(super) fn next_unread_chat(&self) -> Option<PeerId> {
        let n = self.chats.len();
        let start = self.sel.and_then(|s| self.chats.iter().position(|c| c.id == s)).map_or(0, |p| p + 1);
        let order: Vec<&ChatInfo> = (0..n).map(|k| &self.chats[(start + k) % n]).filter(|c| Some(c.id) != self.sel).collect();
        order
            .iter()
            .find(|c| c.mentions > 0)
            .or_else(|| order.iter().find(|c| c.unread > 0 && !self.is_muted(c)))
            .or_else(|| order.iter().find(|c| c.unread > 0))
            .map(|c| c.id)
    }

    /// Ctrl+N и `/next`.
    pub(super) fn cmd_next(&mut self) {
        match self.next_unread_chat() {
            Some(id) => self.open_chat(id),
            None => self.status = tr!("Непрочитанных чатов больше нет", "No more unread chats").into(),
        }
    }

    /// Открывает чат: в виде «Два окна» — в активном окне, иначе выбирает его в списке.
    pub(super) fn open_chat(&mut self, id: PeerId) {
        if !self.chats.iter().any(|c| c.id == id) {
            // Список (быстрый переход, черновики, вкладки) мог устареть: чат успел пропасть, открывать нечего.
            self.status = tr!("Этого чата уже нет в списке", "This chat is no longer in the list").into();
            return;
        }
        if self.two_panes() {
            self.open_in_panes(id);
            return;
        }
        if self.sel == Some(id) {
            return;
        }
        self.reveal_chat(id);
        self.scroll = 0;
        self.pending_load = Some(Instant::now());
    }

    // --- вид «Два окна»

    /// Чат в активное окно. Если он уже стоит во втором окне, активным становится то окно.
    pub(super) fn open_in_panes(&mut self, id: PeerId) {
        if self.sel == Some(id) {
            return;
        }
        if self.panes.other == Some(id) {
            self.switch_pane(!self.panes.right_active);
            return;
        }
        self.set_sel(Some(id));
        self.scroll = 0;
        self.pending_load = Some(Instant::now());
    }

    /// Запоминает чат как вкладку; лишние вкладки (самые давние, не стоящие в окнах) закрываются.
    pub(super) fn note_open(&mut self, id: PeerId) {
        if !self.panes.open.contains(&id) {
            self.panes.open.push(id);
        }
        while self.panes.open.len() > MAX_OPEN_TABS {
            let shown = [self.sel, self.panes.other];
            match self.panes.open.iter().position(|c| !shown.contains(&Some(*c))) {
                Some(pos) => {
                    self.panes.open.remove(pos);
                }
                None => break,
            }
        }
    }

    /// Ctrl+←/→ и F5: сделать активным левое или правое окно. Чаты окон меняются местами с выбранным.
    pub(super) fn switch_pane(&mut self, right: bool) {
        if !self.two_panes() || right == self.panes.right_active {
            return;
        }
        let other = self.panes.other.take();
        let old = self.sel;
        std::mem::swap(&mut self.scroll, &mut self.panes.other_scroll);
        self.panes.right_active = right;
        self.set_sel(other);
        self.panes.other = old;
        if let Some(id) = self.sel
            && !self.histories.contains_key(&id)
        {
            self.pending_load = Some(Instant::now());
        }
    }

    /// Tab и Shift+Tab: следующий или предыдущий открытый чат в активном окне (чат второго окна пропускается).
    pub(super) fn cycle_open(&mut self, delta: i32) {
        let tabs: Vec<PeerId> = self.panes.open.iter().copied().filter(|c| Some(*c) != self.panes.other).collect();
        if tabs.is_empty() {
            return;
        }
        let next = match self.sel.and_then(|s| tabs.iter().position(|c| *c == s)) {
            Some(p) => tabs[(p as i32 + delta).rem_euclid(tabs.len() as i32) as usize],
            None => tabs[0],
        };
        if Some(next) != self.sel {
            self.set_sel(Some(next));
            self.scroll = 0;
            self.pending_load = Some(Instant::now());
        }
    }

    /// Ctrl+W: закрыть вкладку активного окна; окно переходит на соседнюю вкладку или остаётся пустым.
    pub(super) fn close_tab(&mut self) {
        let Some(cur) = self.sel else { return };
        let pos = self.panes.open.iter().position(|c| *c == cur).unwrap_or(0);
        self.panes.open.retain(|c| *c != cur);
        let rest: Vec<PeerId> = self.panes.open.iter().copied().filter(|c| Some(*c) != self.panes.other).collect();
        let next = rest.get(pos.min(rest.len().saturating_sub(1))).copied();
        self.set_sel(next);
        self.scroll = 0;
        if next.is_some() {
            self.pending_load = Some(Instant::now());
        }
    }

    /// В двух окнах выбор не привязан к списку: убирает исчезнувшие чаты, а при запуске открывает первый.
    /// Возвращает true, если выбранный чат сменился.
    pub(super) fn ensure_panes(&mut self) -> bool {
        let known: HashSet<PeerId> = self.chats.iter().map(|c| c.id).collect();
        self.panes.open.retain(|id| known.contains(id));
        if self.panes.other.is_some_and(|id| !known.contains(&id)) {
            self.panes.other = None;
        }
        let mut changed = false;
        if self.sel.is_some_and(|id| !known.contains(&id)) {
            self.set_sel(None);
            self.scroll = 0;
            changed = true;
        }
        if self.sel.is_some() {
            self.panes.started = true;
        } else if !self.panes.started && !self.chats.is_empty() {
            // Первое окно заполняется один раз, когда пришёл список чатов; закрытое вручную потом пустым и остаётся.
            self.panes.started = true;
            if let Some(first) = self.visible_ids().first().copied() {
                self.set_sel(Some(first));
                self.scroll = 0;
                self.pending_load = Some(Instant::now());
                changed = true;
            }
        }
        changed
    }

    /// Переключает вид окна. Состояние прежнего вида сбрасывается; выбранный чат остаётся.
    pub(super) fn set_view(&mut self, v: u8) {
        let v = v.min(2);
        if self.settings.view == v {
            return;
        }
        self.settings.view = v;
        self.panes = Panes::default();
        if v == 2 {
            self.panes.started = true;
            if let Some(id) = self.sel {
                self.panes.open.push(id);
            }
        } else {
            self.ensure_selection();
        }
        self.new_from.clear();
        self.status = trf!("Вид окна: {}", "Window layout: {}", self.settings.view_name());
    }

    /// `/view [классический|мессенджер|окна]`; без слова — следующий вид по кругу.
    pub(super) fn cmd_view(&mut self, arg: &str, saver: &Arc<Saver>) {
        let v = match arg.trim().to_lowercase().as_str() {
            "" => (self.settings.view + 1) % 3,
            "0" | "classic" | "классический" | "обычный" => 0,
            "1" | "messenger" | "мессенджер" | "d" => 1,
            "2" | "panes" | "two" | "окна" | "два" | "два окна" | "f" => 2,
            _ => {
                self.status = tr!(
                    "Вид окна: /view classic, /view messenger, /view panes (без слова — по кругу)",
                    "Window layout: /view classic, /view messenger, /view panes (no word: next one)"
                )
                .into();
                return;
            }
        };
        self.set_view(v);
        self.apply_settings(saver);
    }

    /// Запоминает последнее сообщение чата для строки-превью в списке.
    pub(super) fn touch_preview(&mut self, chat: PeerId, msg: &ChatMsg) {
        if let Some(c) = self.chats.iter_mut().find(|c| c.id == chat) {
            c.preview = tg::preview_line(c.kind, msg.outgoing, &msg.sender, &msg.text);
            if msg.ts > 0 {
                c.last_ts = msg.ts;
            }
        }
    }

    /// Номер первого непрочитанного сообщения: его запоминаем при открытии чата, пока счётчик ещё не сброшен.
    /// `unread` — сколько чужих сообщений было не прочитано. Разделитель нужен, только если выше есть прочитанное.
    pub(super) fn mark_new_from(&mut self, chat: PeerId, unread: i32) {
        self.new_from.remove(&chat);
        if unread <= 0 || self.view() == View::Classic {
            return;
        }
        let Some(h) = self.histories.get(&chat) else { return };
        let incoming: Vec<usize> = h.iter().enumerate().filter(|(_, m)| !m.outgoing && m.id > 0).map(|(i, _)| i).collect();
        let n = unread as usize;
        if n == 0 || n > incoming.len() {
            return;
        }
        let first = incoming[incoming.len() - n];
        if first > 0 {
            self.new_from.insert(chat, h[first].id);
        }
    }

    // --- Ctrl+K

    /// Строки окна быстрого перехода: чаты по названию или, если запрос начинается с «/», команды.
    pub(super) fn quick_items(&self, query: &str) -> Vec<QuickItem> {
        let q = query.trim();
        if let Some(rest) = q.strip_prefix('/') {
            let rest = rest.to_lowercase();
            return COMMANDS
                .iter()
                .filter(|c| c[1..].starts_with(rest.as_str()))
                .take(QUICK_LIMIT)
                .map(|c| QuickItem { label: (*c).to_string(), hint: command_info(c).to_string(), action: QuickAction::Command(c) })
                .collect();
        }
        let favorites = self.favorites();
        let mut scored: Vec<(u32, usize)> = Vec::new();
        for (i, c) in self.chats.iter().enumerate() {
            let base = fuzzy_score(&c.name, q);
            if base == 0 {
                continue;
            }
            // Без запроса сверху непрочитанные (упоминания первыми) и закреплённые; с запросом решает совпадение.
            let bonus = if q.is_empty() {
                let unread = if c.mentions > 0 {
                    3000
                } else if c.unread > 0 && !self.is_muted(c) {
                    2000
                } else if c.unread > 0 {
                    1000
                } else {
                    0
                };
                unread + if self.is_pinned(c) { 500 } else { 0 }
            } else {
                u32::from(c.unread > 0) * 5
            };
            scored.push((base + bonus, i));
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored
            .into_iter()
            .take(QUICK_LIMIT)
            .map(|(_, i)| {
                let c = &self.chats[i];
                let tag = match c.kind {
                    Kind::User => ' ',
                    Kind::Group => tr!("Г", "G").chars().next().unwrap_or('G'),
                    Kind::Channel => tr!("К", "C").chars().next().unwrap_or('C'),
                };
                let num = favorites.iter().position(|f| *f == c.id).map_or_else(|| " ".to_string(), |p| (p + 1).to_string());
                let mut hint = Vec::new();
                if c.mentions > 0 {
                    hint.push(format!("@{}", c.mentions));
                }
                if c.unread > 0 {
                    hint.push(trf!("{} нов.", "{} new", c.unread.min(999)));
                }
                if self.is_muted(c) {
                    hint.push(tr!("без звука", "muted").to_string());
                }
                QuickItem { label: format!("{tag}{num} {}", c.name), hint: hint.join(" · "), action: QuickAction::Chat(c.id) }
            })
            .collect()
    }

    /// Ctrl+K: открывает окно быстрого перехода.
    pub(super) fn open_quick(&mut self) {
        self.quick = Some(Quick { query: String::new(), items: self.quick_items(""), sel: 0 });
        self.mode = Mode::Quick;
    }

    fn close_quick(&mut self) {
        self.quick = None;
        self.mode = Mode::Chat;
    }

    pub(super) fn handle_quick_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(p) = self.quick.as_mut() else {
            self.mode = Mode::Chat;
            return;
        };
        let len = p.items.len();
        match key.code {
            KeyCode::Esc => self.close_quick(),
            KeyCode::F(10) => {
                self.should_quit = true;
                self.close_quick();
            }
            KeyCode::Char(c) if ctrl && ctrl_letter(&key) == Some('k') && c != '\0' => self.close_quick(),
            KeyCode::Up | KeyCode::BackTab => p.sel = p.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => p.sel = (p.sel + 1).min(len.saturating_sub(1)),
            KeyCode::PageUp => p.sel = p.sel.saturating_sub(8),
            KeyCode::PageDown => p.sel = (p.sel + 8).min(len.saturating_sub(1)),
            KeyCode::Home => p.sel = 0,
            KeyCode::End => p.sel = len.saturating_sub(1),
            KeyCode::Backspace => {
                p.query.pop();
                let q = p.query.clone();
                let items = self.quick_items(&q);
                if let Some(p) = self.quick.as_mut() {
                    p.items = items;
                    p.sel = 0;
                }
            }
            KeyCode::Enter => {
                let action = p.items.get(p.sel).map(|i| i.action);
                self.close_quick();
                match action {
                    Some(QuickAction::Chat(id)) => self.open_chat(id),
                    Some(QuickAction::Command(c)) => {
                        // Команду вставляем в поле ввода: аргумент можно дописать, Enter — выполнить.
                        self.input = format!("{c} ");
                        self.cursor = self.input_len();
                        self.hist_pos = None;
                    }
                    None => {}
                }
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                p.query.push(c);
                let q = p.query.clone();
                let items = self.quick_items(&q);
                if let Some(p) = self.quick.as_mut() {
                    p.items = items;
                    p.sel = 0;
                }
            }
            _ => {}
        }
    }

    // --- /drafts

    /// Чаты с неотправленным текстом: черновики, оставленные в Tg-Lite, и (если включено) черновики из Telegram.
    /// Текст в открытом чате не считается: он и так перед глазами, а команда `/drafts` его заменяет.
    pub(super) fn draft_items(&self) -> Vec<OverlayItem> {
        let mut found: Vec<(PeerId, String)> = Vec::new();
        for (id, (text, _)) in &self.drafts {
            if !text.trim().is_empty() && !text.starts_with('/') && self.sel != Some(*id) {
                found.push((*id, text.clone()));
            }
        }
        if self.settings.cloud_drafts {
            for c in &self.chats {
                if !c.draft.trim().is_empty() && self.sel != Some(c.id) && !found.iter().any(|(f, _)| *f == c.id) {
                    found.push((c.id, c.draft.clone()));
                }
            }
        }
        // В порядке списка чатов.
        found.sort_by_key(|(id, _)| self.chats.iter().position(|c| c.id == *id).unwrap_or(usize::MAX));
        found
            .into_iter()
            .filter_map(|(id, text)| {
                let c = self.chats.iter().find(|c| c.id == id)?;
                let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
                Some(OverlayItem { label: format!("{}: {}", c.name, flat), data: flat, target: Some((id, 0, 0)), id: 0 })
            })
            .collect()
    }

    pub(super) fn cmd_drafts(&mut self) {
        let items = self.draft_items();
        if items.is_empty() {
            self.status = tr!("Неотправленных сообщений нет", "No unsent messages").into();
            return;
        }
        let title = trf!(" Неотправленное: {} ", " Unsent: {} ", items.len());
        self.open_overlay(OverlayKind::Drafts, &title, items);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_swap_both_ways() {
        assert_eq!(swap_layout("ghbdtn"), "привет");
        assert_eq!(swap_layout("привет"), "ghbdtn");
        assert_eq!(swap_layout("Ghbdtn 12"), "привет 12", "регистр не сохраняется: запрос всё равно сравнивается без учёта регистра");
        assert_eq!("qwertyuiop[]asdfghjkl;'zxcvbnm,.`".chars().count(), "йцукенгшщзхъфывапролджэячсмитьбюё".chars().count());
    }

    #[test]
    fn fuzzy_prefers_prefixes_and_word_starts_and_needs_every_word() {
        assert_eq!(fuzzy_score("Мама", ""), 1);
        assert_eq!(fuzzy_score("Мама", "   "), 1);
        let prefix = fuzzy_score("Работа чат", "раб");
        let inside = fuzzy_score("Моя работа", "раб");
        let word = fuzzy_score("Моя Работа", "раб");
        let middle = fuzzy_score("Подработка", "раб");
        assert!(prefix > word && word > middle, "{prefix} {word} {middle}");
        assert!(inside > middle);
        assert_eq!(fuzzy_score("Мама", "папа"), 0);
        assert!(fuzzy_score("Рабочий чат отдела", "раб отд") > 0, "оба слова запроса находятся");
        assert_eq!(fuzzy_score("Рабочий чат", "раб отд"), 0, "второго слова в названии нет");
        // по буквам в нужном порядке
        assert!(fuzzy_score("Рабочая группа", "рчг") > 0);
        assert_eq!(fuzzy_score("Рабочая группа", "гчр"), 0);
        // регистр не важен, раскладка тоже
        assert!(fuzzy_score("Мама", "МАМ") > 0);
        assert!(fuzzy_score("Привет", "ghbd") > 0, "запрос набран в английской раскладке");
        assert!(fuzzy_score("Новости", "yjdj") > 0);
    }

    #[test]
    fn gaps_are_counted_between_scattered_letters() {
        let h: Vec<char> = "abcdef".chars().collect();
        assert_eq!(scattered_gaps(&h, &['a', 'c']), Some(1));
        assert_eq!(scattered_gaps(&h, &['a', 'b']), Some(0));
        assert_eq!(scattered_gaps(&h, &['c', 'a']), None);
        assert_eq!(scattered_gaps(&h, &[]), None);
        assert_eq!(find_run(&h, &['c', 'd']), Some(2));
        assert_eq!(find_run(&h, &['d', 'c']), None);
        assert_eq!(find_run(&h, &[]), None);
    }

    #[test]
    fn every_command_has_a_description() {
        for c in COMMANDS {
            assert!(!command_info(c).is_empty(), "у команды {c} нет описания для окна быстрого перехода");
        }
    }

    #[test]
    fn view_numbers_map_to_views() {
        assert_eq!(View::from_setting(0), View::Classic);
        assert_eq!(View::from_setting(1), View::Messenger);
        assert_eq!(View::from_setting(2), View::TwoPanes);
        assert_eq!(View::from_setting(9), View::Classic, "неизвестное значение — обычный вид");
    }
}
