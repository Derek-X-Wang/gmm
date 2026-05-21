//! Thin Tauri command shells over the Core API.
//!
//! Commands serialise errors to strings — the React side only needs a
//! flat message for now (slice 1c will introduce structured errors).

use std::path::PathBuf;

use serde::Deserialize;
use tauri::State;

use crate::core::{Core, GameCode, Mod};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptArgs {
    pub game: GameCode,
    pub source_path: PathBuf,
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
