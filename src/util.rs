//! Общие мелочи: папка данных и простой журнал ошибок.

use std::io::Write;
use std::path::PathBuf;

/// Папка данных: рядом с exe (удобно носить с собой). Можно переопределить переменной TGLITE_DIR.
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("TGLITE_DIR") {
        return PathBuf::from(d);
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

const MAX_LOG_BYTES: u64 = 200 * 1024;

/// Дописывает строку в `tglite.log` рядом с exe. Пишутся только служебные сообщения об ошибках,
/// без текстов переписки. Файл не растёт бесконечно: после 200 КБ старый журнал заменяется.
pub fn log(msg: &str) {
    let dir = data_dir();
    let path = dir.join("tglite.log");
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > MAX_LOG_BYTES {
            let _ = std::fs::rename(&path, dir.join("tglite.old.log"));
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let now = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::system());
        let _ = writeln!(f, "{} {}", now.strftime("%Y-%m-%d %H:%M:%S"), msg);
    }
}

/// Имя файла из названия чата: только буквы, цифры, пробел, `-` и `_`.
pub fn safe_filename(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let s = s.trim().chars().take(40).collect::<String>();
    if s.is_empty() { "chat".to_string() } else { s }
}

/// Сохраняет текст в `export/<имя>.txt` рядом с программой. Возвращает путь к файлу.
pub fn write_export(chat_name: &str, text: &str) -> std::io::Result<PathBuf> {
    let dir = data_dir().join("export");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.txt", safe_filename(chat_name)));
    std::fs::write(&path, text)?;
    Ok(path)
}

/// Можно ли безопасно вывести символ в консоль Windows. Эмодзи, флаги, невидимые символы
/// (склейки, вариационные селекторы) в её шрифте не рисуются: вместо них появляются
/// квадратики, а разная трактовка ширины сдвигает и стирает соседний текст.
pub fn is_safe_char(c: char) -> bool {
    matches!(c as u32,
        0x20..=0x7E            // ASCII
        | 0xA1..=0x24F         // латиница (без неразрывного пробела и мягкого переноса: ниже)
        | 0x370..=0x3FF        // греческий
        | 0x400..=0x482 | 0x48A..=0x4FF // кириллица без комбинируемых знаков
        | 0x2010..=0x2015 | 0x2018..=0x201E | 0x2020..=0x2022 | 0x2026 | 0x2030 | 0x2039 | 0x203A
        | 0x20AC | 0x2116 | 0x2122
        | 0x2190..=0x2193
        | 0x2500..=0x259F      // рамки и блоки
    ) && c as u32 != 0xAD
}

/// Очищает текст от символов, которые консоль не умеет рисовать. Переводы строк сохраняются,
/// неразрывные пробелы и табуляция становятся обычными пробелами.
pub fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push('\n'),
            '\t' | '\u{a0}' | '\u{2002}'..='\u{200a}' | '\u{3000}' => out.push(' '),
            c if is_safe_char(c) => out.push(c),
            _ => {}
        }
    }
    out
}

/// То же, но если после очистки ничего не осталось (например, одни эмодзи), возвращает `placeholder`.
pub fn clean_or(s: &str, placeholder: &str) -> String {
    let c = clean(s);
    if c.trim().is_empty() && !s.trim().is_empty() { placeholder.to_string() } else { c }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_emoji_and_invisibles() {
        assert_eq!(clean("Привет 😀 мир"), "Привет  мир");
        assert_eq!(clean("Анастасия ❤️ Тимофей"), "Анастасия  Тимофей");
        assert_eq!(clean("a\u{200d}b\u{fe0f}c"), "abc");
        assert_eq!(clean("🇷🇺 Россия"), " Россия");
        assert_eq!(clean("строка1\nстрока2"), "строка1\nстрока2");
        assert_eq!(clean("a\u{a0}b"), "a b");
        assert_eq!(clean("№5 — «да» €"), "№5 — «да» €");
        assert_eq!(clean_or("😀😀", "[эмодзи]"), "[эмодзи]");
        assert_eq!(clean_or("", "[эмодзи]"), "");
        assert_eq!(clean_or("ok", "[эмодзи]"), "ok");
    }

    #[test]
    fn filenames_are_safe() {
        assert_eq!(safe_filename("Мама"), "Мама");
        assert_eq!(safe_filename("a/b\\c:d*?"), "a_b_c_d__");
        assert_eq!(safe_filename("   "), "chat");
        assert_eq!(safe_filename(&"я".repeat(100)).chars().count(), 40);
    }

    #[test]
    fn keeps_normal_text() {
        let s = "Код для входа: 12345. «Не давайте» код — никому, ёЁ ї ґ";
        assert_eq!(clean(s), s);
    }
}
