//! Windows (MSVC): embed assets/windows.manifest, which opts the binary into the Segment Heap.
//! With the default NT heap a crawl that keeps 50k+ pages in memory slows from ~3500 to under
//! 600 URLs/s as the heap grows; with the Segment Heap the rate stays flat. Windows 10 2004+
//! honours it, older versions ignore it.
fn main() {
    println!("cargo:rerun-if-changed=assets/windows.manifest");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        let manifest = std::path::Path::new(&dir).join("assets").join("windows.manifest");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
    }
}
