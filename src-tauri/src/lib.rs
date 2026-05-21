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
