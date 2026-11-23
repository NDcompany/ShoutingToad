// Embeds the app icon and version info into the Windows executable.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico")
        .set("ProductName", "ShoutingToad")
        .set("FileDescription", "ShoutingToad SEO Spider")
        .set("CompanyName", "N&D Co.")
        .set("LegalCopyright", "Copyright N&D Co. Licensed under AGPL-3.0-or-later.");
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed the Windows icon: {e}");
    }
}
