//! Сведения о компьютере для `/report`: точная версия Windows, процессор, память, диск, терминал, процесс.
//!
//! Значения собирает тонкая обёртка (реестр и несколько вызовов kernel32 на Windows, `/proc` в Linux),
//! а разбор и оформление не зависят от системы и проверяются тестами. Вызовы Windows описаны в
//! модуле `ffi`: на других системах вместо них стоят заглушки с теми же сигнатурами, поэтому весь код
//! вокруг них собирается и проверяется на любой системе.
//!
//! В отчёт не попадают переписка, пароли, номер телефона, имя пользователя Windows и адреса прокси.

use std::ffi::c_void;
use std::fmt::Write as _;
use std::ptr::null_mut;

/// Раздел отчёта: заголовок и строки «имя: значение».
pub struct Section {
    pub title: &'static str,
    pub rows: Vec<(String, String)>,
}

impl Section {
    pub fn new(title: &'static str) -> Self {
        Self { title, rows: Vec::new() }
    }

    /// Добавляет строку; пустые значения пропускаются.
    pub fn add(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        let value = value.trim();
        if !value.is_empty() {
            self.rows.push((key.to_string(), value.to_string()));
        }
    }
}

/// Оформляет разделы текстом: `[раздел]`, затем выровненные строки `имя : значение`.
pub fn render(sections: &[Section]) -> String {
    let mut out = String::new();
    for s in sections.iter().filter(|s| !s.rows.is_empty()) {
        let _ = writeln!(out, "[{}]", s.title);
        let width = s.rows.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
        for (k, v) in &s.rows {
            let _ = writeln!(out, "{k:<width$} : {v}");
        }
        out.push('\n');
    }
    out
}

// ------------------------------------------------------------------ чистые помощники

/// Название Windows по номеру версии: в реестре у Windows 11 до сих пор написано «Windows 10».
pub fn windows_name(major: u32, minor: u32, build: u32) -> &'static str {
    match (major, minor) {
        (10, 0) if build >= 22000 => "Windows 11",
        (10, 0) => "Windows 10",
        (6, 3) => "Windows 8.1",
        (6, 2) => "Windows 8",
        (6, 1) => "Windows 7",
        (6, 0) => "Windows Vista",
        (5, 2) => "Windows XP x64 / Server 2003",
        (5, 1) => "Windows XP",
        _ => "Windows",
    }
}

/// Название продукта: из реестра (`ProductName`), но с правкой для Windows 11.
pub fn windows_product(product_name: &str, major: u32, minor: u32, build: u32) -> String {
    let product = product_name.trim();
    if product.is_empty() {
        return windows_name(major, minor, build).to_string();
    }
    if build >= 22000
        && major == 10
        && let Some(rest) = product.strip_prefix("Windows 10")
    {
        return format!("Windows 11{rest}");
    }
    product.to_string()
}

/// Разрядность по значению `PROCESSOR_ARCHITECTURE` или `std::env::consts::ARCH`.
pub fn arch_label(raw: &str) -> &'static str {
    match raw.to_ascii_lowercase().as_str() {
        "amd64" | "x86_64" | "x64" => "x64",
        "x86" | "i386" | "i586" | "i686" => "x86",
        "arm64" | "aarch64" => "ARM64",
        "ia64" => "IA-64",
        _ => "unknown",
    }
}

/// Размер для человека: `512 B`, `12 KB`, `3.4 MB`, `1.9 GB`.
pub fn bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    let f = n as f64;
    if f >= KB * KB * KB {
        format!("{:.1} GB", f / (KB * KB * KB))
    } else if f >= KB * KB {
        format!("{:.1} MB", f / (KB * KB))
    } else if f >= KB {
        format!("{:.0} KB", f / KB)
    } else {
        format!("{n} B")
    }
}

/// Длительность: `45 s`, `12 min 5 s`, `3 h 12 min`, `2 d 4 h`.
pub fn duration(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60, secs % 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else if m > 0 {
        format!("{m} min {s} s")
    } else {
        format!("{s} s")
    }
}

/// Сворачивает повторные пробелы: в названиях процессоров их бывает много.
pub fn squeeze(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Убирает из текста имя пользователя: `C:\Users\Иван\…` → `C:\Users\<user>\…`, `/home/ivan/…` → `/home/<user>/…`,
/// а само имя (из `USERNAME`/`USER`) заменяется везде, кроме самых общих («user», «admin» и т. п.).
pub fn hide_user(text: &str, user_name: Option<&str>) -> String {
    const MARK: &str = "\u{1}"; // временная метка: настоящее имя не должно путаться с уже вставленным «<user>»
    const COMMON: [&str; 7] = ["user", "users", "admin", "administrator", "owner", "guest", "default"];
    let mut out = text.to_string();
    if let Some(name) = user_name.map(str::trim).filter(|n| n.chars().count() >= 3 && !COMMON.contains(&n.to_lowercase().as_str())) {
        for variant in [name.to_string(), name.to_lowercase(), name.to_uppercase()] {
            out = out.replace(&variant, MARK);
        }
    }
    // Сравниваем по строке в нижнем регистре только для ASCII: длины в байтах при этом не меняются.
    for marker in ["\\users\\", "/users/", "/home/"] {
        let mut from = 0;
        while from < out.len() {
            let Some(rel) = out.to_ascii_lowercase()[from..].find(marker) else { break };
            let start = from + rel + marker.len();
            let end = out[start..].find(['\\', '/', ' ', '"', '\'']).map_or(out.len(), |i| start + i);
            if end > start && out[start..end] != *"<user>" {
                out.replace_range(start..end, "<user>");
                from = start + "<user>".len();
            } else {
                from = start.max(end);
            }
        }
    }
    out.replace(MARK, "<user>")
}

/// Адрес прокси без самого адреса: тип и «локальный/удалённый».
pub fn describe_proxy(addr: &str) -> String {
    let a = addr.trim();
    if a.is_empty() {
        return "none".into();
    }
    let kind = match a.split_once("://").map(|(s, _)| s.to_ascii_lowercase()) {
        Some(s) if s.starts_with("socks") => "socks5",
        Some(s) if s.starts_with("http") => "http",
        _ => "address",
    };
    format!("{kind}, {} (host and port hidden)", if is_local_host(host_of(a)) { "local" } else { "remote" })
}

/// Метка на месте спрятанного адреса.
const HIDDEN: &str = "<hidden>";

/// Адрес этого же компьютера: в нём нет ничего личного, а для разбора неполадок он полезен.
fn is_local_host(host: &str) -> bool {
    let h = host.trim().trim_matches(['[', ']']).to_ascii_lowercase();
    h == "localhost" || h == "::1" || h.starts_with("127.")
}

/// `хост:порт` из `схема://логин:пароль@хост:порт/путь`, `логин:пароль@хост:порт` или просто `хост:порт`.
fn host_port_of(addr: &str) -> &str {
    let a = addr.trim();
    let a = a.split_once("://").map_or(a, |(_, r)| r);
    let a = a.split(['/', '?', '#']).next().unwrap_or(a);
    a.rsplit_once('@').map_or(a, |(_, hp)| hp)
}

/// Хост из любой из форм выше; порт отбрасывается, `[::1]:1080` даёт `::1`.
fn host_of(addr: &str) -> &str {
    let hp = host_port_of(addr);
    if let Some(rest) = hp.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match hp.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => h,
        _ => hp,
    }
}

/// Замена без учёта регистра ASCII (имена хостов регистра не различают). Пустой образец ничего не меняет.
fn replace_ci(text: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return text.to_string();
    }
    if !needle.is_ascii() {
        return text.replace(needle, with);
    }
    // Нижний регистр только для ASCII не меняет длины в байтах, поэтому позиции годятся и для исходной строки.
    let (lower, n) = (text.to_ascii_lowercase(), needle.to_ascii_lowercase());
    let (mut out, mut last) = (String::with_capacity(text.len()), 0);
    while let Some(rel) = lower[last..].find(&n) {
        out.push_str(&text[last..last + rel]);
        out.push_str(with);
        last += rel + n.len();
    }
    out.push_str(&text[last..]);
    out
}

/// Ссылки `схема://логин:пароль@хост:порт/…`: чужой адрес заменяется на `<hidden>`, адрес этого компьютера остаётся
/// (без логина и пароля).
fn hide_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        let end =
            tail.find(|c: char| c.is_whitespace() || matches!(c, '/' | '?' | '#' | '"' | '\'' | ')' | ',' | ';')).unwrap_or(tail.len());
        let (authority, after) = tail.split_at(end);
        // Знаки препинания в конце («…:1080: timeout») к адресу не относятся.
        let core = authority.trim_end_matches([':', '.', '!']);
        if !core.is_empty() {
            if core == HIDDEN {
                out.push_str(HIDDEN);
            } else if is_local_host(host_of(core)) {
                out.push_str(host_port_of(core));
            } else {
                out.push_str(HIDDEN);
            }
            out.push_str(&authority[core.len()..]);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Адреса без `схемы://`, как их пишет сама программа: «via SOCKS5 proxy ***@host:1080», «через HTTP-прокси host:3128»,
/// «bridge for host:1080». Слово после «proxy»/«прокси» заменяется, только если похоже на адрес.
fn hide_after_markers(text: &str) -> String {
    let mut out = text.to_string();
    for marker in ["proxy ", "прокси ", "bridge for "] {
        let mut from = 0;
        while from < out.len() {
            let lower = out.to_ascii_lowercase();
            let Some(rel) = lower[from..].find(marker) else { break };
            let start = from + rel + marker.len();
            let end = out[start..].find(char::is_whitespace).map_or(out.len(), |i| start + i);
            let core = out[start..end].trim_end_matches([':', ',', ';', '.', ')']);
            let address_like = core.contains(['@', '.', ':']) || core.chars().any(|c| c.is_ascii_digit());
            if address_like && core != HIDDEN && !is_local_host(host_of(core)) {
                let len = core.len();
                out.replace_range(start..start + len, HIDDEN);
                from = start + HIDDEN.len();
            } else {
                from = end.max(start);
            }
        }
    }
    out
}

/// Адреса прокси, которые заданы сейчас (в настройках и переменных окружения): их прячем везде, где бы они ни встретились.
pub fn known_proxy_addresses(manual: &str) -> Vec<String> {
    let mut known = vec![manual.to_string()];
    for name in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
        for var in [name.to_string(), name.to_lowercase()] {
            if let Ok(v) = std::env::var(&var) {
                known.push(v);
            }
        }
    }
    known.retain(|k| !k.trim().is_empty());
    known
}

/// Прячет адреса прокси в произвольном тексте (журнал, результат проверки связи): ссылки, адреса после слова
/// «proxy»/«прокси» и точные адреса из `known`, а заодно их логин и пароль. Адреса этого компьютера остаются.
pub fn hide_addresses(text: &str, known: &[String]) -> String {
    let mut out = hide_after_markers(&hide_urls(text));
    for k in known {
        let k = k.trim();
        let host = host_of(k);
        if host.chars().count() >= 3 && !is_local_host(host) {
            out = replace_ci(&replace_ci(&out, host_port_of(k), HIDDEN), host, HIDDEN);
        }
        if let (Some((user, pass)), _) = crate::proxy::split_auth(k.split_once("://").map_or(k, |(_, r)| r)) {
            for secret in [user, pass] {
                if secret.chars().count() >= 3 {
                    out = out.replace(&secret, HIDDEN);
                }
            }
        }
    }
    out
}

/// Значение строки `Имя: 123 kB` из `/proc/meminfo` или `/proc/self/status`, в килобайтах.
pub fn proc_kb(text: &str, name: &str) -> Option<u64> {
    text.lines().find_map(|l| {
        let rest = l.strip_prefix(name)?.strip_prefix(':')?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

// ------------------------------------------------------------------ вызовы Windows

#[repr(C)]
struct MemoryStatusEx {
    length: u32,
    load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page: u64,
    avail_page: u64,
    total_virt: u64,
    avail_virt: u64,
    avail_ext: u64,
}

#[repr(C)]
struct ProcessMemoryCounters {
    cb: u32,
    page_faults: u32,
    peak_working_set: usize,
    working_set: usize,
    quota_peak_paged: usize,
    quota_paged: usize,
    quota_peak_nonpaged: usize,
    quota_nonpaged: usize,
    pagefile: usize,
    peak_pagefile: usize,
}

/// Вызовы Windows API. На других системах их заменяют заглушки, которые сообщают «не получилось».
#[allow(non_snake_case, clippy::missing_safety_doc)]
mod ffi {
    use super::{MemoryStatusEx, ProcessMemoryCounters};
    use std::ffi::c_void;

    #[cfg(windows)]
    #[link(name = "advapi32")]
    unsafe extern "system" {
        pub fn RegGetValueW(
            hkey: isize,
            sub_key: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut c_void,
            len: *mut u32,
        ) -> i32;
    }

    #[cfg(windows)]
    unsafe extern "system" {
        pub fn GlobalMemoryStatusEx(buf: *mut MemoryStatusEx) -> i32;
        pub fn GetDiskFreeSpaceExW(dir: *const u16, available: *mut u64, total: *mut u64, free: *mut u64) -> i32;
        pub fn GetTickCount64() -> u64;
        pub fn GetUserDefaultLocaleName(buf: *mut u16, len: i32) -> i32;
        pub fn GetConsoleOutputCP() -> u32;
        pub fn GetConsoleCP() -> u32;
        pub fn GetCurrentProcess() -> isize;
        pub fn K32GetProcessMemoryInfo(process: isize, counters: *mut ProcessMemoryCounters, size: u32) -> i32;
    }

    #[cfg(not(windows))]
    pub unsafe fn RegGetValueW(_: isize, _: *const u16, _: *const u16, _: u32, _: *mut u32, _: *mut c_void, _: *mut u32) -> i32 {
        2 // ERROR_FILE_NOT_FOUND
    }
    #[cfg(not(windows))]
    pub unsafe fn GlobalMemoryStatusEx(_: *mut MemoryStatusEx) -> i32 {
        0
    }
    #[cfg(not(windows))]
    pub unsafe fn GetDiskFreeSpaceExW(_: *const u16, _: *mut u64, _: *mut u64, _: *mut u64) -> i32 {
        0
    }
    #[cfg(not(windows))]
    pub unsafe fn GetTickCount64() -> u64 {
        0
    }
    #[cfg(not(windows))]
    pub unsafe fn GetUserDefaultLocaleName(_: *mut u16, _: i32) -> i32 {
        0
    }
    #[cfg(not(windows))]
    pub unsafe fn GetConsoleOutputCP() -> u32 {
        0
    }
    #[cfg(not(windows))]
    pub unsafe fn GetConsoleCP() -> u32 {
        0
    }
    #[cfg(not(windows))]
    pub unsafe fn GetCurrentProcess() -> isize {
        -1
    }
    #[cfg(not(windows))]
    pub unsafe fn K32GetProcessMemoryInfo(_: isize, _: *mut ProcessMemoryCounters, _: u32) -> i32 {
        0
    }
}

const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
const RRF_RT_REG_SZ: u32 = 0x0000_0002;
const RRF_RT_REG_DWORD: u32 = 0x0000_0010;
const RRF_SUBKEY_WOW6464KEY: u32 = 0x0001_0000;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Строка из реестра (`REG_SZ`).
fn reg_string(root: isize, key: &str, name: &str) -> Option<String> {
    let (key, name) = (wide(key), wide(name));
    let flags = RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY;
    let mut size: u32 = 0;
    // Безопасность: обе строки оканчиваются нулём; первый вызов без буфера только сообщает нужный размер в байтах.
    let rc = unsafe { ffi::RegGetValueW(root, key.as_ptr(), name.as_ptr(), flags, null_mut(), null_mut(), &mut size) };
    if rc != 0 || !(2..=64 * 1024).contains(&size) {
        return None;
    }
    let mut buf = vec![0u16; size as usize / 2 + 1];
    let mut size = (buf.len() * 2) as u32;
    // Безопасность: буфер и его длина в байтах согласованы.
    let rc =
        unsafe { ffi::RegGetValueW(root, key.as_ptr(), name.as_ptr(), flags, null_mut(), buf.as_mut_ptr().cast::<c_void>(), &mut size) };
    if rc != 0 {
        return None;
    }
    let units = (size as usize / 2).min(buf.len());
    let text = String::from_utf16_lossy(&buf[..units]);
    let text = text.trim_end_matches('\0').trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Число из реестра (`REG_DWORD`).
fn reg_dword(root: isize, key: &str, name: &str) -> Option<u32> {
    let (key, name) = (wide(key), wide(name));
    let mut value: u32 = 0;
    let mut size: u32 = 4;
    // Безопасность: под число выделено 4 байта, размер передан.
    let rc = unsafe {
        ffi::RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD | RRF_SUBKEY_WOW6464KEY,
            null_mut(),
            (&mut value as *mut u32).cast::<c_void>(),
            &mut size,
        )
    };
    (rc == 0).then_some(value)
}

/// Память компьютера: (всего, свободно, занято в процентах, файл подкачки всего, свободно).
fn memory_status() -> Option<(u64, u64, u32, u64, u64)> {
    let mut m = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page: 0,
        avail_page: 0,
        total_virt: 0,
        avail_virt: 0,
        avail_ext: 0,
    };
    // Безопасность: структура целиком наша, длина записана в первое поле, как требует вызов.
    let ok = unsafe { ffi::GlobalMemoryStatusEx(&mut m) } != 0;
    (ok && m.total_phys > 0).then_some((m.total_phys, m.avail_phys, m.load, m.total_page, m.avail_page))
}

/// Память этой программы: (рабочий набор, пик, выделено под неё).
fn process_memory() -> Option<(u64, u64, u64)> {
    let mut c = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        page_faults: 0,
        peak_working_set: 0,
        working_set: 0,
        quota_peak_paged: 0,
        quota_paged: 0,
        quota_peak_nonpaged: 0,
        quota_nonpaged: 0,
        pagefile: 0,
        peak_pagefile: 0,
    };
    let size = c.cb;
    // Безопасность: `GetCurrentProcess` возвращает псевдодескриптор, закрывать его не нужно; структура наша, размер передан.
    let ok = unsafe { ffi::K32GetProcessMemoryInfo(ffi::GetCurrentProcess(), &mut c, size) } != 0;
    (ok && c.working_set > 0).then_some((c.working_set as u64, c.peak_working_set as u64, c.pagefile as u64))
}

/// Место на диске, где лежит папка: (всего, свободно).
fn disk_space(dir: &std::path::Path) -> Option<(u64, u64)> {
    let path = wide(&dir.to_string_lossy());
    let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
    // Безопасность: путь оканчивается нулём, три числа наши.
    let ok = unsafe { ffi::GetDiskFreeSpaceExW(path.as_ptr(), &mut available, &mut total, &mut free) } != 0;
    (ok && total > 0).then_some((total, available))
}

fn locale_name() -> Option<String> {
    let mut buf = [0u16; 85]; // LOCALE_NAME_MAX_LENGTH
    // Безопасность: длина буфера передана в символах.
    let n = unsafe { ffi::GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    (n > 1).then(|| String::from_utf16_lossy(&buf[..(n as usize - 1).min(buf.len())]))
}

// ------------------------------------------------------------------ сбор сведений

const NT_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

/// Система, процессор, память, диск: всё, что обычно спрашивают в первую очередь.
pub fn system_sections(data_dir: &std::path::Path) -> Vec<Section> {
    let mut system = Section::new("system");
    let mut hardware = Section::new("hardware");
    let arch_os = std::env::var("PROCESSOR_ARCHITEW6432").or_else(|_| std::env::var("PROCESSOR_ARCHITECTURE")).unwrap_or_default();
    let arch_prog = arch_label(std::env::consts::ARCH);
    if cfg!(windows) {
        windows_system(&mut system, &mut hardware);
        let os_arch = if arch_os.is_empty() { "unknown" } else { arch_label(&arch_os) };
        system.add("architecture", format!("system {os_arch}, program {arch_prog}"));
        if let Some(loc) = locale_name() {
            system.add("locale", loc);
        }
        // Безопасность: простые вызовы без аргументов-указателей.
        let (out_cp, in_cp) = unsafe { (ffi::GetConsoleOutputCP(), ffi::GetConsoleCP()) };
        if out_cp != 0 {
            system.add("console code page", format!("output {out_cp}, input {in_cp}"));
        }
        let ticks = unsafe { ffi::GetTickCount64() };
        system.add("since boot", duration(ticks / 1000));
        if let Some((total, free, load, page_total, page_free)) = memory_status() {
            hardware.add("ram", format!("{} total, {} free, {load}% in use", bytes(total), bytes(free)));
            hardware.add("page file", format!("{} total, {} free", bytes(page_total), bytes(page_free)));
        }
    } else {
        linux_system(&mut system, &mut hardware);
        system.add("architecture", format!("program {arch_prog}"));
    }
    let threads = std::thread::available_parallelism().map_or(0, |n| n.get());
    if threads > 0 {
        hardware.add("logical processors", threads.to_string());
    }
    if let Some((total, free)) = disk_space(data_dir) {
        hardware.add("disk of the program folder", format!("{} free of {}", bytes(free), bytes(total)));
    }
    let tz = jiff::tz::TimeZone::system();
    let zoned = jiff::Timestamp::now().to_zoned(tz.clone());
    system.add("time zone", format!("{} (UTC{})", tz.iana_name().unwrap_or("unknown"), zoned.strftime("%:z")));
    vec![system, hardware]
}

fn windows_system(system: &mut Section, hardware: &mut Section) {
    let nt = |name: &str| reg_string(HKEY_LOCAL_MACHINE, NT_KEY, name);
    let build: u32 = nt("CurrentBuildNumber").or_else(|| nt("CurrentBuild")).and_then(|b| b.trim().parse().ok()).unwrap_or(0);
    let version = reg_dword(HKEY_LOCAL_MACHINE, NT_KEY, "CurrentMajorVersionNumber").zip(reg_dword(
        HKEY_LOCAL_MACHINE,
        NT_KEY,
        "CurrentMinorVersionNumber",
    ));
    let (major, minor) = version
        .or_else(|| {
            let v = nt("CurrentVersion")?;
            let (a, b) = v.split_once('.')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .unwrap_or((0, 0));
    let ubr = reg_dword(HKEY_LOCAL_MACHINE, NT_KEY, "UBR");
    system.add("os", windows_product(&nt("ProductName").unwrap_or_default(), major, minor, build));
    let full = match ubr {
        Some(u) => format!("{major}.{minor}.{build}.{u}"),
        None => format!("{major}.{minor}.{build}"),
    };
    let release = nt("DisplayVersion").or_else(|| nt("ReleaseId"));
    system.add(
        "version",
        match release {
            Some(r) => format!("{full} ({r})"),
            None => full,
        },
    );
    system.add("service pack", nt("CSDVersion").unwrap_or_default());
    let edition = [nt("EditionID"), nt("InstallationType")].into_iter().flatten().collect::<Vec<_>>().join(", ");
    system.add("edition", edition);
    system.add("build lab", nt("BuildLabEx").unwrap_or_default());
    if let Some(t) = reg_dword(HKEY_LOCAL_MACHINE, NT_KEY, "InstallDate")
        && let Ok(ts) = jiff::Timestamp::from_second(i64::from(t))
    {
        system.add("installed", ts.to_zoned(jiff::tz::TimeZone::UTC).strftime("%Y-%m-%d").to_string());
    }

    const CPU_KEY: &str = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0";
    let cpu = reg_string(HKEY_LOCAL_MACHINE, CPU_KEY, "ProcessorNameString").map(|s| squeeze(&s)).unwrap_or_default();
    let mhz = reg_dword(HKEY_LOCAL_MACHINE, CPU_KEY, "~MHz").map(|m| format!(", {m} MHz")).unwrap_or_default();
    hardware.add("cpu", format!("{cpu}{mhz}"));
    hardware.add("cpu id", reg_string(HKEY_LOCAL_MACHINE, CPU_KEY, "Identifier").unwrap_or_default());

    const BIOS_KEY: &str = r"HARDWARE\DESCRIPTION\System\BIOS";
    let bios = |n: &str| reg_string(HKEY_LOCAL_MACHINE, BIOS_KEY, n).map(|s| squeeze(&s)).unwrap_or_default();
    hardware.add("computer", format!("{} {}", bios("SystemManufacturer"), bios("SystemProductName")));
    hardware.add("bios", format!("{} {} {}", bios("BIOSVendor"), bios("BIOSVersion"), bios("BIOSReleaseDate")));

    const GPU_CLASS: &str = r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11cd-bfc1-08002be10318}";
    for i in 0..4 {
        let key = format!(r"{GPU_CLASS}\{i:04}");
        let Some(name) = reg_string(HKEY_LOCAL_MACHINE, &key, "DriverDesc") else { continue };
        let driver = reg_string(HKEY_LOCAL_MACHINE, &key, "DriverVersion").unwrap_or_default();
        hardware.add(&format!("video adapter {}", i + 1), format!("{} (driver {driver})", squeeze(&name)));
    }
}

fn linux_system(system: &mut Section, hardware: &mut Section) {
    let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let pretty = os.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).map(|v| v.trim_matches('"').to_string());
    system.add("os", pretty.unwrap_or_else(|| "Linux".into()));
    system.add("kernel", std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default().trim().to_string());
    let cpu = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let model = cpu.lines().find_map(|l| l.strip_prefix("model name")?.split_once(':').map(|(_, v)| squeeze(v)));
    hardware.add("cpu", model.unwrap_or_default());
    let mem = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    if let (Some(total), Some(avail)) = (proc_kb(&mem, "MemTotal"), proc_kb(&mem, "MemAvailable")) {
        hardware.add("ram", format!("{} total, {} free", bytes(total * 1024), bytes(avail * 1024)));
    }
}

/// Память самой программы.
pub fn process_section() -> Section {
    let mut s = Section::new("process");
    if let Some((working, peak, private)) = process_memory() {
        s.add("memory", format!("working set {}, peak {}, private {}", bytes(working), bytes(peak), bytes(private)));
    } else if let Ok(status) = std::fs::read_to_string("/proc/self/status")
        && let Some(rss) = proc_kb(&status, "VmRSS")
    {
        let peak = proc_kb(&status, "VmHWM").unwrap_or(rss);
        s.add("memory", format!("resident {}, peak {}", bytes(rss * 1024), bytes(peak * 1024)));
    }
    s
}

/// Терминал: где запущено и какого размера окно.
pub fn terminal_section(size: Option<(u16, u16)>) -> Section {
    let mut s = Section::new("terminal");
    let env = |n: &str| std::env::var(n).ok().filter(|v| !v.is_empty());
    let kind = if std::env::var_os("WT_SESSION").is_some() {
        "Windows Terminal"
    } else if std::env::var_os("ConEmuPID").is_some() {
        "ConEmu"
    } else if env("TERM_PROGRAM").is_some() {
        "terminal program (see below)"
    } else if cfg!(windows) {
        "classic Windows console (conhost)"
    } else {
        "other terminal"
    };
    s.add("terminal", kind);
    if let Some((w, h)) = size {
        s.add("window size", format!("{w}x{h} cells"));
    }
    s.add("TERM_PROGRAM", env("TERM_PROGRAM").unwrap_or_default());
    s.add("TERM", env("TERM").unwrap_or_default());
    s.add("COLORTERM", env("COLORTERM").unwrap_or_default());
    s
}

/// Что есть в самой Windows и какие программы по умолчанию настроены: от этого зависят «открыть» и уведомления.
pub fn components_section() -> Section {
    let mut s = Section::new("windows components");
    if !cfg!(windows) {
        return s;
    }
    let root = std::env::var_os("SystemRoot").map_or_else(|| std::path::PathBuf::from(r"C:\Windows"), std::path::PathBuf::from);
    let sys = root.join("System32");
    let files = [
        ("powershell", sys.join("WindowsPowerShell").join("v1.0").join("powershell.exe")),
        ("cmd", sys.join("cmd.exe")),
        ("rundll32", sys.join("rundll32.exe")),
        ("explorer", root.join("explorer.exe")),
        ("conhost", sys.join("conhost.exe")),
        ("winmm", sys.join("winmm.dll")),
    ];
    let present: Vec<String> = files.iter().map(|(n, p)| format!("{n}={}", if p.exists() { "yes" } else { "NO" })).collect();
    s.add("programs", present.join(" "));
    let openers: Vec<String> = ["jpg", "png", "pdf", "txt", "ogg", "mp3", "mp4"]
        .iter()
        .map(|e| format!("{e}={}", if crate::util::has_handler(e) { "yes" } else { "NO" }))
        .collect();
    s.add("default programs", openers.join(" "));
    const INET: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    let enabled = reg_dword(HKEY_CURRENT_USER, INET, "ProxyEnable").unwrap_or(0) != 0;
    let server = reg_string(HKEY_CURRENT_USER, INET, "ProxyServer").unwrap_or_default();
    let pac = reg_string(HKEY_CURRENT_USER, INET, "AutoConfigURL").is_some();
    s.add(
        "system proxy",
        format!(
            "{}{}; auto-config script: {}",
            if enabled { "on" } else { "off" },
            if server.is_empty() { String::new() } else { format!(" ({})", describe_proxy(&server)) },
            if pac { "yes" } else { "no" }
        ),
    );
    s
}

/// Переменные окружения про прокси: только «задана или нет», значения не показываются.
pub fn proxy_env_row() -> String {
    let set: Vec<&str> = ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"]
        .into_iter()
        .filter(|n| std::env::var_os(n).is_some() || std::env::var_os(n.to_lowercase()).is_some())
        .collect();
    if set.is_empty() { "none set".into() } else { format!("set: {} (values hidden)", set.join(", ")) }
}

/// Файлы программы: имена и размеры (содержимое не читается). Только наши: остальное в папке не показываем.
pub fn files_section(dir: &std::path::Path) -> Section {
    let mut s = Section::new("program folder");
    let Ok(entries) = std::fs::read_dir(dir) else { return s };
    let mut rows: Vec<(String, String)> = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let lower = name.to_lowercase();
        let ours = lower.starts_with("tglite") || lower.starts_with("tg-lite") || lower == "export" || lower == "downloads";
        if !ours {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        let what = if meta.is_dir() { "folder".to_string() } else { bytes(meta.len()) };
        rows.push((name, what));
    }
    rows.sort();
    for (n, w) in rows.into_iter().take(24) {
        s.add(&n, w);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_versions_are_named_like_people_say_them() {
        assert_eq!(windows_name(10, 0, 19044), "Windows 10");
        assert_eq!(windows_name(10, 0, 22621), "Windows 11");
        assert_eq!(windows_name(6, 1, 7601), "Windows 7");
        assert_eq!(windows_name(6, 3, 9600), "Windows 8.1");
        assert_eq!(windows_name(99, 0, 0), "Windows");
        // реестр Windows 11 всё ещё пишет «Windows 10»
        assert_eq!(windows_product("Windows 10 Pro", 10, 0, 22631), "Windows 11 Pro");
        assert_eq!(windows_product("Windows 10 Enterprise LTSC 2021", 10, 0, 19044), "Windows 10 Enterprise LTSC 2021");
        assert_eq!(windows_product("", 6, 1, 7601), "Windows 7");
        assert_eq!(arch_label("AMD64"), "x64");
        assert_eq!(arch_label("x86"), "x86");
        assert_eq!(arch_label("ARM64"), "ARM64");
        assert_eq!(arch_label("???"), "unknown");
    }

    #[test]
    fn sizes_and_durations_are_readable() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(12 * 1024), "12 KB");
        assert_eq!(bytes(3_565_158), "3.4 MB");
        assert_eq!(bytes(2 * 1024 * 1024 * 1024), "2.0 GB");
        assert_eq!(duration(45), "45 s");
        assert_eq!(duration(12 * 60 + 5), "12 min 5 s");
        assert_eq!(duration(3 * 3600 + 12 * 60), "3 h 12 min");
        assert_eq!(duration(2 * 86_400 + 4 * 3600), "2 d 4 h");
        assert_eq!(squeeze("Pentium(R)   Dual-Core  CPU      T4400  @ 2.20GHz"), "Pentium(R) Dual-Core CPU T4400 @ 2.20GHz");
    }

    #[test]
    fn user_names_and_proxy_addresses_never_reach_the_report() {
        assert_eq!(hide_user(r"C:\Users\Иван\Desktop\tg-lite.exe", None), r"C:\Users\<user>\Desktop\tg-lite.exe");
        assert_eq!(hide_user(r"c:\USERS\maks\AppData", None), r"c:\USERS\<user>\AppData");
        assert_eq!(hide_user("/home/ivan/tg-lite", None), "/home/<user>/tg-lite");
        assert_eq!(hide_user("ошибка у Maksim: не найден", Some("Maksim")), "ошибка у <user>: не найден");
        assert_eq!(hide_user("MAKSIM и maksim", Some("Maksim")), "<user> и <user>");
        // короткие имена не заменяем: «ab» встречается в любом тексте
        assert_eq!(hide_user("about", Some("ab")), "about");
        // повторные вызовы ничего не портят
        let once = hide_user(r"C:\Users\Иван\a и C:\Users\Оля\b", None);
        assert_eq!(once, r"C:\Users\<user>\a и C:\Users\<user>\b");
        assert_eq!(hide_user(&once, None), once);
        let p = describe_proxy("socks5://user:pass@127.0.0.1:10808");
        assert!(p.contains("socks5") && p.contains("local") && !p.contains("127") && !p.contains("pass") && !p.contains("10808"), "{p}");
        let p = describe_proxy("http://proxy.example.com:3128");
        assert!(p.contains("http") && p.contains("remote") && !p.contains("example") && !p.contains("3128"), "{p}");
        assert_eq!(describe_proxy(""), "none");
        assert!(describe_proxy("[::1]:1080").contains("local"));
    }

    #[test]
    fn proxy_addresses_are_hidden_in_every_form_the_program_writes_them() {
        let known = vec!["socks5://bob:hunter2@proxy.example.org:1080".to_string()];
        let h = |t: &str| hide_addresses(t, &known);
        // ссылки
        assert_eq!(h("connect via socks5://bob:hunter2@10.0.0.1:1080 failed"), "connect via socks5://<hidden> failed");
        assert_eq!(h("http://user:pw@proxy.corp:3128/x, ok"), "http://<hidden>/x, ok");
        assert_eq!(h("ошибка socks5://10.1.2.3:1080: timeout"), "ошибка socks5://<hidden>: timeout");
        // свой компьютер остаётся, но без логина и пароля
        assert_eq!(h("socks5://127.0.0.1:10808 refused"), "socks5://127.0.0.1:10808 refused");
        assert_eq!(h("socks5://u:p@localhost:1080 refused"), "socks5://localhost:1080 refused");
        assert_eq!(h("socks5://[::1]:1080"), "socks5://[::1]:1080");
        // как пишет сама программа: подпись маршрута и «bridge for»
        assert_eq!(h("connected via SOCKS5 proxy ***@10.0.0.1:1080"), "connected via SOCKS5 proxy <hidden>");
        assert_eq!(h("connect через HTTP-прокси 203.0.113.7:3128: timeout"), "connect через HTTP-прокси <hidden>: timeout");
        assert_eq!(h("bridge for ***@203.0.113.7:3128: refused"), "bridge for <hidden>: refused");
        assert_eq!(h("via SOCKS5 proxy 127.0.0.1:1080"), "via SOCKS5 proxy 127.0.0.1:1080");
        // обычные слова после «proxy» не трогаем
        assert_eq!(h("proxy is missing from url"), "proxy is missing from url");
        assert_eq!(h("the proxy server."), "the proxy server.");
        // заданный сейчас адрес прячется где угодно, вместе с логином и паролем
        assert_eq!(h("dial PROXY.example.org:1080 failed"), "dial <hidden> failed");
        assert_eq!(h("dial proxy.example.org failed"), "dial <hidden> failed");
        assert_eq!(h("auth bob/hunter2 rejected"), "auth <hidden>/<hidden> rejected");
        // обычный текст и повторный вызов ничего не портят
        assert_eq!(h("2026-11-09 10:00:00 chats: 12"), "2026-11-09 10:00:00 chats: 12");
        let once = h("via SOCKS5 proxy ***@10.0.0.1:1080 and socks5://a:b@x.example:1");
        assert_eq!(h(&once), once);
        // пустые и короткие адреса в списке известных ничего не ломают
        assert_eq!(hide_addresses("abc", &[String::new(), "ab".into(), "  ".into()]), "abc");
        assert_eq!(replace_ci("A.b a.B", "a.b", "X"), "X X");
        assert_eq!(replace_ci("abc", "", "X"), "abc");
    }

    #[test]
    fn proc_files_are_parsed() {
        let meminfo = "MemTotal:        2043124 kB\nMemFree:          100000 kB\nMemAvailable:     800000 kB\n";
        assert_eq!(proc_kb(meminfo, "MemTotal"), Some(2_043_124));
        assert_eq!(proc_kb(meminfo, "MemAvailable"), Some(800_000));
        assert_eq!(proc_kb(meminfo, "Nope"), None);
        assert_eq!(proc_kb("VmRSS:\t   3456 kB\n", "VmRSS"), Some(3456));
    }

    #[test]
    fn sections_render_aligned_and_skip_empty_ones_and_values() {
        let mut a = Section::new("one");
        a.add("short", "1");
        a.add("a longer key", "2");
        a.add("empty", "   ");
        let b = Section::new("none");
        let text = render(&[a, b]);
        assert_eq!(text, "[one]\nshort        : 1\na longer key : 2\n\n");
    }

    #[test]
    fn collecting_works_on_this_system_and_never_panics_when_windows_calls_fail() {
        let dir = std::env::temp_dir();
        let sections = system_sections(&dir);
        let text = render(&sections);
        assert!(text.contains("[system]") && text.contains("os "), "{text}");
        assert!(text.contains("architecture") && text.contains("time zone"), "{text}");
        // Обёртки над реестром и kernel32 на этой системе — заглушки: они отвечают «не получилось», а не падают.
        assert_eq!(reg_string(HKEY_LOCAL_MACHINE, NT_KEY, "ProductName").is_some(), cfg!(windows));
        assert!(!cfg!(not(windows)) || (reg_dword(HKEY_LOCAL_MACHINE, NT_KEY, "UBR").is_none() && memory_status().is_none()));
        let _ = process_section();
        let _ = terminal_section(Some((120, 30)));
        let _ = components_section();
        let _ = files_section(&dir);
        assert!(proxy_env_row().len() > 3);
    }
}
