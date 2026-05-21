//! Thin Tauri command shells over the Core API.
//!
//! Commands serialise errors to strings — the React side only needs a
//! flat message for now (slice 1c will introduce structured errors).

use std::path::PathBuf;

use serde::Deserialize;
use tauri::State;

use std::collections::HashMap;

use serde::Serialize;

use crate::core::conflicts::ConflictReport;
use crate::core::detect;
use crate::core::diagnostics;
use crate::core::importer::{self, InstallReport, LatestRelease, DEFAULT_LOADER_EXE};
use crate::core::network::{ProxyConfig, ProxyConfigPublic};
use crate::core::reconcile::ReconcileResult;
use crate::core::updates::UpdateStatus;
use crate::core::variants::Variant;
use crate::core::{Core, GameCode, ImportZipOptions, Mod, MoveReport};

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

/// Effective + default library paths, returned to the Settings UI so
/// it can render the global root + each per-game override row.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPaths {
    /// Default root passed to [`crate::core::Core::new`] — what the user
    /// would see if every override is cleared.
    pub default_root: PathBuf,
    /// Explicit user override (empty when the user has never changed it).
    pub root_override: Option<PathBuf>,
    /// Resolved root after applying any override.
    pub effective_root: PathBuf,
    /// Per-game override map (keys = lowercased game codes); `None`
    /// means "no override, fall back to global root".
    pub per_game_overrides: HashMap<String, Option<PathBuf>>,
    /// Effective per-game library path (always present).
    pub per_game_effective: HashMap<String, PathBuf>,
}

const ALL_GAMES: &[GameCode] = &[
    GameCode::Gimi,
    GameCode::Srmi,
    GameCode::Zzmi,
    GameCode::Wwmi,
    GameCode::Himi,
    GameCode::Efmi,
];

#[tauri::command]
pub async fn get_library_paths(core: State<'_, Core>) -> Result<LibraryPaths, String> {
    let default_root = core.default_library_root().to_path_buf();
    let root_override = core
        .library_root_override()
        .await
        .map_err(|e| e.to_string())?;
    let effective_root = core
        .resolved_library_root()
        .await
        .map_err(|e| e.to_string())?;

    let mut per_game_overrides = HashMap::new();
    let mut per_game_effective = HashMap::new();
    for game in ALL_GAMES {
        let key = game.as_str().to_string();
        let over = core
            .library_root_override_for_game(*game)
            .await
            .map_err(|e| e.to_string())?;
        let eff = core
            .resolved_library_root_for(*game)
            .await
            .map_err(|e| e.to_string())?;
        per_game_overrides.insert(key.clone(), over);
        per_game_effective.insert(key, eff);
    }

    Ok(LibraryPaths {
        default_root,
        root_override,
        effective_root,
        per_game_overrides,
        per_game_effective,
    })
}

#[tauri::command]
pub async fn set_library_root(
    core: State<'_, Core>,
    path: Option<PathBuf>,
) -> Result<MoveReport, String> {
    core.set_library_root(path.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_library_path_for_game(
    core: State<'_, Core>,
    game: GameCode,
    path: Option<PathBuf>,
) -> Result<MoveReport, String> {
    core.set_library_path_for_game(game, path.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModVariants {
    pub variants: Vec<Variant>,
    pub active_variant_id: Option<String>,
}

#[tauri::command]
pub async fn list_variants(core: State<'_, Core>, mod_id: String) -> Result<ModVariants, String> {
    let variants = core
        .list_variants(&mod_id)
        .await
        .map_err(|e| e.to_string())?;
    let active_variant_id = core
        .active_variant_id(&mod_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(ModVariants {
        variants,
        active_variant_id,
    })
}

#[tauri::command]
pub async fn set_active_variant(
    core: State<'_, Core>,
    mod_id: String,
    variant_id: String,
    game: GameCode,
) -> Result<(), String> {
    let install = core
        .game_install_path(game)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Set the game install path before switching variants.".to_string())?;
    let mods_dir = install.join("Mods");
    std::fs::create_dir_all(&mods_dir)
        .map_err(|e| format!("create {}: {e}", mods_dir.display()))?;
    core.set_active_variant(&mod_id, &variant_id, &mods_dir)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn detect_conflicts(
    core: State<'_, Core>,
    game: GameCode,
) -> Result<ConflictReport, String> {
    core.detect_conflicts(game).await.map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameBananaImportArgs {
    pub game: GameCode,
    pub url_or_id: String,
}

#[tauri::command]
pub async fn import_gamebanana(
    core: State<'_, Core>,
    args: GameBananaImportArgs,
) -> Result<Mod, String> {
    core.import_gamebanana(args.game, &args.url_or_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn check_importer_update(
    core: State<'_, Core>,
    game: GameCode,
) -> Result<UpdateStatus, String> {
    let (repo, filter) = importer_repo_for(game)?;
    core.check_importer_update(game, repo, filter)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn check_loader_update(core: State<'_, Core>) -> Result<UpdateStatus, String> {
    core.check_loader_update().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_importer_pinned(
    core: State<'_, Core>,
    game: GameCode,
    version: Option<String>,
) -> Result<(), String> {
    core.set_importer_pinned(game, version.as_deref())
        .await
        .map_err(|e| e.to_string())
}

/// Resolve the GitHub repo + asset filter for a Game's importer.
/// Slice 3 wires GIMI only; other games are added in their port issues.
fn importer_repo_for(game: GameCode) -> Result<(&'static str, &'static str), String> {
    match game {
        GameCode::Gimi => Ok(("SpectrumQT/GIMI-Package", "GIMI")),
        _ => Err("Importer auto-install for this game is not wired in this slice.".to_string()),
    }
}

#[tauri::command]
pub async fn fetch_latest_importer_release(
    core: State<'_, Core>,
    game: GameCode,
) -> Result<Option<LatestRelease>, String> {
    let (repo, filter) = importer_repo_for(game)?;
    let client = core.http_client().await.map_err(|e| e.to_string())?;
    importer::fetch_latest_release(&client, repo, filter, None)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn install_importer(
    core: State<'_, Core>,
    game: GameCode,
) -> Result<InstallReport, String> {
    let install = core
        .game_install_path(game)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Set the game install path in Settings before installing.".to_string())?;
    let (repo, filter) = importer_repo_for(game)?;

    let client = core.http_client().await.map_err(|e| e.to_string())?;
    let release = importer::fetch_latest_release(&client, repo, filter, None)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no release returned for importer repo".to_string())?;

    let data = crate::data_dir().map_err(|e| e.to_string())?;
    let backups_root = data.join("backups").join(game.as_str());
    let downloads_dir = data.join("downloads").join(game.as_str());
    let zip_path = downloads_dir.join(&release.asset_name);
    importer::download_to(&client, &release.asset_url, &zip_path)
        .await
        .map_err(|e| e.to_string())?;

    let report = tokio::task::spawn_blocking(move || {
        importer::install_from_local_zip(&zip_path, &install, &backups_root, DEFAULT_LOADER_EXE)
    })
    .await
    .map_err(|e| format!("install task join error: {e}"))?
    .map_err(|e| e.to_string())?;

    // Record the installed tag so the update-check pass can compare
    // against it next launch. Best-effort; never fails the install.
    let _ = core
        .set_importer_installed(game, &release.tag_name)
        .await
        .map_err(|e| e.to_string());

    Ok(report)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyArgs {
    pub url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[tauri::command]
pub async fn get_proxy_config(core: State<'_, Core>) -> Result<ProxyConfigPublic, String> {
    core.proxy_config_public().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_proxy_config(
    core: State<'_, Core>,
    args: ProxyArgs,
) -> Result<ProxyConfigPublic, String> {
    let cfg = ProxyConfig {
        url: args.url.filter(|s| !s.is_empty()),
        username: args.username.filter(|s| !s.is_empty()),
        password: args.password.filter(|s| !s.is_empty()),
    };
    core.set_proxy_config(&cfg)
        .await
        .map_err(|e| e.to_string())?;
    core.proxy_config_public().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn test_proxy_connection(core: State<'_, Core>) -> Result<(), String> {
    core.test_proxy_connection()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rollback_importer(
    core: State<'_, Core>,
    game: GameCode,
) -> Result<Option<PathBuf>, String> {
    let install = core
        .game_install_path(game)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Set the game install path in Settings before rolling back.".to_string())?;
    let data = crate::data_dir().map_err(|e| e.to_string())?;
    let backups_root = data.join("backups").join(game.as_str());

    if !backups_root.exists() {
        return Ok(None);
    }
    // Pick the lexicographically-newest backup (we name them with an
    // ISO-8601 timestamp).
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&backups_root)
        .map_err(|e| format!("read backups dir: {e}"))?
        .filter_map(|r| r.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    entries.sort();
    let Some(latest) = entries.pop() else {
        return Ok(None);
    };

    let latest_for_blocking = latest.clone();
    let install_for_blocking = install.clone();
    tokio::task::spawn_blocking(move || {
        importer::rollback_to(&latest_for_blocking, &install_for_blocking)
    })
    .await
    .map_err(|e| format!("rollback task join error: {e}"))?
    .map_err(|e| e.to_string())?;

    Ok(Some(latest))
}

/// Tauri command — reconcile junctions for a game in place. Used by
/// the UI on demand; the startup pass runs the same logic.
#[tauri::command]
pub async fn reconcile_junctions(
    core: State<'_, Core>,
    game: GameCode,
) -> Result<ReconcileResult, String> {
    let install = core
        .game_install_path(game)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Set the game install path in Settings first.".to_string())?;
    let mods_dir = install.join("Mods");
    core.reconcile_junctions(game, &mods_dir)
        .await
        .map_err(|e| e.to_string())
}

/// Tauri command — drop and recreate every junction for `game` against
/// the current Library. Use after the user relocates the Library
/// directory.
#[tauri::command]
pub async fn rebuild_junctions(
    core: State<'_, Core>,
    game: GameCode,
) -> Result<ReconcileResult, String> {
    let install = core
        .game_install_path(game)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Set the game install path in Settings first.".to_string())?;
    let mods_dir = install.join("Mods");
    core.rebuild_junctions(game, &mods_dir)
        .await
        .map_err(|e| e.to_string())
}

/// Tauri command — auto-detect a game's install path. On success the
/// detected path is persisted into the `games` table and returned.
/// Returns `Ok(None)` when no candidate matched, so the frontend can
/// surface the "Couldn't find Genshin automatically" copy and fall
/// back to the manual picker.
///
/// Only GIMI (Genshin) is wired in this slice; other Game codes return
/// `Ok(None)` until their port issues land (see #16–#20).
#[tauri::command]
pub async fn detect_game_install_path(
    core: State<'_, Core>,
    game: GameCode,
) -> Result<Option<PathBuf>, String> {
    let detected = match game {
        GameCode::Gimi => tokio::task::spawn_blocking(detect::genshin::detect)
            .await
            .map_err(|e| format!("detect task join error: {e}"))?,
        _ => None,
    };
    if let Some(path) = detected.as_ref() {
        core.set_game_install_path(game, path)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(detected)
}
