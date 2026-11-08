//! Общие мелочи: папка данных и простой журнал ошибок.

use std::io::Write;
use std::path::PathBuf;

/// Папка данных: рядом с exe (удобно носить с собой). Можно переопределить переменной TGLITE_DIR.
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("TGLITE_DIR") {
        return PathBuf::from(d);
    }
    std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_else(|| PathBuf::from("."))
}

/// Короткий мягкий щелчок (WAV, около 9 КБ), встроенный в exe.
#[cfg(any(windows, test))]
static NOTIFY_WAV: &[u8] = include_bytes!("../assets/notify.wav");

/// Звуковой сигнал о новом сообщении. На Windows проигрывает встроенный щелчок через системный звук
/// (PlaySound из памяти, асинхронно, интерфейс не задерживается); если не получилось, подаёт два тона
/// функцией Beep. Не зависит от схемы системных звуков и от консоли.
pub fn play_alert() {
    #[cfg(windows)]
    {
        #[link(name = "winmm")]
        unsafe extern "system" {
            fn PlaySoundW(sound: *const u16, module: *mut core::ffi::c_void, flags: u32) -> i32;
        }
        unsafe extern "system" {
            fn Beep(freq: u32, millis: u32) -> i32;
        }
        const SND_ASYNC: u32 = 0x0001;
        const SND_NODEFAULT: u32 = 0x0002;
        const SND_MEMORY: u32 = 0x0004;
        // Буфер статический, поэтому остаётся доступным, пока звук играет в фоне.
        let ok = unsafe { PlaySoundW(NOTIFY_WAV.as_ptr().cast(), core::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT) };
        if ok == 0 {
            std::thread::spawn(|| unsafe {
                Beep(988, 80);
                Beep(1319, 120);
            });
        }
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
    if let Ok(meta) = std::fs::metadata(&path)
        && meta.len() > MAX_LOG_BYTES
    {
        let _ = std::fs::rename(&path, dir.join("tglite.old.log"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let now = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::system());
        let _ = writeln!(f, "{} {}", now.strftime("%Y-%m-%d %H:%M:%S"), msg);
    }
}

/// Имя файла из названия чата: только буквы, цифры, пробел, `-` и `_`.
pub fn safe_filename(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect();
    let s = s.trim().chars().take(40).collect::<String>();
    if s.is_empty() {
        "chat".to_string()
    } else if is_reserved_name(&s) {
        format!("_{s}")
    } else {
        s
    }
}

/// Имена устройств Windows (CON, NUL, COM1…LPT9): файл с таким именем создать нельзя.
fn is_reserved_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim().to_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit()
}

/// Имя файла для скачивания: точки и расширение сохраняются, недопустимые для Windows символы заменяются.
pub fn safe_file_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            // Управляющие символы и символы смены направления текста (имя `photo_\u{202E}fdp.exe` выглядело бы как `photo_exe.pdf`).
            let bidi = matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}');
            if c.is_control() || bidi || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }
        })
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
    // Windows отбрасывает пробелы и точки в конце имени.
    let s = s.trim_end_matches([' ', '.']).to_string();
    if s.is_empty() {
        "file".to_string()
    } else if is_reserved_name(&s) {
        format!("_{s}")
    } else {
        s
    }
}

/// Занимает свободное имя в папке (создаёт пустой файл): `a.pdf`, `a (1).pdf`, …
/// Два одновременных скачивания не получат одно и то же имя и ничего не перезапишут.
pub fn reserve_unique(dir: &std::path::Path, name: &str) -> std::io::Result<PathBuf> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 0..1000 {
        let file = if n == 0 { name.to_string() } else { format!("{stem} ({n}){ext}") };
        let p = dir.join(file);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(_) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "too many files with the same name"))
}

/// Находит ссылки в тексте (http, https, t.me, www), без хвостовой пунктуации и без повторов.
pub fn extract_links(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tok in text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '«' | '»')) {
        let t = tok.trim_start_matches(['(', '[', '\'']);
        let lower = t.to_lowercase();
        let is_link =
            lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("t.me/") || lower.starts_with("www.");
        if !is_link {
            continue;
        }
        let mut t = t.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '\'']).to_string();
        // Схема всегда в нижнем регистре: `HTTPS://x.com` иначе нашлась бы, но не открылась.
        for scheme in ["https://", "http://"] {
            if lower.starts_with(scheme) {
                if let Some(rest) = t.get(scheme.len()..) {
                    t = format!("{scheme}{rest}");
                }
                break;
            }
        }
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
            if let Ok(out) = std::process::Command::new(prog).args(args).output()
                && out.status.success()
            {
                return Some(String::from_utf8_lossy(&out.stdout).into_owned());
            }
        }
        None
    }
}

/// Картинка из буфера обмена (скриншот) в виде PNG. На Windows читается формат CF_DIB,
/// на других системах пробуем wl-paste / xclip.
pub fn clipboard_image_png() -> Option<Vec<u8>> {
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
        const CF_DIB: u32 = 8;
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
        let mut data: Option<Vec<u8>> = None;
        unsafe {
            if IsClipboardFormatAvailable(CF_DIB) != 0 {
                let h = GetClipboardData(CF_DIB);
                if !h.is_null() {
                    let size = GlobalSize(h);
                    let p = GlobalLock(h) as *const u8;
                    if !p.is_null() {
                        data = Some(std::slice::from_raw_parts(p, size).to_vec());
                        GlobalUnlock(h);
                    }
                }
            }
            CloseClipboard();
        }
        dib_to_png(&data?)
    }
    #[cfg(not(windows))]
    {
        let tools: [(&str, &[&str]); 2] =
            [("wl-paste", &["--type", "image/png"]), ("xclip", &["-selection", "clipboard", "-t", "image/png", "-o"])];
        for (prog, args) in tools {
            if let Ok(out) = std::process::Command::new(prog).args(args).output()
                && out.status.success()
                && out.stdout.starts_with(&[0x89, b'P', b'N', b'G'])
            {
                return Some(out.stdout);
            }
        }
        None
    }
}

/// Превращает «DIB» из буфера обмена Windows (24 или 32 бита на точку) в PNG.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |o: usize| dib.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let header = u32_at(0)? as usize;
    let width = u32_at(4)? as i32;
    let height = u32_at(8)? as i32;
    let bits = dib.get(14..16).map(|b| u16::from_le_bytes([b[0], b[1]]))? as usize;
    let compression = u32_at(16)?;
    if header < 40 || width <= 0 || height == 0 || !(bits == 24 || bits == 32) || !(compression == 0 || compression == 3) {
        return None;
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    if w > 16_384 || h > 16_384 {
        return None;
    }
    let top_down = height < 0;
    let offset = header + if compression == 3 && header == 40 { 12 } else { 0 };
    let stride = (w * bits).div_ceil(32) * 4;
    let bytes = bits / 8;
    if dib.len() < offset + stride * h {
        return None;
    }
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let row = if top_down { y } else { h - 1 - y };
        let line = &dib[offset + row * stride..offset + row * stride + w * bytes];
        for px in line.chunks_exact(bytes) {
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let mut writer = enc.write_header().ok()?;
    writer.write_image_data(&rgb).ok()?;
    writer.finish().ok()?;
    Some(out)
}

/// Декодирует JPEG в RGB: (ширина, высота, точки подряд).
pub fn decode_jpeg(bytes: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let mut dec = jpeg_decoder::Decoder::new(bytes);
    let data = dec.decode().ok()?;
    let info = dec.info()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let rgb = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => data,
        jpeg_decoder::PixelFormat::L8 => data.iter().flat_map(|&g| [g, g, g]).collect(),
        _ => return None,
    };
    (rgb.len() == w * h * 3 && w > 0 && h > 0).then_some((w, h, rgb))
}

/// Размер картинки `w`×`h` точек, вписанной в `cols` столбцов и `rows` строк консоли.
/// Каждая ячейка — две точки по вертикали («▀»), поэтому по высоте точек вдвое больше, чем строк.
pub fn fit_cells(w: usize, h: usize, cols: usize, rows: usize) -> (usize, usize) {
    if w == 0 || h == 0 || cols == 0 || rows == 0 {
        return (0, 0);
    }
    let (maxw, maxh) = (cols, rows * 2);
    // Масштаб в точках; увеличивать маленькие картинки в разы не нужно, но на пару раз можно.
    let scale = (maxw as f64 / w as f64).min(maxh as f64 / h as f64).min(3.0);
    let pw = ((w as f64 * scale).round() as usize).clamp(1, maxw);
    let ph = ((h as f64 * scale).round() as usize).clamp(2, maxh);
    (pw, ph.div_ceil(2) * 2)
}

/// Уменьшает RGB-картинку до `pw`×`ph` точек усреднением (чётче, чем выбор ближайшей).
pub fn shrink_rgb(w: usize, h: usize, rgb: &[u8], pw: usize, ph: usize) -> Vec<[u8; 3]> {
    let mut out = Vec::with_capacity(pw * ph);
    for y in 0..ph {
        let (y0, y1) = (y * h / ph, ((y + 1) * h / ph).max(y * h / ph + 1).min(h));
        for x in 0..pw {
            let (x0, x1) = (x * w / pw, ((x + 1) * w / pw).max(x * w / pw + 1).min(w));
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let i = (yy * w + xx) * 3;
                    r += u32::from(rgb[i]);
                    g += u32::from(rgb[i + 1]);
                    b += u32::from(rgb[i + 2]);
                    n += 1;
                }
            }
            out.push([(r / n) as u8, (g / n) as u8, (b / n) as u8]);
        }
    }
    out
}

/// Кладёт текст в буфер обмена. На Windows через встроенную программу `clip` (кодировка UTF-16 с меткой порядка
/// байтов, поэтому русский текст и эмодзи не портятся), на других системах через wl-copy / xclip / xsel / pbcopy.
pub fn clipboard_set(text: &str) -> bool {
    use std::process::{Command, Stdio};
    let run = |mut cmd: Command, data: &[u8]| -> bool {
        let Ok(mut child) = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { return false };
        if let Some(mut stdin) = child.stdin.take()
            && stdin.write_all(data).is_err()
        {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
        // Ждём не дольше пары секунд: зависшая программа буфера обмена не должна замораживать интерфейс.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(10)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
            }
        }
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new("clip");
        cmd.creation_flags(0x0800_0000); // без окна консоли
        run(cmd, &utf16le_with_bom(&windows_newlines(text)))
    }
    #[cfg(not(windows))]
    {
        let tools: [(&str, &[&str]); 4] =
            [("wl-copy", &[]), ("xclip", &["-selection", "clipboard", "-i"]), ("xsel", &["--clipboard", "--input"]), ("pbcopy", &[])];
        for (prog, args) in tools {
            let mut cmd = Command::new(prog);
            cmd.args(args);
            if run(cmd, text.as_bytes()) {
                return true;
            }
        }
        false
    }
}

/// Переводы строк Windows (`\r\n`): без них многострочный текст вставляется в Блокнот одной строкой.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n").replace('\n', "\r\n")
}

/// Текст в UTF-16 (little endian) с меткой порядка байтов: так `clip.exe` понимает Юникод.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn utf16le_with_bom(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for u in text.encode_utf16() {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

/// Расширения, которые можно открывать программой по умолчанию прямо из чата: картинки, PDF, простой текст,
/// звук и видео. Исполняемые файлы, скрипты, ярлыки и документы Office с макросами сюда не входят.
const OPENABLE: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "pdf", "txt", "md", "log", "json", "mp3", "ogg", "oga", "opus", "m4a", "wav", "flac",
    "mp4", "mkv", "webm", "avi", "mov",
];

/// Максимальный размер файла для `/open` (байт): больше качайте через `/save`.
pub const MAX_OPEN_BYTES: u64 = 50 * 1024 * 1024;

/// Можно ли открывать файл с таким именем программой по умолчанию.
pub fn is_openable(file_name: &str) -> bool {
    match file_name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => OPENABLE.contains(&ext.to_ascii_lowercase().as_str()),
        _ => false,
    }
}

/// Папка для файлов, скачанных только для просмотра (`/open`): `tglite-view` рядом с программой.
/// Имя особое, чтобы очистка никогда не задела чужую папку.
pub fn view_dir() -> PathBuf {
    data_dir().join("tglite-view")
}

/// Удаляет файлы из папки. Возвращает, сколько удалить не удалось (например, файл ещё открыт в просмотрщике).
pub fn clear_dir(dir: &std::path::Path) -> usize {
    // Ссылка или точка соединения вместо папки: не идём по ней, чтобы не стереть чужое.
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
        _ => return 0,
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut left = 0;
    for e in rd.flatten() {
        let p = e.path();
        let is_real_dir = e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink());
        let ok = if is_real_dir { std::fs::remove_dir_all(&p).is_ok() } else { std::fs::remove_file(&p).is_ok() };
        if !ok {
            left += 1;
        }
    }
    if left == 0 {
        let _ = std::fs::remove_dir(dir);
    }
    left
}

/// Стирает временные файлы просмотра. Вызывается при блокировке, выходе и запуске.
pub fn clear_view_dir() -> usize {
    clear_dir(&view_dir())
}

/// Открывает файл программой по умолчанию. Только файлы из списка разрешённых расширений.
pub fn open_path(path: &std::path::Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !is_openable(name) || !path.is_file() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("rundll32").arg("url.dll,FileProtocolHandler").arg(path).creation_flags(0x0800_0000).spawn().is_ok()
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn().is_ok()
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn().is_ok()
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

/// Смайлик реакции без невидимого селектора вариации: так «❤» и «❤️» считаются одним и тем же.
pub fn reaction_key(emoji: &str) -> String {
    emoji.chars().filter(|c| *c != '\u{fe0f}').collect()
}

/// Как показать реакцию в консоли, где эмодзи не рисуются: короткое слово вместо картинки.
pub fn reaction_label(emoji: &str) -> &'static str {
    match reaction_key(emoji).as_str() {
        "👍" => "+1",
        "👎" => "-1",
        "❤" | "💘" | "💕" => "<3",
        "🔥" => "fire",
        "😂" | "🤣" => "lol",
        "😮" | "😲" => "wow",
        "😢" | "😭" => "sad",
        "🎉" => "tada",
        "🤔" => "hmm",
        "👏" => "clap",
        "🙏" => "pray",
        "🤯" => "boom",
        "😁" | "😀" => "grin",
        "🥰" | "😍" => "love",
        "🤬" | "😡" => "angry",
        "💩" => "poo",
        "👌" => "ok",
        "😱" => "omg",
        "🤮" => "yuck",
        "🥱" => "yawn",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn notify_sound_is_a_small_valid_wav() {
        let w = super::NOTIFY_WAV;
        assert!(w.len() > 1000 && w.len() < 20_000, "{}", w.len());
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([w[20], w[21]]), 1, "обычный PCM");
    }

    use super::*;

    #[test]
    fn file_names_are_safe_on_windows() {
        assert_eq!(safe_file_name("photo_\u{202E}fdp.exe"), "photo__fdp.exe");
        assert_eq!(safe_file_name("nul.txt"), "_nul.txt");
        assert_eq!(safe_file_name("COM7.log"), "_COM7.log");
        assert_eq!(safe_file_name("lpt9"), "_lpt9");
        assert_eq!(safe_file_name("nul .txt"), "_nul .txt");
        assert_eq!(safe_file_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_file_name("report. . "), "report");
        assert_eq!(safe_file_name("..."), "file");
        assert_eq!(safe_file_name("COM10.txt"), "COM10.txt"); // не устройство
        assert_eq!(safe_filename("con"), "_con");
        assert_eq!(safe_filename("Рабочий чат"), "Рабочий чат");
    }

    #[test]
    fn only_harmless_file_types_can_be_opened() {
        for ok in ["a.jpg", "A.PNG", "doc.pdf", "notes.txt", "v.mp4", "song.mp3", "a.b.webp"] {
            assert!(is_openable(ok), "{ok}");
        }
        for bad in [
            "a.csv",
            "a.xml",
            "a.exe",
            "a.EXE",
            "a.bat",
            "a.cmd",
            "a.scr",
            "a.msi",
            "a.ps1",
            "a.vbs",
            "a.js",
            "a.lnk",
            "a.hta",
            "a.jar",
            "a.reg",
            "a.dll",
            "a.docm",
            "a.xlsm",
            "a.zip",
            "a.svg",
            "a.html",
            "a.jpg.exe",
            "noext",
            ".jpg",
            "photo.",
            "",
        ] {
            assert!(!is_openable(bad), "{bad}");
        }
        assert!(!open_path(std::path::Path::new("/definitely/not/here.png")));
    }

    #[test]
    fn utf16_for_clip_has_bom_and_handles_non_bmp() {
        assert_eq!(utf16le_with_bom(""), vec![0xFF, 0xFE]);
        assert_eq!(utf16le_with_bom("Aя"), vec![0xFF, 0xFE, 0x41, 0x00, 0x4F, 0x04]);
        // эмодзи за пределами BMP: пара суррогатов
        assert_eq!(utf16le_with_bom("\u{1F600}"), vec![0xFF, 0xFE, 0x3D, 0xD8, 0x00, 0xDE]);
    }

    #[test]
    fn newlines_become_crlf_for_windows() {
        assert_eq!(windows_newlines("a\nb"), "a\r\nb");
        assert_eq!(windows_newlines("a\r\nb\rc"), "a\r\nb\r\nc");
        assert_eq!(windows_newlines("one"), "one");
    }

    #[cfg(unix)]
    #[test]
    fn clear_dir_does_not_follow_symlinks() {
        let base = std::env::temp_dir().join(format!("tglite-test-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let target = base.join("precious");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("keep.txt"), b"keep").unwrap();
        let link = base.join("view");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(clear_dir(&link), 0);
        assert!(target.join("keep.txt").exists(), "содержимое цели ссылки цело");
        // вложенная ссылка внутри папки просмотра удаляется как ссылка, цель не трогается
        let dir = base.join("tglite-view");
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(&target, dir.join("inner")).unwrap();
        clear_dir(&dir);
        assert!(target.join("keep.txt").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn clear_dir_removes_files_and_the_folder() {
        let dir = std::env::temp_dir().join(format!("tglite-test-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.png"), b"x").unwrap();
        std::fs::write(dir.join("sub").join("b.txt"), b"y").unwrap();
        assert_eq!(clear_dir(&dir), 0);
        assert!(!dir.exists());
        assert_eq!(clear_dir(&dir), 0, "нет папки: не ошибка");
    }

    #[test]
    fn uppercase_scheme_links_are_normalized_and_openable() {
        let l = extract_links("см. HTTPS://Example.com/Path, и Http://a.bc/x");
        assert_eq!(l, vec!["https://Example.com/Path", "http://a.bc/x"]);
        assert!(l.iter().all(|u| u.starts_with("http")));
    }

    #[test]
    fn reserve_unique_never_reuses_a_name() {
        let dir = std::env::temp_dir().join(format!("tglt-res-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = reserve_unique(&dir, "a.pdf").unwrap();
        let b = reserve_unique(&dir, "a.pdf").unwrap();
        let c = reserve_unique(&dir, "noext").unwrap();
        let d = reserve_unique(&dir, "noext").unwrap();
        assert_eq!(a, dir.join("a.pdf"));
        assert_eq!(b, dir.join("a (1).pdf"));
        assert_eq!(c, dir.join("noext"));
        assert_eq!(d, dir.join("noext (1)"));
        // параллельно: все имена разные
        let names: Vec<_> = (0..8)
            .map(|_| {
                let dir = dir.clone();
                std::thread::spawn(move || reserve_unique(&dir, "x.zip").unwrap())
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect();
        let set: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(set.len(), 8);
        let _ = std::fs::remove_dir_all(&dir);
    }

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
    fn links_are_extracted_cleanly() {
        let t =
            "Смотри https://example.com/a?b=1&c=2, ещё (https://rust-lang.org). и t.me/durov! www.site.ru/x https://example.com/a?b=1&c=2";
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

#[cfg(test)]
mod media_tests {
    use super::*;

    #[test]
    fn dib_from_the_windows_clipboard_becomes_a_png() {
        // 2×2, 24 бита, строки снизу вверх: нижний ряд красный/зелёный, верхний синий/белый
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&2i32.to_le_bytes());
        dib.extend_from_slice(&2i32.to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&24u16.to_le_bytes());
        dib.extend_from_slice(&[0u8; 24]);
        for row in [[[0u8, 0, 255], [0, 255, 0]], [[255, 0, 0], [255, 255, 255]]] {
            for px in row {
                dib.extend_from_slice(&px); // BGR
            }
            dib.extend_from_slice(&[0, 0]); // строка выравнивается до 4 байт
        }
        let png = dib_to_png(&dib).expect("png");
        assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);
        let mut reader = png::Decoder::new(png.as_slice()).read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        reader.next_frame(&mut buf).unwrap();
        assert_eq!(&buf[..6], &[0, 0, 255, 255, 255, 255][..], "верхний ряд: синий и белый");
        assert_eq!(&buf[6..12], &[255, 0, 0, 0, 255, 0][..], "нижний ряд: красный и зелёный");
    }

    #[test]
    fn broken_or_unsupported_dib_is_refused() {
        assert!(dib_to_png(&[]).is_none());
        assert!(dib_to_png(&[0; 40]).is_none());
        let mut dib = vec![0u8; 44];
        dib[..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&100i32.to_le_bytes());
        dib[8..12].copy_from_slice(&100i32.to_le_bytes());
        dib[14..16].copy_from_slice(&24u16.to_le_bytes());
        assert!(dib_to_png(&dib).is_none(), "данных меньше, чем заявлено");
        dib[14..16].copy_from_slice(&8u16.to_le_bytes());
        assert!(dib_to_png(&dib).is_none(), "палитровые картинки не поддерживаются");
    }

    #[test]
    fn preview_scaling_keeps_aspect_and_fits_the_window() {
        assert_eq!(fit_cells(800, 400, 100, 40), (100, 50));
        let (w, h) = fit_cells(300, 600, 100, 20);
        assert!(w <= 100 && h <= 40 && h % 2 == 0);
        assert!(w * 2 <= h + 2, "портретная картинка осталась портретной");
        assert_eq!(fit_cells(0, 10, 10, 10), (0, 0));
        let px = shrink_rgb(2, 2, &[10, 10, 10, 30, 30, 30, 50, 50, 50, 70, 70, 70], 1, 1);
        assert_eq!(px, vec![[40, 40, 40]], "усреднение");
    }

    #[test]
    fn jpeg_decoding_rejects_garbage() {
        assert!(decode_jpeg(b"not a jpeg").is_none());
        assert!(decode_jpeg(&[]).is_none());
    }
}
