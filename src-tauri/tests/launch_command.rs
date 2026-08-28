//! Host-runnable coverage for the `launch_game` orchestration (issue #53).
//!
//! `commands.rs::launch_game` is a thin `#[tauri::command]` shell over
//! [`gmm_lib::runtime::launch::launch`]; this file drives that production
//! function directly rather than reconstructing the sequence out of
//! `Core` + `gmm_loader` calls the way `tests/e2e_windows.rs` does.
//!
//! Everything in here exercises the guard rails that fire **before** the
//! first Windows-only call (`Loader::load`), which is why it runs on any
//! host: the double-launch refusal, the unset-install-path refusal, and
//! the missing-Model-Importer refusal. Each asserts the same three
//! cleanup properties — no Game Session persisted, no live session
//! installed, and no `session-started` event on the wire.
//!
//! The spawn / inject / watcher half of the same function lives in
//! `tests/launch_command_windows.rs`.

use std::sync::{Arc, Mutex};

use chrono::Utc;
use gmm_lib::core::{Core, GameCode, SessionInfo};
use gmm_lib::runtime::launch::{self, LaunchOptions};
use gmm_lib::runtime::{SessionRuntime, SESSION_ENDED_EVENT, SESSION_STARTED_EVENT};
use tauri::test::{mock_app, MockRuntime};
use tauri::{App, Listener};
use tempfile::TempDir;

/// Records session events in emission order so tests can assert the
/// sequence, not just membership.
#[derive(Clone, Default)]
struct EventLog {
    events: Arc<Mutex<Vec<String>>>,
}

impl EventLog {
    fn attach(app: &App<MockRuntime>) -> Self {
        let log = Self::default();
        for name in [SESSION_STARTED_EVENT, SESSION_ENDED_EVENT] {
            let sink = log.events.clone();
            app.listen(name, move |_| {
                sink.lock()
                    .expect("event log poisoned")
                    .push(name.to_string());
            });
        }
        log
    }

    fn names(&self) -> Vec<String> {
        self.events.lock().expect("event log poisoned").clone()
    }
}

async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init core")
}

/// Assert the three properties every refused launch must hold: nothing
/// persisted, nothing live, nothing on the wire.
async fn assert_nothing_started(core: &Core, runtime: &SessionRuntime, events: &EventLog) {
    assert_eq!(
        core.session_info().await.expect("session info"),
        None,
        "a refused launch must not persist a Game Session",
    );
    assert!(
        !runtime.has_session(),
        "a refused launch must not install a live session",
    );
    assert!(
        events.names().is_empty(),
        "a refused launch emits nothing, got: {:?}",
        events.names(),
    );
    core.set_library_root(None)
        .await
        .expect("a refused launch must retire its pre-spawn Library blocker");
}

#[tokio::test]
async fn refuses_a_second_launch_while_a_game_session_is_active() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let app = mock_app();
    let events = EventLog::attach(&app);
    let runtime = SessionRuntime::new();

    // A Game Session is already persisted — as if the user's first
    // launch is still running.
    let active = SessionInfo {
        game: GameCode::Gimi,
        pid: 4242,
        started_at: Utc::now(),
    };
    core.start_session(&active).await.expect("start session");

    let err = launch::launch(
        app.handle(),
        &core,
        &runtime,
        GameCode::Srmi,
        &LaunchOptions::default(),
    )
    .await
    .expect_err("a second launch must be refused while a session is active");

    assert!(
        err.message.contains("already running"),
        "error should name the active session, got: {err}",
    );
    assert_eq!(
        core.session_info().await.expect("session info"),
        Some(active),
        "the refused launch must leave the first Game Session untouched",
    );
    assert!(
        !runtime.has_session(),
        "a refused launch must not install a live session",
    );
    assert!(
        events.names().is_empty(),
        "a refused launch emits nothing, got: {:?}",
        events.names(),
    );
}

#[tokio::test]
async fn refuses_to_launch_before_the_game_install_path_is_set() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let app = mock_app();
    let events = EventLog::attach(&app);
    let runtime = SessionRuntime::new();

    let err = launch::launch(
        app.handle(),
        &core,
        &runtime,
        GameCode::Gimi,
        &LaunchOptions::default(),
    )
    .await
    .expect_err("no install path means nothing to launch");

    assert!(
        err.message.contains("install path"),
        "error should point the user at Settings, got: {err}",
    );
    assert_nothing_started(&core, &runtime, &events).await;
}

#[tokio::test]
async fn refuses_to_launch_when_no_game_executable_is_present() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let app = mock_app();
    let events = EventLog::attach(&app);
    let runtime = SessionRuntime::new();

    // An install directory that exists but holds none of the Game's
    // executable candidates.
    let install = tmp.path().join("Genshin Impact Game");
    std::fs::create_dir_all(&install).expect("install dir");
    core.set_game_install_path(GameCode::Gimi, &install)
        .await
        .expect("persist install path");

    let err = launch::launch(
        app.handle(),
        &core,
        &runtime,
        GameCode::Gimi,
        &LaunchOptions::default(),
    )
    .await
    .expect_err("a directory with no game exe cannot be launched");

    for candidate in GameCode::Gimi.profile().executable_candidates {
        assert!(
            err.message.contains(candidate),
            "error should list the candidates it looked for ({candidate}), got: {err}",
        );
    }
    assert_nothing_started(&core, &runtime, &events).await;
}

#[tokio::test]
async fn refuses_to_launch_before_the_model_importer_is_installed() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let app = mock_app();
    let events = EventLog::attach(&app);
    let runtime = SessionRuntime::new();

    // Game exe present, Model Importer `d3d11.dll` absent — launching
    // now would start the game unmodded with a live CBT hook, so the
    // check has to fire before anything is spawned.
    let install = tmp.path().join("Genshin Impact Game");
    std::fs::create_dir_all(&install).expect("install dir");
    std::fs::write(install.join("GenshinImpact.exe"), b"not a real PE").expect("fake exe");
    core.set_game_install_path(GameCode::Gimi, &install)
        .await
        .expect("persist install path");

    let err = launch::launch(
        app.handle(),
        &core,
        &runtime,
        GameCode::Gimi,
        &LaunchOptions::default(),
    )
    .await
    .expect_err("no Model Importer means no modded launch");

    assert!(
        err.message.contains("d3d11.dll") && err.message.contains("importer"),
        "error should name the missing Model Importer DLL, got: {err}",
    );
    assert_nothing_started(&core, &runtime, &events).await;
}
