//! Windows build steps:
//! - embed the app icon and version info;
//! - (MSVC) embed assets/windows.manifest, which opts the binary into the Segment Heap. With the
//!   default NT heap a crawl that keeps 50k+ pages in memory slows from ~3500 to under 600 URLs/s
//!   as the heap grows; with the Segment Heap the rate stays flat. Windows 10 2004+ honours it.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/windows.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico")
        .set("ProductName", "ShoutingToad")
        .set("FileDescription", "ShoutingToad SEO Spider")
        .set("CompanyName", "N&D Co.")
        .set(
            "LegalCopyright",
            "Copyright N&D Co. Licensed under AGPL-3.0-or-later.",
        );
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed the Windows icon: {e}");
    }
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        let manifest = std::path::Path::new(&dir)
            .join("assets")
            .join("windows.manifest");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
