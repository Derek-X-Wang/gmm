pub mod command_error;
pub mod commands;
#[deny(clippy::disallowed_methods)]
pub mod core;
pub mod runtime;

#[cfg(test)]
extern crate self as gmm_lib;

use std::path::PathBuf;

use crate::core::diagnostics;
use crate::core::instance_lock::{self, InstanceLockError};
use crate::core::reconcile::StartupReconcileState;
use crate::core::Core;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let data_dir = resolve_data_dir().expect("resolve GMM data directory");
    let logs_dir = data_dir.join("logs");

    // Best-effort: install the JSON-lines subscriber + prune anything
    // older than the retention window. Failures here must not stop the
    // app from starting — diagnostics are nice-to-have, not blocking.
    let _log_guard = diagnostics::install_subscriber(&logs_dir).ok();
    if let Err(e) = diagnostics::prune_old_logs(&logs_dir, diagnostics::DEFAULT_LOG_RETENTION_DAYS)
    {
        tracing::warn!(error = %e, "prune_old_logs failed at startup");
    }

    // Single-instance gate (issue #58). Must come before `build_core`:
    // opening the pool runs migrations, and two cold instances racing
    // there is one of the pairings we refuse to support. See
    // `core::instance_lock` for why the lock is scoped to the data
    // directory rather than to the executable.
    //
    // Held for the rest of `run()` — the binding must not be `_`, or the
    // lock would drop immediately and gate nothing.
    let _instance_lock = match instance_lock::acquire(&data_dir) {
        Ok(lock) => Some(lock),
        Err(e @ InstanceLockError::AlreadyRunning { .. }) => {
            tracing::warn!(
                target: "gmm::instance",
                error = %e,
                "refusing to start: another GMM instance owns this data directory",
            );
            report_already_running(&e.to_string());
            return;
        }
        // Fail open. If the lock file itself is unopenable — an
        // antivirus holding it mid-scan is the realistic cause, and
        // `core::av` exists because that happens to GMM users — the
        // safety net is unavailable, but bricking every launch is a
        // worse outcome than the race it was guarding against.
        Err(e) => {
            tracing::warn!(
                target: "gmm::instance",
                error = %e,
                "could not take the single-instance lock; starting anyway without it",
            );
            None
        }
    };

    let core = match build_core(&data_dir) {
        Ok(core) => core,
        Err(error) => {
            let message = format!("GMM could not start safely: {error}");
            tracing::error!(
                target: "gmm::library",
                error = %error,
                "refusing to start after core initialization failure",
            );
            report_startup_failure(&message);
            return;
        }
    };

    // Best-effort startup reconcile across every game whose install path is
    // set. It is never fatal, but per-game failures are retained for React so
    // "could not determine" cannot become an apparently healthy screen.
    //
    // Pre-pass: clear any orphan active_session row left by a crashed
    // GMM. If after cleanup a session is STILL marked active (meaning
    // the PID happens to be alive), skip reconcile — yanking junctions
    // out from under a running game corrupts it.
    let startup_reconcile_state = StartupReconcileState::default();
    {
        let core_for_pass = core.clone();
        let state_for_pass = startup_reconcile_state.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build reconcile runtime");
            rt.block_on(async move {
                if let Err(e) = core_for_pass.clean_stale_session().await {
                    tracing::warn!(error = %e, "startup clean_stale_session errored");
                }
                match core_for_pass.session_info().await {
                    Ok(Some(info)) => {
                        tracing::warn!(
                            game = %info.game.as_str(),
                            pid = info.pid,
                            "skipping startup reconcile — a game session is active",
                        );
                        state_for_pass.finish(Vec::new());
                        return;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "startup session_info errored");
                    }
                }
                match core_for_pass
                    .reconcile_all_set_games_at_startup(&state_for_pass)
                    .await
                {
                    Ok(()) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "startup reconcile pass errored");
                        state_for_pass.finish(Vec::new());
                    }
                }
            });
        });
    }

    // Refresh GMM's curated `recommended-importers.json` once per launch
    // (ADR 0005 / #96). Its own thread, and nothing waits on it: the
    // cached manifest is already in force, a refresh only applies when it
    // lands, and "GitHub is slow today" must never become "GMM won't
    // start". A failure has no user-visible consequence — the cache is
    // still in force and still correct — so it is logged and dropped.
    {
        let core_for_manifest = core.clone();
        let manifest_url_override =
            crate::core::recommended_importers::loopback_manifest_url_override();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build manifest refresh runtime");
            rt.block_on(async move {
                diagnostics::record_manifest_refresh_started();
                let refresh = match manifest_url_override {
                    Some(url) => {
                        core_for_manifest
                            .refresh_recommended_importers_from_loopback_override(&url)
                            .await
                    }
                    None => core_for_manifest.refresh_recommended_importers().await,
                };
                // Expected transport failures, including the held-open smoke
                // request timing out, are `Ok(Refreshed::Unreachable(..))`.
                // That makes this terminal `finished` event reachable for
                // every network outcome; installer-smoke.ps1 relies on it. An
                // `Err` is an internal refresh failure and deliberately leaves
                // the smoke to time out while waiting for the terminal event.
                match refresh {
                    Ok(outcome) => tracing::info!(
                        target: "gmm::recommendations",
                        outcome = ?outcome,
                        "recommended-importers refresh finished",
                    ),
                    Err(e) => tracing::warn!(
                        target: "gmm::recommendations",
                        error = %e,
                        "recommended-importers refresh could not run",
                    ),
                }
            });
        });
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(core)
        .manage(startup_reconcile_state)
        .manage(crate::runtime::SessionRuntime::new())
        .invoke_handler(tauri::generate_handler![
            commands::list_mods,
            commands::retry_reinstall_recovery,
            commands::retire_interrupted_enabled_transition,
            commands::get_importer_evacuation_recovery,
            commands::retry_importer_evacuation_recovery,
            commands::retire_interrupted_importer_evacuation,
            commands::adopt_folder,
            commands::import_zip,
            commands::set_mod_enabled,
            commands::get_game_install_path,
            commands::set_game_install_path,
            commands::log_frontend_error,
            commands::export_diagnostics_bundle,
            commands::diagnostics_log_dir,
            commands::detect_game_install_path,
            commands::reconcile_junctions,
            commands::get_startup_reconcile_status,
            commands::rebuild_junctions,
            commands::get_library_paths,
            commands::audit_library,
            commands::resolve_duplicate_mods,
            commands::reveal_unreferenced_library_dir,
            commands::recover_unreferenced_library_dir,
            commands::delete_unreferenced_library_dir,
            commands::set_library_root,
            commands::set_library_path_for_game,
            commands::fetch_latest_importer_release,
            commands::install_importer,
            commands::rollback_importer,
            commands::get_proxy_config,
            commands::set_proxy_config,
            commands::test_proxy_connection,
            commands::list_variants,
            commands::set_active_variant,
            commands::detect_conflicts,
            commands::import_gamebanana,
            commands::check_importer_update,
            commands::check_loader_update,
            commands::set_importer_pinned,
            commands::importer_origin_status,
            commands::set_importer_origin_override,
            commands::accept_importer_origin_proposal,
            commands::dismiss_importer_origin,
            commands::restore_importer_origin,
            commands::importer_recommendations_enabled,
            commands::set_importer_recommendations_enabled,
            commands::list_mod_updates,
            commands::check_mod_updates_now,
            commands::set_mod_update_check_enabled,
            commands::set_mod_updates_globally_enabled,
            commands::mod_updates_globally_enabled,
            commands::apply_mod_update,
            commands::launch_game,
            commands::current_session,
            commands::clean_stale_session,
            commands::interrupted_session_launches,
            commands::retire_interrupted_session_launch,
            commands::av_guidance,
            commands::list_supported_games,
            commands::is_onboarding_complete,
            commands::mark_onboarding_complete,
            commands::reset_onboarding,
            commands::detect_all_games,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Tell the user why the second instance vanished.
///
/// Release builds set `windows_subsystem = "windows"`, so there is no
/// console to print to: a bare `return` would look exactly like the
/// shortcut doing nothing. A native message box is the only channel
/// available before a Tauri window exists.
///
/// Focusing the already-running window instead would be nicer, but that
/// needs an IPC channel to the live instance. Tracked as a follow-up.
#[cfg(windows)]
fn report_already_running(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND,
    };

    let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    let caption: Vec<u16> = "GMM".encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: both buffers are NUL-terminated and outlive the call; a
    // null owner HWND is documented as "no owner window".
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
        );
    }
}

/// Non-Windows hosts only ever run GMM from a terminal (dev builds), so
/// stderr reaches a human.
#[cfg(not(windows))]
fn report_already_running(message: &str) {
    eprintln!("GMM: {message}");
}

/// Surface a fatal pre-window startup error. A release build has no console,
/// and silently returning here would leave the user retrying operations in a
/// state GMM deliberately refused to open.
#[cfg(windows)]
fn report_startup_failure(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND,
    };

    let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    let caption: Vec<u16> = "GMM".encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: both buffers are NUL-terminated and outlive the call; a
    // null owner HWND is documented as "no owner window".
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

#[cfg(not(windows))]
fn report_startup_failure(message: &str) {
    eprintln!("GMM: {message}");
}

/// Resolve `%AppData%/GMM` (or the platform equivalent), creating it if
/// needed. Pulled out of [`build_core`] so the log dir setup can run
/// before Core init.
fn resolve_data_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let data_dir = dirs::data_dir()
        .ok_or("could not resolve OS data directory")?
        .join("GMM");
    std::fs::create_dir_all(&data_dir)?;
    Ok(data_dir)
}

/// Public entry point for the `diagnostics_log_dir` Tauri command.
pub fn log_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(resolve_data_dir()?.join("logs"))
}

/// Where importer install backups + downloaded ZIPs land. Public so
/// the importer Tauri commands can compose paths without re-resolving.
pub fn data_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    resolve_data_dir()
}

/// Build the GMM Core against the user's app-data directory. Synchronous
/// wrapper around the async constructor so it fits into Tauri's startup.
fn build_core(data_dir: &std::path::Path) -> Result<Core, Box<dyn std::error::Error>> {
    let library_root = data_dir.join("library");
    let db_path = data_dir.join("gmm.db");
    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    #[allow(
        clippy::disallowed_methods,
        reason = "Library mutation policy exemption: this synchronous startup adapter delegates only to Core construction, whose empty-root exemption is declared at the filesystem statement"
    )]
    let core = rt.block_on(Core::new(library_root, &db_url))?;
    // Leak the runtime so it stays alive — Core's sqlx pool needs it for
    // future async calls invoked from Tauri commands.
    Box::leak(Box::new(rt));
    Ok(core)
}
