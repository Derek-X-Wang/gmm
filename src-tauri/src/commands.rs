//! Thin Tauri command shells over the Core API.
//!
//! Commands serialise errors to strings — the React side only needs a
//! flat message for now (slice 1c will introduce structured errors).

use std::path::PathBuf;

use serde::Deserialize;
use tauri::State;

use crate::core::diagnostics;
use crate::core::{Core, GameCode, ImportZipOptions, Mod};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptArgs {
    pub game: GameCode,
    pub source_path: PathBuf,
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportZipArgs {
    pub game: GameCode,
    pub zip_path: PathBuf,
    pub name: String,
}

#[tauri::command]
pub async fn list_mods(core: State<'_, Core>, game: GameCode) -> Result<Vec<Mod>, String> {
    core.list_mods(game).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn adopt_folder(core: State<'_, Core>, args: AdoptArgs) -> Result<Mod, String> {
    core.adopt_folder(args.game, &args.source_path, &args.name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn import_zip(core: State<'_, Core>, args: ImportZipArgs) -> Result<Mod, String> {
    core.import_zip(
        args.game,
        &args.zip_path,
        &args.name,
        ImportZipOptions::default(),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_mod_enabled(
    core: State<'_, Core>,
    id: String,
    enabled: bool,
    game: GameCode,
) -> Result<(), String> {
    let install = core
        .game_install_path(game)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Set the game install path in Settings before enabling mods.".to_string())?;
    let mods_dir = install.join("Mods");
    std::fs::create_dir_all(&mods_dir)
        .map_err(|e| format!("create {}: {e}", mods_dir.display()))?;
    core.set_enabled(&id, enabled, &mods_dir)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_game_install_path(
    core: State<'_, Core>,
    game: GameCode,
) -> Result<Option<PathBuf>, String> {
    core.game_install_path(game)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_game_install_path(
    core: State<'_, Core>,
    game: GameCode,
    path: PathBuf,
) -> Result<(), String> {
    core.set_game_install_path(game, &path)
        .await
        .map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendError {
    pub message: String,
    #[serde(default)]
    pub stack: Option<String>,
    #[serde(default)]
    pub route: Option<String>,
}

/// Tauri command — frontend error boundary calls this when a render
/// throws. Goes through the same JSON-lines logger as the backend.
#[tauri::command]
pub fn log_frontend_error(error: FrontendError) {
    diagnostics::record_frontend_error(
        &error.message,
        error.stack.as_deref(),
        error.route.as_deref(),
    );
}

/// Tauri command — user-initiated bundle export. Writes a zip to
/// `dest_path`. The zip contains the last 7 days of logs plus a redacted
/// `settings.json` snapshot.
#[tauri::command]
pub async fn export_diagnostics_bundle(
    core: State<'_, Core>,
    log_dir: PathBuf,
    dest_path: PathBuf,
) -> Result<(), String> {
    let snapshot = core.settings_snapshot().await.map_err(|e| e.to_string())?;
    // The build is sync I/O; offload so we don't block the Tauri event
    // loop while the zip is being written.
    let log_dir_owned = log_dir.clone();
    let dest_path_owned = dest_path.clone();
    tokio::task::spawn_blocking(move || {
        diagnostics::build_bundle(
            &log_dir_owned,
            &snapshot,
            &dest_path_owned,
            diagnostics::DEFAULT_BUNDLE_LOG_DAYS,
        )
    })
    .await
    .map_err(|e| format!("bundle task join error: {e}"))?
    .map_err(|e| e.to_string())
}

/// Tauri command — surfaces the directory we write logs into so the
/// frontend can show "Open log folder" / save dialog defaults.
#[tauri::command]
pub fn diagnostics_log_dir() -> Result<PathBuf, String> {
    crate::log_dir().map_err(|e| e.to_string())
}
