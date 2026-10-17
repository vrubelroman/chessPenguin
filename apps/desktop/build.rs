// Embeds the application icon into the Windows executable.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../packaging/windows/chesspenguin.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the Windows icon: {e}");
        }
    }
}
