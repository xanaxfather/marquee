// Embeds the app icon and version info into the Windows .exe.
fn main() {
    println!("cargo:rerun-if-changed=assets/marquee.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/marquee.ico");
        res.set("ProductName", "Marquee");
        res.set("FileDescription", "Marquee — Discord Rich Presence for what you're watching");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed Windows resources: {e}");
        }
    }
}
