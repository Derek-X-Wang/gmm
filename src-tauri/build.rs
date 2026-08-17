fn main() {
    emit_shipped_loader_version();

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

/// Bake the version of the Loader this build ships into
/// `GMM_LOADER_VERSION`, read from the upstream `Manifest.json` that
/// is vendored alongside `3dmloader.dll`.
///
/// Before #78 the "installed" Loader version was read from a
/// `loader.installed_version` settings row that no code path ever
/// wrote, so it was permanently `None` and the update check had no
/// left-hand side. The Loader is embedded, not installed (ADR 0001) —
/// there is exactly one Loader per GMM build, and this is it.
///
/// `Manifest.json` is upstream's own signed statement about the DLL
/// beside it, and `vendor/3dmloader/README.md` requires replacing both
/// together when the pin moves. The `rerun-if-changed` lines below
/// mean the constant is regenerated whenever either file changes, so
/// it cannot go stale against the tree.
fn emit_shipped_loader_version() {
    let vendor = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"))
        .join("../vendor/3dmloader");
    let manifest = vendor.join("Manifest.json");
    let dll = vendor.join("3dmloader.dll");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-changed={}", dll.display());

    let raw = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest.display()));
    let json: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", manifest.display()));
    let version = json
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("{} has no string `version`", manifest.display()));

    // Upstream records a bare `0.8.8` in the manifest but tags its
    // releases `v0.8.8`. Normalise to tag form so the comparison
    // against a release `tag_name` is like-for-like.
    let tag = if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{version}")
    };
    println!("cargo:rustc-env=GMM_LOADER_VERSION={tag}");
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
