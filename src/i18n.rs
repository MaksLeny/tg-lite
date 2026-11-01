//! Язык интерфейса: русский (по умолчанию) или английский.
//!
//! Строки пишутся парой «русский, английский» прямо в месте использования:
//! `tr!("Привет", "Hello")` возвращает `&'static str`, а `trf!("Осталось {n}", "Left {n}", n = 3)`
//! работает как `format!`. Выбранный язык хранится в файле `tglite.lang` рядом с данными
//! (он не секретный), поэтому экран входа тоже переводится.

#[cfg(not(test))]
mod state {
    use std::sync::atomic::{AtomicBool, Ordering};
    static ENGLISH: AtomicBool = AtomicBool::new(false);
    pub fn get() -> bool {
        ENGLISH.load(Ordering::Relaxed)
    }
    pub fn put(v: bool) {
        ENGLISH.store(v, Ordering::Relaxed);
    }
}

// В тестах язык свой у каждого потока, чтобы параллельные тесты не мешали друг другу.
#[cfg(test)]
mod state {
    use std::cell::Cell;
    thread_local!(static ENGLISH: Cell<bool> = const { Cell::new(false) });
    pub fn get() -> bool {
        ENGLISH.with(Cell::get)
    }
    pub fn put(v: bool) {
        ENGLISH.with(|c| c.set(v));
    }
}

/// Включён ли английский язык.
pub fn en() -> bool {
    state::get()
}

/// Переключает язык (0 — русский, иначе английский).
pub fn set(lang: u8) {
    state::put(lang != 0);
}

/// Текущий язык как число для настроек: 0 — русский, 1 — английский.
pub fn current() -> u8 {
    u8::from(en())
}

fn lang_file() -> std::path::PathBuf {
    crate::util::data_dir().join("tglite.lang")
}

/// Читает язык из файла рядом с данными (если файла нет — русский).
pub fn load() {
    let lang = std::fs::read_to_string(lang_file()).map(|s| s.trim() == "en").unwrap_or(false);
    set(u8::from(lang));
}

/// Запоминает язык в файле, чтобы экран входа открывался на нём же.
pub fn save() {
    let _ = std::fs::write(lang_file(), if en() { "en" } else { "ru" });
}

/// `tr!("русский", "english")` -> `&'static str` на текущем языке.
#[macro_export]
macro_rules! tr {
    ($ru:expr, $en:expr) => {
        if $crate::i18n::en() { $en } else { $ru }
    };
}

/// `trf!("русский {x}", "english {x}", x = 1)` -> `String`, как `format!`.
#[macro_export]
macro_rules! trf {
    ($ru:literal, $en:literal $(, $($arg:tt)*)?) => {
        if $crate::i18n::en() { format!($en $(, $($arg)*)?) } else { format!($ru $(, $($arg)*)?) }
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn macros_follow_language() {
        super::set(0);
        assert_eq!(tr!("да", "yes"), "да");
        assert_eq!(trf!("есть {n}", "have {n}", n = 2), "есть 2");
        super::set(1);
        assert_eq!(tr!("да", "yes"), "yes");
        assert_eq!(trf!("есть {n}", "have {n}", n = 2), "have 2");
        assert_eq!(trf!("пусто", "empty"), "empty");
        super::set(0);
    }
}
