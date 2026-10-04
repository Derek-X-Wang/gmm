fn main() {
    // The CLI links Tauri-backed runtime code that imports Common-Controls v6
    // (TaskDialogIndirect). The root package's linker arguments do not reach
    // this package, so the executable must embed the same manifest itself.
    // Match the root build's native-Windows guard: cross-linking with lld-link
    // otherwise requires mt.exe on the non-Windows host.
    let targets_windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if cfg!(windows) && targets_windows {
        let manifest =
            std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"))
                .join("../../windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
}
