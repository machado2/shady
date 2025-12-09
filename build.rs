fn main() {
    // Only embed resources on Windows targets.
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }

    // Use winres to embed the application manifest.
    // This works with any linker (MSVC link.exe, rust-lld, etc.)
    let mut res = winres::WindowsResource::new();
    res.set_manifest_file("shady.manifest");
    if let Err(e) = res.compile() {
        eprintln!("warning: failed to embed manifest: {}", e);
    }
}

