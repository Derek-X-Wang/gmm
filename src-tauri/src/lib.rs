pub mod commands;
pub mod core;

use std::path::PathBuf;

use crate::core::diagnostics;
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

    let core = build_core(&data_dir).expect("initialise GMM core");

    // Best-effort startup reconcile across every game whose install
    // path is set. Logs per-game via tracing (NEW-LOG); never fatal.
    {
        let core_for_pass = core.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build reconcile runtime");
            rt.block_on(async move {
                if let Err(e) = core_for_pass.reconcile_all_set_games().await {
                    tracing::warn!(error = %e, "startup reconcile pass errored");
                }
            });
        });
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(core)
        .invoke_handler(tauri::generate_handler![
            commands::list_mods,
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
            commands::rebuild_junctions,
            commands::get_library_paths,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
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
    let core = rt.block_on(Core::new(library_root, &db_url))?;
    // Leak the runtime so it stays alive — Core's sqlx pool needs it for
    // future async calls invoked from Tauri commands.
    Box::leak(Box::new(rt));
    Ok(core)
}
