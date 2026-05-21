//! Slice 4b Windows smoke: launch a stub game-like process, verify the
//! Loader hook installs, the mod-mutation lock fires, and teardown is
//! clean.
//!
//! Reuses the `victim` crate from slice 4a as the stub game and
//! `noop_dll` as the stand-in for the per-game Model Importer DLL.
//!
//! On non-Windows hosts the test is a no-op (returns immediately) so
//! Linux CI stays green; the meaningful run is `build (windows-latest)`.

#![cfg(windows)]

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use chrono::Utc;
use gmm_lib::core::{Core, GameCode, SessionInfo};
use gmm_loader::Loader;
use tempfile::TempDir;

// We don't call WaitForInjection here — that's slice 4a's smoke
// (cargo xtask test-loader), which still runs in CI alongside this
// test. Slice 4b's job is to verify the session state machine + lock
// + cleanup, NOT to re-prove the loader binding.

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR points at src-tauri/. The workspace root is one
    // level up; the repo root is one more.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn target_dir() -> PathBuf {
    workspace_root().join("target/debug")
}

fn vendor_loader_dll() -> PathBuf {
    workspace_root()
        .parent()
        .expect("repo root")
        .join("vendor/3dmloader/3dmloader.dll")
}

/// Slice 6 (#16) — SRMI mirrors slice 4b's session machinery. The
/// loader / hook / victim plumbing is GameCode-agnostic, so the value
/// of a per-game smoke is verifying that
/// `start_session(SessionInfo { game: Srmi, ... })` round-trips through
/// the singleton lock just like GIMI did. Cheap to add (no extra
/// victim.exe spawn) and lights up the AC's "Windows runner smoke
/// mirroring slice 4b's" bullet.
#[tokio::test]
async fn windows_smoke_srmi_session_round_trip() {
    let tmp = TempDir::new().expect("tmp");
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init core");

    let info = SessionInfo {
        game: GameCode::Srmi,
        pid: 0,
        started_at: Utc::now(),
    };
    core.start_session(&info).await.expect("start srmi session");

    let active = core
        .session_info()
        .await
        .expect("session info")
        .expect("session row");
    assert_eq!(
        active.game.as_str(),
        "srmi",
        "SRMI session tagged correctly"
    );

    core.end_session().await.expect("end srmi session");
    assert!(
        core.session_info().await.expect("info").is_none(),
        "session row cleared after end_session",
    );
}

#[tokio::test]
async fn windows_smoke_full_session_round_trip() {
    let target = target_dir();
    let victim_exe = target.join("victim.exe");
    let noop_dll = target.join("noop_dll.dll");
    let loader_dll = vendor_loader_dll();

    assert!(victim_exe.exists(), "victim.exe missing at {victim_exe:?}");
    assert!(noop_dll.exists(), "noop_dll.dll missing at {noop_dll:?}");
    assert!(
        loader_dll.exists(),
        "3dmloader.dll missing at {loader_dll:?}"
    );

    let tmp = TempDir::new().expect("tmp");
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root.clone(), &db_url)
        .await
        .expect("init core");

    let game_mods = tmp.path().join("Genshin/Mods");
    std::fs::create_dir_all(&game_mods).expect("game mods");

    // Adopt one mod BEFORE the session starts so we can attempt to
    // toggle it later and observe the lock.
    let fixture = tmp.path().join("fixture/Mod1");
    std::fs::create_dir_all(&fixture).expect("fixture");
    std::fs::write(fixture.join("merged.ini"), "").expect("ini");
    let m = core
        .adopt_folder(GameCode::Gimi, &fixture, "Smoke Mod")
        .await
        .expect("adopt");

    // Install the hook BEFORE spawning the victim — CBT hooks must be
    // in place when the target window is created. We don't wait for
    // injection here (see top-of-file note); the assertion is that
    // HookLibrary returns status 0, which Loader::hook surfaces as
    // Ok(_).
    let loader = Loader::load(&loader_dll).expect("load 3dmloader");
    let session = loader.hook(&noop_dll).expect("install hook");

    let mut victim = Command::new(&victim_exe).spawn().expect("spawn victim");

    // Register the session as active. set_enabled must now refuse.
    core.start_session(&SessionInfo {
        game: GameCode::Gimi,
        pid: victim.id(),
        started_at: Utc::now(),
    })
    .await
    .expect("start session");

    let lock_err = core
        .set_enabled(&m.id, true, &game_mods)
        .await
        .expect_err("mod-mutation lock must fire while a session is active");
    assert!(
        lock_err.to_string().to_lowercase().contains("session")
            || lock_err.to_string().to_lowercase().contains("game running"),
        "lock error should mention session, got: {lock_err}",
    );

    // Kill victim → it exits cleanly enough for our purposes.
    let _ = victim.kill();
    let start = Instant::now();
    loop {
        if let Ok(Some(_)) = victim.try_wait() {
            break;
        }
        if start.elapsed() > Duration::from_secs(30) {
            panic!("victim did not exit within 30 s after kill");
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // Drop the HookSession → unhook via RAII. Drop the loader → FreeLibrary.
    drop(session);
    drop(loader);

    // Clear the persisted session row, then assert the lock is gone.
    core.end_session().await.expect("end session");
    assert!(
        core.session_info().await.expect("info").is_none(),
        "session cleared after end_session",
    );
    core.set_enabled(&m.id, true, &game_mods)
        .await
        .expect("set_enabled works again post-teardown");
}
