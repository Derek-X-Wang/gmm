//! Full-vertical end-to-end test on a **fake game install**.
//!
//! This is the automated stand-in for the manual Windows smoke a human
//! would otherwise run: point GMM at a game, install the Model Importer,
//! adopt a mod, toggle it on, launch the game, confirm the importer DLL
//! is injected, then tear everything down.
//!
//! The fixtures are the ones slices 4a/4b already paid for:
//!
//! ```text
//! <tmp>/game/
//! ├── GenshinImpact.exe     ← target/debug/victim.exe, renamed
//! ├── GenshinImpact_Data/   ← marker dir the detector requires
//! ├── d3d11.dll             ← target/debug/noop_dll.dll, renamed
//! │                           (exports CBTProc, which is what
//! │                            3dmloader GetProcAddress's)
//! └── Mods/                 ← junctions land here
//! ```
//!
//! Because `victim.exe` creates a real window and `noop_dll.dll` exports
//! the entry point 3dmloader expects, the loader pipeline behaves the
//! same way it would against a real game — without shipping a gacha game
//! to a CI runner.
//!
//! Windows-only: NTFS junctions, the CBT hook, and the PE fixtures all
//! require it. The whole file compiles away elsewhere.

#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use chrono::Utc;
use gmm_lib::core::{detect, importer, Core, GameCode, SessionInfo};
use gmm_loader::Loader;
use tempfile::TempDir;

const TARGET_PROCESS: &str = "GenshinImpact.exe";
const WAIT_TIMEOUT_SECS: i32 = 30;

/// The DLL the injection step actually hooks.
///
/// The importer installs a real `d3d11.dll` and we assert on that — but
/// we deliberately do **not** inject it here. `LoadLibraryW` resolves by
/// module base name: if a DLL called `d3d11.dll` is already mapped into
/// the target process (it usually is, from System32), Windows hands back
/// the existing module instead of loading ours, and
/// `WaitForInjection`'s exact path comparison then never matches.
///
/// A real game doesn't hit this, because the DLL search order loads the
/// proxy from the game's own directory before anything pulls in the
/// system copy. Our `victim.exe` stand-in never loads D3D at all, so the
/// collision is an artefact of the fixture, not a product bug. Using a
/// unique name keeps the injection assertion honest while leaving the
/// path realistic (game directory, spaces, temp-dir depth).
const PROBE_DLL: &str = "gmm-e2e-probe.dll";

fn src_tauri_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn build_artifact(name: &str) -> PathBuf {
    let p = src_tauri_dir().join("target/debug").join(name);
    assert!(
        p.exists(),
        "{name} missing at {p:?} — run `cargo build --workspace` before this test",
    );
    p
}

fn vendor_loader_dll() -> PathBuf {
    let p = src_tauri_dir()
        .parent()
        .expect("repo root")
        .join("vendor/3dmloader/3dmloader.dll");
    assert!(p.exists(), "3dmloader.dll missing at {p:?}");
    p
}

/// Lay out a directory that passes `detect::genshin::validate` and can
/// host both the Model Importer and the junctions.
fn make_fake_game(tmp: &Path) -> PathBuf {
    let game = tmp.join("game/Genshin Impact Game");
    fs::create_dir_all(game.join("GenshinImpact_Data")).expect("data dir");
    fs::create_dir_all(game.join("Mods")).expect("mods dir");
    fs::copy(build_artifact("victim.exe"), game.join(TARGET_PROCESS)).expect("copy victim.exe");
    game
}

/// Build a zip shaped like a `*MI-Package` release so the real importer
/// install path (checksum, staging, swap, d3dx.ini rewrite) runs
/// unmodified. `d3d11.dll` inside the zip is our noop DLL, so the
/// installed importer is one 3dmloader can actually inject.
fn make_fake_importer_zip(dest: &Path) {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    let file = fs::File::create(dest).expect("create zip");
    let mut zw = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let noop = fs::read(build_artifact("noop_dll.dll")).expect("read noop_dll");
    zw.start_file("d3d11.dll", opts).expect("start d3d11");
    zw.write_all(&noop).expect("write d3d11");

    // Minimal d3dx.ini carrying the loader line the installer rewrites.
    zw.start_file("d3dx.ini", opts).expect("start d3dx");
    zw.write_all(b"[Loader]\nloader = XXMI Launcher.exe\n\n[Rendering]\ntexture_hash = 0\n")
        .expect("write d3dx");

    zw.finish().expect("finish zip");
}

/// Build a directory shaped like an extracted GameBanana mod.
fn make_fake_mod(dir: &Path, hash: &str) {
    fs::create_dir_all(dir).expect("mod dir");
    fs::write(
        dir.join("merged.ini"),
        format!("[TextureOverrideBody]\nhash = {hash}\nib = 12345\n"),
    )
    .expect("mod ini");
    fs::write(dir.join("Body.buf"), b"\x00\x01\x02\x03").expect("mod buf");
}

/// The whole product, headless.
///
/// 1. detection accepts the fake install
/// 2. the Model Importer installs into it (d3dx.ini rewritten to gmm.exe)
/// 3. a mod is adopted into the Library
/// 4. enabling it creates a real NTFS junction under `<game>/Mods/`
/// 5. the loader hooks, the game spawns, the importer DLL is injected
/// 6. the mod-mutation lock refuses edits while the session is live
/// 7. teardown unhooks, clears the session, and re-permits edits
/// 8. disabling removes the junction but leaves the Library copy alone
#[tokio::test(flavor = "multi_thread")]
async fn full_vertical_against_a_fake_game_install() {
    let tmp = TempDir::new().expect("tmp");
    let game_dir = make_fake_game(tmp.path());

    // ---- 1. detection ------------------------------------------------
    assert!(
        detect::genshin::validate(&game_dir),
        "fixture must look like a real Genshin install to the detector",
    );
    let found = detect::genshin::detect_from_paths([game_dir.clone()])
        .expect("detect_from_paths should accept the fixture");
    assert_eq!(found, game_dir);

    // ---- 2. importer install ----------------------------------------
    let zip_path = tmp.path().join("GIMI-Package.zip");
    make_fake_importer_zip(&zip_path);
    let backups = tmp.path().join("backups");
    let report = importer::install_from_local_zip(&zip_path, &game_dir, &backups, "gmm.exe")
        .expect("importer install");
    assert!(
        game_dir.join("d3d11.dll").exists(),
        "importer install must place d3d11.dll: {report:?}",
    );
    let d3dx = fs::read_to_string(game_dir.join("d3dx.ini")).expect("read d3dx.ini");
    assert!(
        d3dx.contains("gmm.exe"),
        "d3dx.ini loader line must be rewritten to gmm.exe, got:\n{d3dx}",
    );
    assert!(
        !d3dx.contains("XXMI Launcher.exe"),
        "old loader line must be gone, got:\n{d3dx}",
    );

    // ---- 3. adopt a mod ----------------------------------------------
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("core");
    core.set_game_install_path(GameCode::Gimi, &game_dir)
        .await
        .expect("persist install path");

    let fixture = tmp.path().join("downloads/HuTaoSkin");
    make_fake_mod(&fixture, "aabbccdd");
    let adopted = core
        .adopt_folder(GameCode::Gimi, &fixture, "Hu Tao Skin")
        .await
        .expect("adopt");
    assert!(!adopted.enabled, "adopted mods start disabled");

    // ---- 4. enable -> real NTFS junction ------------------------------
    let mods_dir = game_dir.join("Mods");
    core.set_enabled(&adopted.id, true, &mods_dir)
        .await
        .expect("enable");

    let link = mods_dir.join("Hu Tao Skin");
    assert!(link.exists(), "junction must exist at {link:?}");
    assert!(
        link.join("merged.ini").exists(),
        "junction must resolve into the Library copy",
    );
    let via_junction = fs::read_to_string(link.join("merged.ini")).expect("read through junction");
    assert!(
        via_junction.contains("aabbccdd"),
        "junction serves mod bytes"
    );

    // ---- 5. launch: hook, spawn, inject -------------------------------
    // Same bytes as the installed d3d11.dll, unique base name — see
    // PROBE_DLL for why the installed one can't be injected here.
    let probe = game_dir.join(PROBE_DLL);
    fs::copy(game_dir.join("d3d11.dll"), &probe).expect("stage probe dll");

    let loader = Loader::load(&vendor_loader_dll()).expect("load 3dmloader");
    let hook = loader.hook(&probe).expect("install CBT hook");

    // Keep the victim alive comfortably longer than the injection wait,
    // otherwise a timeout tells us nothing: WaitForInjection would just
    // be polling a process that already exited on its own timer.
    let mut game = Command::new(game_dir.join(TARGET_PROCESS))
        .current_dir(&game_dir)
        .env(
            "GMM_VICTIM_TIMEOUT_SECS",
            (WAIT_TIMEOUT_SECS + 30).to_string(),
        )
        .spawn()
        .expect("spawn fake game");

    core.start_session(&SessionInfo {
        game: GameCode::Gimi,
        pid: game.id(),
        started_at: Utc::now(),
    })
    .await
    .expect("start session");

    if let Err(e) = hook.wait_for_injection(TARGET_PROCESS, WAIT_TIMEOUT_SECS) {
        // A bare timeout says nothing about *why*. Dump the state that
        // actually distinguishes the candidate causes before failing:
        // did the victim die early, is it visible to a process
        // snapshot, and which modules did it end up loading?
        let alive = matches!(game.try_wait(), Ok(None));
        let modules = diagnostics::modules_of(game.id());
        let _ = game.kill();
        panic!(
            "injection never verified: {e}\n\
             victim pid           = {}\n\
             victim still running = {alive}\n\
             probe dll           = {}\n\
             modules loaded in victim ({}):\n{}",
            game.id(),
            probe.display(),
            modules.len(),
            modules
                .iter()
                .map(|m| format!("  {m}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    // ---- 6. mutation lock while the session is live -------------------
    let locked = core
        .set_enabled(&adopted.id, false, &mods_dir)
        .await
        .expect_err("mod edits must be refused during a session");
    let msg = locked.to_string().to_lowercase();
    assert!(
        msg.contains("session") || msg.contains("running"),
        "lock error should name the session, got: {locked}",
    );
    assert!(
        link.exists(),
        "the refused toggle must not have removed the junction",
    );

    // ---- 7. teardown --------------------------------------------------
    drop(hook); // RAII unhook
    let _ = game.kill();
    let start = Instant::now();
    loop {
        if matches!(game.try_wait(), Ok(Some(_))) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "fake game did not exit within 30 s of kill()",
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    core.end_session().await.expect("end session");
    drop(loader);

    // ---- 8. disable -> junction gone, Library intact -------------------
    core.set_enabled(&adopted.id, false, &mods_dir)
        .await
        .expect("disable after teardown");
    assert!(!link.exists(), "junction removed on disable");
    assert!(
        adopted.library_path.join("merged.ini").exists(),
        "Library copy survives disable — junctions never own the bytes",
    );
}

/// Importer rollback against a *populated* game directory: install once,
/// install again (which backs the first up), then roll back and confirm
/// the directory is byte-identical to the first install.
#[tokio::test(flavor = "multi_thread")]
async fn importer_rollback_restores_a_populated_game_dir() {
    let tmp = TempDir::new().expect("tmp");
    let game_dir = make_fake_game(tmp.path());
    let backups = tmp.path().join("backups");

    let zip_path = tmp.path().join("GIMI-Package.zip");
    make_fake_importer_zip(&zip_path);

    importer::install_from_local_zip(&zip_path, &game_dir, &backups, "gmm.exe")
        .expect("first install");
    let first_dll = fs::read(game_dir.join("d3d11.dll")).expect("read installed dll");
    let first_ini = fs::read_to_string(game_dir.join("d3dx.ini")).expect("read installed ini");

    // A second install backs up the first one.
    let report = importer::install_from_local_zip(&zip_path, &game_dir, &backups, "gmm.exe")
        .expect("second install");
    let backup_dir = report
        .backup_dir
        .as_ref()
        .expect("second install must produce a backup of the first");

    // Corrupt the live install, then roll back.
    fs::write(game_dir.join("d3d11.dll"), b"corrupted").expect("corrupt dll");
    importer::rollback_to(backup_dir, &game_dir).expect("rollback");

    assert_eq!(
        fs::read(game_dir.join("d3d11.dll")).expect("read restored dll"),
        first_dll,
        "rollback must restore the DLL byte-for-byte",
    );
    assert_eq!(
        fs::read_to_string(game_dir.join("d3dx.ini")).expect("read restored ini"),
        first_ini,
        "rollback must restore d3dx.ini byte-for-byte",
    );
    assert!(
        game_dir.join(TARGET_PROCESS).exists(),
        "rollback must not disturb the game's own files",
    );
}

/// Process-introspection helpers used only to explain a failure.
///
/// `WaitForInjection` returning a bare status code tells us nothing
/// about which link in the chain broke. Enumerating the target's module
/// list separates "the DLL never loaded" from "it loaded under a
/// different path than the one we compared against" — the two failure
/// modes look identical from the outside.
mod diagnostics {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, MODULEENTRY32W, TH32CS_SNAPMODULE,
        TH32CS_SNAPMODULE32,
    };

    /// Full paths of every module mapped into `pid`. Empty when the
    /// process is gone or the snapshot fails — this is best-effort
    /// diagnostic output, never an assertion source.
    pub fn modules_of(pid: u32) -> Vec<String> {
        let mut out = Vec::new();
        // SAFETY: the snapshot handle is checked against
        // INVALID_HANDLE_VALUE and closed on every path out. The
        // MODULEENTRY32W is zeroed with its dwSize set, as the API
        // requires.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
            if snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut entry: MODULEENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
            if Module32FirstW(snap, &mut entry) != 0 {
                loop {
                    let len = entry
                        .szExePath
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExePath.len());
                    out.push(String::from_utf16_lossy(&entry.szExePath[..len]));
                    if Module32NextW(snap, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
        out
    }
}
