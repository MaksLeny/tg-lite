// Встраивает иконку и описание в tg-lite.exe при сборке под Windows.
// Если встроить не получилось, сборка всё равно продолжается (просто без иконки).
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=TGLITE_API_ID");
    println!("cargo:rerun-if-env-changed=TGLITE_API_HASH");

    #[cfg(windows)]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("assets/icon.ico");
            res.set("ProductName", "Tg-Lite");
            res.set("FileVersion", env!("CARGO_PKG_VERSION"));
            res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
            res.set("FileDescription", "Tg-Lite: lightweight Telegram client");
            if let Err(e) = res.compile() {
                println!("cargo:warning=icon was not embedded: {e}");
            }
        }
    }
}
