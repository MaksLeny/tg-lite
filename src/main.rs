mod tg;
mod ui;
mod vault;

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use grammers_client::SignInError;
use tokio::sync::mpsc;

use vault::{Saver, SessionBlob, VaultFile, VaultSession};

fn data_dir() -> PathBuf {
    // Рядом с exe: удобно носить с собой. Можно переопределить переменной TGLITE_DIR.
    if let Some(d) = std::env::var_os("TGLITE_DIR") {
        return PathBuf::from(d);
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
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

/// Первый запуск: спрашиваем ключи API и придумываем пароль приложения.
fn first_run_setup() -> Result<(i32, String, String)> {
    println!("Первый запуск. Нужны api_id и api_hash с https://my.telegram.org (раздел API development tools).");
    let api_id: i32 = ask("api_id: ")?.parse().map_err(|_| anyhow!("api_id должен быть числом"))?;
    let api_hash = ask("api_hash: ")?;
    if api_hash.len() < 16 {
        return Err(anyhow!("api_hash выглядит неправильно"));
    }
    println!("\nПридумайте пароль приложения. Им шифруется всё, включая вход в Telegram.");
    println!("Забытый пароль восстановить нельзя: придётся входить в аккаунт заново.");
    let password = loop {
        let p1 = ask_secret("Пароль: ")?;
        if p1.chars().count() < 6 {
            println!("Слишком короткий, минимум 6 символов.");
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

async fn run() -> Result<()> {
    let vault = VaultFile::new(data_dir().join("tglite.vault"));

    // 1. Пароль и ключи.
    let (api_id, api_hash, password, blob) = if vault.exists() {
        println!("Tg-Lite · файл данных: {}", vault.path().display());
        let mut tries = 0;
        loop {
            let pw = ask_secret("Пароль: ")?;
            match vault.load(&pw) {
                Ok(s) => break (s.api_id, s.api_hash, pw, s.session),
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
        (id, hash, pw, SessionBlob::default())
    };

    // 2. Подключение (при необходимости через SOCKS5-прокси).
    let session = Arc::new(VaultSession::from_blob(blob));
    let saver = Arc::new(Saver { vault, api_id, api_hash: api_hash.clone(), session: Arc::clone(&session), password });
    saver.save()?; // файл создаётся сразу, ключи не потеряются

    let proxy_file = data_dir().join("proxy.txt");
    let mut proxy = std::env::var("TGLITE_PROXY").ok().filter(|s| !s.trim().is_empty()).or_else(|| {
        std::fs::read_to_string(&proxy_file).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    });
    let (conn, authorized) = loop {
        match &proxy {
            Some(p) => println!("Подключаюсь к Telegram через прокси {p}…"),
            None => println!("Подключаюсь к Telegram…"),
        }
        let conn = tg::connect(Arc::clone(&session), api_id, proxy.clone());
        match tokio::time::timeout(Duration::from_secs(25), conn.client.is_authorized()).await {
            Ok(Ok(a)) => break (conn, a),
            res => {
                let why = match res {
                    Ok(Err(e)) => e.to_string(),
                    _ => "нет ответа за 25 секунд".to_string(),
                };
                conn.quit.quit();
                println!("\nНе удалось подключиться: {why}.");
                println!("Если VPN работает как расширение или «системный прокси», программа его не видит.");
                println!("Введите SOCKS5-прокси из вашего VPN-клиента, например 127.0.0.1:1080");
                println!("(или Enter, чтобы просто повторить; 0 — убрать прокси; q — выйти).");
                let a = ask("Прокси: ")?;
                match a.as_str() {
                    "q" | "Q" => return Err(anyhow!("подключение отменено")),
                    "" => {}
                    "0" => {
                        proxy = None;
                        let _ = std::fs::remove_file(&proxy_file);
                    }
                    other => {
                        let url = if other.contains("://") { other.to_string() } else { format!("socks5://{other}") };
                        let _ = std::fs::write(&proxy_file, &url);
                        proxy = Some(url);
                    }
                }
            }
        }
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
        eprintln!("\nОшибка: {e}");
        eprint!("Нажмите Enter, чтобы закрыть…");
        let _ = io::stderr().flush();
        let mut s = String::new();
        let _ = io::stdin().read_line(&mut s);
        std::process::exit(1);
    }
}
