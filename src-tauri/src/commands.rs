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
use crate::core::mod_updates::ModUpdateRow;
use crate::core::network::{ProxyConfig, ProxyConfigPublic};
use crate::core::reconcile::ReconcileResult;
use crate::core::updates::UpdateStatus;
use crate::core::variants::Variant;
use crate::core::{Core, GameCode, ImportZipOptions, Mod, MoveReport, SessionInfo};
use crate::runtime::{SessionRuntime, SESSION_ENDED_EVENT, SESSION_STARTED_EVENT};
use tauri::{AppHandle, Emitter};

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

/// Error string returned when a user tries to enable a mod before
/// the game install path has been set. Extracted as a constant so
/// tests can assert against it without duplicating the literal.
pub const NO_INSTALL_PATH_FOR_ENABLE_MSG: &str =
    "Set the game install path in Settings before enabling mods.";

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
        .ok_or_else(|| NO_INSTALL_PATH_FOR_ENABLE_MSG.to_string())?;
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

#[tauri::command]
pub async fn list_mod_updates(
    core: State<'_, Core>,
    game: GameCode,
) -> Result<Vec<ModUpdateRow>, String> {
    core.list_mod_updates(game).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn check_mod_updates_now(
    core: State<'_, Core>,
    game: GameCode,
) -> Result<Vec<ModUpdateRow>, String> {
    core.check_mod_updates_now(game)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_mod_update_check_enabled(
    core: State<'_, Core>,
    mod_id: String,
    enabled: bool,
) -> Result<(), String> {
    core.set_mod_update_check_enabled(&mod_id, enabled)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_mod_updates_globally_enabled(
    core: State<'_, Core>,
    enabled: bool,
) -> Result<(), String> {
    core.set_mod_updates_globally_enabled(enabled)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn mod_updates_globally_enabled(core: State<'_, Core>) -> Result<bool, String> {
    core.mod_updates_globally_enabled()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn apply_mod_update(core: State<'_, Core>, mod_id: String) -> Result<(), String> {
    core.reinstall_gamebanana_mod(&mod_id)
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

// ---- slice 4b (#12) — game session commands ----

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
/// directory. Currently GIMI-specific (the per-game ports — #16–#20 —
/// fill in the other five).
fn resolve_game_exe(game: GameCode, install: &std::path::Path) -> Result<PathBuf, String> {
    match game {
        GameCode::Gimi => {
            for candidate in ["GenshinImpact.exe", "YuanShen.exe"] {
                let p = install.join(candidate);
                if p.exists() {
                    return Ok(p);
                }
            }
            Err(format!(
                "GenshinImpact.exe / YuanShen.exe not found under {}.",
                install.display()
            ))
        }
        _ => Err(format!(
            "Launching {:?} is not wired yet — see the per-game port issues.",
            game
        )),
    }
}

#[tauri::command]
pub async fn current_session(core: State<'_, Core>) -> Result<Option<SessionInfo>, String> {
    core.session_info().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn clean_stale_session(core: State<'_, Core>) -> Result<Option<SessionInfo>, String> {
    core.clean_stale_session().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn launch_game(
    app: AppHandle,
    core: State<'_, Core>,
    runtime: State<'_, SessionRuntime>,
    game: GameCode,
) -> Result<SessionInfo, String> {
    use gmm_loader::Loader;

    // Reject if a session is already active.
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

    // Load + hook BEFORE spawning the game so the CBT hook is in place
    // when the game's window appears.
    let loader = Loader::load(&loader_dll).map_err(|e| format!("load loader: {e}"))?;
    let hook = loader
        .hook(&dll_to_inject)
        .map_err(|e| format!("install hook: {e}"))?;
    // The HookSession borrows the Loader lifetime; transmute to 'static
    // is sound because HookSession owns an Arc<LoadedDll> internally.
    // SAFETY: the Arc keeps the DLL mapped for the session's lifetime,
    // and we never drop the Loader before the HookSession.
    let hook_static: gmm_loader::HookSession<'static> = unsafe { std::mem::transmute(hook) };

    let child = std::process::Command::new(&game_exe)
        .current_dir(&install)
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", game_exe.display()))?;

    let info = SessionInfo {
        game,
        pid: child.id(),
        started_at: chrono::Utc::now(),
    };
    core.start_session(&info).await.map_err(|e| e.to_string())?;

    runtime.install(crate::runtime::session::LiveSession {
        info: info.clone(),
        child,
        _hook: hook_static,
        _loader: loader,
    });

    // Emit to the frontend so the banner appears immediately.
    let _ = app.emit(SESSION_STARTED_EVENT, &info);

    // Spawn the exit watcher. It polls every 500 ms; on child exit it
    // drops the LiveSession (which unhooks via RAII), clears the DB
    // row, and emits SESSION_ENDED_EVENT.
    let app_for_watch = app.clone();
    let runtime_for_watch = runtime.inner_clone();
    let core_for_watch = (*core).clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            match runtime_for_watch.try_wait_child() {
                Ok(Some(_status)) => break,
                Ok(None) => continue,
                Err(_) => break, // process gone / handle invalid
            }
        }
        // Drop the LiveSession → unhook + close child handle.
        let _ = runtime_for_watch.take();
        // Best-effort: clear the persisted row.
        if let Err(e) = core_for_watch.end_session().await {
            tracing::warn!(error = %e, "end_session failed in watcher");
        }
        let _ = app_for_watch.emit(SESSION_ENDED_EVENT, ());
    });

    Ok(info)
}
