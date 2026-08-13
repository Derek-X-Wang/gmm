fn main() {
    let mut attributes = tauri_build::Attributes::new();

    // Windows: embed the application manifest ourselves instead of
    // letting tauri-build do it.
    //
    // tauri-build routes the manifest through `embed-resource`, which
    // links it with `cargo:rustc-link-arg-bins` — bins only. Test
    // binaries therefore link without the Common-Controls v6 dependency,
    // and any test that builds a Tauri `App` (`tauri::test::mock_app`)
    // dies at load with STATUS_ENTRYPOINT_NOT_FOUND before running a
    // single case. See tauri-apps/tauri#13419; this is the workaround
    // Tauri's own maintainers recommend. `rustc-link-arg` without the
    // `-bins` suffix covers every linked artifact, tests included.
    //
    // Host *and* target must be Windows. `/MANIFEST:EMBED` is handled
    // by link.exe directly, but lld-link — what `cargo xwin` uses to
    // cross-link from macOS/Linux — shells out to `mt.exe` and fails
    // without it. Cross-builds can compile the Windows tests but never
    // run them, so they lose nothing by skipping the manifest; this also
    // leaves cross-built artefacts exactly as they are today, since
    // tauri-build's own resource step already no-ops there.
    let targets_windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if cfg!(windows) && targets_windows {
        attributes = attributes
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
        embed_app_manifest();
    }

    tauri_build::try_build(attributes).expect("tauri-build failed");
}

/// Embed `windows-app-manifest.xml` into every artifact this crate
/// links. The file is a verbatim copy of tauri-build's own default
/// manifest, so the shipped exe declares exactly what it did before.
fn embed_app_manifest() {
    let manifest =
        std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"))
            .join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
}
