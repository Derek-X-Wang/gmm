//! Windows-gated coverage for the spawning half of the `launch_game`
//! orchestration (issue #53).
//!
//! `tests/e2e_windows.rs` reconstructs the launch sequence out of plain
//! `Core` and `gmm_loader` calls; this file drives the orchestration in
//! [`gmm_lib::runtime::launch::launch`] — the same function
//! `commands.rs::launch_game` delegates to — against the fake-game
//! fixture, and asserts the parts that reconstruction can never reach:
//! `ChildGuard` cleanup on every post-spawn failure, the atomic session
//! claim, `session-started` / `session-ended` ordering, and the exit
//! watcher's teardown.
//!
//! Fixture, per test: a temp install directory holding one of the Game's
//! executable candidates plus a `d3d11.dll` stand-in for the Model
//! Importer (`noop_dll.dll`, which exports the `CBTProc` symbol
//! 3dmloader resolves). Cargo runs these tests concurrently, so each one
//! picks a different Game — and therefore a different executable name —
//! wherever it asserts over a process snapshot by name. The two that
//! share `Endfield-Win64-Shipping.exe` assert on their own PID instead.
//!
//! Windows-only: the Loader FFI, the CBT hook, and the PE fixtures all
//! require it. The whole file compiles away elsewhere. The pre-spawn
//! guard rails of the same function are host-runnable and live in
//! `tests/launch_command.rs`.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use gmm_lib::core::{Core, GameCode, SessionInfo};
use gmm_lib::runtime::launch::{self, LaunchOptions};
use gmm_lib::runtime::session::LiveSession;
use gmm_lib::runtime::{SessionRuntime, SESSION_ENDED_EVENT, SESSION_STARTED_EVENT};
use gmm_loader::Loader;
use tauri::test::{mock_app, MockRuntime};
use tauri::{App, Listener};
use tempfile::TempDir;

/// The CBT hook 3dmloader installs is process-global and upstream
/// guards it with a named mutex, so two tests installing one at the same
/// time is a coin flip over which gets it. Tests that reach the Hook
/// path take this first. (Same reasoning as the registry tests, which
/// serialise on the global HKCU scan.)
static HOOK_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Timings for the tests: short enough that a failure path costs
/// seconds, long enough that a loaded CI runner still gets there.
fn fast_options() -> LaunchOptions {
    LaunchOptions {
        injection_timeout_secs: 5,
        inject_settle: Duration::from_millis(300),
        watch_poll_interval: Duration::from_millis(100),
    }
}

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

/// A game install directory holding `exe_name` (a copy of `source_exe`)
/// and a Model Importer `d3d11.dll`.
fn make_install_dir(tmp: &Path, exe_name: &str, source_exe: &Path) -> PathBuf {
    let install = tmp.join("game");
    std::fs::create_dir_all(&install).expect("install dir");
    std::fs::copy(source_exe, install.join(exe_name)).expect("copy game exe");
    std::fs::copy(build_artifact("noop_dll.dll"), install.join("d3d11.dll"))
        .expect("stage importer dll");
    install
}

/// A process that outlives the test: `victim.exe` creates a window and
/// self-destructs after its own timer.
fn long_lived_exe() -> PathBuf {
    build_artifact("victim.exe")
}

/// A process that exits almost immediately — stands in for a game that
/// dies during startup, which is what the injection wait has to survive.
fn instant_exit_exe() -> PathBuf {
    let p = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()))
        .join("System32/hostname.exe");
    assert!(p.exists(), "expected a stock hostname.exe at {p:?}");
    p
}

/// PIDs of every running process whose image name is `image`. Used to
/// prove `ChildGuard` actually killed what it spawned — the failing
/// launch never hands the PID back.
fn pids_named(image: &str) -> Vec<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut out = Vec::new();
    // SAFETY: the snapshot handle is checked against INVALID_HANDLE_VALUE
    // and closed on every path out. PROCESSENTRY32W is zeroed with its
    // dwSize set, as the API requires.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut entry) != 0 {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                if name.eq_ignore_ascii_case(image) {
                    out.push(entry.th32ProcessID);
                }
                if Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

/// Block until no process named `image` survives, or fail. A kill is
/// asynchronous from the killer's point of view, so a bare snapshot
/// right after the call is racy.
fn assert_no_process_named(image: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let pids = pids_named(image);
        if pids.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{image} still running after the launch failed: {pids:?} — ChildGuard leaked it",
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Block until `pid` is gone. Narrower than [`assert_no_process_named`]
/// — tests that share an executable name with a concurrent test can only
/// make claims about their own child.
fn assert_pid_gone(image: &str, pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while pids_named(image).contains(&pid) {
        assert!(
            Instant::now() < deadline,
            "{image} pid {pid} still running 20 s after it should have exited",
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn kill_pid(pid: u32) {
    let status = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .status()
        .expect("run taskkill");
    assert!(status.success(), "taskkill failed for pid {pid}");
}

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

async fn fresh_core(tmp: &Path) -> Core {
    let library_root = tmp.join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.display());
    Core::new(library_root, &db_url).await.expect("init core")
}

/// The happy path end to end: EFMI's `Inject` mode spawns first and
/// injects against the live PID, so the session claim, the live-session
/// install, `session-started`, and the watcher's teardown all run
/// exactly as they do for a real Endfield launch.
#[tokio::test(flavor = "multi_thread")]
async fn launches_a_session_then_the_watcher_tears_it_down_when_the_game_exits() {
    const EXE: &str = "Endfield-Win64-Shipping.exe";

    let tmp = TempDir::new().expect("tmp");
    let install = make_install_dir(tmp.path(), EXE, &long_lived_exe());
    let core = fresh_core(tmp.path()).await;
    core.set_game_install_path(GameCode::Efmi, &install)
        .await
        .expect("persist install path");

    let app = mock_app();
    let events = EventLog::attach(&app);
    let runtime = SessionRuntime::new();

    let outcome = launch::launch(
        app.handle(),
        &core,
        &runtime,
        GameCode::Efmi,
        &fast_options(),
    )
    .await
    .expect("launch against the fake Endfield install");

    assert_eq!(outcome.info.game, GameCode::Efmi);
    assert_ne!(outcome.info.pid, 0, "the session must carry a real pid");
    assert_eq!(
        core.session_info().await.expect("session info"),
        Some(outcome.info.clone()),
        "a successful launch persists the Game Session it returns",
    );
    assert!(
        runtime.has_session(),
        "a successful launch installs the live session",
    );
    assert_eq!(
        events.names(),
        vec![SESSION_STARTED_EVENT],
        "the banner event fires as soon as the session is claimed",
    );

    // The game exits — the only clean way a Game Session ends.
    kill_pid(outcome.info.pid);
    outcome.watcher.await.expect("watcher must not panic");

    assert_eq!(
        events.names(),
        vec![SESSION_STARTED_EVENT, SESSION_ENDED_EVENT],
        "session-ended follows session-started, and only once",
    );
    assert_eq!(
        core.session_info().await.expect("session info"),
        None,
        "the watcher clears the persisted Game Session",
    );
    assert!(
        !runtime.has_session(),
        "the watcher drops the live session (unhooking via RAII)",
    );
}

/// A game that dies during startup: the injection wait can never
/// succeed, so the launch must fail, reap the child, and leave no trace
/// of a Game Session behind.
#[tokio::test(flavor = "multi_thread")]
async fn an_injection_timeout_leaves_no_session_and_no_stray_process() {
    const EXE: &str = "StarRail.exe";

    let _hook_guard = HOOK_LOCK.lock().await;
    let tmp = TempDir::new().expect("tmp");
    let install = make_install_dir(tmp.path(), EXE, &instant_exit_exe());
    let core = fresh_core(tmp.path()).await;
    core.set_game_install_path(GameCode::Srmi, &install)
        .await
        .expect("persist install path");

    let app = mock_app();
    let events = EventLog::attach(&app);
    let runtime = SessionRuntime::new();

    let err = launch::launch(
        app.handle(),
        &core,
        &runtime,
        GameCode::Srmi,
        &fast_options(),
    )
    .await
    .expect_err("a game that exits before injection cannot start a session");

    assert!(
        err.contains("wait_for_injection"),
        "error should name the step that gave up, got: {err}",
    );
    assert_eq!(
        core.session_info().await.expect("session info"),
        None,
        "a failed injection must not persist a Game Session",
    );
    assert!(
        !runtime.has_session(),
        "a failed injection must not install a live session",
    );
    assert!(
        events.names().is_empty(),
        "a failed launch emits nothing, got: {:?}",
        events.names(),
    );
    assert_no_process_named(EXE);
}

/// The atomic claim is the real double-launch gate: the cheap pre-check
/// can be raced. When `start_session` loses that race the game is
/// already spawned, so `ChildGuard` has to kill it — and the session
/// that won must survive untouched.
#[tokio::test(flavor = "multi_thread")]
async fn losing_the_session_claim_race_kills_the_spawned_game() {
    const EXE: &str = "Endfield.exe";

    let tmp = TempDir::new().expect("tmp");
    let install = make_install_dir(tmp.path(), EXE, &long_lived_exe());
    let core = fresh_core(tmp.path()).await;
    core.set_game_install_path(GameCode::Efmi, &install)
        .await
        .expect("persist install path");

    let app = mock_app();
    let events = EventLog::attach(&app);
    let runtime = SessionRuntime::new();

    // Claim the session from another task while the launch under test is
    // between spawn and claim (it sleeps `inject_settle` in there).
    let winner = SessionInfo {
        game: GameCode::Gimi,
        pid: 4242,
        started_at: Utc::now(),
    };
    let racer_core = core.clone();
    let racer_info = winner.clone();
    let racer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        racer_core
            .start_session(&racer_info)
            .await
            .expect("racing session claim");
    });

    let opts = LaunchOptions {
        inject_settle: Duration::from_secs(2),
        ..fast_options()
    };
    let err = launch::launch(app.handle(), &core, &runtime, GameCode::Efmi, &opts)
        .await
        .expect_err("the launch that loses the claim race must fail");
    racer.await.expect("racer task");

    assert!(
        err.contains("start_session"),
        "error should name the failed claim, got: {err}",
    );
    assert_eq!(
        core.session_info().await.expect("session info"),
        Some(winner),
        "the session that won the race must be untouched",
    );
    assert!(
        !runtime.has_session(),
        "the loser must not install a live session over the winner",
    );
    assert!(
        events.names().is_empty(),
        "the loser emits nothing, got: {:?}",
        events.names(),
    );
    assert_no_process_named(EXE);
}

/// Build a live session by hand, the way a launch would have, so tests
/// can stage the state a dead watcher leaves behind.
fn stage_live_session(install: &Path, exe: &str, game: GameCode) -> (SessionInfo, LiveSession) {
    let child = std::process::Command::new(install.join(exe))
        .current_dir(install)
        .spawn()
        .expect("spawn stand-in game");
    let info = SessionInfo {
        game,
        pid: child.id(),
        started_at: Utc::now(),
    };
    let loader = Loader::load(&vendor_loader_dll()).expect("load 3dmloader");
    (
        info.clone(),
        LiveSession {
            info,
            child,
            _loader: loader,
        },
    )
}

/// If the exit watcher dies (a poisoned session lock panics it), the
/// live session is never taken and the DB row is never cleared. Once the
/// game itself exits, that state is stale in both places — and the next
/// launch has to recover from it rather than panicking on the
/// already-occupied slot.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_live_session_whose_game_exited_is_reclaimed_by_the_next_launch() {
    const EXE: &str = "ZenlessZoneZero.exe";

    let _hook_guard = HOOK_LOCK.lock().await;
    let tmp = TempDir::new().expect("tmp");
    let install = make_install_dir(tmp.path(), EXE, &long_lived_exe());
    let core = fresh_core(tmp.path()).await;
    core.set_game_install_path(GameCode::Zzmi, &install)
        .await
        .expect("persist install path");

    let (stale_info, live) = stage_live_session(&install, EXE, GameCode::Zzmi);
    core.start_session(&stale_info).await.expect("stale claim");
    let runtime = SessionRuntime::new();
    assert!(runtime.install(live).is_ok(), "slot starts empty");

    // The game exits; no watcher is around to notice. Wait for it to
    // really be gone — `taskkill` returns before the process does, and a
    // still-live child would (correctly) be refused rather than
    // reclaimed.
    kill_pid(stale_info.pid);
    assert_pid_gone(EXE, stale_info.pid);

    let app = mock_app();
    let events = EventLog::attach(&app);

    // ZZMI is Hook mode and this fixture can't verify injection, so the
    // launch is expected to fail *later* — what matters is that it gets
    // past the stale session at all instead of reporting "already
    // running" forever or panicking on the occupied slot.
    let err = launch::launch(
        app.handle(),
        &core,
        &runtime,
        GameCode::Zzmi,
        &LaunchOptions {
            injection_timeout_secs: 2,
            ..fast_options()
        },
    )
    .await
    .expect_err("the fixture cannot complete injection");

    assert!(
        !err.to_lowercase().contains("running"),
        "a stale session whose game exited must not block the next launch: {err}",
    );
    assert_eq!(
        core.session_info().await.expect("session info"),
        None,
        "reclaiming a stale session clears its persisted row",
    );
    assert!(
        !runtime.has_session(),
        "reclaiming a stale session empties the live slot",
    );
    assert!(
        events.names().is_empty(),
        "reclaiming is silent — no session started, so nothing to announce, got: {:?}",
        events.names(),
    );
    assert_no_process_named(EXE);
}

/// The other half of the dead-watcher story: the live session's game is
/// still running. Installing over it would drop a running game's handle
/// on the floor, so the launch must refuse — with an error, never a
/// panic out of a Tauri command.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_session_with_no_persisted_row_refuses_the_next_launch() {
    const EXE: &str = "BH3.exe";

    let tmp = TempDir::new().expect("tmp");
    let install = make_install_dir(tmp.path(), EXE, &long_lived_exe());
    let core = fresh_core(tmp.path()).await;
    core.set_game_install_path(GameCode::Himi, &install)
        .await
        .expect("persist install path");

    // Live session installed, no persisted row — what `clean_stale_session`
    // leaves behind if it runs while the game is still up.
    let (live_info, live) = stage_live_session(&install, EXE, GameCode::Himi);
    let runtime = SessionRuntime::new();
    assert!(runtime.install(live).is_ok(), "slot starts empty");

    let app = mock_app();
    let events = EventLog::attach(&app);

    let err = launch::launch(
        app.handle(),
        &core,
        &runtime,
        GameCode::Himi,
        &fast_options(),
    )
    .await
    .expect_err("a running game blocks the next launch");

    assert!(
        err.to_lowercase().contains("running"),
        "error should say a game is still running, got: {err}",
    );
    assert!(
        runtime.has_session(),
        "the running game's live session must survive the refusal",
    );
    assert_eq!(
        core.session_info().await.expect("session info"),
        None,
        "the refused launch must not leave a half-claimed session row",
    );
    assert!(
        events.names().is_empty(),
        "a refused launch emits nothing, got: {:?}",
        events.names(),
    );

    kill_pid(live_info.pid);
    drop(runtime.take());
    assert_no_process_named(EXE);
}

/// `clean_stale_session` can empty the live slot from under a running
/// watcher. The watcher has nothing left to poll at that point, so it
/// has to finish — otherwise every such cleanup leaks an immortal task
/// that wakes twice a second for the rest of the app's life.
#[tokio::test(flavor = "multi_thread")]
async fn the_watcher_finishes_when_the_live_session_is_cleared_from_under_it() {
    const EXE: &str = "Endfield-Win64-Shipping.exe";

    let tmp = TempDir::new().expect("tmp");
    let install = make_install_dir(tmp.path(), EXE, &long_lived_exe());
    let core = fresh_core(tmp.path()).await;
    core.set_game_install_path(GameCode::Efmi, &install)
        .await
        .expect("persist install path");

    let app = mock_app();
    let runtime = SessionRuntime::new();
    let outcome = launch::launch(
        app.handle(),
        &core,
        &runtime,
        GameCode::Efmi,
        &fast_options(),
    )
    .await
    .expect("launch against the fake Endfield install");

    // What `clean_stale_session` does: take the live session out while
    // the game — and the watcher — are still going.
    drop(runtime.take());

    tokio::time::timeout(Duration::from_secs(10), outcome.watcher)
        .await
        .expect("watcher must finish once the slot is empty, not spin forever")
        .expect("watcher must not panic");

    kill_pid(outcome.info.pid);
    assert_pid_gone(EXE, outcome.info.pid);
}
