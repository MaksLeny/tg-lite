//! Настройки приложения. Хранятся внутри зашифрованного хранилища вместе с сессией.

use serde::{Deserialize, Serialize};

use crate::{tr, trf};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProxyMode {
    /// Системный прокси Windows и переменные окружения, иначе напрямую.
    Auto,
    /// Всегда напрямую.
    Off,
    /// Только указанный вручную адрес (`/proxy адрес`).
    Manual,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Settings {
    /// Через сколько минут бездействия блокировать приложение (0 = никогда).
    pub lock_minutes: u32,
    pub proxy_mode: ProxyMode,
    pub proxy_manual: String,
    /// Звуковой сигнал при новом сообщении.
    pub sound: bool,
    /// Показывать «в сети» / «был(а) в сети».
    pub show_status: bool,
    /// Сколько последних сообщений загружать при открытии чата.
    pub history_limit: u32,
    /// Номер цвета акцента (см. accent_names()).
    pub accent: u8,
    /// Закреплённые чаты (bot-API id), показываются в списке первыми.
    pub pinned: Vec<i64>,
    /// Стереть данные после стольких неверных паролей подряд на экране блокировки (0 — выключено).
    pub wipe_after: u32,
    /// Фон экрана блокировки (номер из `lock::bg_names()`).
    pub lock_bg: u8,
    /// Ширина списка чатов: 0 узкий, 1 обычный, 2 широкий.
    pub list_width: u8,
    /// Ширина сообщений в ленте: 0 узкие, 1 обычные, 2 широкие.
    pub bubble_width: u8,
    /// Язык интерфейса: 0 русский, 1 английский.
    pub lang: u8,
    /// Мышь: клик по чату и вкладке, колесо для прокрутки.
    pub mouse: bool,
    /// Черновики хранятся в Telegram и видны на других устройствах.
    pub cloud_drafts: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            lock_minutes: 5,
            proxy_mode: ProxyMode::Auto,
            proxy_manual: String::new(),
            sound: false,
            show_status: true,
            history_limit: 60,
            accent: 0,
            pinned: Vec::new(),
            wipe_after: 0,
            lock_bg: 0,
            list_width: 1,
            bubble_width: 1,
            lang: 0,
            mouse: true,
            cloud_drafts: true,
        }
    }
}

pub const LOCK_CHOICES: [u32; 8] = [0, 1, 2, 5, 10, 15, 30, 60];
pub const WIPE_CHOICES: [u32; 4] = [0, 3, 5, 10];
pub const HISTORY_CHOICES: [u32; 4] = [30, 60, 100, 150];
pub fn accent_names() -> [&'static str; 5] {
    [tr!("Голубой", "Blue"), tr!("Зелёный", "Green"), tr!("Янтарный", "Amber"), tr!("Розовый", "Pink"), tr!("Фиолетовый", "Purple")]
}

/// Переключает значение по кругу вперёд (`dir > 0`) или назад.
pub fn cycle<T: PartialEq + Copy>(list: &[T], cur: T, dir: i32) -> T {
    let n = list.len() as i32;
    // Неизвестное значение: вперёд начинаем с первого элемента, назад с последнего.
    let i = list.iter().position(|x| *x == cur).map(|i| i as i32).unwrap_or(if dir > 0 { -1 } else { 0 });
    list[(i + dir.signum()).rem_euclid(n) as usize]
}

impl Settings {
    pub fn lock_label(&self) -> String {
        match self.lock_minutes {
            0 => tr!("никогда", "never").to_string(),
            1 => tr!("1 минута", "1 minute").to_string(),
            m @ 2..=4 => trf!("{m} минуты", "{m} minutes"),
            m => trf!("{m} минут", "{m} minutes"),
        }
    }

    pub fn proxy_label(&self) -> String {
        match self.proxy_mode {
            ProxyMode::Auto => tr!("авто (системный)", "auto (system)").to_string(),
            ProxyMode::Off => tr!("выключен", "off").to_string(),
            ProxyMode::Manual if self.proxy_manual.is_empty() => tr!("вручную (адрес не задан)", "manual (no address set)").to_string(),
            ProxyMode::Manual => trf!("вручную: {}", "manual: {}", crate::proxy::redact(&self.proxy_manual)),
        }
    }

    pub fn wipe_label(&self) -> String {
        match self.wipe_after {
            0 => tr!("выключено", "off").to_string(),
            n => trf!("после {n} неверных паролей", "after {n} wrong passwords"),
        }
    }

    pub fn lock_bg_name(&self) -> &'static str {
        crate::lock::bg_names()[(self.lock_bg as usize).min(crate::lock::BG_COUNT as usize - 1)]
    }

    /// Доля ширины окна под список чатов и предел в клетках.
    pub fn list_dims(&self) -> (u16, u16) {
        match self.list_width {
            0 => (22, 26),
            2 => (36, 46),
            _ => (28, 34),
        }
    }

    /// Доля ширины ленты, которую занимает сообщение, в процентах.
    pub fn bubble_pct(&self) -> usize {
        match self.bubble_width {
            0 => 60,
            2 => 88,
            _ => 72,
        }
    }

    pub fn width_name(v: u8, names: [&'static str; 3]) -> &'static str {
        names[(v as usize).min(2)]
    }

    pub fn accent_name(&self) -> &'static str {
        accent_names()[(self.accent as usize).min(accent_names().len() - 1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_wraps_both_ways() {
        assert_eq!(cycle(&LOCK_CHOICES, 60, 1), 0);
        assert_eq!(cycle(&LOCK_CHOICES, 0, -1), 60);
        assert_eq!(cycle(&LOCK_CHOICES, 5, 1), 10);
        assert_eq!(cycle(&LOCK_CHOICES, 7, 1), 0); // неизвестное значение: вперёд — первый элемент
        assert_eq!(cycle(&LOCK_CHOICES, 7, -1), 60);
    }

    #[test]
    fn old_vault_without_settings_still_loads() {
        // Хранилище версий 0.0.1/0.0.2 не содержало настроек: берутся значения по умолчанию.
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, Settings::default());
        let s: Settings = serde_json::from_str(r#"{"lock_minutes":15}"#).unwrap();
        assert_eq!(s.lock_minutes, 15);
        assert!(s.show_status);
    }

    #[test]
    fn labels() {
        let mut s = Settings::default();
        assert_eq!(s.lock_label(), "5 минут");
        s.lock_minutes = 2;
        assert_eq!(s.lock_label(), "2 минуты");
        s.lock_minutes = 0;
        assert_eq!(s.lock_label(), "никогда");
    }

    #[test]
    fn safe_defaults() {
        let s = Settings::default();
        assert_eq!(s.wipe_after, 0, "самоуничтожение по умолчанию выключено");
        assert!(!s.sound);
        assert_eq!(s.lang, 0);
        assert_eq!(s.lock_minutes, 5);
        assert!(s.pinned.is_empty());
        assert_eq!(s.proxy_mode, ProxyMode::Auto);
    }

    #[test]
    fn settings_roundtrip_through_json_and_ignore_unknown_fields() {
        let mut s = Settings::default();
        s.lang = 1;
        s.wipe_after = 5;
        s.pinned = vec![1, -100123];
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), s);
        let s2: Settings = serde_json::from_str(r#"{"lang":1,"from_the_future":true}"#).unwrap();
        assert_eq!(s2.lang, 1);
    }

    #[test]
    fn out_of_range_values_never_panic() {
        let mut s = Settings::default();
        s.accent = 200;
        s.lock_bg = 200;
        s.list_width = 200;
        s.bubble_width = 200;
        s.lang = 200;
        let _ = (s.accent_name(), s.lock_bg_name(), s.list_dims(), s.bubble_pct());
        let (min, max) = s.list_dims();
        assert!(min <= max);
        assert_eq!(Settings::width_name(9, ["a", "b", "c"]), "c");
    }

    #[test]
    fn english_labels_have_no_russian_text() {
        crate::i18n::set(1);
        let mut s = Settings::default();
        let cyr = |t: &str| t.chars().any(|c| ('\u{400}'..='\u{4ff}').contains(&c));
        for m in LOCK_CHOICES {
            s.lock_minutes = m;
            assert!(!cyr(&s.lock_label()), "{}", s.lock_label());
        }
        for mode in [ProxyMode::Auto, ProxyMode::Off, ProxyMode::Manual] {
            s.proxy_mode = mode;
            assert!(!cyr(&s.proxy_label()));
        }
        for w in WIPE_CHOICES {
            s.wipe_after = w;
            assert!(!cyr(&s.wipe_label()));
        }
        assert!(accent_names().iter().all(|n| !cyr(n)));
        for bg in 0..crate::lock::BG_COUNT {
            s.lock_bg = bg;
            assert!(!cyr(s.lock_bg_name()));
        }
        crate::i18n::set(0);
    }
}
