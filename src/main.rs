mod proxy;
mod settings;
mod tg;
mod ui;
mod util;
mod vault;

use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use grammers_client::SignInError;
use tokio::sync::mpsc;

use proxy::Route;
use settings::{ProxyMode, Settings};
use util::{data_dir, log};
use vault::{Saver, SessionBlob, VaultFile, VaultSession};

/// Ключи API можно вшить в сборку (секреты GitHub Actions TGLITE_API_ID / TGLITE_API_HASH),
/// тогда при первом запуске их вводить не придётся.
fn embedded_keys() -> Option<(i32, String)> {
    let id = option_env!("TGLITE_API_ID")?.trim().parse::<i32>().ok()?;
    let hash = option_env!("TGLITE_API_HASH")?.trim().to_string();
    if hash.len() < 16 { None } else { Some((id, hash)) }
}

fn ask(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

fn ask_secret(prompt: &str) -> Result<String> {
    // Подсказку печатаем сами (rpassword на Windows портит кириллицу), а ввод скрываем им.
    print!("{prompt}");
    io::stdout().flush()?;
    let pw = rpassword::read_password()?;
    println!();
    Ok(pw)
}

/// Первый запуск: ключи API (если не вшиты в сборку) и пароль приложения любой длины.
fn first_run_setup() -> Result<(i32, String, String)> {
    let (api_id, api_hash) = match embedded_keys() {
        Some(k) => k,
        None => {
            println!("Первый запуск. Нужны api_id и api_hash с https://my.telegram.org (раздел API development tools).");
            let api_id: i32 = ask("api_id: ")?.parse().map_err(|_| anyhow!("api_id должен быть числом"))?;
            let api_hash = ask("api_hash: ")?;
            if api_hash.len() < 16 {
                return Err(anyhow!("api_hash выглядит неправильно"));
            }
            (api_id, api_hash)
        }
    };
    println!("\nПридумайте пароль приложения (любой длины, даже один символ). Им шифруется всё, включая вход в Telegram.");
    println!("Забытый пароль восстановить нельзя: придётся входить в аккаунт заново.");
    let password = loop {
        let p1 = ask_secret("Пароль: ")?;
        if p1.is_empty() {
            println!("Пароль не может быть пустым.");
            continue;
        }
        let p2 = ask_secret("Повторите пароль: ")?;
        if p1 == p2 {
            break p1;
        }
        println!("Пароли не совпали, ещё раз.");
    };
    Ok((api_id, api_hash, password))
}

async fn sign_in_flow(client: &grammers_client::Client, api_hash: &str) -> Result<()> {
    println!("\nВход в Telegram.");
    let phone = ask("Номер телефона (+79001234567): ")?;
    let token = client.request_login_code(&phone, api_hash).await.map_err(|e| anyhow!("{e}"))?;
    let code = ask("Код из Telegram (придёт в приложение или SMS): ")?;
    match client.sign_in(&token, &code).await {
        Ok(_) => {}
        Err(SignInError::PasswordRequired(pt)) => {
            let hint = pt.hint().unwrap_or("нет").to_string();
            let pw = ask_secret(&format!("Облачный пароль Telegram (подсказка: {hint}): "))?;
            client.check_password(pt, pw.trim()).await.map_err(|e| anyhow!("{e}"))?;
        }
        Err(SignInError::InvalidCode) => return Err(anyhow!("неверный код")),
        Err(e) => return Err(anyhow!("{e}")),
    }
    println!("Вход выполнен.");
    Ok(())
}

/// Пробует подключиться по очереди всеми подходящими маршрутами. Возвращает рабочее подключение.
async fn connect_auto(
    session: &Arc<VaultSession>,
    api_id: i32,
    routes: &[Route],
) -> Option<(tg::Connection, bool)> {
    for route in routes {
        let url = match route {
            Route::Direct => None,
            Route::Socks5(a) => Some(format!("socks5://{a}")),
            Route::HttpBridge(a) => match proxy::start_bridge(a.clone()).await {
                Ok(local) => Some(format!("socks5://{local}")),
                Err(e) => {
                    log(&format!("bridge for {a}: {e}"));
                    continue;
                }
            },
        };
        println!("Подключаюсь к Telegram {}…", route.label());
        let conn = tg::connect(Arc::clone(session), api_id, url);
        match tokio::time::timeout(Duration::from_secs(12), conn.client.is_authorized()).await {
            Ok(Ok(authorized)) => {
                log(&format!("connected {}", route.label()));
                return Some((conn, authorized));
            }
            Ok(Err(e)) => log(&format!("connect {}: {e}", route.label())),
            Err(_) => log(&format!("connect {}: timeout", route.label())),
        }
        conn.quit.quit();
    }
    None
}

async fn run() -> Result<()> {
    let vault = VaultFile::new(data_dir().join("tglite.vault"));

    // 1. Пароль, ключи и настройки.
    let (api_id, api_hash, password, blob, mut settings) = if vault.exists() {
        println!("Tg-Lite {} · файл данных: {}", env!("CARGO_PKG_VERSION"), vault.path().display());
        let mut tries = 0;
        loop {
            let pw = ask_secret("Пароль: ")?;
            match vault.load(&pw) {
                Ok(s) => break (s.api_id, s.api_hash, pw, s.session, s.settings),
                Err(e) => {
                    tries += 1;
                    println!("{e}");
                    if tries >= 5 {
                        return Err(anyhow!("слишком много неверных попыток"));
                    }
                }
            }
        }
    } else {
        let (id, hash, pw) = first_run_setup()?;
        (id, hash, pw, SessionBlob::default(), Settings::default())
    };

    // Прокси, заданный вручную в версии 0.0.2 (proxy.txt), переезжает в настройки.
    let legacy = data_dir().join("proxy.txt");
    if let Ok(old) = std::fs::read_to_string(&legacy) {
        let old = old.trim().to_string();
        if !old.is_empty() && settings.proxy_manual.is_empty() {
            settings.proxy_manual = old;
        }
        let _ = std::fs::remove_file(&legacy);
    }

    // 2. Подключение: маршруты перебираются автоматически, вводить ничего не нужно.
    let session = Arc::new(VaultSession::from_blob(blob));
    let saver = Arc::new(Saver::new(vault, api_id, api_hash.clone(), Arc::clone(&session), password, settings));
    saver.save()?; // файл создаётся сразу, ключи не потеряются

    let (conn, authorized) = loop {
        let routes = proxy::resolve_routes(proxy::candidates(&saver.settings())).await;
        if let Some(found) = connect_auto(&session, api_id, &routes).await {
            break found;
        }
        // Ни один маршрут не сработал: последняя возможность — подсказать адрес вручную.
        println!("\nНе удалось подключиться к Telegram ни одним способом.");
        println!("Проверьте интернет и VPN. Если VPN работает через прокси, введите его адрес,");
        println!("например 127.0.0.1:10808 (Enter — повторить попытку, 0 — без прокси, q — выйти).");
        let a = ask("Прокси: ")?;
        let mut s = saver.settings();
        match a.as_str() {
            "q" | "Q" => return Err(anyhow!("подключение отменено")),
            "" => {}
            "0" => s.proxy_mode = ProxyMode::Off,
            other => {
                s.proxy_manual = other.to_string();
                s.proxy_mode = ProxyMode::Manual;
            }
        }
        saver.set_settings(s);
        let _ = saver.save();
    };
    if !authorized {
        sign_in_flow(&conn.client, &api_hash).await?;
        saver.save()?;
    }

    // 3. Интерфейс.
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let quit_handle = conn.quit.clone();
    let cmd_tx = tg::spawn_worker(conn, ev_tx);

    let terminal = ratatui::init();
    let _ = crossterm::execute!(io::stdout(), crossterm::terminal::SetTitle("Tg-Lite"));
    let result = ui::run(terminal, cmd_tx, ev_rx, Arc::clone(&saver)).await;
    ratatui::restore();

    // 4. Сохранение и выход.
    let _ = saver.save();
    quit_handle.quit();
    result
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(e) = run().await {
        log(&format!("fatal: {e}"));
        eprintln!("\nОшибка: {e}");
        eprint!("Нажмите Enter, чтобы закрыть…");
        let _ = io::stderr().flush();
        let mut s = String::new();
        let _ = io::stdin().read_line(&mut s);
        std::process::exit(1);
    }
}
