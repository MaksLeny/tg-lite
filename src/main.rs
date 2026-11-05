#![allow(clippy::field_reassign_with_default)]
mod i18n;
mod lock;
mod proxy;
mod settings;
mod tg;
mod ui;
mod util;
mod vault;

/// Счётчик выделенной памяти для теста «бюджет ОЗУ»: считает только в потоке теста.
#[cfg(test)]
pub mod alloc_count {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    pub struct Counting;

    thread_local! {
        static LIVE: Cell<isize> = const { Cell::new(0) };
    }

    /// Сколько байт сейчас занято выделениями этого потока (относительно начала).
    pub fn live() -> isize {
        LIVE.with(Cell::get)
    }

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            LIVE.with(|c| c.set(c.get() + l.size() as isize));
            unsafe { System.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            LIVE.with(|c| c.set(c.get() - l.size() as isize));
            unsafe { System.dealloc(p, l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
            LIVE.with(|c| c.set(c.get() + new as isize - l.size() as isize));
            unsafe { System.realloc(p, l, new) }
        }
    }
}

#[cfg(test)]
#[global_allocator]
static ALLOC: alloc_count::Counting = alloc_count::Counting;

use std::io::{self, IsTerminal, Write};
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
    if io::stdin().read_line(&mut s)? == 0 {
        return Err(anyhow!(tr!("ввод закрыт", "input closed"))); // иначе при закрытом вводе цикл не кончился бы
    }
    Ok(s.trim().to_string())
}

fn ask_secret(prompt: &str) -> Result<String> {
    // Подсказку печатаем сами (rpassword на Windows портит кириллицу), а ввод скрываем им.
    print!("{prompt}");
    io::stdout().flush()?;
    let pw = rpassword::read_password()?;
    println!();
    if pw.is_empty() && !io::stdin().is_terminal() {
        return Err(anyhow!(tr!("ввод закрыт", "input closed")));
    }
    Ok(pw)
}

/// Первый запуск: ключи API (если не вшиты в сборку) и пароль приложения любой длины.
fn first_run_setup() -> Result<(i32, String, String)> {
    // Шаг 1: язык (если ещё не выбран раньше).
    if !data_dir().join("tglite.lang").exists() {
        println!("Tg-Lite {}\n", env!("CARGO_PKG_VERSION"));
        println!("Step 1/4. Language / Язык:  1) Русский   2) English");
        let choice = ask("> ")?;
        i18n::set(u8::from(choice.trim() == "2" || choice.trim().eq_ignore_ascii_case("en")));
        i18n::save();
    }
    println!("\n{}", tr!("Первый запуск: четыре коротких шага.", "First run: four short steps."));
    let (api_id, api_hash) = match embedded_keys() {
        Some(k) => k,
        None => {
            println!("\n{}", tr!("Шаг 2/4. Ключи Telegram (api_id и api_hash).", "Step 2/4. Telegram keys (api_id and api_hash)."));
            println!(
                "{}",
                tr!(
                    "  1) Откройте https://my.telegram.org и войдите по номеру телефона.\n  2) Выберите «API development tools» и создайте приложение (название любое).\n  3) Скопируйте api_id (число) и api_hash (длинная строка) и вставьте их ниже (Ctrl+V или правая кнопка мыши).",
                    "  1) Open https://my.telegram.org and sign in with your phone number.\n  2) Choose \"API development tools\" and create an app (any name).\n  3) Copy api_id (a number) and api_hash (a long string) and paste them below (Ctrl+V or right-click)."
                )
            );
            let api_id: i32 = ask("api_id: ")?.parse().map_err(|_| anyhow!(tr!("api_id должен быть числом", "api_id must be a number")))?;
            let api_hash = ask("api_hash: ")?;
            if api_hash.len() < 16 {
                return Err(anyhow!(tr!("api_hash выглядит неправильно", "api_hash looks wrong")));
            }
            (api_id, api_hash)
        }
    };
    println!("\n{}", tr!("Шаг 3/4. Пароль приложения.", "Step 3/4. App password."));
    println!(
        "{}",
        tr!(
            "Придумайте пароль (любой длины, даже один символ). Им шифруется всё, включая вход в Telegram. При вводе символы не показываются.",
            "Choose a password (any length, even one character). It encrypts everything, including your Telegram login. Characters are not shown while you type."
        )
    );
    println!(
        "{}",
        tr!(
            "Забытый пароль восстановить нельзя: придётся входить в аккаунт заново.",
            "A forgotten password cannot be recovered: you will have to sign in to your account again."
        )
    );
    let password = loop {
        let p1 = ask_secret(tr!("Пароль: ", "Password: "))?;
        if p1.is_empty() {
            println!("{}", tr!("Пароль не может быть пустым.", "The password cannot be empty."));
            continue;
        }
        let p2 = ask_secret(tr!("Повторите пароль: ", "Repeat password: "))?;
        if p1 == p2 {
            break p1;
        }
        println!("{}", tr!("Пароли не совпали, ещё раз.", "Passwords do not match, try again."));
    };
    Ok((api_id, api_hash, password))
}

async fn sign_in_flow(client: &grammers_client::Client, api_hash: &str) -> Result<()> {
    println!("\n{}", tr!("Шаг 4/4. Вход в Telegram.", "Step 4/4. Signing in to Telegram."));
    let phone = ask(tr!("Номер телефона (+79001234567): ", "Phone number (+15551234567): "))?;
    let token = client.request_login_code(&phone, api_hash).await.map_err(|e| anyhow!("{e}"))?;
    let code = ask(tr!("Код из Telegram (придёт в приложение или SMS): ", "Telegram code (sent to the app or by SMS): "))?;
    match client.sign_in(&token, &code).await {
        Ok(_) => {}
        Err(SignInError::PasswordRequired(pt)) => {
            let hint = pt.hint().unwrap_or(tr!("нет", "none")).to_string();
            let pw = ask_secret(&trf!("Облачный пароль Telegram (подсказка: {hint}): ", "Telegram cloud password (hint: {hint}): "))?;
            client.check_password(pt, &pw).await.map_err(|e| anyhow!("{e}"))?;
        }
        Err(SignInError::InvalidCode) => return Err(anyhow!(tr!("неверный код", "invalid code"))),
        Err(e) => return Err(anyhow!("{e}")),
    }
    println!("{}", tr!("Вход выполнен.", "Signed in."));
    Ok(())
}

/// Пробует подключиться по очереди всеми подходящими маршрутами. Возвращает рабочее подключение.
async fn connect_auto(session: &Arc<VaultSession>, api_id: i32, routes: &[Route]) -> Option<(tg::Connection, bool)> {
    for route in routes {
        let url = match route {
            Route::Direct => None,
            Route::Socks5(a) => Some(format!("socks5://{a}")),
            Route::HttpBridge(a) => match proxy::start_bridge(a.clone()).await {
                Ok(local) => Some(format!("socks5://{local}")),
                Err(e) => {
                    log(&format!("bridge for {}: {e}", proxy::redact(a)));
                    continue;
                }
            },
        };
        println!("{}", trf!("Подключаюсь к Telegram {}…", "Connecting to Telegram {}…", route.label()));
        util::debug(&format!("trying route {}", route.label()));
        let conn = tg::connect(Arc::clone(session), api_id, url);
        match tokio::time::timeout(Duration::from_secs(12), conn.client.is_authorized()).await {
            Ok(Ok(authorized)) => {
                log(&format!("connected {}", route.label()));
                return Some((conn, authorized));
            }
            Ok(Err(e)) => log(&format!("connect {}: {}", route.label(), proxy::redact_text(&e.to_string()))),
            Err(_) => log(&format!("connect {}: timeout", route.label())),
        }
        conn.quit.quit();
    }
    None
}

/// Запрос пароля обычной строкой (запасной вариант без экрана входа).
fn console_login(vault: &VaultFile) -> Result<(vault::Secrets, String)> {
    println!("{}", trf!("Tg-Lite {} · файл данных: {}", "Tg-Lite {} · data file: {}", env!("CARGO_PKG_VERSION"), vault.path().display()));
    let mut tries = 0;
    loop {
        let pw = ask_secret(tr!("Пароль: ", "Password: "))?;
        match vault.load(&pw) {
            Ok(s) => return Ok((s, pw)),
            Err(e) => {
                tries += 1;
                println!("{e}");
                if tries >= 5 {
                    return Err(anyhow!(tr!("слишком много неверных попыток", "too many wrong attempts")));
                }
            }
        }
    }
}

async fn run() -> Result<()> {
    i18n::load();
    let vault = VaultFile::new(data_dir().join("tglite.vault"));

    // 1. Пароль, ключи и настройки. Вход идёт на экране с анимированным фоном;
    // если он недоступен (ввод перенаправлен), пароль спрашивается обычной строкой.
    let (api_id, api_hash, password, blob, mut settings) = if vault.exists() {
        let accent = ratatui::style::Color::Rgb(42, 171, 238);
        let (s, pw) = match lock::login(&vault, env!("CARGO_PKG_VERSION"), accent)? {
            lock::Login::Ok(s, pw) => (*s, pw),
            lock::Login::Quit => return Ok(()),
            lock::Login::TooMany => return Err(anyhow!(tr!("слишком много неверных попыток", "too many wrong attempts"))),
            lock::Login::NoTerminal => console_login(&vault)?,
        };
        (s.api_id, s.api_hash, pw, s.session, s.settings)
    } else {
        let (id, hash, pw) = first_run_setup()?;
        (id, hash, pw, SessionBlob::default(), Settings::default())
    };

    settings.lang = i18n::current();

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
        println!("\n{}", tr!("Не удалось подключиться к Telegram ни одним способом.", "Could not connect to Telegram by any route."));
        println!(
            "{}",
            tr!(
                "Проверьте интернет и VPN. Если VPN работает через прокси, введите его адрес,",
                "Check your internet and VPN. If your VPN works through a proxy, enter its address,"
            )
        );
        println!(
            "{}",
            tr!(
                "например 127.0.0.1:10808 (Enter — повторить попытку, 0 — без прокси, q — выйти).",
                "e.g. 127.0.0.1:10808 (Enter - retry, 0 - no proxy, q - quit)."
            )
        );
        let a = ask(tr!("Прокси: ", "Proxy: "))?;
        let mut s = saver.settings();
        match a.as_str() {
            "q" | "Q" => return Err(anyhow!(tr!("подключение отменено", "connection cancelled"))),
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
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);

    // 4. Сохранение и выход.
    let _ = saver.save();
    quit_handle.quit();
    if saver.is_wiped() {
        println!("{}", tr!("Данные Tg-Lite удалены (слишком много неверных паролей).", "Tg-Lite data deleted (too many wrong passwords)."));
        println!(
            "{}",
            tr!(
                "Чтобы закрыть доступ к аккаунту, завершите эту сессию в Telegram: Настройки → Устройства.",
                "To block access to your account, end this session in Telegram: Settings → Devices."
            )
        );
        std::thread::sleep(Duration::from_secs(4));
    }
    result
}

fn help() -> &'static str {
    tr!(
        "Tg-Lite: лёгкий клиент Telegram для терминала.

Использование: tg-lite [параметры]
  --debug      подробный журнал в tglite.log (без текстов переписки)
  --version    показать версию
  --help       эта справка

Папка данных (tglite.vault, tglite.log, downloads, export) рядом с программой;
другую можно задать переменной окружения TGLITE_DIR.",
        "Tg-Lite: a lightweight Telegram client for the terminal.

Usage: tg-lite [options]
  --debug      verbose log in tglite.log (no message texts)
  --version    show the version
  --help       this help

The data folder (tglite.vault, tglite.log, downloads, export) is next to the program;
set the TGLITE_DIR environment variable to use another one."
    )
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    i18n::load();
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--version" | "-V" => {
                println!("tg-lite {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--help" | "-h" | "/?" => {
                println!("{}", help());
                return;
            }
            "--debug" => util::set_debug(true),
            other => {
                eprintln!("{}\n\n{}", trf!("Неизвестный параметр: {other}", "Unknown option: {other}"), help());
                return;
            }
        }
    }
    util::debug(&format!("start {}", env!("CARGO_PKG_VERSION")));
    if let Err(e) = run().await {
        log(&format!("fatal: {e}"));
        eprintln!("\n{}", trf!("Ошибка: {e}", "Error: {e}"));
        eprint!("{}", tr!("Нажмите Enter, чтобы закрыть…", "Press Enter to close…"));
        let _ = io::stderr().flush();
        let mut s = String::new();
        let _ = io::stdin().read_line(&mut s);
        std::process::exit(1);
    }
}
