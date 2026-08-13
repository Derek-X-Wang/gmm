//! Game Session launch orchestration.
//!
//! This is the body of the `launch_game` Tauri command, lifted out of the
//! `#[tauri::command]` shell so it can be driven from tests. The shell in
//! `commands.rs` does nothing but supply [`LaunchOptions::default`] and
//! unwrap the outcome — everything that can go wrong (the double-launch
//! refusal, the ChildGuard cleanup, the ordering of the session lock
//! against the unsafe filesystem work, the event emission, the exit
//! watcher) happens here.
//!
//! Why the indirection: `tauri::test::mock_builder()` ships no ACL, so
//! routing a command through the mock runtime fails with
//! `"<cmd> not allowed. Plugin not found"` (issue #26). Testing the
//! orchestration therefore means calling it as a plain function. It stays
//! generic over `R: Runtime` so tests can hand it a `MockRuntime` handle
//! and still exercise the real `Emitter` implementation.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Runtime};

use gmm_loader::Loader;

use crate::core::av;
use crate::core::games::InjectMode;
use crate::core::{Core, GameCode, SessionInfo};
use crate::runtime::session::LiveSession;
use crate::runtime::{SessionRuntime, SESSION_ENDED_EVENT, SESSION_STARTED_EVENT};

/// Timings the launch flow would otherwise hard-code. Production always
/// uses [`LaunchOptions::default`]; tests shrink the waits so the
/// injection-timeout path costs seconds instead of a minute.
#[derive(Debug, Clone)]
pub struct LaunchOptions {
    /// How long `WaitForInjection` blocks before giving up, in seconds.
    pub injection_timeout_secs: i32,
    /// Pause between spawning and injecting on the [`InjectMode::Inject`]
    /// path — injecting into a process that has not finished creating
    /// its main thread is fragile. 1 s is what XXMI uses in practice.
    pub inject_settle: Duration,
    /// Exit-watcher poll interval.
    pub watch_poll_interval: Duration,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            injection_timeout_secs: 60,
            inject_settle: Duration::from_secs(1),
            watch_poll_interval: Duration::from_millis(500),
        }
    }
}

/// What a successful launch hands back: the durable session record plus
/// the handle of the exit watcher that owns teardown. The command shell
/// drops the handle (the task is detached in production); tests await it
/// to observe teardown deterministically instead of sleeping.
#[derive(Debug)]
pub struct LaunchOutcome {
    pub info: SessionInfo,
    pub watcher: tokio::task::JoinHandle<()>,
}

/// RAII wrapper that kills + reaps the wrapped child on drop. Used
/// during [`launch`] between `Command::spawn` and the moment the
/// `Child` is moved into `LiveSession`; on every error-return path
/// the guard's drop runs so we never leak a started game.
struct ChildGuard {
    child: Option<std::process::Child>,
}

impl ChildGuard {
    fn new(child: std::process::Child) -> Self {
        Self { child: Some(child) }
    }

    fn pid(&self) -> u32 {
        self.child.as_ref().map(|c| c.id()).unwrap_or(0)
    }

    fn into_inner(mut self) -> std::process::Child {
        self.child.take().expect("ChildGuard already consumed")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Locate the bundled / vendored `3dmloader.dll`. Resolution order:
/// 1. `GMM_LOADER_DLL` env var (override for smoke tests + dev)
/// 2. `<exe-dir>/3dmloader.dll` (production bundle layout)
/// 3. `<repo-root>/vendor/3dmloader/3dmloader.dll` (dev convenience)
fn locate_loader_dll() -> Result<PathBuf, String> {
    if let Ok(env_path) = std::env::var("GMM_LOADER_DLL") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return Ok(p);
        }
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if let Some(dir) = exe.parent() {
        let candidate = dir.join("3dmloader.dll");
        if candidate.exists() {
            return Ok(candidate);
        }
        // Dev fallback: target/<profile>/gmm[.exe] → ../../../vendor/...
        let mut walker = dir.to_path_buf();
        for _ in 0..6 {
            let candidate = walker.join("vendor/3dmloader/3dmloader.dll");
            if candidate.exists() {
                return Ok(candidate);
            }
            if !walker.pop() {
                break;
            }
        }
    }
    Err("Couldn't find 3dmloader.dll. Set GMM_LOADER_DLL or reinstall.".to_string())
}

/// Pick the game executable to launch given a Game and its install
/// directory. Looks at `GameProfile::executable_candidates` so each
/// per-game port (slices #16–#20) just adds its exe list to the
/// registry in `core::games`.
fn resolve_game_exe(game: GameCode, install: &Path) -> Result<PathBuf, String> {
    let profile = game.profile();
    if profile.executable_candidates.is_empty() {
        return Err(format!(
            "Launching {} is not wired yet — see the per-game port issues.",
            profile.display_name,
        ));
    }
    for candidate in profile.executable_candidates {
        let p = install.join(candidate);
        if p.exists() {
            return Ok(p);
        }
    }
    Err(format!(
        "{} not found under {}.",
        profile.executable_candidates.join(" / "),
        install.display(),
    ))
}

/// Spawn `game`, get the Model Importer injected, claim the Game Session,
/// and start the exit watcher.
///
/// Every failure is routed through [`av::wrap_launch_error`], which
/// prefixes the wire message with `AV-PATTERN: ` when the error text
/// matches a known antivirus / SmartScreen signature (slice NEW-AV /
/// #13); the React `LaunchGameButton` then swaps to the structured
/// `<AvGuidance>` component instead of dumping a raw Win32 error onto
/// the user.
pub async fn launch<R: Runtime>(
    app: &AppHandle<R>,
    core: &Core,
    runtime: &SessionRuntime,
    game: GameCode,
    opts: &LaunchOptions,
) -> Result<LaunchOutcome, String> {
    let result: Result<LaunchOutcome, String> = async {
        // Cheap pre-check; the atomic INSERT in start_session is the
        // real gate against double-launch races.
        if let Some(existing) = core.session_info().await.map_err(|e| e.to_string())? {
            return Err(format!(
                "{} is already running (since {}).",
                existing.game.as_str(),
                existing.started_at
            ));
        }

        let install = core
            .game_install_path(game)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Set the game install path in Settings before launching.".to_string())?;

        let game_exe = resolve_game_exe(game, &install)?;
        let dll_to_inject = install.join("d3d11.dll");
        if !dll_to_inject.exists() {
            return Err(format!(
                "Model Importer DLL not found at {}. Install the importer for this game first.",
                dll_to_inject.display()
            ));
        }
        let loader_dll = locate_loader_dll()?;

        // Loading the 3dmloader DLL is the most common AV-quarantine
        // target (Defender frequently flags the vendored DLL on first
        // run); errors from this step land in the AV classifier via
        // the outer `wrap_launch_error`.
        let loader = Loader::load(&loader_dll).map_err(|e| format!("load loader: {e}"))?;

        let inject_mode = game.profile().inject_mode;
        let child_guard = match inject_mode {
            InjectMode::Hook => {
                // CBT hook MUST be installed before spawning so it
                // catches the window-creation event the game fires on
                // startup.
                let hook = loader
                    .hook(&dll_to_inject)
                    .map_err(|e| format!("install hook: {e}"))?;

                let child = std::process::Command::new(&game_exe)
                    .current_dir(&install)
                    .spawn()
                    .map_err(|e| format!("spawn {}: {e}", game_exe.display()))?;
                let child_guard = ChildGuard::new(child);

                // Block until the importer DLL lands in a process
                // whose image name matches the game exe, then DROP
                // the hook session — holding the global CBT hook for
                // the whole game session would inject the DLL into
                // every unrelated process that creates a window.
                let target_process = game_exe
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "GenshinImpact.exe".to_string());
                hook.wait_for_injection(&target_process, opts.injection_timeout_secs)
                    .map_err(|e| format!("wait_for_injection: {e}"))?;
                // Explicitly drop so the unhook runs immediately
                // rather than at end-of-scope. clippy::drop_non_drop
                // fires on the non-Windows stub HookSession (no Drop
                // impl); silence it — on Windows this is the
                // load-bearing line that takes the CBT hook back
                // down.
                #[allow(clippy::drop_non_drop)]
                drop(hook);

                child_guard
            }
            InjectMode::Inject => {
                // EFMI path (slice 10 / #20): spawn first, then call
                // `Loader::inject` against the live PID. Upstream
                // XXMI uses `custom_launch_inject_mode = 'Inject'`
                // here; the CBT-hook path doesn't fire for EFMI's
                // launch sequence.
                let child = std::process::Command::new(&game_exe)
                    .current_dir(&install)
                    .spawn()
                    .map_err(|e| format!("spawn {}: {e}", game_exe.display()))?;
                let child_guard = ChildGuard::new(child);

                // Give the process a beat to start its main module
                // before injecting; injecting into a process that has
                // not finished creating its main thread is fragile.
                tokio::time::sleep(opts.inject_settle).await;

                let pid = child_guard.pid();
                loader
                    .inject(pid, &dll_to_inject)
                    .map_err(|e| format!("inject into pid {pid}: {e}"))?;

                child_guard
            }
        };

        let info = SessionInfo {
            game,
            pid: child_guard.pid(),
            started_at: chrono::Utc::now(),
        };

        // Atomic claim: plain INSERT, no OR REPLACE. If anyone raced
        // past the pre-check above, the singleton CHECK gives us a
        // unique-constraint error and ChildGuard's drop kills our
        // spawned game.
        core.start_session(&info)
            .await
            .map_err(|e| format!("start_session: {e}"))?;

        let child = child_guard.into_inner();

        runtime.install(LiveSession {
            info: info.clone(),
            child,
            _loader: loader,
        });

        // Emit to the frontend so the banner appears immediately.
        let _ = app.emit(SESSION_STARTED_EVENT, &info);

        // Spawn the exit watcher. It polls until the child exits, then
        // drops the LiveSession (which unhooks via RAII), clears the DB
        // row, and emits SESSION_ENDED_EVENT.
        let watcher = spawn_exit_watcher(app.clone(), core.clone(), runtime.inner_clone(), opts);

        Ok(LaunchOutcome { info, watcher })
    }
    .await;

    result.map_err(av::wrap_launch_error)
}

/// The exit watcher: the only place a healthy Game Session ends.
fn spawn_exit_watcher<R: Runtime>(
    app: AppHandle<R>,
    core: Core,
    runtime: SessionRuntime,
    opts: &LaunchOptions,
) -> tokio::task::JoinHandle<()> {
    let poll = opts.watch_poll_interval;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(poll).await;
            match runtime.try_wait_child() {
                Ok(Some(_status)) => break,
                Ok(None) => continue,
                Err(_) => break, // process gone / handle invalid
            }
        }
        // Drop the LiveSession → unhook + close child handle.
        let _ = runtime.take();
        // Best-effort: clear the persisted row.
        if let Err(e) = core.end_session().await {
            tracing::warn!(error = %e, "end_session failed in watcher");
        }
        let _ = app.emit(SESSION_ENDED_EVENT, ());
    })
}
