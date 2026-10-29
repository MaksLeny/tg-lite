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
            Route::Direct => "напрямую".to_string(),
            Route::Socks5(a) => format!("через SOCKS5-прокси {a}"),
            Route::HttpBridge(a) => format!("через HTTP-прокси {a}"),
        }
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
    // Логин и пароль в адресе не поддерживаются мостом; для SOCKS5 оставляем как есть.
    if addr.is_empty() || !addr.contains(':') {
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
            if !addr.contains(':') {
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

/// За пару секунд выясняет, что слушает на адресе: SOCKS5, что-то другое или ничего.
pub async fn probe(addr: &str) -> Probe {
    let Ok(Ok(mut s)) = timeout(Duration::from_millis(2000), TcpStream::connect(addr)).await else {
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
                let _ = serve_socks5(client, &proxy).await;
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
        return Err(io::Error::other("не SOCKS5"));
    }
    let mut methods = vec![0u8; head[1] as usize];
    client.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        client.write_all(&[5, 0xFF]).await?;
        return Err(io::Error::other("нужна авторизация"));
    }
    client.write_all(&[5, 0]).await?;

    // Запрос: версия, команда, резерв, тип адреса.
    let mut req = [0u8; 4];
    client.read_exact(&mut req).await?;
    if req[0] != 5 || req[1] != 1 {
        client.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        return Err(io::Error::other("поддерживается только CONNECT"));
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
            return Err(io::Error::other("неизвестный тип адреса"));
        }
    };
    let mut port = [0u8; 2];
    client.read_exact(&mut port).await?;
    let port = u16::from_be_bytes(port);

    // Соединение с HTTP-прокси и просьба «CONNECT host:port».
    let mut upstream = match TcpStream::connect(http_proxy).await {
        Ok(s) => s,
        Err(e) => {
            client.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            return Err(e);
        }
    };
    let target = format!("{host}:{port}");
    upstream
        .write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Connection: keep-alive\r\n\r\n").as_bytes())
        .await?;

    // Читаем ответ до пустой строки по одному байту, чтобы не «съесть» данные туннеля.
    let mut resp = Vec::with_capacity(256);
    let mut b = [0u8; 1];
    while !resp.ends_with(b"\r\n\r\n") {
        if resp.len() > 8192 || upstream.read(&mut b).await? == 0 {
            client.write_all(&[5, 1, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            return Err(io::Error::other("прокси закрыл соединение"));
        }
        resp.push(b[0]);
    }
    let status_ok = resp.starts_with(b"HTTP/1.") && resp.get(9..12) == Some(b"200");
    if !status_ok {
        client.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        return Err(io::Error::other("прокси отказал в соединении"));
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
            Route::Socks5(socks.clone()), Route::HttpBridge(socks.clone()), // смешанный порт -> только SOCKS5
            Route::Socks5(http.clone()), Route::HttpBridge(http.clone()),   // HTTP-прокси -> только мост
            Route::Socks5(dead.clone()), Route::HttpBridge(dead.clone()),   // мёртвый -> ничего
            Route::Direct,
        ];
        let got = resolve_routes(routes).await;
        assert_eq!(got, vec![Route::Socks5(socks), Route::HttpBridge(http), Route::Direct]);
        // явно указанный SOCKS5, который отвечает «не так», всё равно пробуем
        let explicit = resolve_routes(vec![Route::Socks5(mock_listener(b"HT").await)]).await;
        assert_eq!(explicit.len(), 1);
    }
}
