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
