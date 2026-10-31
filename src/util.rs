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

/// Звуковой сигнал о новом сообщении. На Windows играет тон через звуковую карту (функция Beep),
/// поэтому работает независимо от схемы системных звуков и от консоли; в отдельном потоке,
/// чтобы не задерживать интерфейс.
pub fn play_alert() {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn Beep(freq: u32, millis: u32) -> i32;
        }
        std::thread::spawn(|| unsafe {
            Beep(988, 80);
            Beep(1319, 120);
        });
    }
    #[cfg(not(windows))]
    {
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x07");
        let _ = out.flush();
    }
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

/// Имя файла для скачивания: точки и расширение сохраняются, недопустимые для Windows символы заменяются.
pub fn safe_file_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_control() || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    let s = s.trim().trim_start_matches('.').trim().to_string();
    let s: String = if s.chars().count() > 100 {
        // Длинное имя укорачиваем, сохраняя расширение.
        let ext: String = s.rsplit_once('.').map(|(_, e)| e.chars().take(10).collect()).unwrap_or_default();
        let stem: String = s.chars().take(90).collect();
        if ext.is_empty() { stem } else { format!("{stem}.{ext}") }
    } else {
        s
    };
    let stem = s.split('.').next().unwrap_or("").to_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "LPT1", "LPT2", "LPT3"];
    if s.is_empty() {
        "file".to_string()
    } else if reserved.contains(&stem.as_str()) {
        format!("_{s}")
    } else {
        s
    }
}

/// Свободное имя в папке: `a.pdf`, затем `a (1).pdf`, `a (2).pdf`…
pub fn unique_path(dir: &std::path::Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 1..1000 {
        let p = dir.join(format!("{stem} ({n}){ext}"));
        if !p.exists() {
            return p;
        }
    }
    first
}

/// Находит ссылки в тексте (http, https, t.me, www), без хвостовой пунктуации и без повторов.
pub fn extract_links(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tok in text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '«' | '»')) {
        let t = tok.trim_start_matches(['(', '[', '\'']);
        let lower = t.to_lowercase();
        let is_link = lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("t.me/") || lower.starts_with("www.");
        if !is_link {
            continue;
        }
        let mut t = t.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '\'']).to_string();
        if !lower.starts_with("http") {
            t = format!("https://{t}");
        }
        if t.len() > 10 && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// Открывает ссылку в браузере по умолчанию. Только http и https, без участия командной строки.
pub fn open_url(url: &str) -> bool {
    if !(url.starts_with("http://") || url.starts_with("https://")) || url.chars().any(|c| c.is_control() || c == ' ') {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .creation_flags(0x0800_0000) // без окна консоли
            .spawn()
            .is_ok()
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn().is_ok()
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn().is_ok()
    }
}

static DEBUG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Включает подробный журнал (`tg-lite --debug`).
pub fn set_debug(on: bool) {
    DEBUG.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Подробная запись в журнал: только при `--debug`. Тексты сообщений сюда не передаются.
pub fn debug(msg: &str) {
    if DEBUG.load(std::sync::atomic::Ordering::Relaxed) {
        log(&format!("debug: {msg}"));
    }
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

/// Текст из буфера обмена (только текст). На Windows читается напрямую через WinAPI,
/// на других системах пробуем wl-paste / xclip / pbpaste.
pub fn clipboard_text() -> Option<String> {
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        unsafe extern "system" {
            fn OpenClipboard(owner: *mut c_void) -> i32;
            fn CloseClipboard() -> i32;
            fn IsClipboardFormatAvailable(format: u32) -> i32;
            fn GetClipboardData(format: u32) -> *mut c_void;
            fn GlobalLock(mem: *mut c_void) -> *mut c_void;
            fn GlobalUnlock(mem: *mut c_void) -> i32;
            fn GlobalSize(mem: *mut c_void) -> usize;
        }
        const CF_UNICODETEXT: u32 = 13;
        // Буфер может быть занят другой программой: пробуем несколько раз.
        let mut opened = false;
        for _ in 0..10 {
            if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        if !opened {
            return None;
        }
        let mut result = None;
        unsafe {
            if IsClipboardFormatAvailable(CF_UNICODETEXT) != 0 {
                let h = GetClipboardData(CF_UNICODETEXT);
                if !h.is_null() {
                    let size = GlobalSize(h) / 2;
                    let p = GlobalLock(h) as *const u16;
                    if !p.is_null() {
                        let mut len = 0;
                        while len < size && *p.add(len) != 0 {
                            len += 1;
                        }
                        result = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
                        GlobalUnlock(h);
                    }
                }
            }
            CloseClipboard();
        }
        result
    }
    #[cfg(not(windows))]
    {
        let tools: [(&str, &[&str]); 4] = [
            ("wl-paste", &["--no-newline"]),
            ("xclip", &["-selection", "clipboard", "-o"]),
            ("xsel", &["--clipboard", "--output"]),
            ("pbpaste", &[]),
        ];
        for (prog, args) in tools {
            if let Ok(out) = std::process::Command::new(prog).args(args).output() {
                if out.status.success() {
                    return Some(String::from_utf8_lossy(&out.stdout).into_owned());
                }
            }
        }
        None
    }
}

/// Приводит вставленный текст к виду для поля ввода: переводы строк `\n`, без символов,
/// которые консоль не рисует, не длиннее `max` символов.
pub fn paste_clean(s: &str, max: usize) -> String {
    let s = s.replace("\r\n", "\n").replace('\r', "\n");
    clean(&s).chars().take(max).collect()
}

/// Вставленный текст для однострочного поля (пароль, адрес): без переводов строк по краям.
pub fn paste_line(s: &str) -> String {
    s.trim_matches(|c| c == '\r' || c == '\n').replace(['\r', '\n'], "")
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
    fn paste_helpers() {
        assert_eq!(paste_clean("a\r\nb\rc", 100), "a\nb\nc");
        assert_eq!(paste_clean("abcdef", 3), "abc");
        assert_eq!(paste_line("pass\r\n"), "pass");
        assert_eq!(paste_line("a\nb"), "ab");
    }

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
    fn download_names_keep_extension_and_avoid_windows_traps() {
        assert_eq!(safe_file_name("отчёт 2026.pdf"), "отчёт 2026.pdf");
        assert_eq!(safe_file_name("a/b\\c:d.txt"), "a_b_c_d.txt");
        assert_eq!(safe_file_name("..\\..\\evil.exe"), "_.._evil.exe");
        assert_eq!(safe_file_name(".hidden"), "hidden");
        assert_eq!(safe_file_name("   "), "file");
        assert_eq!(safe_file_name("con.txt"), "_con.txt");
        let long = format!("{}.mp4", "я".repeat(200));
        let n = safe_file_name(&long);
        assert!(n.chars().count() <= 100 && n.ends_with(".mp4"), "{n}");
    }

    #[test]
    fn unique_path_adds_counter() {
        let dir = std::env::temp_dir().join(format!("tglt-uniq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = unique_path(&dir, "a.pdf");
        assert_eq!(a, dir.join("a.pdf"));
        std::fs::write(&a, b"x").unwrap();
        let b = unique_path(&dir, "a.pdf");
        assert_eq!(b, dir.join("a (1).pdf"));
        std::fs::write(&b, b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.pdf"), dir.join("a (2).pdf"));
        assert_eq!(unique_path(&dir, "noext"), dir.join("noext"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn links_are_extracted_cleanly() {
        let t = "Смотри https://example.com/a?b=1&c=2, ещё (https://rust-lang.org). и t.me/durov! www.site.ru/x https://example.com/a?b=1&c=2";
        assert_eq!(
            extract_links(t),
            vec!["https://example.com/a?b=1&c=2", "https://rust-lang.org", "https://t.me/durov", "https://www.site.ru/x"]
        );
        assert!(extract_links("просто текст без ссылок").is_empty());
        assert!(extract_links("ftp://x.y/z javascript:alert(1)").is_empty());
    }

    #[test]
    fn only_web_links_are_opened() {
        assert!(!open_url("file:///C:/Windows/System32/calc.exe"));
        assert!(!open_url("javascript:alert(1)"));
        assert!(!open_url("https://a.b/c d"));
        assert!(!open_url("calc.exe"));
    }

    #[test]
    fn keeps_normal_text() {
        let s = "Код для входа: 12345. «Не давайте» код — никому, ёЁ ї ґ";
        assert_eq!(clean(s), s);
    }
}
