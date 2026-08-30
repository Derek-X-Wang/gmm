//! Thin Tauri command shells over the Core API.
//!
//! Every command returns [`CommandResult`], even when its current body is
//! infallible. That keeps the structured failure boundary closed by default.

use std::path::PathBuf;

use serde::Deserialize;
use tauri::State;

use std::collections::HashMap;

use serde::Serialize;

use crate::core::av;
use crate::core::conflicts::ConflictReport;
use crate::core::diagnostics;
use crate::core::importer::{InstallReport, LatestRelease};
use crate::core::importer_origin::{ImporterOrigin, OriginStatus};
use crate::core::mod_updates::ModUpdateRow;
use crate::core::network::{ProxyConfig, ProxyConfigPublic};
use crate::core::reconcile::{ReconcileResult, StartupReconcileState, StartupReconcileStatus};
use crate::core::updates::{LoaderVersionStatus, UpdateStatus};
use crate::core::variants::Variant;
use crate::core::{
    Core, DeletedLibraryDir, DuplicateResolution, Error, GameCode, ImportZipOptions,
    ImporterEvacuationRecovery, InterruptedSessionLaunch, LibraryAuditReport, Mod, MoveReport,
    ReinstallRecoveryOutcome, ReviewedDuplicateMod, SessionInfo,
};
use crate::runtime::launch::{self, LaunchOptions};
use crate::runtime::SessionRuntime;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use crate::command_error::{CommandError, CommandResult};

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
pub async fn list_mods(core: State<'_, Core>, game: GameCode) -> CommandResult<Vec<Mod>> {
    core.list_mods(game).await.map_err(CommandError::from)
}

#[tauri::command]
pub async fn retry_reinstall_recovery(
    core: State<'_, Core>,
    mod_id: String,
) -> CommandResult<ReinstallRecoveryOutcome> {
    core.retry_reinstall_recovery(&mod_id)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn retire_interrupted_enabled_transition(
    core: State<'_, Core>,
    mod_id: String,
) -> CommandResult<()> {
    core.retire_interrupted_enabled_transition(&mod_id)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn get_importer_evacuation_recovery(
    core: State<'_, Core>,
    game: GameCode,
) -> CommandResult<Option<ImporterEvacuationRecovery>> {
    core.importer_evacuation_recovery(game)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn retry_importer_evacuation_recovery(
    core: State<'_, Core>,
    game: GameCode,
) -> CommandResult<()> {
    core.retry_importer_evacuation_recovery(game)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn retire_interrupted_importer_evacuation(
    core: State<'_, Core>,
    game: GameCode,
) -> CommandResult<()> {
    core.retire_interrupted_importer_evacuation(game)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn adopt_folder(core: State<'_, Core>, args: AdoptArgs) -> CommandResult<Mod> {
    core.adopt_folder(args.game, &args.source_path, &args.name)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn import_zip(core: State<'_, Core>, args: ImportZipArgs) -> CommandResult<Mod> {
    core.import_zip(
        args.game,
        &args.zip_path,
        &args.name,
        ImportZipOptions::default(),
    )
    .await
    .map_err(CommandError::from)
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
) -> CommandResult<()> {
    let install = core
        .game_install_path(game)
        .await
        .map_err(CommandError::from)?
        .ok_or_else(|| CommandError::other(NO_INSTALL_PATH_FOR_ENABLE_MSG))?;
    let mods_dir = install.join("Mods");
    std::fs::create_dir_all(&mods_dir)
        .map_err(|error| CommandError::other(format!("create {}: {error}", mods_dir.display())))?;
    core.set_enabled(&id, enabled, &mods_dir)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn get_game_install_path(
    core: State<'_, Core>,
    game: GameCode,
) -> CommandResult<Option<PathBuf>> {
    core.game_install_path(game)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn set_game_install_path(
    core: State<'_, Core>,
    game: GameCode,
    path: PathBuf,
) -> CommandResult<()> {
    core.set_game_install_path(game, &path)
        .await
        .map_err(CommandError::from)
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
pub fn log_frontend_error(error: FrontendError) -> CommandResult<()> {
    diagnostics::record_frontend_error(
        &error.message,
        error.stack.as_deref(),
        error.route.as_deref(),
    );
    Ok(())
}

/// Tauri command — user-initiated bundle export. Writes a zip to
/// `dest_path`. The zip contains the last 7 days of logs plus a redacted
/// `settings.json` snapshot.
#[tauri::command]
pub async fn export_diagnostics_bundle(
    core: State<'_, Core>,
    log_dir: PathBuf,
    dest_path: PathBuf,
) -> CommandResult<()> {
    let snapshot = core.settings_snapshot().await.map_err(CommandError::from)?;
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
    .map_err(|error| CommandError::other(format!("bundle task join error: {error}")))?
    .map_err(CommandError::from)
}

/// Tauri command — surfaces the directory we write logs into so the
/// frontend can show "Open log folder" / save dialog defaults.
#[tauri::command]
pub fn diagnostics_log_dir() -> CommandResult<PathBuf> {
    crate::log_dir().map_err(|error| CommandError::other(error.to_string()))
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
    /// Persisted roots that cannot safely be used until the user changes them.
    /// This is report data, not a command failure: Settings must remain open so
    /// the invalid configuration can be repaired in-app.
    pub overlaps: Vec<LibraryRootOverlap>,
    /// Mods whose recorded Library path still resolves inside the importer
    /// backup tree, including after the configured root itself is repaired.
    pub mod_overlaps: Vec<LibraryModPathOverlap>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRootOverlap {
    /// `None` means the global root; otherwise this is one explicit per-game
    /// override. Inherited per-game paths are covered by the global report.
    pub game: Option<GameCode>,
    pub path: PathBuf,
    pub backups: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryModPathOverlap {
    pub mod_id: String,
    pub mod_name: String,
    pub game: GameCode,
    pub path: PathBuf,
    pub backups: PathBuf,
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
pub async fn get_library_paths(core: State<'_, Core>) -> CommandResult<LibraryPaths> {
    library_paths_for_core(&core).await
}

#[doc(hidden)]
pub async fn library_paths_for_core(core: &Core) -> CommandResult<LibraryPaths> {
    let default_root = core.default_library_root().to_path_buf();
    let root_override = core
        .library_root_override()
        .await
        .map_err(CommandError::from)?;
    let effective_root = root_override
        .clone()
        .unwrap_or_else(|| default_root.clone());
    let mut overlaps = Vec::new();
    match core.resolved_library_root().await {
        Ok(_) => {}
        Err(Error::LibraryRootOverlapsBackups { path, backups }) => {
            overlaps.push(LibraryRootOverlap {
                game: None,
                path,
                backups,
            });
        }
        Err(error) => return Err(CommandError::from(error)),
    }

    let mut per_game_overrides = HashMap::new();
    let mut per_game_effective = HashMap::new();
    for game in ALL_GAMES {
        let key = game.as_str().to_string();
        let over = core
            .library_root_override_for_game(*game)
            .await
            .map_err(CommandError::from)?;
        if over.is_some() {
            match core.resolved_library_root_for(*game).await {
                Ok(_) => {}
                Err(Error::LibraryRootOverlapsBackups { path, backups }) => {
                    overlaps.push(LibraryRootOverlap {
                        game: Some(*game),
                        path,
                        backups,
                    });
                }
                Err(error) => return Err(CommandError::from(error)),
            }
        }
        let eff = over
            .clone()
            .unwrap_or_else(|| effective_root.join(game.as_str()));
        per_game_overrides.insert(key.clone(), over);
        per_game_effective.insert(key, eff);
    }

    let mod_overlaps = core
        .mod_library_path_overlaps()
        .await
        .map_err(CommandError::from)?
        .into_iter()
        .map(|overlap| LibraryModPathOverlap {
            mod_id: overlap.mod_id,
            mod_name: overlap.mod_name,
            game: overlap.game,
            path: overlap.path,
            backups: overlap.backups,
        })
        .collect();

    Ok(LibraryPaths {
        default_root,
        root_override,
        effective_root,
        per_game_overrides,
        per_game_effective,
        overlaps,
        mod_overlaps,
    })
}

/// Read-only Library consistency report consumed by the Settings warning.
#[tauri::command]
pub async fn audit_library(
    core: State<'_, Core>,
    game: GameCode,
) -> CommandResult<LibraryAuditReport> {
    core.audit_library(game).await.map_err(CommandError::from)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveDuplicateModsArgs {
    pub keeper_id: String,
    pub reviewed_mods: Vec<ReviewedDuplicateMod>,
}

/// Discard only the duplicate Mod records the user reviewed and rejected.
#[tauri::command]
pub async fn resolve_duplicate_mods(
    core: State<'_, Core>,
    args: ResolveDuplicateModsArgs,
) -> CommandResult<DuplicateResolution> {
    core.resolve_duplicate_mods(&args.keeper_id, &args.reviewed_mods)
        .await
        .map_err(CommandError::from)
}

/// Arguments for recovering an unreferenced Library directory.
///
/// The name is the user's, exactly as in [`AdoptArgs`]: nothing on disk
/// records what the interrupted import was called, so GMM asks rather than
/// inventing a display name and presenting it as a recovered fact.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverLibraryDirArgs {
    pub game: GameCode,
    pub path: PathBuf,
    pub name: String,
}

/// Open the user's file manager on an unreferenced Library folder so they
/// can see what is inside before choosing to recover or delete it.
#[tauri::command]
pub async fn reveal_unreferenced_library_dir(
    core: State<'_, Core>,
    app: AppHandle,
    game: GameCode,
    path: PathBuf,
) -> CommandResult<()> {
    let path = core
        .unreferenced_library_dir_for_reveal(game, &path)
        .await
        .map_err(CommandError::from)?;
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| CommandError::other(format!("could not open {}: {e}", path.display())))
}

/// Adopt an unreferenced Library folder as a Mod without copying it.
#[tauri::command]
pub async fn recover_unreferenced_library_dir(
    core: State<'_, Core>,
    args: RecoverLibraryDirArgs,
) -> CommandResult<Mod> {
    core.recover_unreferenced_library_dir(args.game, &args.path, &args.name)
        .await
        .map_err(CommandError::from)
}

/// Permanently delete one unreferenced Library folder the user confirmed.
///
/// One path, never a list: the confirmation the user answered named a
/// single folder and its size, and a bulk variant of this command would be
/// a bulk confirmation waiting to happen.
#[tauri::command]
pub async fn delete_unreferenced_library_dir(
    core: State<'_, Core>,
    game: GameCode,
    path: PathBuf,
) -> CommandResult<DeletedLibraryDir> {
    core.delete_unreferenced_library_dir(game, &path)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn set_library_root(
    core: State<'_, Core>,
    path: Option<PathBuf>,
) -> CommandResult<MoveReport> {
    core.set_library_root(path.as_deref())
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn set_library_path_for_game(
    core: State<'_, Core>,
    game: GameCode,
    path: Option<PathBuf>,
) -> CommandResult<MoveReport> {
    core.set_library_path_for_game(game, path.as_deref())
        .await
        .map_err(CommandError::from)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModVariants {
    pub variants: Vec<Variant>,
    pub active_variant_id: Option<String>,
}

#[tauri::command]
pub async fn list_variants(core: State<'_, Core>, mod_id: String) -> CommandResult<ModVariants> {
    let variants = core
        .list_variants(&mod_id)
        .await
        .map_err(CommandError::from)?;
    let active_variant_id = core
        .active_variant_id(&mod_id)
        .await
        .map_err(CommandError::from)?;
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
) -> CommandResult<()> {
    let install = core
        .game_install_path(game)
        .await
        .map_err(CommandError::from)?
        .ok_or_else(|| {
            CommandError::other("Set the game install path before switching variants.")
        })?;
    let mods_dir = install.join("Mods");
    std::fs::create_dir_all(&mods_dir)
        .map_err(|error| CommandError::other(format!("create {}: {error}", mods_dir.display())))?;
    core.set_active_variant(&mod_id, &variant_id, &mods_dir)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn detect_conflicts(
    core: State<'_, Core>,
    game: GameCode,
) -> CommandResult<ConflictReport> {
    core.detect_conflicts(game)
        .await
        .map_err(CommandError::from)
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
) -> CommandResult<Mod> {
    core.import_gamebanana(args.game, &args.url_or_id)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn check_importer_update(
    core: State<'_, Core>,
    game: GameCode,
) -> CommandResult<UpdateStatus> {
    core.check_importer_update_for(game)
        .await
        .map_err(CommandError::from)
}

/// Informational Loader version report. Returns the version GMM
/// ships and the latest upstream release; a failed check surfaces in
/// `checkError` rather than masquerading as "up to date" (#78).
#[tauri::command]
pub async fn check_loader_update(core: State<'_, Core>) -> CommandResult<LoaderVersionStatus> {
    core.check_loader_update().await.map_err(CommandError::from)
}

#[tauri::command]
pub async fn set_importer_pinned(
    core: State<'_, Core>,
    game: GameCode,
    version: Option<String>,
) -> CommandResult<()> {
    core.set_importer_pinned(game, version.as_deref())
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn list_mod_updates(
    core: State<'_, Core>,
    game: GameCode,
) -> CommandResult<Vec<ModUpdateRow>> {
    core.list_mod_updates(game)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn check_mod_updates_now(
    core: State<'_, Core>,
    game: GameCode,
) -> CommandResult<Vec<ModUpdateRow>> {
    core.check_mod_updates_now(game)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn set_mod_update_check_enabled(
    core: State<'_, Core>,
    mod_id: String,
    enabled: bool,
) -> CommandResult<()> {
    core.set_mod_update_check_enabled(&mod_id, enabled)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn set_mod_updates_globally_enabled(
    core: State<'_, Core>,
    enabled: bool,
) -> CommandResult<()> {
    core.set_mod_updates_globally_enabled(enabled)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn mod_updates_globally_enabled(core: State<'_, Core>) -> CommandResult<bool> {
    core.mod_updates_globally_enabled()
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn apply_mod_update(core: State<'_, Core>, mod_id: String) -> CommandResult<()> {
    core.reinstall_gamebanana_mod(&mod_id)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn fetch_latest_importer_release(
    core: State<'_, Core>,
    game: GameCode,
) -> CommandResult<Option<LatestRelease>> {
    core.latest_importer_release(game)
        .await
        .map_err(CommandError::from)
}

/// Download and install the Game's Model Importer from its resolved
/// Importer Origin (ADR 0005).
///
/// A thin wrapper on purpose. The install used to be written out here,
/// which put the only interesting failure — a successful file install
/// whose bookkeeping did not persist — somewhere no test could reach
/// (#122). It now lives on `Core`, where `install_importer_with_endpoints`
/// drives the same code against a stand-in upstream.
#[tauri::command]
pub async fn install_importer(
    core: State<'_, Core>,
    game: GameCode,
) -> CommandResult<InstallReport> {
    refuse_during_session(&core).await?;
    core.install_importer(game)
        .await
        .map_err(CommandError::from)
}

// ---- Importer Origin surface (ADR 0005 / #109) ----

/// A GitHub Importer Origin as the override control sends it.
///
/// Three fields rather than a serialised [`ImporterOrigin`], because
/// this is what a user typed: the shape the UI collects, validated on
/// the way in by [`ImporterOrigin::from_user_input`]. Accepting the
/// domain type's own JSON here would make the frontend responsible for
/// GMM's internal representation and would let an unvalidated origin in
/// through the back door.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImporterOriginInput {
    pub owner: String,
    pub repo: String,
    pub asset_pattern: String,
}

impl ImporterOriginInput {
    fn build(&self) -> CommandResult<ImporterOrigin> {
        ImporterOrigin::from_user_input(&self.owner, &self.repo, &self.asset_pattern)
            .map_err(CommandError::other)
    }
}

/// Everything one game's Importer Origin surface needs, in one read.
#[tauri::command]
pub async fn importer_origin_status(
    core: State<'_, Core>,
    game: GameCode,
) -> CommandResult<OriginStatus> {
    core.importer_origin_status(game)
        .await
        .map_err(CommandError::from)
}

/// Set or clear the user's per-game Importer Origin override (layer 1).
///
/// `None` clears it, returning the game to following GMM's
/// recommendation and then the compiled-in default.
#[tauri::command]
pub async fn set_importer_origin_override(
    core: State<'_, Core>,
    game: GameCode,
    origin: Option<ImporterOriginInput>,
) -> CommandResult<()> {
    let origin = origin.map(|o| o.build()).transpose()?;
    core.set_importer_origin_override(game, origin.as_ref())
        .await
        .map_err(CommandError::from)
}

/// Accept the Importer Origin change GMM is offering: install from the
/// proposed origin.
///
/// Refused during a Game Session like every other write into a game
/// directory — this one rewrites the Model Importer wholesale.
#[tauri::command]
pub async fn accept_importer_origin_proposal(
    core: State<'_, Core>,
    game: GameCode,
) -> CommandResult<InstallReport> {
    refuse_during_session(&core).await?;
    core.accept_importer_origin_proposal(game)
        .await
        .map_err(CommandError::from)
}

/// Decline an Importer Origin: remember it and stop proposing it.
///
/// Takes the origin explicitly rather than "whatever is proposed right
/// now", so a click always dismisses the proposal the user was actually
/// reading — the manifest can change under an open window.
#[tauri::command]
pub async fn dismiss_importer_origin(
    core: State<'_, Core>,
    game: GameCode,
    origin: ImporterOriginInput,
) -> CommandResult<()> {
    core.dismiss_importer_origin(game, &origin.build()?)
        .await
        .map_err(CommandError::from)
}

/// Undo a dismissal from the affected game's own surface.
#[tauri::command]
pub async fn restore_importer_origin(
    core: State<'_, Core>,
    game: GameCode,
    origin: ImporterOriginInput,
) -> CommandResult<()> {
    core.restore_importer_origin(game, &origin.build()?)
        .await
        .map_err(CommandError::from)
}

/// The global recommendations switch. On for a user who has never
/// touched it.
#[tauri::command]
pub async fn importer_recommendations_enabled(core: State<'_, Core>) -> CommandResult<bool> {
    core.importer_recommendations_enabled()
        .await
        .map_err(CommandError::from)
}

/// Switch GMM's curated recommendations on or off.
///
/// Switching **on** kicks off a refresh rather than waiting for the next
/// launch: the cached manifest may be months old, or absent entirely for
/// a user who has never had the layer enabled, and a switch that appears
/// to do nothing until a restart is a switch users conclude is broken.
/// It is spawned and unawaited for the same reason the startup refresh
/// is (#96) — nothing waits on the network, and the cache is already in
/// force.
#[tauri::command]
pub async fn set_importer_recommendations_enabled(
    core: State<'_, Core>,
    enabled: bool,
) -> CommandResult<()> {
    core.set_importer_recommendations_enabled(enabled)
        .await
        .map_err(CommandError::from)?;
    if enabled {
        let core = Core::clone(&core);
        tauri::async_runtime::spawn(async move {
            match core.refresh_recommended_importers().await {
                Ok(outcome) => tracing::info!(
                    target: "gmm::recommendations",
                    outcome = ?outcome,
                    "refresh after switching recommendations on",
                ),
                Err(e) => tracing::warn!(
                    target: "gmm::recommendations",
                    error = %e,
                    "refresh after switching recommendations on could not run",
                ),
            }
        });
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyArgs {
    pub url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[tauri::command]
pub async fn get_proxy_config(core: State<'_, Core>) -> CommandResult<ProxyConfigPublic> {
    core.proxy_config_public().await.map_err(CommandError::from)
}

#[tauri::command]
pub async fn set_proxy_config(
    core: State<'_, Core>,
    args: ProxyArgs,
) -> CommandResult<ProxyConfigPublic> {
    let cfg = ProxyConfig {
        url: args.url.filter(|s| !s.is_empty()),
        username: args.username.filter(|s| !s.is_empty()),
        password: args.password.filter(|s| !s.is_empty()),
    };
    core.set_proxy_config(&cfg)
        .await
        .map_err(CommandError::from)?;
    core.proxy_config_public().await.map_err(CommandError::from)
}

#[tauri::command]
pub async fn test_proxy_connection(core: State<'_, Core>) -> CommandResult<()> {
    core.test_proxy_connection()
        .await
        .map_err(CommandError::from)
}

/// Roll the Game's Model Importer back to its most recent backup.
///
/// Thin wrapper: the rollback restores GMM's record of what is
/// installed as well as the files, which is DB work and belongs on
/// `Core` (#126).
#[tauri::command]
pub async fn rollback_importer(
    core: State<'_, Core>,
    game: GameCode,
) -> CommandResult<Option<PathBuf>> {
    refuse_during_session(&core).await?;
    core.rollback_importer(game)
        .await
        .map_err(CommandError::from)
}

/// Tauri command — reconcile junctions for a game in place. Used by
/// the UI on demand; the startup pass runs the same logic.
#[tauri::command]
pub async fn reconcile_junctions(
    core: State<'_, Core>,
    game: GameCode,
) -> CommandResult<ReconcileResult> {
    refuse_during_session(&core).await?;
    let install = core
        .game_install_path(game)
        .await
        .map_err(CommandError::from)?
        .ok_or_else(|| CommandError::other("Set the game install path in Settings first."))?;
    let mods_dir = install.join("Mods");
    core.reconcile_junctions(game, &mods_dir)
        .await
        .map_err(CommandError::from)
}

/// Snapshot the best-effort startup reconcile pass. React polls until
/// `finished` so a fast backend pass cannot race the first render.
#[tauri::command]
pub fn get_startup_reconcile_status(
    state: State<'_, StartupReconcileState>,
) -> CommandResult<StartupReconcileStatus> {
    Ok(state.snapshot())
}

/// Tauri command — drop and recreate every junction for `game` against
/// the current Library. Use after the user relocates the Library
/// directory.
#[tauri::command]
pub async fn rebuild_junctions(
    core: State<'_, Core>,
    game: GameCode,
) -> CommandResult<ReconcileResult> {
    refuse_during_session(&core).await?;
    let install = core
        .game_install_path(game)
        .await
        .map_err(CommandError::from)?
        .ok_or_else(|| CommandError::other("Set the game install path in Settings first."))?;
    let mods_dir = install.join("Mods");
    core.rebuild_junctions(game, &mods_dir)
        .await
        .map_err(CommandError::from)
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
) -> CommandResult<Option<PathBuf>> {
    // Dispatch via the GameProfile registry so each per-game port
    // adds itself to `core::games::GAME_PROFILES` and nothing else.
    let detected = match game.profile().detect {
        Some(f) => tokio::task::spawn_blocking(f)
            .await
            .map_err(|error| CommandError::other(format!("detect task join error: {error}")))?,
        None => None,
    };
    if let Some(path) = detected.as_ref() {
        core.set_game_install_path(game, path)
            .await
            .map_err(CommandError::from)?;
    }
    Ok(detected)
}

// ---- slice 4b (#12) — game session commands ----

/// Helper: bail with a session-related error string if a session is
/// currently active. Pair with mutating Tauri commands (importer
/// install/rollback, reconcile/rebuild junctions, library moves) so
/// the caller never gets a partial mutation while the game is
/// running.
async fn refuse_during_session(core: &State<'_, Core>) -> CommandResult<()> {
    if let Some(info) = core.session_info().await.map_err(CommandError::from)? {
        return Err(CommandError::other(format!(
            "{} is running (session active since {}). Close the game before changing this.",
            info.game.as_str(),
            info.started_at,
        )));
    }
    Ok(())
}

#[tauri::command]
pub async fn current_session(core: State<'_, Core>) -> CommandResult<Option<SessionInfo>> {
    core.session_info().await.map_err(CommandError::from)
}

#[tauri::command]
pub async fn clean_stale_session(core: State<'_, Core>) -> CommandResult<Option<SessionInfo>> {
    core.clean_stale_session().await.map_err(CommandError::from)
}

#[tauri::command]
pub async fn interrupted_session_launches(
    core: State<'_, Core>,
) -> CommandResult<Vec<InterruptedSessionLaunch>> {
    core.interrupted_session_launches()
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn retire_interrupted_session_launch(
    core: State<'_, Core>,
    id: String,
) -> CommandResult<()> {
    core.retire_interrupted_session_launch(&id)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn launch_game(
    app: AppHandle,
    core: State<'_, Core>,
    runtime: State<'_, SessionRuntime>,
    game: GameCode,
) -> CommandResult<SessionInfo> {
    // Thin shell: everything worth testing lives in
    // `runtime::launch::launch`, which is generic over the Tauri runtime
    // so `tests/launch_command*.rs` can drive it against a MockRuntime
    // handle. The detached watcher handle is deliberately dropped — in
    // production nothing joins it.
    launch::launch(&app, &core, &runtime, game, &LaunchOptions::default())
        .await
        .map(|outcome| outcome.info)
}

/// Tauri command — return the structured AV / SmartScreen guidance the
/// in-app launch error component and the onboarding wizard both
/// render. Single source of truth; see
/// `docs/antivirus-and-smartscreen.md` and the `av` module for the
/// drift-protection contract.
#[tauri::command]
pub fn av_guidance() -> CommandResult<av::AvGuidance> {
    Ok(av::guidance())
}

/// Light per-game summary the React tab strip uses to render only the
/// games whose backend wiring (detect + importer repo + exe
/// candidates) is complete. Slices #16–#20 each light up the next
/// game's row in `core::games::GAME_PROFILES`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameSummary {
    pub code: GameCode,
    pub display_name: &'static str,
}

#[tauri::command]
pub fn list_supported_games() -> CommandResult<Vec<GameSummary>> {
    Ok(GameCode::ported()
        .map(|p| GameSummary {
            code: p.code,
            display_name: p.display_name,
        })
        .collect())
}

// ---- slice 16-b (#24) — first-run onboarding wizard ----

/// State the React App router uses to decide between rendering the
/// onboarding wizard vs. the main app. `skipped == true` keeps the
/// "Finish setup" banner alive on Settings until the user resumes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingStatus {
    /// `true` once the user finished the wizard OR pressed Skip
    /// setup. Either way the wizard does not auto-open on the next
    /// launch.
    pub complete: bool,
    /// `true` iff the user skipped instead of completing.
    pub skipped: bool,
}

#[tauri::command]
pub async fn is_onboarding_complete(core: State<'_, Core>) -> CommandResult<OnboardingStatus> {
    // Doubles as the IPC readiness marker (#54). This is the App
    // router's own query — first invoke of every session, on both the
    // wizard and main-app branches — and reaching this line means the
    // WebView booted, the IPC channel came up, the command was
    // registered, and the ACL allowed it. `installer-smoke.ps1` waits
    // for the resulting log line; without it, a build where every
    // command is denied still passes the smoke.
    diagnostics::record_ipc_ready();
    core.onboarding_status().await.map_err(CommandError::from)
}

#[tauri::command]
pub async fn mark_onboarding_complete(core: State<'_, Core>, skipped: bool) -> CommandResult<()> {
    core.mark_onboarding_complete(skipped)
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn reset_onboarding(core: State<'_, Core>) -> CommandResult<()> {
    core.reset_onboarding().await.map_err(CommandError::from)
}

/// Per-game install-path detection result returned by
/// [`detect_all_games`]. The wizard's Step 2 renders one row per
/// supported game; rows with `detected_path == None` fall through to
/// the manual browse/skip controls.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameDetection {
    pub code: GameCode,
    pub display_name: &'static str,
    /// `None` when no candidate passed the per-game validator, or
    /// when the game has not been ported yet (its `GameProfile`
    /// `detect` field is `None`).
    pub detected_path: Option<PathBuf>,
}

#[tauri::command]
pub async fn detect_all_games(core: State<'_, Core>) -> CommandResult<Vec<GameDetection>> {
    // Fan out one blocking task per ported game. Detection is IO-bound
    // (registry probes + path stats) so blocking pool is the right
    // venue. Order in the output preserves `GAME_PROFILES` order so
    // the wizard rows render Genshin first, etc.
    let profiles: Vec<_> = GameCode::ported().collect();
    let mut tasks = Vec::with_capacity(profiles.len());
    for p in &profiles {
        let detect = p.detect.expect("ported profile has detect fn");
        tasks.push(tokio::task::spawn_blocking(detect));
    }

    let mut out = Vec::with_capacity(profiles.len());
    for (p, t) in profiles.into_iter().zip(tasks) {
        let detected = t.await.map_err(|error| {
            CommandError::other(format!(
                "detect task join error for {}: {error}",
                p.code.as_str()
            ))
        })?;
        if let Some(path) = detected.as_ref() {
            // Persist eagerly so subsequent wizard steps see the
            // path without an extra round-trip.
            core.set_game_install_path(p.code, path)
                .await
                .map_err(CommandError::from)?;
        }
        out.push(GameDetection {
            code: p.code,
            display_name: p.display_name,
            detected_path: detected,
        });
    }
    Ok(out)
}
