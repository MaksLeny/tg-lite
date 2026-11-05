//! Выбор маршрута до Telegram без участия пользователя.
//!
//! Порядок (режим «авто»): системный прокси Windows и переменные окружения, затем напрямую.
//! Прокси типа SOCKS5 используется как есть. Обычный HTTP-прокси библиотека Telegram
//! не умеет, поэтому для него запускается крошечный локальный мост SOCKS5 → HTTP CONNECT.

use std::collections::HashMap;
use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use crate::settings::{ProxyMode, Settings};
use crate::{tr, trf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Direct,
    /// `host:port` SOCKS5-прокси.
    Socks5(String),
    /// `host:port` HTTP-прокси (через локальный мост).
    HttpBridge(String),
}

impl Route {
    pub fn label(&self) -> String {
        match self {
            Route::Direct => tr!("напрямую", "directly").to_string(),
            Route::Socks5(a) => {
                let a = redact(a);
                trf!("через SOCKS5-прокси {a}", "via SOCKS5 proxy {a}")
            }
            Route::HttpBridge(a) => {
                let a = redact(a);
                trf!("через HTTP-прокси {a}", "via HTTP proxy {a}")
            }
        }
    }
}

/// В адресе (после `логин:пароль@`) есть числовой порт: `host:1080`, `[::1]:1080`.
fn has_port(addr: &str) -> bool {
    let hp = addr.rsplit_once('@').map_or(addr, |(_, hp)| hp);
    hp.rsplit_once(':').is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok())
}

/// Адрес для показа на экране и в журнале: `логин:пароль@` заменяется на `***@`.
pub fn redact(addr: &str) -> String {
    match addr.rsplit_once('@') {
        Some((_, hp)) => format!("***@{hp}"),
        None => addr.to_string(),
    }
}

/// Убирает `логин:пароль@` из любого текста (например, сообщения об ошибке библиотеки): `://user:pw@host` -> `://***@host`.
pub fn redact_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        let end = tail.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(tail.len());
        match tail[..end].rfind('@') {
            Some(at) => {
                out.push_str("***@");
                rest = &tail[at + 1..];
            }
            None => rest = tail,
        }
    }
    out.push_str(rest);
    out
}

/// Раскодирует `%40` и подобное в логине и пароле из адреса прокси.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) =
                s.get(i + 1..i + 3).filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit())).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Делит `логин:пароль@хост:порт` на (логин, пароль) и `хост:порт`. Без `@` или без `:` в учётных данных логина нет.
pub fn split_auth(addr: &str) -> (Option<(String, String)>, &str) {
    let Some((cred, hp)) = addr.rsplit_once('@') else { return (None, addr) };
    match cred.split_once(':') {
        Some((u, p)) if !u.is_empty() => (Some((percent_decode(u), percent_decode(p))), hp),
        _ => (None, hp),
    }
}

/// Base64 для заголовка Proxy-Authorization (без внешних зависимостей).
fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Строка `Proxy-Authorization: Basic …\r\n` для адреса с логином или пустая строка.
fn auth_header(addr: &str) -> String {
    match split_auth(addr).0 {
        Some((u, p)) => format!("Proxy-Authorization: Basic {}\r\n", base64(format!("{u}:{p}").as_bytes())),
        None => String::new(),
    }
}

/// `socks5://h:p` -> Socks5, `http://h:p` -> HttpBridge, `h:p` -> оба варианта (смешанный порт).
pub fn parse_proxy_url(s: &str) -> Vec<Route> {
    let s = s.trim();
    if s.is_empty() {
        return vec![];
    }
    let (scheme, rest) = match s.split_once("://") {
        Some((sc, r)) => (sc.to_ascii_lowercase(), r),
        None => (String::new(), s),
    };
    let addr = rest.trim_end_matches('/');
    // Логин и пароль (`user:pass@host:port`) остаются в адресе: мост добавляет их в запрос к HTTP-прокси.
    if !has_port(addr) {
        return vec![];
    }
    match scheme.as_str() {
        "socks5" | "socks5h" | "socks" | "socks4" => vec![Route::Socks5(addr.to_string())],
        "http" | "https" => vec![Route::Socks5(addr.to_string()), Route::HttpBridge(addr.to_string())],
        _ => vec![Route::Socks5(addr.to_string()), Route::HttpBridge(addr.to_string())],
    }
}

/// Разбирает значение ProxyServer из реестра Windows:
/// `127.0.0.1:10809` или `http=127.0.0.1:10809;https=...;socks=127.0.0.1:10808`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_proxy_server(s: &str) -> Vec<Route> {
    let s = s.trim();
    let mut out: Vec<Route> = Vec::new();
    if s.contains('=') {
        let mut socks = Vec::new();
        let mut http = Vec::new();
        for part in s.split(';') {
            let Some((proto, addr)) = part.split_once('=') else { continue };
            let addr = addr.trim();
            if !has_port(addr) {
                continue;
            }
            match proto.trim().to_ascii_lowercase().as_str() {
                "socks" | "socks5" => socks.push(Route::Socks5(addr.to_string())),
                "http" | "https" => http.push(Route::HttpBridge(addr.to_string())),
                _ => {}
            }
        }
        // «Смешанный» порт часто указан как http=..., но понимает и SOCKS5: пробуем и так.
        for r in &http {
            if let Route::HttpBridge(a) = r {
                socks.push(Route::Socks5(a.clone()));
            }
        }
        out.extend(socks);
        out.extend(http);
    } else {
        out.extend(parse_proxy_url(s));
    }
    dedup(out)
}

fn dedup(v: Vec<Route>) -> Vec<Route> {
    let mut out: Vec<Route> = Vec::new();
    for r in v {
        if !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

/// Достаёт значение параметра из вывода `reg query`:
/// `    ProxyServer    REG_SZ    127.0.0.1:10809`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_reg_value(output: &str, name: &str) -> Option<String> {
    for line in output.lines() {
        let mut it = line.split_whitespace();
        if it.next() != Some(name) {
            continue;
        }
        let _ty = it.next()?;
        let value: Vec<&str> = it.collect();
        return Some(value.join(" "));
    }
    None
}

#[cfg(windows)]
fn reg_query(name: &str) -> Option<String> {
    let out = std::process::Command::new("reg")
        .args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings", "/v", name])
        .output()
        .ok()?;
    parse_reg_value(&String::from_utf8_lossy(&out.stdout), name)
}

/// Системный прокси Windows, если включён.
fn system_proxy_routes() -> Vec<Route> {
    #[cfg(windows)]
    {
        let enabled = reg_query("ProxyEnable").map(|v| v.trim() == "0x1" || v.trim() == "1").unwrap_or(false);
        if enabled {
            if let Some(server) = reg_query("ProxyServer") {
                return parse_proxy_server(&server);
            }
        }
    }
    vec![]
}

fn env_proxy_routes() -> Vec<Route> {
    let mut out = Vec::new();
    for var in ["TGLITE_PROXY", "ALL_PROXY", "all_proxy", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        if let Ok(v) = std::env::var(var) {
            out.extend(parse_proxy_url(&v));
        }
    }
    out
}

/// Список маршрутов в порядке попыток.
pub fn candidates(settings: &Settings) -> Vec<Route> {
    match settings.proxy_mode {
        ProxyMode::Off => vec![Route::Direct],
        ProxyMode::Manual => {
            let r = parse_proxy_url(&settings.proxy_manual);
            if r.is_empty() { vec![Route::Direct] } else { dedup(r) }
        }
        ProxyMode::Auto => {
            // Сначала адрес, введённый ранее вручную (если есть), затем то, что видно в системе.
            let mut v = parse_proxy_url(&settings.proxy_manual);
            v.extend(env_proxy_routes());
            v.extend(system_proxy_routes());
            v = dedup(v);
            v.push(Route::Direct);
            v
        }
    }
}

// ------------------------------------------------------------------ быстрая проверка прокси

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Probe {
    /// Порт закрыт или не отвечает.
    Dead,
    /// Отвечает как SOCKS5.
    Socks5,
    /// Что-то другое (обычно HTTP-прокси).
    Other,
}

/// Адрес без `логин:пароль@`: для TCP-соединения нужны только хост и порт.
fn host_port(addr: &str) -> &str {
    addr.rsplit_once('@').map_or(addr, |(_, hp)| hp)
}

/// За пару секунд выясняет, что слушает на адресе: SOCKS5, что-то другое или ничего.
pub async fn probe(addr: &str) -> Probe {
    let Ok(Ok(mut s)) = timeout(Duration::from_millis(2000), TcpStream::connect(host_port(addr))).await else {
        return Probe::Dead;
    };
    if s.write_all(&[5, 1, 0]).await.is_err() {
        return Probe::Dead;
    }
    let mut r = [0u8; 2];
    match timeout(Duration::from_millis(2000), s.read_exact(&mut r)).await {
        Ok(Ok(_)) if r == [5, 0] => Probe::Socks5,
        _ => Probe::Other,
    }
}

/// Убирает заведомо нерабочие маршруты и для «смешанных» пар (`host:port` как SOCKS5 и как HTTP)
/// оставляет тот вариант, который реально отвечает. Так не приходится ждать таймауты впустую.
pub async fn resolve_routes(routes: Vec<Route>) -> Vec<Route> {
    let mut seen: HashMap<String, Probe> = HashMap::new();
    let mut out = Vec::new();
    for r in &routes {
        let addr = match r {
            Route::Direct => {
                out.push(r.clone());
                continue;
            }
            Route::Socks5(a) | Route::HttpBridge(a) => a,
        };
        let p = match seen.get(addr) {
            Some(p) => *p,
            None => {
                let p = probe(addr).await;
                seen.insert(addr.clone(), p);
                p
            }
        };
        let has_http_twin = routes.contains(&Route::HttpBridge(addr.clone()));
        let keep = match (r, p) {
            (_, Probe::Dead) => false,
            (Route::Socks5(_), Probe::Socks5) => true,
            (Route::Socks5(_), Probe::Other) => !has_http_twin, // явно указанный SOCKS5 пробуем всё равно
            (Route::HttpBridge(_), Probe::Socks5) => false,     // смешанный порт уже подойдёт как SOCKS5
            (Route::HttpBridge(_), Probe::Other) => true,
            (Route::Direct, _) => true,
        };
        if keep {
            out.push(r.clone());
        }
    }
    out
}

// ------------------------------------------------------------------ мост SOCKS5 -> HTTP CONNECT

/// Запускает локальный SOCKS5-сервер, который пробрасывает соединения через HTTP-прокси.
/// Возвращает адрес вида `127.0.0.1:порт`.
pub async fn start_bridge(http_proxy: String) -> io::Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        loop {
            let Ok((client, _)) = listener.accept().await else { break };
            let proxy = http_proxy.clone();
            tokio::spawn(async move {
                if let Err(e) = serve_socks5(client, &proxy).await {
                    crate::util::debug(&format!("bridge {}: {e}", redact(&proxy)));
                }
            });
        }
    });
    Ok(addr.to_string())
}

async fn serve_socks5(mut client: TcpStream, http_proxy: &str) -> io::Result<()> {
    // Приветствие: версия, число методов, методы.
    let mut head = [0u8; 2];
    client.read_exact(&mut head).await?;
    if head[0] != 5 {
        return Err(io::Error::other(tr!("не SOCKS5", "not SOCKS5")));
    }
    let mut methods = vec![0u8; head[1] as usize];
    client.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        client.write_all(&[5, 0xFF]).await?;
        return Err(io::Error::other(tr!("нужна авторизация", "authentication required")));
    }
    client.write_all(&[5, 0]).await?;

    // Запрос: версия, команда, резерв, тип адреса.
    let mut req = [0u8; 4];
    client.read_exact(&mut req).await?;
    if req[0] != 5 || req[1] != 1 {
        client.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        return Err(io::Error::other(tr!("поддерживается только CONNECT", "only CONNECT is supported")));
    }
    let host = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            client.read_exact(&mut a).await?;
            std::net::Ipv4Addr::from(a).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            client.read_exact(&mut name).await?;
            String::from_utf8_lossy(&name).into_owned()
        }
        4 => {
            let mut a = [0u8; 16];
            client.read_exact(&mut a).await?;
            format!("[{}]", std::net::Ipv6Addr::from(a))
        }
        _ => {
            client.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            return Err(io::Error::other(tr!("неизвестный тип адреса", "unknown address type")));
        }
    };
    let mut port = [0u8; 2];
    client.read_exact(&mut port).await?;
    let port = u16::from_be_bytes(port);
    // Имя узла попадает в строку запроса к HTTP-прокси: никаких пробелов и управляющих символов (иначе можно вписать свои заголовки).
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']')) {
        client.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        return Err(io::Error::other(tr!("недопустимое имя узла", "invalid host name")));
    }

    // Соединение с HTTP-прокси и просьба «CONNECT host:port».
    let mut upstream = match TcpStream::connect(host_port(http_proxy)).await {
        Ok(s) => s,
        Err(e) => {
            client.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            return Err(e);
        }
    };
    let target = format!("{host}:{port}");
    let auth = auth_header(http_proxy);
    upstream
        .write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n{auth}Proxy-Connection: keep-alive\r\n\r\n").as_bytes())
        .await?;

    // Читаем ответ до пустой строки по одному байту, чтобы не «съесть» данные туннеля.
    let mut resp = Vec::with_capacity(256);
    let mut b = [0u8; 1];
    while !resp.ends_with(b"\r\n\r\n") {
        if resp.len() > 8192 || upstream.read(&mut b).await? == 0 {
            client.write_all(&[5, 1, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            return Err(io::Error::other(tr!("прокси закрыл соединение", "proxy closed the connection")));
        }
        resp.push(b[0]);
    }
    let status_ok = resp.starts_with(b"HTTP/1.") && resp.get(9..12) == Some(b"200");
    if !status_ok {
        client.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        if resp.get(9..12) == Some(b"407") {
            return Err(io::Error::other(tr!(
                "прокси требует логин и пароль: /proxy логин:пароль@адрес:порт",
                "the proxy needs a login: /proxy user:password@host:port"
            )));
        }
        return Err(io::Error::other(tr!("прокси отказал в соединении", "proxy refused the connection")));
    }
    client.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_mixed_system_proxy() {
        assert_eq!(
            parse_proxy_server("127.0.0.1:10809"),
            vec![Route::Socks5("127.0.0.1:10809".into()), Route::HttpBridge("127.0.0.1:10809".into())]
        );
        let r = parse_proxy_server("http=127.0.0.1:10809;https=127.0.0.1:10809;socks=127.0.0.1:10808");
        assert_eq!(r[0], Route::Socks5("127.0.0.1:10808".into())); // SOCKS в приоритете
        assert!(r.contains(&Route::HttpBridge("127.0.0.1:10809".into())));
        assert!(r.contains(&Route::Socks5("127.0.0.1:10809".into()))); // смешанный порт
        let r = parse_proxy_server("http=127.0.0.1:8080");
        assert_eq!(r, vec![Route::Socks5("127.0.0.1:8080".into()), Route::HttpBridge("127.0.0.1:8080".into())]);
        assert!(parse_proxy_server("").is_empty());
        assert!(parse_proxy_server("garbage").is_empty());
    }

    #[test]
    fn parses_urls() {
        assert_eq!(parse_proxy_url("socks5://127.0.0.1:1080"), vec![Route::Socks5("127.0.0.1:1080".into())]);
        assert_eq!(parse_proxy_url("127.0.0.1:1080/").len(), 2);
        assert_eq!(
            parse_proxy_url("http://127.0.0.1:3128"),
            vec![Route::Socks5("127.0.0.1:3128".into()), Route::HttpBridge("127.0.0.1:3128".into())]
        );
        assert!(parse_proxy_url("  ").is_empty());
    }

    #[test]
    fn parses_reg_output() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\r\n    ProxyServer    REG_SZ    127.0.0.1:10809\r\n\r\n";
        assert_eq!(parse_reg_value(out, "ProxyServer").as_deref(), Some("127.0.0.1:10809"));
        let out = "    ProxyEnable    REG_DWORD    0x1\r\n";
        assert_eq!(parse_reg_value(out, "ProxyEnable").as_deref(), Some("0x1"));
        assert_eq!(parse_reg_value(out, "ProxyServer"), None);
    }

    #[test]
    fn candidates_follow_mode() {
        let mut s = Settings::default();
        s.proxy_mode = ProxyMode::Off;
        assert_eq!(candidates(&s), vec![Route::Direct]);
        s.proxy_mode = ProxyMode::Manual;
        s.proxy_manual = "socks5://127.0.0.1:1080".into();
        assert_eq!(candidates(&s), vec![Route::Socks5("127.0.0.1:1080".into())]);
        s.proxy_manual.clear();
        assert_eq!(candidates(&s), vec![Route::Direct]);
        s.proxy_mode = ProxyMode::Auto;
        s.proxy_manual = "127.0.0.1:1080".into();
        let c = candidates(&s);
        assert_eq!(c.first(), Some(&Route::Socks5("127.0.0.1:1080".into())), "ручной адрес пробуется первым");
        assert_eq!(c.last(), Some(&Route::Direct), "напрямую — в самом конце");
    }

    /// Поднимаем «HTTP-прокси» с эхо-сервером, подключаемся к мосту как SOCKS5-клиент
    /// и проверяем, что данные проходят туда и обратно.
    #[tokio::test]
    async fn bridge_tunnels_through_http_connect() {
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy.local_addr().unwrap().to_string();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let seen2 = seen.clone();
        tokio::spawn(async move {
            let (mut s, _) = proxy.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut b = [0u8; 1];
            while !buf.ends_with(b"\r\n\r\n") {
                s.read_exact(&mut b).await.unwrap();
                buf.push(b[0]);
            }
            *seen2.lock().unwrap() = String::from_utf8_lossy(&buf).into_owned();
            s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await.unwrap();
            let mut data = [0u8; 5];
            s.read_exact(&mut data).await.unwrap();
            s.write_all(&data).await.unwrap(); // эхо
        });

        let bridge = start_bridge(proxy_addr).await.unwrap();
        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(&[5, 1, 0]).await.unwrap();
        let mut r = [0u8; 2];
        c.read_exact(&mut r).await.unwrap();
        assert_eq!(r, [5, 0]);
        // CONNECT 149.154.167.51:443
        c.write_all(&[5, 1, 0, 1, 149, 154, 167, 51, 0x01, 0xBB]).await.unwrap();
        let mut r = [0u8; 10];
        c.read_exact(&mut r).await.unwrap();
        assert_eq!(&r[..2], &[5, 0], "мост должен ответить успехом");
        c.write_all(b"hello").await.unwrap();
        let mut echo = [0u8; 5];
        c.read_exact(&mut echo).await.unwrap();
        assert_eq!(&echo, b"hello");
        assert!(seen.lock().unwrap().starts_with("CONNECT 149.154.167.51:443 HTTP/1.1"));
    }

    #[tokio::test]
    async fn bridge_reports_failure_when_proxy_refuses() {
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut s, _) = proxy.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut b = [0u8; 1];
            while !buf.ends_with(b"\r\n\r\n") {
                s.read_exact(&mut b).await.unwrap();
                buf.push(b[0]);
            }
            s.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").await.unwrap();
        });
        let bridge = start_bridge(proxy_addr).await.unwrap();
        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(&[5, 1, 0]).await.unwrap();
        let mut r = [0u8; 2];
        c.read_exact(&mut r).await.unwrap();
        c.write_all(&[5, 1, 0, 3, 11, b'e', b'x', b'a', b'm', b'p', b'l', b'e', b'.', b'c', b'o', b'm', 0, 80]).await.unwrap();
        let mut r = [0u8; 10];
        c.read_exact(&mut r).await.unwrap();
        assert_eq!(r[1], 5, "должна быть ошибка соединения");
    }

    async fn mock_listener(reply: &'static [u8]) -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { break };
                tokio::spawn(async move {
                    let mut b = [0u8; 3];
                    let _ = s.read_exact(&mut b).await;
                    let _ = s.write_all(reply).await;
                    tokio::time::sleep(Duration::from_millis(300)).await;
                });
            }
        });
        addr
    }

    #[test]
    fn splits_and_redacts_credentials() {
        assert_eq!(split_auth("user:pw@127.0.0.1:3128"), (Some(("user".into(), "pw".into())), "127.0.0.1:3128"));
        assert_eq!(split_auth("127.0.0.1:3128"), (None, "127.0.0.1:3128"));
        // @ и : в пароле, закодированные и сырые
        assert_eq!(split_auth("u:p%40ss%3Aw@h:1").0, Some(("u".into(), "p@ss:w".into())));
        assert_eq!(split_auth("u:p@ss@h:1"), (Some(("u".into(), "p@ss".into())), "h:1"));
        assert_eq!(split_auth("@h:1").0, None, "пустой логин не считается");
        assert_eq!(redact("user:secret@h:1"), "***@h:1");
        assert_eq!(redact("h:1"), "h:1");
        assert!(!Route::HttpBridge("user:secret@h:1".into()).label().contains("secret"));
        assert!(!Route::Socks5("user:secret@h:1".into()).label().contains("user"));
    }

    #[test]
    fn redact_text_hides_credentials_in_any_message() {
        assert_eq!(
            redact_text("proxy port is missing from url: socks5://bob:secret@host"),
            "proxy port is missing from url: socks5://***@host"
        );
        assert_eq!(redact_text("a http://u:p@h:1/x and https://h2/y"), "a http://***@h:1/x and https://h2/y");
        assert_eq!(redact_text("no urls here"), "no urls here");
        assert_eq!(redact_text("socks5://127.0.0.1:1080"), "socks5://127.0.0.1:1080");
        assert!(!redact_text("socks5://b:p@ss@host").contains("ss@"));
    }

    #[test]
    fn proxy_address_needs_a_numeric_port() {
        assert!(parse_proxy_url("bob:secret@host").is_empty(), "без порта: не принимаем (и пароль не попадёт в сообщения об ошибках)");
        assert!(parse_proxy_url("host:abc").is_empty());
        assert!(parse_proxy_url("host:99999").is_empty());
        assert!(!parse_proxy_url("bob:secret@host:1080").is_empty());
        assert!(!parse_proxy_url("socks5://[::1]:1080").is_empty());
        assert!(parse_proxy_url(":1080").is_empty());
    }

    #[test]
    fn percent_decode_ignores_malformed_escapes() {
        assert_eq!(percent_decode("a%+4b"), "a%+4b");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%4"), "%4");
        assert_eq!(percent_decode("%41%7a"), "Az");
    }

    #[tokio::test]
    async fn bridge_rejects_host_names_that_could_inject_headers() {
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy.local_addr().unwrap().to_string();
        let touched = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let t2 = touched.clone();
        tokio::spawn(async move {
            if proxy.accept().await.is_ok() {
                t2.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        });
        let bridge = start_bridge(proxy_addr).await.unwrap();
        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(&[5, 1, 0]).await.unwrap();
        let mut r = [0u8; 2];
        c.read_exact(&mut r).await.unwrap();
        let host = b"a.com\r\nX: y";
        let mut req = vec![5, 1, 0, 3, host.len() as u8];
        req.extend_from_slice(host);
        req.extend_from_slice(&[0, 80]);
        c.write_all(&req).await.unwrap();
        let mut r = [0u8; 10];
        c.read_exact(&mut r).await.unwrap();
        assert_eq!(r[1], 8, "отказ без обращения к прокси");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!touched.load(std::sync::atomic::Ordering::SeqCst), "к HTTP-прокси не подключались");
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"Aladdin:open sesame"), "QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
        assert_eq!(auth_header("Aladdin:open sesame@h:1"), "Proxy-Authorization: Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==\r\n");
        assert_eq!(auth_header("h:1"), "");
    }

    #[tokio::test]
    async fn bridge_sends_proxy_authorization() {
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy.local_addr().unwrap().to_string();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let seen2 = seen.clone();
        tokio::spawn(async move {
            let (mut s, _) = proxy.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut b = [0u8; 1];
            while !buf.ends_with(b"\r\n\r\n") {
                s.read_exact(&mut b).await.unwrap();
                buf.push(b[0]);
            }
            *seen2.lock().unwrap() = String::from_utf8_lossy(&buf).into_owned();
            s.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
        let bridge = start_bridge(format!("Aladdin:open sesame@{proxy_addr}")).await.unwrap();
        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(&[5, 1, 0]).await.unwrap();
        let mut r = [0u8; 2];
        c.read_exact(&mut r).await.unwrap();
        c.write_all(&[5, 1, 0, 1, 1, 2, 3, 4, 0x01, 0xBB]).await.unwrap();
        let mut r = [0u8; 10];
        c.read_exact(&mut r).await.unwrap();
        assert_eq!(&r[..2], &[5, 0]);
        let req = seen.lock().unwrap().clone();
        assert!(req.contains("Proxy-Authorization: Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==\r\n"), "{req}");
        assert!(req.starts_with("CONNECT 1.2.3.4:443 HTTP/1.1"));
    }

    #[tokio::test]
    async fn bridge_survives_407_without_credentials() {
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut s, _) = proxy.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut b = [0u8; 1];
            while !buf.ends_with(b"\r\n\r\n") {
                s.read_exact(&mut b).await.unwrap();
                buf.push(b[0]);
            }
            assert!(!String::from_utf8_lossy(&buf).contains("Proxy-Authorization"));
            s.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n").await.unwrap();
        });
        let bridge = start_bridge(proxy_addr).await.unwrap();
        let mut c = TcpStream::connect(&bridge).await.unwrap();
        c.write_all(&[5, 1, 0]).await.unwrap();
        let mut r = [0u8; 2];
        c.read_exact(&mut r).await.unwrap();
        c.write_all(&[5, 1, 0, 1, 1, 2, 3, 4, 0x01, 0xBB]).await.unwrap();
        let mut r = [0u8; 10];
        c.read_exact(&mut r).await.unwrap();
        assert_eq!(r[1], 5);
    }

    #[test]
    fn host_port_strips_credentials() {
        assert_eq!(host_port("user:pw@127.0.0.1:1080"), "127.0.0.1:1080");
        assert_eq!(host_port("127.0.0.1:1080"), "127.0.0.1:1080");
        assert_eq!(host_port("u:p@w@h:1"), "h:1");
    }

    #[tokio::test]
    async fn probe_tells_socks_http_and_dead_apart() {
        let socks = mock_listener(&[5, 0]).await;
        let http = mock_listener(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
        let dead = {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().to_string() // сразу закрываем: порт свободен
        };
        assert_eq!(probe(&socks).await, Probe::Socks5);
        assert_eq!(probe(&http).await, Probe::Other);
        assert_eq!(probe(&dead).await, Probe::Dead);
    }

    #[tokio::test]
    async fn resolve_keeps_only_working_variant() {
        let socks = mock_listener(&[5, 0]).await;
        let http = mock_listener(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
        let dead = {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().to_string()
        };
        let routes = vec![
            Route::Socks5(socks.clone()),
            Route::HttpBridge(socks.clone()), // смешанный порт -> только SOCKS5
            Route::Socks5(http.clone()),
            Route::HttpBridge(http.clone()), // HTTP-прокси -> только мост
            Route::Socks5(dead.clone()),
            Route::HttpBridge(dead.clone()), // мёртвый -> ничего
            Route::Direct,
        ];
        let got = resolve_routes(routes).await;
        assert_eq!(got, vec![Route::Socks5(socks), Route::HttpBridge(http), Route::Direct]);
        // явно указанный SOCKS5, который отвечает «не так», всё равно пробуем
        let explicit = resolve_routes(vec![Route::Socks5(mock_listener(b"HT").await)]).await;
        assert_eq!(explicit.len(), 1);
    }
}
