//! Настройки приложения. Хранятся внутри зашифрованного хранилища вместе с сессией.

use serde::{Deserialize, Serialize};

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
    /// Номер цвета акцента (см. ACCENT_NAMES).
    pub accent: u8,
    /// Закреплённые чаты (bot-API id), показываются в списке первыми.
    pub pinned: Vec<i64>,
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
        }
    }
}

pub const LOCK_CHOICES: [u32; 8] = [0, 1, 2, 5, 10, 15, 30, 60];
pub const HISTORY_CHOICES: [u32; 4] = [30, 60, 100, 150];
pub const ACCENT_NAMES: [&str; 5] = ["Голубой", "Зелёный", "Янтарный", "Розовый", "Фиолетовый"];

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
            0 => "никогда".to_string(),
            1 => "1 минута".to_string(),
            m @ 2..=4 => format!("{m} минуты"),
            m => format!("{m} минут"),
        }
    }

    pub fn proxy_label(&self) -> String {
        match self.proxy_mode {
            ProxyMode::Auto => "авто (системный)".to_string(),
            ProxyMode::Off => "выключен".to_string(),
            ProxyMode::Manual if self.proxy_manual.is_empty() => "вручную (адрес не задан)".to_string(),
            ProxyMode::Manual => format!("вручную: {}", self.proxy_manual),
        }
    }

    pub fn accent_name(&self) -> &'static str {
        ACCENT_NAMES[(self.accent as usize).min(ACCENT_NAMES.len() - 1)]
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
}
