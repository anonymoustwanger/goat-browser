// Give the UI thread an 8 MB stack on Windows (the default is only 1 MB).
fn main() {
    println!("cargo:rerun-if-changed=assets");
    // The installer embeds bundle/Goatr.exe; an empty one means "download from GitHub".
    let bundle = std::path::Path::new("bundle/Goatr.exe");
    if !bundle.exists() {
        let _ = std::fs::create_dir_all("bundle");
        let _ = std::fs::write(bundle, b"");
    }
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows-msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    } else if target.contains("windows-gnu") {
        println!("cargo:rustc-link-arg-bins=-Wl,--stack,8388608");
        // Goat logo as the .exe icon.
        let out = std::env::var("OUT_DIR").unwrap();
        let obj = format!("{out}/goatr-icon.o");
        let ok = std::process::Command::new("x86_64-w64-mingw32-windres")
            .args(["assets/goatr.rc", "-O", "coff", "-o", &obj])
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            println!("cargo:rustc-link-arg-bins={obj}");
        }
    }
}
