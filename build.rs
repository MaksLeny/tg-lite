// Встраивает иконку и описание в tg-lite.exe при сборке под Windows.
// Если встроить не получилось, сборка всё равно продолжается (просто без иконки).
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("assets/icon.ico");
            res.set("ProductName", "Tg-Lite");
            res.set("FileDescription", "Tg-Lite: lightweight Telegram client");
            if let Err(e) = res.compile() {
                println!("cargo:warning=icon was not embedded: {e}");
            }
        }
    }
}
