//! Pure-Rust core of GMM.
//!
//! Tauri commands are thin shells over the functions in this module; the
//! integration tests in `src-tauri/tests/` exercise this module directly so
//! they can run on macOS without spinning up the Tauri runtime.

pub mod av;
pub mod conflicts;
pub mod crash_points;
pub mod detect;
pub mod diagnostics;
pub mod error;
pub mod gamebanana;
pub mod games;
pub mod importer;
pub mod importer_origin;
pub mod instance_lock;
pub mod junction;
pub mod library_audit;
mod library_identity;
mod library_mutation;
mod library_ownership;
pub mod library_recovery;
pub mod mod_updates;
pub mod mods;
pub mod network;
pub mod recommended_importers;
pub mod reconcile;
pub mod session;
pub mod settings;
pub mod updates;
pub mod variants;
pub mod volume;
pub mod zip_import;

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use chrono::Utc;
use sqlx::{sqlite::SqliteConnectOptions, Executor, Row, Sqlite, SqlitePool};
use ulid::Ulid;

pub use error::{Error, Result};
pub use games::GameCode;
pub use library_audit::{
    DuplicateModGroup, DuplicateModRecord, DuplicateModVariant, DuplicateResolution,
    LibraryAuditReport, ReviewedDuplicateMod, UnreferencedLibraryDir,
};
#[doc(hidden)]
pub use library_mutation::{
    DURABLE_WITNESS_TABLES, REINSTALL_SWAP_COLUMNS, STAGED_LIBRARY_OPERATION_COLUMNS,
};
pub use library_recovery::{DeletedLibraryDir, LibraryReclamationOutcome};
pub use mods::{Mod, ReinstallRecovery, ReinstallRecoveryOutcome, Source};
pub use session::SessionInfo;
pub use zip_import::ImportZipOptions;

use settings::{get as get_setting, keys, put as put_setting};

/// The Core owns the SQLite pool and the Library root. Everything that
/// reads from or writes to the user's data goes through here.
#[derive(Clone)]
pub struct Core {
    pool: SqlitePool,
    default_library_root: PathBuf,
    /// Test-only failure injection (issue #59). `None` in every shipped
    /// code path; only `crates/probe` ever sets it. See
    /// [`crash_points`] for why this is an injected field rather than a
    /// cfg flag or an environment variable.
    crash_hook: Option<CrashHook>,
}

#[derive(Clone, Copy)]
enum ManifestRedirects {
    FollowShippedUrl,
    RefuseLoopbackOverride,
}

/// Callback invoked at each named point in a durable mutation. See
/// [`crash_points`].
pub type CrashHook = Arc<dyn Fn(&str) + Send + Sync>;

impl Core {
    /// Open (or create) the DB at `db_url`, run pending migrations, and
    /// ensure the Library root exists.
    pub async fn new(default_library_root: PathBuf, db_url: &str) -> Result<Self> {
        Self::new_inner(default_library_root, db_url, None).await
    }

    /// Test seam for startup crash points. Production callers use [`Core::new`]
    /// and therefore install no hook while startup recovery runs.
    #[doc(hidden)]
    pub async fn new_with_crash_hook(
        default_library_root: PathBuf,
        db_url: &str,
        crash_hook: CrashHook,
    ) -> Result<Self> {
        Self::new_inner(default_library_root, db_url, Some(crash_hook)).await
    }

    async fn new_inner(
        default_library_root: PathBuf,
        db_url: &str,
        crash_hook: Option<CrashHook>,
    ) -> Result<Self> {
        std::fs::create_dir_all(&default_library_root).map_err(|source| Error::Io {
            path: default_library_root.clone(),
            source,
        })?;

        let opts: SqliteConnectOptions = db_url
            .parse::<SqliteConnectOptions>()?
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;

        let core = Self {
            pool,
            default_library_root,
            crash_hook,
        };
        if let Err(recovery) = core.resolve_interrupted_staging_at_startup().await {
            tracing::warn!(
                target: "gmm::library",
                error = %recovery,
                "could not release interrupted Library staging witnesses at startup",
            );
        }
        core.recover_interrupted_reinstalls_at_startup().await?;
        core.crash_point(crash_points::STARTUP_AFTER_REINSTALL_RECOVERY);
        if let Err(recovery) = core.finish_interrupted_library_deletes().await {
            tracing::warn!(
                target: "gmm::library",
                error = %recovery,
                "could not finish interrupted Library deletes at startup",
            );
        }
        Ok(core)
    }

    /// Install a failure-injection hook (issue #59). Test-only: nothing
    /// in the app calls this, so `crash_point` is a null check that
    /// never fires in a shipped build.
    pub fn with_crash_hook(mut self, hook: CrashHook) -> Self {
        self.crash_hook = Some(hook);
        self
    }

    /// Fire the crash hook, if one is installed. Placed after each step
    /// of a mutation that has already been made durable.
    fn crash_point(&self, name: &str) {
        if let Some(hook) = &self.crash_hook {
            hook(name);
        }
    }

    /// Default Library root as supplied to [`Core::new`]. Not the
    /// effective root — the user may have overridden it via settings.
    /// Use [`Core::resolved_library_root`] when you actually need the
    /// effective path.
    pub fn default_library_root(&self) -> &Path {
        &self.default_library_root
    }

    /// Effective Library root after applying any user override stored
    /// in the `settings` table. Falls back to the default supplied at
    /// construction time.
    pub async fn resolved_library_root(&self) -> Result<PathBuf> {
        let override_path = get_setting(&self.pool, keys::library_root()).await?;
        Ok(override_path
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_library_root.clone()))
    }

    /// Effective Library subtree for `game`. Per-game override wins; if
    /// none, fall back to `<resolved_library_root>/<game>`.
    pub async fn resolved_library_root_for(&self, game: GameCode) -> Result<PathBuf> {
        let per_game = get_setting(&self.pool, &keys::library_root_for_game(game)).await?;
        if let Some(p) = per_game {
            return Ok(PathBuf::from(p));
        }
        Ok(self.resolved_library_root().await?.join(game.as_str()))
    }

    /// Slice 16-b (#24): read the persisted onboarding state. The
    /// App router uses this on every cold start to decide between
    /// showing the wizard vs. the main app.
    pub async fn onboarding_status(&self) -> Result<crate::commands::OnboardingStatus> {
        let complete = get_setting(&self.pool, keys::onboarding_complete())
            .await?
            .map(|v| v == "true")
            .unwrap_or(false);
        let skipped = get_setting(&self.pool, keys::onboarding_skipped())
            .await?
            .map(|v| v == "true")
            .unwrap_or(false);
        Ok(crate::commands::OnboardingStatus { complete, skipped })
    }

    /// Slice 16-b (#24): persist that the user finished or skipped
    /// the wizard. `skipped == true` keeps the "Finish setup" banner
    /// alive in Settings until the user resumes.
    pub async fn mark_onboarding_complete(&self, skipped: bool) -> Result<()> {
        put_setting(&self.pool, keys::onboarding_complete(), Some("true")).await?;
        put_setting(
            &self.pool,
            keys::onboarding_skipped(),
            Some(if skipped { "true" } else { "false" }),
        )
        .await?;
        Ok(())
    }

    /// Slice 16-b (#24): re-open the wizard on the next launch. Used
    /// by the Help → Run setup again entry point.
    pub async fn reset_onboarding(&self) -> Result<()> {
        put_setting(&self.pool, keys::onboarding_complete(), Some("false")).await?;
        put_setting(&self.pool, keys::onboarding_skipped(), Some("false")).await?;
        Ok(())
    }

    /// Read the user-set override (if any) for the global library root.
    pub async fn library_root_override(&self) -> Result<Option<PathBuf>> {
        Ok(get_setting(&self.pool, keys::library_root())
            .await?
            .map(PathBuf::from))
    }

    /// Read the user-set override (if any) for a per-game library root.
    pub async fn library_root_override_for_game(&self, game: GameCode) -> Result<Option<PathBuf>> {
        Ok(get_setting(&self.pool, &keys::library_root_for_game(game))
            .await?
            .map(PathBuf::from))
    }

    /// Load the proxy config from settings. Includes the password —
    /// caller must not leak it. UI code should use
    /// [`Core::proxy_config_public`] instead.
    pub async fn proxy_config(&self) -> Result<network::ProxyConfig> {
        network::load(&self.pool).await
    }

    /// Password-free view of the proxy config for the UI.
    pub async fn proxy_config_public(&self) -> Result<network::ProxyConfigPublic> {
        Ok(network::load(&self.pool).await?.public())
    }

    /// Persist a proxy config (URL/username/password). Pass `None`
    /// fields to clear.
    pub async fn set_proxy_config(&self, cfg: &network::ProxyConfig) -> Result<()> {
        network::save(&self.pool, cfg).await
    }

    /// Build a reqwest `ClientBuilder` honouring the persisted proxy
    /// config. Use this instead of `reqwest::Client::builder()` so
    /// every outbound HTTP path routes through the user's proxy.
    pub async fn http_client_builder(&self) -> Result<reqwest::ClientBuilder> {
        let cfg = self.proxy_config().await?;
        network::client_builder(&cfg)
    }

    /// Convenience: build a ready-to-use `reqwest::Client` from the
    /// builder above.
    pub async fn http_client(&self) -> Result<reqwest::Client> {
        self.http_client_builder()
            .await?
            .build()
            .map_err(|e| Error::Network(format!("client build: {e}")))
    }

    /// Probe the configured proxy by issuing a HEAD on a known-good
    /// endpoint (`api.github.com`). Returns `Ok(())` on 2xx/3xx. The
    /// error message is friendly enough for the UI to render verbatim.
    pub async fn test_proxy_connection(&self) -> Result<()> {
        let cfg = self.proxy_config().await?;
        let client = network::client_builder(&cfg)?
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| Error::Network(format!("client build: {e}")))?;
        let res = client
            .head("https://api.github.com/")
            .send()
            .await
            .map_err(|e| Error::Network(network::classify_error(&e, cfg.is_configured())))?;
        if res.status().is_success() || res.status().is_redirection() {
            Ok(())
        } else {
            Err(Error::Network(format!(
                "Proxy reachable but probe returned {} from api.github.com",
                res.status()
            )))
        }
    }

    /// Override the **global** Library root. Walks every Mod whose
    /// current `library_path` sits under the previous effective root,
    /// moves it on disk, and rewrites its DB entry. Junctions for
    /// affected games are dropped + rebuilt via the standard reconcile
    /// path. `new_root = None` resets the override to the default.
    pub async fn set_library_root(&self, new_root: Option<&Path>) -> Result<MoveReport> {
        let mut fence = self
            .begin_library_mutation(library_mutation::LibraryMutation::SetLibraryRoot)
            .await?;
        let previous = self.resolved_library_root_in_mutation(&mut fence).await?;
        let next = new_root
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.default_library_root.clone());

        if previous == next {
            put_setting(
                &mut *fence.transaction,
                keys::library_root(),
                new_root.map(|p| p.to_string_lossy().to_string()).as_deref(),
            )
            .await?;
            fence.commit().await?;
            return Ok(MoveReport::default());
        }

        volume::require_ntfs(&next)?;

        // Move every game's subtree from previous to next. Per-game
        // overrides are unaffected — they're absolute and live elsewhere.
        let (mut report, previously_enabled) = self
            .move_root(&previous, &next, /* per_game */ None, &mut fence)
            .await?;
        put_setting(
            &mut *fence.transaction,
            keys::library_root(),
            new_root.map(|p| p.to_string_lossy().to_string()).as_deref(),
        )
        .await?;
        report.failed_junction_restores = self
            .restore_relocated_junctions(previously_enabled, &mut fence)
            .await;
        fence.commit().await?;
        self.crash_point(crash_points::RELOCATE_AFTER_FENCE_COMMIT);
        Ok(report)
    }

    /// Override the Library root for one game. Behaviour mirrors
    /// [`Core::set_library_root`] but only the named game's subtree is
    /// touched.
    pub async fn set_library_path_for_game(
        &self,
        game: GameCode,
        new_path: Option<&Path>,
    ) -> Result<MoveReport> {
        let mut fence = self
            .begin_library_mutation(library_mutation::LibraryMutation::SetLibraryPathForGame)
            .await?;
        let previous = self
            .resolved_library_root_for_in_mutation(game, &mut fence)
            .await?;
        let next = new_path.map(Path::to_path_buf).unwrap_or_else(|| {
            // When clearing, the effective path becomes
            // `resolved_root().join(game)`. We compute it eagerly
            // so the move flow knows where files go.
            // (`resolved_library_root_for(game)` would still hit
            // the now-cleared override, so we mirror its fallback
            // here.)
            PathBuf::new()
        });

        let fallback = self
            .resolved_library_root_in_mutation(&mut fence)
            .await?
            .join(game.as_str());
        let next_effective = if next.as_os_str().is_empty() {
            fallback
        } else {
            next.clone()
        };

        if previous == next_effective {
            put_setting(
                &mut *fence.transaction,
                &keys::library_root_for_game(game),
                new_path.map(|p| p.to_string_lossy().to_string()).as_deref(),
            )
            .await?;
            fence.commit().await?;
            return Ok(MoveReport::default());
        }

        volume::require_ntfs(&next_effective)?;

        let (mut report, previously_enabled) = self
            .move_root(&previous, &next_effective, Some(game), &mut fence)
            .await?;
        put_setting(
            &mut *fence.transaction,
            &keys::library_root_for_game(game),
            new_path.map(|p| p.to_string_lossy().to_string()).as_deref(),
        )
        .await?;
        report.failed_junction_restores = self
            .restore_relocated_junctions(previously_enabled, &mut fence)
            .await;
        fence.commit().await?;
        self.crash_point(crash_points::RELOCATE_AFTER_FENCE_COMMIT);
        Ok(report)
    }

    /// Shared body for the global + per-game moves.
    ///
    /// `per_game = Some(g)` restricts the move to mods for `g`.
    /// `per_game = None` walks every game.
    async fn move_root(
        &self,
        previous: &Path,
        next: &Path,
        per_game: Option<GameCode>,
        fence: &mut library_mutation::LibraryMutationFence,
    ) -> Result<(MoveReport, Vec<(String, GameCode)>)> {
        let cleanup_roots: Vec<PathBuf> = match per_game {
            Some(_) => vec![previous.to_path_buf()],
            None => games::GAME_PROFILES
                .iter()
                .map(|profile| previous.join(profile.code.as_str()))
                .collect(),
        };
        if library_recovery::has_owned_delete_quarantine(&cleanup_roots)? {
            return Err(Error::LibraryRelocationBlockedByCleanup);
        }

        // A same-volume rename preserves the filesystem identities recorded by
        // an in-flight reinstall witness, but the cross-volume copy fallback
        // does not. Refuse every relocation that would carry such a witness.
        // `path_within` is required because the Mod row can retain an NTFS
        // alias or differently-cased drive spelling for the same root.
        // This check is under the same writer fence as witness creation, so it
        // happens before any Junction or Library byte is touched.
        let active_reinstalls =
            library_mutation::load_reinstall_swap_witnesses(&mut fence.transaction).await?;
        for witness in active_reinstalls {
            let witness = self.rebase_reinstall_swap_witness(witness, fence).await?;
            if path_within(witness.library_path(), previous) {
                return Err(Error::LibraryRelocationBlockedByReinstall {
                    mod_id: witness.mod_id().to_string(),
                });
            }
        }

        // Snapshot mods that need their library_path rewritten. For the
        // global case we include every mod across every game; for the
        // per-game case only that game.
        let rows = match per_game {
            Some(game) => {
                sqlx::query(
                    "SELECT id, game_code, library_path, enabled FROM mods WHERE game_code = ?",
                )
                .bind(game.as_str())
                .fetch_all(&mut *fence.transaction)
                .await?
            }
            None => {
                sqlx::query("SELECT id, game_code, library_path, enabled FROM mods")
                    .fetch_all(&mut *fence.transaction)
                    .await?
            }
        };
        self.crash_point(crash_points::RELOCATE_AFTER_MOD_SNAPSHOT);

        // Disable affected mods first to drop their junctions. We don't
        // need the persisted enabled=0 flip — we'll re-enable in the
        // same transaction-shaped flow below.
        let mut previously_enabled: Vec<(String, GameCode)> = Vec::new();
        for row in &rows {
            let enabled: i64 = row.try_get("enabled")?;
            if enabled == 0 {
                continue;
            }
            let id: String = row.try_get("id")?;
            let game_code: String = row.try_get("game_code")?;
            let game = GameCode::from_str(&game_code)?;
            let game_row = sqlx::query("SELECT install_path FROM games WHERE code = ?")
                .bind(game.as_str())
                .fetch_one(&mut *fence.transaction)
                .await?;
            let install = game_row
                .try_get::<Option<String>, _>("install_path")?
                .map(PathBuf::from);
            if let Some(install) = install {
                let mods_dir = install.join("Mods");
                let junction_row = sqlx::query("SELECT junction_dir_name FROM mods WHERE id = ?")
                    .bind(&id)
                    .fetch_one(&mut *fence.transaction)
                    .await?;
                let junction_dir_name: String = junction_row.try_get("junction_dir_name")?;
                let link = mods_dir.join(junction_dir_name);
                if link_exists(&link)? {
                    junction::remove(&link)?;
                }
            }
            previously_enabled.push((id, game));
        }

        // Move bytes. We move the **per-game** subtree as a unit when
        // possible (one fs::rename per game). If that fails (cross-device,
        // partial move) we fall back to a per-mod move with copy+delete.
        let mut report = MoveReport::default();
        std::fs::create_dir_all(next).map_err(|source| Error::Io {
            path: next.to_path_buf(),
            source,
        })?;

        match per_game {
            Some(_) => {
                // The whole `previous` directory is a single game's
                // subtree; move it whole.
                move_subtree(previous, next, &mut report)?;
            }
            None => {
                // Global move: each game subdirectory under `previous`
                // moves to the matching subdirectory under `next`.
                for game in [
                    GameCode::Gimi,
                    GameCode::Srmi,
                    GameCode::Zzmi,
                    GameCode::Wwmi,
                    GameCode::Himi,
                    GameCode::Efmi,
                ] {
                    let from = previous.join(game.as_str());
                    let to = next.join(game.as_str());
                    if from.exists() {
                        move_subtree(&from, &to, &mut report)?;
                    }
                }
            }
        }

        // Rewrite mods.library_path entries. We use a literal
        // `previous` → `next` string prefix swap; both paths are
        // absolute and canonicalised on insert.
        let previous_prefix = previous.to_string_lossy().to_string();
        let next_prefix = next.to_string_lossy().to_string();
        for row in &rows {
            let id: String = row.try_get("id")?;
            let library_path: String = row.try_get("library_path")?;
            if !library_path.starts_with(&previous_prefix) {
                continue;
            }
            let rewritten = format!("{}{}", next_prefix, &library_path[previous_prefix.len()..]);
            sqlx::query("UPDATE mods SET library_path = ? WHERE id = ?")
                .bind(&rewritten)
                .bind(&id)
                .execute(&mut *fence.transaction)
                .await?;
            report.relocated.push(id);
        }

        Ok((report, previously_enabled))
    }

    /// Restore every previously-enabled Mod's junction while the writer fence
    /// still excludes Game Session claims. The rows remain enabled throughout;
    /// persisting a temporary disabled state would let a session claim strand
    /// the remaining Mods as disabled.
    async fn restore_relocated_junctions(
        &self,
        previously_enabled: Vec<(String, GameCode)>,
        fence: &mut library_mutation::LibraryMutationFence,
    ) -> Vec<JunctionRestoreFailure> {
        let mut failures = Vec::new();
        for (id, game) in previously_enabled {
            if let Err(error) = self.restore_relocated_junction(&id, game, fence).await {
                failures.push(JunctionRestoreFailure {
                    mod_id: id,
                    game,
                    kind: error.surface_failure_kind(),
                    error: error.to_string(),
                });
            }
        }

        failures
    }

    async fn restore_relocated_junction(
        &self,
        id: &str,
        game: GameCode,
        fence: &mut library_mutation::LibraryMutationFence,
    ) -> Result<()> {
        let game_row = sqlx::query("SELECT install_path FROM games WHERE code = ?")
            .bind(game.as_str())
            .fetch_one(&mut *fence.transaction)
            .await?;
        let install = game_row
            .try_get::<Option<String>, _>("install_path")?
            .map(PathBuf::from);
        let Some(install) = install else {
            return Ok(());
        };
        let mods_dir = install.join("Mods");
        std::fs::create_dir_all(&mods_dir).map_err(|source| Error::Io {
            path: mods_dir.clone(),
            source,
        })?;
        let row = sqlx::query("SELECT junction_dir_name, library_path FROM mods WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *fence.transaction)
            .await?;
        let junction_dir_name: String = row.try_get("junction_dir_name")?;
        let library_path = PathBuf::from(row.try_get::<String, _>("library_path")?);
        let target = self
            .junction_target_for(id, &library_path, &mut *fence.transaction)
            .await?;
        let link = mods_dir.join(junction_dir_name);
        volume::require_ntfs_pair(&mods_dir, &target)?;
        junction::create(&link, &target)?;
        self.crash_point(crash_points::RELOCATE_AFTER_JUNCTION_RESTORE);

        Ok(())
    }

    /// Adopt an already-extracted folder into the Library as a Mod with
    /// `source = manual`. Copies the source tree into
    /// `<resolved_library_root_for(game)>/<ulid>/` and records the row.
    pub async fn adopt_folder(
        &self,
        game: GameCode,
        source_path: &Path,
        display_name: &str,
    ) -> Result<Mod> {
        let id = Ulid::new().to_string();
        let (root, staged) = self
            .create_staged_library_directory(
                game,
                &id,
                library_mutation::LibraryMutation::AdoptFolder,
            )
            .await?;
        let library_path = staged.path().to_path_buf();

        let first_file = std::sync::Once::new();
        let after_file = || {
            first_file.call_once(|| self.crash_point(crash_points::ADOPT_DURING_LIBRARY_COPY));
        };
        if let Err(error) = copy_dir_recursive(source_path, &library_path, Some(&after_file)) {
            self.cleanup_staged_library_dir(
                &root,
                staged,
                library_mutation::LibraryMutation::AdoptFolder,
            )
            .await;
            return Err(error);
        }
        self.crash_point(crash_points::ADOPT_AFTER_LIBRARY_COPY);

        // Variant detection recursively traverses user-supplied content and
        // is therefore unbounded. Complete it before acquiring the Library
        // writer fence, then persist the detected shape with the Mod row in
        // the single transaction below.
        let detected_variants = match variants::detect_variants(&library_path) {
            Ok(detected) => detected,
            Err(error) => {
                self.cleanup_staged_library_dir(
                    &root,
                    staged,
                    library_mutation::LibraryMutation::AdoptFolder,
                )
                .await;
                return Err(error);
            }
        };

        self.commit_staged_mod(
            root,
            staged,
            game,
            &id,
            display_name,
            Source::Manual,
            &library_path,
            detected_variants,
            library_mutation::LibraryMutation::AdoptFolder,
            crash_points::ADOPT_AFTER_ROW_INSERT,
            crash_points::ADOPT_AFTER_FENCE_COMMIT,
        )
        .await?;

        Ok(Mod {
            id,
            game,
            name: display_name.to_string(),
            source: Source::Manual,
            library_path,
            enabled: false,
            gamebanana_id: None,
            source_url: None,
            author: None,
            version: None,
            screenshot_url: None,
            reinstall_recovery: None,
        })
    }

    /// List the GameBanana mods for `game` along with their current
    /// install vs. upstream-version state. Does NOT hit the network —
    /// it only reads what the last poll wrote.
    pub async fn list_mod_updates(&self, game: GameCode) -> Result<Vec<mod_updates::ModUpdateRow>> {
        let rows = sqlx::query(
            "SELECT id, name, version, upstream_version, update_check_enabled
             FROM mods
             WHERE game_code = ? AND source = ?",
        )
        .bind(game.as_str())
        .bind(Source::Gamebanana.as_str())
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let installed: Option<String> = row.try_get("version")?;
            let upstream: Option<String> = row.try_get("upstream_version")?;
            out.push(mod_updates::ModUpdateRow {
                mod_id: row.try_get("id")?,
                name: row.try_get("name")?,
                upstream_ahead: mod_updates::upstream_ahead(
                    installed.as_deref(),
                    upstream.as_deref(),
                ),
                installed_version: installed,
                upstream_version: upstream,
                update_check_enabled: row.try_get::<i64, _>("update_check_enabled")? != 0,
            });
        }
        Ok(out)
    }

    /// Poll upstream for every GameBanana mod whose
    /// `update_check_enabled` is true. Updates `upstream_version` in
    /// the DB and persists `mod_updates.last_check_at`. Honours the
    /// global toggle: if `mod_updates.enabled` is `false`, returns the
    /// existing rows without a fetch.
    pub async fn check_mod_updates_now(
        &self,
        game: GameCode,
    ) -> Result<Vec<mod_updates::ModUpdateRow>> {
        self.check_mod_updates_now_with_endpoints(game, &gamebanana::Endpoints::default())
            .await
    }

    /// Test seam: like `check_mod_updates_now`, but takes the
    /// GameBanana endpoint base URL so mockito-driven tests can avoid
    /// hitting the live API.
    pub async fn check_mod_updates_now_with_endpoints(
        &self,
        game: GameCode,
        endpoints: &gamebanana::Endpoints,
    ) -> Result<Vec<mod_updates::ModUpdateRow>> {
        if !self.mod_updates_globally_enabled().await? {
            return self.list_mod_updates(game).await;
        }

        let rows = sqlx::query(
            "SELECT id, gamebanana_id, update_check_enabled
             FROM mods
             WHERE game_code = ? AND source = ?",
        )
        .bind(game.as_str())
        .bind(Source::Gamebanana.as_str())
        .fetch_all(&self.pool)
        .await?;

        let client = self.http_client().await?;
        for row in rows {
            let enabled: i64 = row.try_get("update_check_enabled")?;
            if enabled == 0 {
                continue;
            }
            let mod_id: String = row.try_get("id")?;
            let gid: Option<i64> = row.try_get("gamebanana_id")?;
            let Some(gid) = gid else { continue };
            // Best-effort: a single failed fetch must not abort the
            // batch. Tracing captures the reason for diagnostics.
            match gamebanana::fetch_submission(&client, endpoints, gid as u64).await {
                Ok(s) => {
                    sqlx::query("UPDATE mods SET upstream_version = ? WHERE id = ?")
                        .bind(s.version.as_deref())
                        .bind(&mod_id)
                        .execute(&self.pool)
                        .await?;
                }
                Err(e) => {
                    tracing::warn!(
                        target: "gmm::mod_updates",
                        mod_id = %mod_id,
                        gamebanana_id = gid,
                        error = %e,
                        "skipping mod update poll",
                    );
                }
            }
        }

        put_setting(
            &self.pool,
            mod_updates::keys::LAST_CHECK_AT,
            Some(Utc::now().to_rfc3339().as_str()),
        )
        .await?;
        self.list_mod_updates(game).await
    }

    /// Read the global mod-update toggle. Defaults to `true` when
    /// nothing has been persisted yet.
    pub async fn mod_updates_globally_enabled(&self) -> Result<bool> {
        Ok(get_setting(&self.pool, mod_updates::keys::GLOBAL_ENABLED)
            .await?
            .map(|v| v != "false")
            .unwrap_or(true))
    }

    /// Persist the global mod-update toggle.
    pub async fn set_mod_updates_globally_enabled(&self, enabled: bool) -> Result<()> {
        put_setting(
            &self.pool,
            mod_updates::keys::GLOBAL_ENABLED,
            Some(if enabled { "true" } else { "false" }),
        )
        .await
    }

    /// Per-mod opt-out toggle.
    pub async fn set_mod_update_check_enabled(&self, mod_id: &str, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE mods SET update_check_enabled = ? WHERE id = ?")
            .bind(if enabled { 1_i64 } else { 0_i64 })
            .bind(mod_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Re-run the slice-11 GameBanana ingest against an existing Mod row.
    /// The replacement is fully extracted and inspected under a durable swap
    /// witness before the installed Library subtree is touched. The final
    /// same-volume swap, metadata/Variant rewrite, and Junction retarget are
    /// fenced; witness deletion is part of that exact metadata transaction.
    pub async fn reinstall_gamebanana_mod(&self, mod_id: &str) -> Result<()> {
        self.reinstall_gamebanana_mod_with_endpoints(mod_id, &gamebanana::Endpoints::default())
            .await
    }

    /// Test seam for `reinstall_gamebanana_mod`. Production calls the
    /// default-endpoint flavour.
    pub async fn reinstall_gamebanana_mod_with_endpoints(
        &self,
        mod_id: &str,
        endpoints: &gamebanana::Endpoints,
    ) -> Result<()> {
        self.ensure_no_active_session().await?;
        let row = sqlx::query(
            "SELECT game_code, gamebanana_id
             FROM mods WHERE id = ?",
        )
        .bind(mod_id)
        .fetch_one(&self.pool)
        .await?;
        let game_code: String = row.try_get("game_code")?;
        let game = GameCode::from_str(&game_code)?;
        let gid: Option<i64> = row.try_get("gamebanana_id")?;
        let gid = gid.ok_or_else(|| {
            Error::GameBanana(format!("mod {mod_id} has no GameBanana submission ID"))
        })? as u64;

        // 1. Resolve metadata + download the fresh ZIP before creating any
        //    filesystem recovery state. The installed Mod remains untouched.
        let client = self.http_client().await?;
        let submission = gamebanana::fetch_submission(&client, endpoints, gid).await?;
        let cache = self
            .default_library_root
            .parent()
            .map(|p| p.join("downloads").join("gamebanana"))
            .unwrap_or_else(|| std::path::PathBuf::from("./downloads/gamebanana"));
        std::fs::create_dir_all(&cache).map_err(|source| Error::Io {
            path: cache.clone(),
            source,
        })?;
        let zip_path = cache.join(format!("{}-{}", gid, submission.file_name));
        gamebanana::download_to(&client, &submission.file_url, &zip_path).await?;

        // 2. Under a short writer fence, re-read the Mod, identify the old
        //    tree, create the staging directory, and commit the durable
        //    witness. A crash from this point until metadata commit always
        //    means rollback to the old tree.
        let token = Ulid::new();
        let mut preparation = self
            .begin_library_mutation(library_mutation::LibraryMutation::ReinstallGamebananaMod)
            .await?;
        let current = sqlx::query(
            "SELECT game_code, gamebanana_id, library_path
             FROM mods WHERE id = ?",
        )
        .bind(mod_id)
        .fetch_one(&mut *preparation.transaction)
        .await?;
        let current_game: String = current.try_get("game_code")?;
        let current_gid: Option<i64> = current.try_get("gamebanana_id")?;
        if current_game != game.as_str() || current_gid != Some(gid as i64) {
            return Err(Error::GameBanana(format!(
                "mod {mod_id} changed while its replacement was downloading; the installed Mod was not touched"
            )));
        }
        let library_path = PathBuf::from(current.try_get::<String, _>("library_path")?);
        let root = self
            .resolved_library_root_for_in_mutation(game, &mut preparation)
            .await?;
        let root_directory =
            library_identity::IdentifiedDirectory::open(&root).map_err(|source| Error::Io {
                path: root.clone(),
                source,
            })?;
        let parent = library_path
            .parent()
            .ok_or_else(|| Error::ReinstallWitnessCorrupt {
                mod_id: mod_id.to_string(),
                reason: "the installed Library path has no parent".to_string(),
            })?;
        let parent_directory =
            library_identity::IdentifiedDirectory::open(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        if parent_directory.identity() != root_directory.identity()
            || library_path.file_name().and_then(|name| name.to_str()) != Some(mod_id)
        {
            return Err(Error::ReinstallWitnessCorrupt {
                mod_id: mod_id.to_string(),
                reason: "the installed Mod is not the expected direct child of its effective Library root".to_string(),
            });
        }
        let old_directory =
            library_identity::IdentifiedDirectory::open(&library_path).map_err(|source| {
                Error::Io {
                    path: library_path.clone(),
                    source,
                }
            })?;
        let staged_path = root.join(format!(
            "{}{token}",
            library_mutation::REINSTALL_STAGING_PREFIX
        ));
        std::fs::create_dir(&staged_path).map_err(|source| Error::Io {
            path: staged_path.clone(),
            source,
        })?;
        let staged_directory = match library_identity::IdentifiedDirectory::open(&staged_path) {
            Ok(directory) => directory,
            Err(source) => {
                let _ = std::fs::remove_dir(&staged_path);
                return Err(Error::Io {
                    path: staged_path,
                    source,
                });
            }
        };
        let staged_identity = staged_directory.identity().clone();
        let staged_identity_key = staged_identity.durable_key();
        let quarantine_path = root.join(format!(
            "{}{}",
            library_recovery::DELETE_QUARANTINE_PREFIX,
            token
        ));
        let witness_insert = library_mutation::insert_reinstall_swap_witness(
            &mut preparation.transaction,
            library_mutation::NewReinstallSwapWitness {
                token,
                mod_id,
                game,
                library_path: &library_path,
                staged_path: &staged_path,
                quarantine_path: &quarantine_path,
                old_identity: old_directory.identity(),
                staged_identity: &staged_identity,
            },
        )
        .await;
        if let Err(reinstall) = witness_insert {
            drop(root_directory);
            drop(parent_directory);
            drop(old_directory);
            drop(staged_directory);
            let _ = preparation.transaction.rollback().await;
            return match remove_reinstall_stage_if_identity_matches(
                &staged_path,
                &staged_identity_key,
            ) {
                Ok(()) => Err(reinstall),
                Err(rollback) => Err(Error::ReinstallRollbackFailed {
                    reinstall: reinstall.to_string(),
                    rollback: rollback.to_string(),
                }),
            };
        }
        drop(root_directory);
        drop(parent_directory);
        drop(old_directory);
        drop(staged_directory);
        if let Err(reinstall) = preparation.commit().await {
            let witness_exists = async {
                let mut connection = self.pool.acquire().await?;
                Ok::<bool, Error>(
                    library_mutation::load_reinstall_swap_witnesses(&mut connection)
                        .await?
                        .into_iter()
                        .any(|witness| witness.token() == token),
                )
            }
            .await;
            let rollback = match witness_exists {
                Ok(true) => self.rollback_reinstall_swap(token).await,
                Ok(false) => {
                    remove_reinstall_stage_if_identity_matches(&staged_path, &staged_identity_key)
                }
                Err(error) => Err(error),
            };
            return match rollback {
                Ok(()) => Err(reinstall),
                Err(rollback) => Err(Error::ReinstallRollbackFailed {
                    reinstall: reinstall.to_string(),
                    rollback: rollback.to_string(),
                }),
            };
        }
        self.crash_point(crash_points::REINSTALL_AFTER_WITNESS_COMMIT);

        // 3. Extraction and inspection are unbounded and therefore outside
        //    the writer fence. Any failure invokes the same deterministic
        //    rollback startup uses; the installed bytes and enabled state have
        //    not changed yet.
        if let Err(reinstall) =
            zip_import::extract(&zip_path, &staged_path, ImportZipOptions::default())
        {
            return match self.rollback_reinstall_swap(token).await {
                Ok(()) => Err(reinstall),
                Err(rollback) => Err(Error::ReinstallRollbackFailed {
                    reinstall: reinstall.to_string(),
                    rollback: rollback.to_string(),
                }),
            };
        }
        let detected_variants = match variants::detect_variants(&staged_path) {
            Ok(variants) => variants,
            Err(reinstall) => {
                return match self.rollback_reinstall_swap(token).await {
                    Ok(()) => Err(reinstall),
                    Err(rollback) => Err(Error::ReinstallRollbackFailed {
                        reinstall: reinstall.to_string(),
                        rollback: rollback.to_string(),
                    }),
                };
            }
        };

        // 4. Reacquire the writer fence and re-prove every name and identity
        //    before committing the two renames and all user-visible state.
        let commit_result: Result<library_recovery::QuarantinedLibraryDirectory> = async {
            let mut commit = self
                .begin_library_mutation(library_mutation::LibraryMutation::ReinstallGamebananaMod)
                .await?;
            let witness = self.reinstall_swap_witness(token, &mut commit).await?;
            let live = library_identity::IdentifiedDirectory::open(witness.library_path())
                .map_err(|source| Error::Io {
                    path: witness.library_path().to_path_buf(),
                    source,
                })?;
            let staged = library_identity::IdentifiedDirectory::open(witness.staged_path())
                .map_err(|source| Error::Io {
                    path: witness.staged_path().to_path_buf(),
                    source,
                })?;
            if live.identity() != witness.old_identity()
                || staged.identity() != witness.staged_identity()
            {
                return Err(Error::ReinstallRecoveryUncertain {
                    mod_id: mod_id.to_string(),
                    reason: "the live or staged directory changed identity before swap commit"
                        .to_string(),
                });
            }

            let state = sqlx::query(
                "SELECT m.enabled, m.junction_dir_name, g.install_path
             FROM mods m JOIN games g ON g.code = m.game_code WHERE m.id = ?",
            )
            .bind(mod_id)
            .fetch_one(&mut *commit.transaction)
            .await?;
            let enabled = state.try_get::<i64, _>("enabled")? != 0;
            let junction_dir_name: String = state.try_get("junction_dir_name")?;
            let mods_dir = state
                .try_get::<Option<String>, _>("install_path")?
                .map(PathBuf::from)
                .map(|install| install.join("Mods"));

            let quarantined = self.quarantine_library_directory_with_token(
                witness.library_path(),
                &live,
                token,
                None,
                None,
            )?;
            drop(live);
            self.crash_point(crash_points::REINSTALL_AFTER_OLD_QUARANTINE_MOVE);
            std::fs::rename(witness.staged_path(), witness.library_path()).map_err(|source| {
                Error::Io {
                    path: witness.staged_path().to_path_buf(),
                    source,
                }
            })?;
            drop(staged);
            let installed = library_identity::IdentifiedDirectory::open(witness.library_path())
                .map_err(|source| Error::Io {
                    path: witness.library_path().to_path_buf(),
                    source,
                })?;
            if installed.identity() != witness.staged_identity() {
                return Err(Error::ReinstallRecoveryUncertain {
                    mod_id: mod_id.to_string(),
                    reason: "the replacement changed identity during its final rename".to_string(),
                });
            }
            drop(installed);
            self.crash_point(crash_points::REINSTALL_AFTER_REPLACEMENT_MOVE);

            let first_variant_id = detected_variants.first().map(|_| Ulid::new().to_string());
            let new_target = detected_variants
                .first()
                .map(|variant| witness.library_path().join(&variant.subpath))
                .unwrap_or_else(|| witness.library_path().to_path_buf());
            if enabled {
                if let Some(mods_dir) = mods_dir.as_ref() {
                    std::fs::create_dir_all(mods_dir).map_err(|source| Error::Io {
                        path: mods_dir.clone(),
                        source,
                    })?;
                    let link = mods_dir.join(&junction_dir_name);
                    if link_exists(&link)? {
                        junction::remove(&link)?;
                    }
                    volume::require_ntfs_pair(mods_dir, &new_target)?;
                    junction::create(&link, &new_target)?;
                }
            }

            sqlx::query("UPDATE mods SET active_variant_id = NULL WHERE id = ?")
                .bind(mod_id)
                .execute(&mut *commit.transaction)
                .await?;
            sqlx::query("DELETE FROM mod_variants WHERE mod_id = ?")
                .bind(mod_id)
                .execute(&mut *commit.transaction)
                .await?;
            for (index, variant) in detected_variants.iter().enumerate() {
                let variant_id = if index == 0 {
                    first_variant_id.clone().expect("first Variant ID")
                } else {
                    Ulid::new().to_string()
                };
                sqlx::query(
                    "INSERT INTO mod_variants (id, mod_id, name, subpath) VALUES (?, ?, ?, ?)",
                )
                .bind(&variant_id)
                .bind(mod_id)
                .bind(&variant.name)
                .bind(variant.subpath.to_string_lossy().as_ref())
                .execute(&mut *commit.transaction)
                .await?;
            }
            sqlx::query(
                "UPDATE mods
               SET active_variant_id = ?, name = ?, author = ?, version = ?,
                   upstream_version = ?, screenshot_url = ?
             WHERE id = ?",
            )
            .bind(&first_variant_id)
            .bind(&submission.name)
            .bind(&submission.author)
            .bind(&submission.version)
            .bind(&submission.version)
            .bind(&submission.screenshot_url)
            .bind(mod_id)
            .execute(&mut *commit.transaction)
            .await?;
            library_mutation::delete_reinstall_swap_witness(&mut commit.transaction, token).await?;
            commit.commit().await?;
            Ok(quarantined)
        }
        .await;
        let quarantined = match commit_result {
            Ok(quarantined) => quarantined,
            Err(reinstall) => {
                return match self.rollback_reinstall_swap(token).await {
                    Ok(()) => Err(reinstall),
                    Err(rollback) => Err(Error::ReinstallRollbackFailed {
                        reinstall: reinstall.to_string(),
                        rollback: rollback.to_string(),
                    }),
                };
            }
        };
        self.crash_point(crash_points::REINSTALL_AFTER_METADATA_COMMIT);

        // Metadata commit made the new tree authoritative. The old tree is
        // now an ordinary owned delete quarantine; reclamation may be deferred
        // without changing the successful reinstall outcome.
        match quarantined.purge(false) {
            Ok(library_recovery::QuarantinePurgeOutcome::Reclaimed(_)) => {}
            Ok(library_recovery::QuarantinePurgeOutcome::Deferred { path, error }) => {
                tracing::warn!(
                    target: "gmm::library",
                    mod_id,
                    quarantine = %path.display(),
                    error = %error,
                    "the reinstall committed, but GMM could not reclaim the verified old bytes now; startup will retry while it can still prove the quarantine",
                );
            }
            Ok(library_recovery::QuarantinePurgeOutcome::OwnershipLost) => {
                tracing::error!(
                    target: "gmm::library",
                    mod_id,
                    "the reinstall committed, but GMM cannot establish whether the old quarantined bytes were reclaimed",
                );
            }
            Err(error) => tracing::warn!(
                target: "gmm::library",
                mod_id,
                error = %error,
                "the reinstall committed, but GMM could not inspect or reclaim the old quarantine",
            ),
        }
        Ok(())
    }

    /// Check whether the upstream importer release for `game` is newer
    /// than the persisted `installed_version`. Returns an
    /// [`updates::UpdateStatus`] that the UI can render directly. The
    /// per-game pin suppresses the `available` flag but is still
    /// surfaced separately so the dialog can show "pinned to vX".
    ///
    /// `repo` and `asset_pattern` are passed in so the caller can decide
    /// which importer origin applies (e.g. `SilentNightSound/GIMI-Package`
    /// for GIMI). Future per-game ports can call this with their own
    /// origin.
    pub async fn check_importer_update(
        &self,
        game: GameCode,
        repo: &str,
        asset_pattern: &str,
    ) -> Result<updates::UpdateStatus> {
        let pattern = importer::AssetPattern::new(asset_pattern)?;
        let client = self.http_client().await?;
        let latest = match importer::fetch_latest_release(
            &client,
            &importer::Endpoints::default(),
            repo,
            &pattern,
            None,
        )
        .await
        {
            Ok(Some(release)) => Ok(release.tag_name),
            // `None` means 304 Not Modified, which needs an ETag we
            // never send. Treat it as "nothing learned" rather than
            // lying that upstream is current.
            Ok(None) => Err("upstream reported no change but GMM sent no ETag".to_string()),
            Err(e) => Err(e.to_string()),
        };
        let installed = updates::importer_installed(&self.pool, game).await?;
        let pinned = updates::importer_pinned(&self.pool, game).await?.is_some();
        Ok(updates::compute_status(installed, latest, pinned))
    }

    /// [`Core::check_importer_update`] against the game's **resolved**
    /// Importer Origin (ADR 0005) rather than compiled-in constants.
    ///
    /// This is what the Tauri command calls, so a user override changes
    /// which repository the badge is computed from.
    /// The origin asked is the one the install **came from**, not the
    /// one that resolves (#109). Comparing a version taken against
    /// origin Y with the latest release of origin X produces a
    /// meaningless `upstream_ahead`; under "a recommendation never
    /// switches an existing install" that comparison cannot arise,
    /// because the update path only ever looks at the origin the install
    /// actually came from.
    pub async fn check_importer_update_for(&self, game: GameCode) -> Result<updates::UpdateStatus> {
        self.check_importer_update_with_endpoints(game, &importer::Endpoints::default())
            .await
    }

    /// Test seam for [`Core::check_importer_update_for`] — production
    /// uses the `Endpoints::default()` overload. It exists so a test can
    /// assert *which repository was asked*, which is the whole content
    /// of the #109 rule and is otherwise unobservable.
    pub async fn check_importer_update_with_endpoints(
        &self,
        game: GameCode,
        endpoints: &importer::Endpoints,
    ) -> Result<updates::UpdateStatus> {
        let target = self.importer_origin_for_install(game).await?;
        self.check_importer_update_against(game, &target, endpoints)
            .await
    }

    /// [`Core::check_importer_update_for`] against an explicit
    /// resolution. Production resolves first; tests use this to drive
    /// the no-origin-in-effect path without waiting on #108 to make
    /// retraction reachable, the same seam
    /// [`Core::check_loader_update_from`] provides for the Loader.
    ///
    /// When **no origin is in effect** GMM warns and does not block
    /// (#97): the status carries an explanatory `check_error` and
    /// claims nothing about upstream. It must never come back looking
    /// like "up to date" — that collapse is the defect #78 fixed, and
    /// #79 removed from the importer path.
    pub async fn check_importer_update_with(
        &self,
        game: GameCode,
        resolution: &importer_origin::OriginResolution,
    ) -> Result<updates::UpdateStatus> {
        let installed = self.installed_importer_origin(game).await?;
        let target = importer_origin::origin_for_install(&installed, resolution);
        self.check_importer_update_against(game, &target, &importer::Endpoints::default())
            .await
    }

    /// The update check against an already-decided
    /// [`importer_origin::InstallOrigin`].
    ///
    /// Every arm that cannot name a repository returns a
    /// `check_error` — never `available: false`, which reads as "we
    /// checked, nothing to apply". That collapse is the defect #78 fixed
    /// for the Loader and #79 removed from this path, and this function
    /// adds one more arm that must not fall into it: an install whose
    /// recorded origin cannot be read.
    async fn check_importer_update_against(
        &self,
        game: GameCode,
        target: &importer_origin::InstallOrigin,
        endpoints: &importer::Endpoints,
    ) -> Result<updates::UpdateStatus> {
        let installed = updates::importer_installed(&self.pool, game).await?;
        let pinned = updates::importer_pinned(&self.pool, game).await?.is_some();

        let origin = match target {
            importer_origin::InstallOrigin::Installed(origin) => origin,
            importer_origin::InstallOrigin::Resolved { origin, .. } => origin,
            importer_origin::InstallOrigin::NoneInEffect { reason } => {
                let mut message = format!(
                    "GMM has no Model Importer origin for {}, so there is nothing to \
                     check for updates.",
                    game.profile().display_name,
                );
                if let Some(reason) = reason {
                    message.push(' ');
                    message.push_str(reason);
                }
                message.push(' ');
                message.push_str(error::SET_AN_ORIGIN_HINT);
                return Ok(updates::compute_status(installed, Err(message), pinned));
            }
            importer_origin::InstallOrigin::InstalledUnreadable { error, .. } => {
                return Ok(updates::compute_status(
                    installed,
                    Err(format!(
                        "GMM recorded a Model Importer install for {} but can no longer \
                         read which Importer Origin it came from ({error}), so it cannot \
                         say whether an update exists. {}",
                        game.profile().display_name,
                        error::SET_AN_ORIGIN_HINT,
                    )),
                    pinned,
                ));
            }
        };

        let pattern = importer::AssetPattern::new(origin.asset_pattern())?;
        let client = self.http_client().await?;
        let latest = match importer::fetch_latest_release(
            &client,
            endpoints,
            &origin.repo_slug(),
            &pattern,
            None,
        )
        .await
        {
            Ok(Some(release)) => Ok(release.tag_name),
            // `None` means 304 Not Modified, which needs an ETag we
            // never send. Treat it as "nothing learned" rather than
            // lying that upstream is current.
            Ok(None) => Err("upstream reported no change but GMM sent no ETag".to_string()),
            Err(e) => Err(e.to_string()),
        };
        Ok(updates::compute_status(installed, latest, pinned))
    }

    /// Report the Loader (`3dmloader.dll` from
    /// [`updates::LOADER_REPO`]) this build ships against the latest
    /// upstream release.
    ///
    /// Informational only. The Loader is embedded via FFI and ships
    /// inside GMM (ADR 0001), so it is not separately installable and
    /// a newer upstream Loader arrives through a GMM release — which
    /// ADR 0004 already governs under the "GMM itself" tier. See #78
    /// for why building a Loader install path was considered and
    /// rejected.
    ///
    /// Fetch failures are returned in
    /// [`updates::LoaderVersionStatus::check_error`] rather than
    /// swallowed: until #78 this used `.ok().flatten()`, which made a
    /// broken check look exactly like a healthy one.
    pub async fn check_loader_update(&self) -> Result<updates::LoaderVersionStatus> {
        self.check_loader_update_from(updates::LOADER_REPO, updates::LOADER_ASSET_PATTERN)
            .await
    }

    /// [`Core::check_loader_update`] against an explicit repo and
    /// asset pattern. Production always passes the
    /// [`updates::LOADER_REPO`] constants; tests use it to drive the
    /// failure path without depending on upstream being reachable.
    pub async fn check_loader_update_from(
        &self,
        repo: &str,
        asset_pattern: &str,
    ) -> Result<updates::LoaderVersionStatus> {
        let pattern = importer::AssetPattern::new(asset_pattern)?;
        let client = self.http_client().await?;
        let latest = importer::fetch_latest_release(
            &client,
            &importer::Endpoints::default(),
            repo,
            &pattern,
            None,
        )
        .await;

        let latest = match latest {
            // `None` means 304 Not Modified, which we can only get by
            // sending an ETag — we never do, so it is unreachable
            // here. Treat it as "nothing learned" rather than lying.
            Ok(Some(release)) => Ok(release.tag_name),
            Ok(None) => Err("upstream reported no change but GMM sent no ETag".to_string()),
            Err(e) => Err(e.to_string()),
        };
        Ok(updates::loader_status(latest))
    }

    /// Pin (or unpin) the per-game importer version. While pinned,
    /// the check still runs but the badge stays clear. Setting `None`
    /// clears the pin.
    pub async fn set_importer_pinned(&self, game: GameCode, version: Option<&str>) -> Result<()> {
        updates::set_importer_pinned(&self.pool, game, version).await
    }

    /// The per-game Importer Pin, or `None` when the game is unpinned.
    ///
    /// The stored value is the version the user is comfortable on, but
    /// [`updates::compute_status`] only ever asks whether *a* pin
    /// exists — which is exactly why an origin change has to delete it
    /// rather than carry it (#110).
    pub async fn importer_pinned(&self, game: GameCode) -> Result<Option<String>> {
        updates::importer_pinned(&self.pool, game).await
    }

    /// Persist the per-game installed importer tag. Production calls
    /// this from inside `install_importer` after a successful apply;
    /// integration tests can call it directly to seed state.
    pub async fn set_importer_installed(&self, game: GameCode, version: &str) -> Result<()> {
        updates::set_importer_installed(&self.pool, game, version).await
    }

    /// The per-game installed importer tag, if one was ever recorded.
    ///
    /// `None` means no install GMM performed — which is *not* the same
    /// as "no importer on disk". A hand-installed importer leaves this
    /// empty (#99), and that is exactly the unknown-origin case
    /// [`Core::installed_importer_origin`] reports.
    pub async fn installed_importer_version(&self, game: GameCode) -> Result<Option<String>> {
        updates::importer_installed(&self.pool, game).await
    }

    // ---- Importer Origin (ADR 0005 / #107) ----

    /// The user's per-game Importer Origin override (layer 1), if they
    /// have set one.
    ///
    /// `None` means **no override set**, which is an input to
    /// [`Core::resolve_importer_origin`] and must never be confused
    /// with [`OriginResolution::NoneInEffect`] ("no origin is in
    /// effect") or with [`InstalledOrigin::Unknown`].
    ///
    /// A stored value that no longer parses comes back as
    /// [`StoredOverride::Unreadable`], never as absence (#124). It used
    /// to be `.ok()`-ed into "no override", which silently dropped the
    /// game to a lower precedence layer — for a user who set an override
    /// *because* the default went bad, that is GMM quietly reinstating
    /// the package they moved away from. Resolution still warns rather
    /// than blocking; it simply refuses to answer a read failure by
    /// applying its own opinion.
    pub async fn importer_origin_override(
        &self,
        game: GameCode,
    ) -> Result<importer_origin::StoredOverride> {
        Self::importer_origin_override_in(&self.pool, game).await
    }

    /// [`Self::importer_origin_override`] against an arbitrary executor,
    /// so a transaction can resolve against the override it has just
    /// written rather than against the committed one (#122).
    async fn importer_origin_override_in<'e, E>(
        executor: E,
        game: GameCode,
    ) -> Result<importer_origin::StoredOverride>
    where
        E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
    {
        let raw = get_setting(executor, &importer_origin::keys::origin_override(game)).await?;
        let stored = importer_origin::StoredOverride::decode(raw);
        if let importer_origin::StoredOverride::Unreadable { raw, error } = &stored {
            // Logged like the cached-manifest path logs its parse
            // failures. Silence on this read is what let it go unnoticed.
            tracing::warn!(
                target: "gmm::importer_origin",
                game = game.as_str(),
                error = %error,
                raw = %raw,
                "stored Importer Origin override could not be read; no origin is \
                 in effect for this game until the user sets one again",
            );
        }
        Ok(stored)
    }

    /// Set or clear the user's per-game Importer Origin override.
    ///
    /// `None` clears it, returning the game to following layers 2 and 3
    /// — the same `Option` idiom as `set_library_path_for_game` and
    /// `set_importer_pinned`.
    ///
    /// Whatever this leaves in force, the game's Importer Pin and
    /// recorded install are reconciled against it before returning
    /// (#110). Clearing an override is as much an origin change as
    /// setting one — it can move the game onto a recommendation or back
    /// onto the compiled-in default — so both go through the same
    /// reconciliation rather than only the obvious half.
    pub async fn set_importer_origin_override(
        &self,
        game: GameCode,
        origin: Option<&importer_origin::ImporterOrigin>,
    ) -> Result<()> {
        let encoded =
            match origin {
                Some(o) => Some(serde_json::to_string(o).map_err(|e| {
                    Error::Importer(format!("could not encode Importer Origin: {e}"))
                })?),
                None => None,
            };
        let mut tx = self.pool.begin().await?;
        put_setting(
            &mut *tx,
            &importer_origin::keys::origin_override(game),
            encoded.as_deref(),
        )
        .await?;

        // Resolve *after* writing so the answer is the origin now in
        // force through all three layers, not the argument — and read it
        // through the transaction, so it sees the write above. With no
        // origin in effect there is nothing to have moved onto, so
        // nothing is invalidated.
        let resolution = Self::resolve_importer_origin_in(&mut tx, game).await?;
        if let Some(now_in_effect) = resolution.origin() {
            Self::reconcile_after_origin_change(&mut tx, game, now_in_effect).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Bring a game's Importer Pin and recorded install in line with an
    /// Importer Origin that is now in force (ADR 0005 / #110).
    ///
    /// Private and called from every path that can move a game onto a
    /// different origin, so no caller can change origin and leave
    /// either behind. Returns what it actually did.
    /// Runs on a connection rather than on the pool so every caller can
    /// wrap it, together with whatever moved the origin, in one
    /// transaction: clearing a pin for a move that then fails to land
    /// discards the user's ban-wave escape hatch for nothing (#122).
    async fn reconcile_after_origin_change(
        conn: &mut sqlx::SqliteConnection,
        game: GameCode,
        now_in_effect: &importer_origin::ImporterOrigin,
    ) -> Result<importer_origin::ChangeEffects> {
        let installed = Self::installed_importer_origin_in(&mut *conn, game).await?;
        let effects = importer_origin::change_effects(&installed, now_in_effect);

        if effects.clears_pin {
            updates::set_importer_pinned(&mut *conn, game, None).await?;
        }
        if effects.invalidates_install {
            put_setting(&mut *conn, &updates::keys::importer_installed(game), None).await?;
            put_setting(
                &mut *conn,
                &importer_origin::keys::installed_origin(game),
                None,
            )
            .await?;
        }
        Ok(effects)
    }

    /// Resolve a game's effective Importer Origin through ADR 0005's
    /// three layers.
    ///
    /// Layer 2 reads the **cached** manifest, never the network (#96):
    /// the last manifest GMM successfully fetched is authoritative until
    /// replaced, so resolution never flaps with connectivity and never
    /// waits on a third-party host.
    pub async fn resolve_importer_origin(
        &self,
        game: GameCode,
    ) -> Result<importer_origin::OriginResolution> {
        let mut conn = self.pool.acquire().await?;
        Self::resolve_importer_origin_in(&mut conn, game).await
    }

    /// [`Self::resolve_importer_origin`] against a specific connection,
    /// so a transaction resolves against its own uncommitted writes
    /// (#122).
    async fn resolve_importer_origin_in(
        conn: &mut sqlx::SqliteConnection,
        game: GameCode,
    ) -> Result<importer_origin::OriginResolution> {
        let user_override = Self::importer_origin_override_in(&mut *conn, game).await?;
        let manifest = Self::cached_recommended_manifest_in(&mut *conn).await?;
        // Both `None`s here mean *fall through*, which is the correct
        // behaviour for every one of them: no manifest cached yet, or a
        // cached manifest that says nothing about this game. Only an
        // explicit `Recommendation::NoRecommendation` retracts, and it
        // survives this expression intact.
        let recommendation = manifest.as_ref().and_then(|m| m.recommendation_for(game));
        let compiled = importer_origin::compiled_in_default(game);
        Ok(importer_origin::resolve(
            &user_override,
            recommendation.as_ref(),
            compiled.as_ref(),
        ))
    }

    /// The Importer Origin change GMM would propose for `game`, or
    /// `None` when it has nothing to propose.
    ///
    /// Reads the resolved origin and the installed one and nothing
    /// else. In particular it never reads the Importer Pin: a pinned
    /// game still gets told its origin is dead (#98). See
    /// [`importer_origin::pending_change`] for the rule.
    ///
    /// Nothing here applies anything — ADR 0005 is explicit that the
    /// manifest proposes and never auto-applies. The surface that shows
    /// the proposal, and remembers a decline, is #109.
    pub async fn pending_importer_origin_change(
        &self,
        game: GameCode,
    ) -> Result<Option<importer_origin::ImporterOrigin>> {
        let resolution = self.resolve_importer_origin(game).await?;
        let installed = self.installed_importer_origin(game).await?;
        Ok(importer_origin::pending_change(
            &resolution,
            &installed,
            importer_origin::compiled_in_default(game).as_ref(),
        )
        .cloned())
    }

    // ---- The recommendation surface (ADR 0005 / #109) ----

    /// Everything one game's Importer Origin surface needs, in one read.
    ///
    /// The reads are ordered so the aggregate is internally consistent:
    /// the proposal is computed from the same resolution and the same
    /// dismissal state that are reported alongside it, rather than from
    /// a second look at the database that could disagree with the first.
    pub async fn importer_origin_status(
        &self,
        game: GameCode,
    ) -> Result<importer_origin::OriginStatus> {
        let enabled = self.importer_recommendations_enabled().await?;
        let resolution = self.resolve_importer_origin(game).await?;
        let installed = self.installed_importer_origin(game).await?;
        let user_override = self.importer_origin_override(game).await?;
        let declines = self.importer_origin_dismissals(game).await?;
        let compiled_default = importer_origin::compiled_in_default(game);
        let install_target = importer_origin::origin_for_install(&installed, &resolution);

        let proposal = self
            .compute_importer_origin_proposal(game, &resolution, &installed, &declines, enabled)
            .await?;

        Ok(importer_origin::OriginStatus {
            game,
            display_name: game.profile().display_name.to_string(),
            install_target: (&install_target).into(),
            resolved: resolution,
            installed,
            user_override: (&user_override).into(),
            compiled_default,
            proposal,
            // While the layer is off there is no dismissed-recommendation
            // affordance either (#95). Offering to un-dismiss a proposal
            // that cannot be made would be a control with no effect.
            dismissed: if enabled {
                declines.origins().to_vec()
            } else {
                Vec::new()
            },
            dismissals_error: if enabled { declines.error() } else { None },
            recommendations_enabled: enabled,
            recommendations_unusable_reason: self.recommended_importers_unusable_reason().await?,
        })
    }

    /// The Importer Origin change GMM is currently offering for `game`,
    /// or `None`.
    pub async fn importer_origin_proposal(
        &self,
        game: GameCode,
    ) -> Result<Option<importer_origin::OriginProposal>> {
        let enabled = self.importer_recommendations_enabled().await?;
        let resolution = self.resolve_importer_origin(game).await?;
        let installed = self.installed_importer_origin(game).await?;
        let declines = self.importer_origin_dismissals(game).await?;
        self.compute_importer_origin_proposal(game, &resolution, &installed, &declines, enabled)
            .await
    }

    async fn compute_importer_origin_proposal(
        &self,
        game: GameCode,
        resolution: &importer_origin::OriginResolution,
        installed: &importer_origin::InstalledOrigin,
        declines: &importer_origin::StoredDeclines,
        enabled: bool,
    ) -> Result<Option<importer_origin::OriginProposal>> {
        // Off means no prompts at all (#95). Resolution has already
        // dropped the manifest layer, but a corrected compiled-in default
        // could still read as a change, and a user who has taken over
        // managing this themselves asked not to be told — ADR 0005
        // records that consequence and accepts it.
        if !enabled {
            return Ok(None);
        }
        let Some(origin) = importer_origin::pending_change(
            resolution,
            installed,
            importer_origin::compiled_in_default(game).as_ref(),
        ) else {
            return Ok(None);
        };
        if declines.suppresses(origin) {
            return Ok(None);
        }

        // The reason belongs to the manifest entry, and only when that
        // entry is what is being proposed: a proposal that comes from a
        // corrected compiled-in default has no reason to offer, and
        // borrowing the manifest's would attribute an explanation to the
        // wrong decision.
        let reason = match self
            .cached_recommended_manifest()
            .await?
            .and_then(|m| m.recommendation_for(game))
        {
            Some(importer_origin::Recommendation::Recommended {
                origin: recommended,
                reason,
            }) if &recommended == origin => reason,
            _ => None,
        };

        Ok(Some(importer_origin::OriginProposal {
            origin: origin.clone(),
            reason,
            replaces: installed.clone(),
        }))
    }

    /// The Importer Origins the user has declined for `game`.
    ///
    /// A stored list that cannot be read comes back as
    /// [`importer_origin::StoredDeclines::Unreadable`], never as an
    /// empty list — see that type for why the prompt is still shown.
    pub async fn importer_origin_dismissals(
        &self,
        game: GameCode,
    ) -> Result<importer_origin::StoredDeclines> {
        let raw = get_setting(&self.pool, &importer_origin::keys::declined_origins(game)).await?;
        let declines = importer_origin::StoredDeclines::decode(raw);
        if let importer_origin::StoredDeclines::Unreadable { raw, error } = &declines {
            tracing::warn!(
                target: "gmm::importer_origin",
                game = game.as_str(),
                error = %error,
                raw = %raw,
                "declined Importer Origins could not be read; proposals are shown \
                 again rather than silently suppressed",
            );
        }
        Ok(declines)
    }

    /// Decline an Importer Origin for `game`: remember it and stop
    /// proposing it.
    ///
    /// Scoped to the origin, so a later recommendation proposing a
    /// *different* one still reaches this user (#95). Nothing else
    /// changes — declining is a judgement about one proposal and must
    /// never escalate into the standing preference the off switch
    /// expresses.
    pub async fn dismiss_importer_origin(
        &self,
        game: GameCode,
        origin: &importer_origin::ImporterOrigin,
    ) -> Result<()> {
        let next = self.importer_origin_dismissals(game).await?.with(origin);
        self.put_importer_origin_dismissals(game, &next).await
    }

    /// Undo a dismissal, from the affected game's own surface.
    ///
    /// Dismissing is a one-click reflex; a dismissal that could not be
    /// undone and was never shown would let a user permanently silence
    /// the fix for their broken game with no trace (#95).
    pub async fn restore_importer_origin(
        &self,
        game: GameCode,
        origin: &importer_origin::ImporterOrigin,
    ) -> Result<()> {
        let next = self.importer_origin_dismissals(game).await?.without(origin);
        self.put_importer_origin_dismissals(game, &next).await
    }

    async fn put_importer_origin_dismissals(
        &self,
        game: GameCode,
        origins: &[importer_origin::ImporterOrigin],
    ) -> Result<()> {
        let encoded = serde_json::to_string(origins).map_err(|e| {
            Error::Importer(format!("could not encode declined Importer Origins: {e}"))
        })?;
        put_setting(
            &self.pool,
            &importer_origin::keys::declined_origins(game),
            Some(&encoded),
        )
        .await
    }

    /// Accept the Importer Origin change GMM is offering for `game`:
    /// install from the proposed origin.
    ///
    /// This is the **only** way an existing install's origin changes
    /// (#109), and — through the install it performs — the only way an
    /// unknown origin becomes known (#99). Recording an origin *without*
    /// installing was explicitly rejected: it books an origin and a
    /// version for files GMM has never seen, and every later decision
    /// then trusts the fiction. A user who wants their existing files
    /// left alone declines.
    ///
    /// It goes through the ordinary install path, so the game directory
    /// is backed up and the move is rollbackable like any other importer
    /// install.
    pub async fn accept_importer_origin_proposal(
        &self,
        game: GameCode,
    ) -> Result<importer::InstallReport> {
        self.accept_importer_origin_proposal_with_endpoints(game, &importer::Endpoints::default())
            .await
    }

    /// Test seam for [`Core::accept_importer_origin_proposal`].
    pub async fn accept_importer_origin_proposal_with_endpoints(
        &self,
        game: GameCode,
        endpoints: &importer::Endpoints,
    ) -> Result<importer::InstallReport> {
        let proposal = self.importer_origin_proposal(game).await?.ok_or_else(|| {
            Error::Importer(format!(
                "GMM has no Importer Origin change to apply for {}.",
                game.profile().display_name,
            ))
        })?;
        self.install_importer_from(game, &proposal.origin, endpoints)
            .await
    }

    // ---- The recommended-importers manifest (ADR 0005 / #108) ----

    /// The cached manifest, or `None` when GMM holds none it can read.
    ///
    /// `None` is deliberately *fall-through*, never retraction: with no
    /// usable cache the whole layer is absent and every game resolves to
    /// its compiled-in default. That is the behaviour #93 requires for a
    /// document the build cannot make sense of — landing on retraction
    /// instead would let one bad commit clear every default for every
    /// user.
    ///
    /// A cached document that no longer parses is only reachable by
    /// downgrading GMM, since nothing is cached without parsing first.
    /// It is logged rather than swallowed.
    pub async fn cached_recommended_manifest(
        &self,
    ) -> Result<Option<recommended_importers::Manifest>> {
        let mut conn = self.pool.acquire().await?;
        Self::cached_recommended_manifest_in(&mut conn).await
    }

    /// [`Self::cached_recommended_manifest`] against a specific
    /// connection, so a transaction reads the layer as it will be after
    /// its own uncommitted writes (#122).
    ///
    /// A connection rather than a generic executor because it makes two
    /// reads — the off switch and the cached document — and both have to
    /// come from the same place, in that order.
    async fn cached_recommended_manifest_in(
        conn: &mut sqlx::SqliteConnection,
    ) -> Result<Option<recommended_importers::Manifest>> {
        // The second half of the off switch, and the half that is easy
        // to leave out (#95). Gating only the fetch looks correct on a
        // machine that has never launched online and does nothing at all
        // on every machine that has, because the cache is written on the
        // first successful launch and stays authoritative afterwards.
        // A retraction that outlived being switched off would go on
        // clearing the user's compiled-in default with no visible cause
        // — the exact "invisible behaviour change" the switch exists to
        // prevent.
        //
        // It belongs *here* rather than at each call site because this
        // is the single door onto layer 2: anything that reads the
        // manifest reads it through this function, so the precondition
        // is structural rather than a rule every future caller has to
        // remember.
        if !Self::importer_recommendations_enabled_in(&mut *conn).await? {
            return Ok(None);
        }
        let Some(raw) =
            get_setting(&mut *conn, recommended_importers::cache_keys::DOCUMENT).await?
        else {
            return Ok(None);
        };
        match recommended_importers::parse(&raw) {
            Ok(manifest) => Ok(Some(manifest)),
            Err(e) => {
                tracing::warn!(
                    target: "gmm::recommendations",
                    error = %e,
                    "cached recommended-importers manifest no longer parses; \
                     falling through to compiled-in defaults",
                );
                Ok(None)
            }
        }
    }

    /// Why the last refresh produced a manifest this build cannot read,
    /// or `None` when it did not.
    ///
    /// This is the signal behind "your build is too old". It is kept
    /// separate from the layer itself because the layer's response to an
    /// unusable document is to fall through — which on its own is
    /// indistinguishable from having no manifest at all, and that
    /// silence is precisely the #78 defect.
    ///
    /// Gated by the off switch like the rest of the layer: a user who
    /// has switched recommendations off is not shown a complaint about a
    /// file GMM is no longer allowed to read.
    pub async fn recommended_importers_unusable_reason(&self) -> Result<Option<String>> {
        if !self.importer_recommendations_enabled().await? {
            return Ok(None);
        }
        get_setting(
            &self.pool,
            recommended_importers::cache_keys::UNUSABLE_REASON,
        )
        .await
    }

    /// Whether GMM's curated recommendations apply at all (#95).
    ///
    /// **On** for a user who has never touched it: this is an opt-out,
    /// and the users layer 2 exists to rescue — stranded on a dead
    /// importer — are exactly the ones who will never go looking for a
    /// switch.
    pub async fn importer_recommendations_enabled(&self) -> Result<bool> {
        Self::importer_recommendations_enabled_in(&self.pool).await
    }

    async fn importer_recommendations_enabled_in<'e, E>(executor: E) -> Result<bool>
    where
        E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
    {
        Ok(
            get_setting(executor, recommended_importers::cache_keys::ENABLED)
                .await?
                .map(|v| v != "false")
                .unwrap_or(true),
        )
    }

    /// Switch GMM's curated recommendations on or off.
    ///
    /// Off removes the **whole layer** — no fetch, no cache read, no
    /// retraction, no prompts. It deliberately does **not** delete the
    /// cached document: the switch is a standing preference, not a
    /// destructive act, and a user who switches it back on should get
    /// the layer they already have rather than waiting for a fetch that,
    /// for the offline user this mechanism exists to rescue, may never
    /// land.
    ///
    /// It also does not touch dismissals. Turning the layer off and back
    /// on must not resurrect previously declined proposals as fresh
    /// prompts, or toggling the switch twice becomes a way to spam
    /// yourself (#95).
    pub async fn set_importer_recommendations_enabled(&self, enabled: bool) -> Result<()> {
        put_setting(
            &self.pool,
            recommended_importers::cache_keys::ENABLED,
            Some(if enabled { "true" } else { "false" }),
        )
        .await
    }

    /// Refresh the cached manifest from
    /// [`recommended_importers::MANIFEST_URL`].
    ///
    /// Background best-effort, once per app start. **Nothing waits on
    /// it**: the cached manifest is already in force, and a refresh only
    /// applies when it lands (#96). A failure has no user-visible
    /// consequence and is logged, not surfaced.
    ///
    /// This is the single entry point to the fetch, so the "off switch"
    /// precondition can be added in front of it rather than as a filter
    /// on its result — off must mean *no request at all*.
    pub async fn refresh_recommended_importers(&self) -> Result<recommended_importers::Refreshed> {
        self.refresh_recommended_importers_from(recommended_importers::MANIFEST_URL)
            .await
    }

    /// [`Core::refresh_recommended_importers`] against an explicit URL.
    ///
    /// Production always passes the constant; tests use this to drive
    /// every outcome without depending on GitHub being reachable — the
    /// same seam [`Core::check_loader_update_from`] provides. This path
    /// deliberately follows up to ten redirects because raw-content hosting
    /// may redirect; the loopback override has its own refusing entry point.
    pub async fn refresh_recommended_importers_from(
        &self,
        url: &str,
    ) -> Result<recommended_importers::Refreshed> {
        self.refresh_recommended_importers_with_redirects(url, ManifestRedirects::FollowShippedUrl)
            .await
    }

    /// Refresh through the packaged startup smoke's URL after it has passed
    /// the numeric-loopback validator. Redirects are refused so a local
    /// response cannot escape that validation by naming an internet target.
    pub async fn refresh_recommended_importers_from_loopback_override(
        &self,
        url: &str,
    ) -> Result<recommended_importers::Refreshed> {
        self.refresh_recommended_importers_with_redirects(
            url,
            ManifestRedirects::RefuseLoopbackOverride,
        )
        .await
    }

    async fn refresh_recommended_importers_with_redirects(
        &self,
        url: &str,
        redirects: ManifestRedirects,
    ) -> Result<recommended_importers::Refreshed> {
        // A **precondition on the fetch**, not a filter on its result
        // (#95). A user running their own importer gets no startup
        // network call about importers at all — and returning before the
        // client is even built is what makes that testable as "zero
        // requests" rather than as "we ignored the answer".
        if !self.importer_recommendations_enabled().await? {
            return Ok(recommended_importers::Refreshed::Disabled);
        }
        let redirect_policy = match redirects {
            // Keep redirects for the permanent raw-content URL: its hosting
            // may legitimately redirect, and refusing that would make a
            // provider detail take the recommendation layer offline.
            ManifestRedirects::FollowShippedUrl => reqwest::redirect::Policy::limited(10),
            ManifestRedirects::RefuseLoopbackOverride => reqwest::redirect::Policy::none(),
        };
        let client = self
            .http_client_builder()
            .await?
            .redirect(redirect_policy)
            .timeout(recommended_importers::FETCH_TIMEOUT)
            .build()
            .map_err(|e| Error::Network(format!("client build: {e}")))?;

        // Only claim to still hold a document when one is actually
        // cached. A conditional request whose 304 we could not act on
        // would turn "unchanged" into "nothing learned".
        let cached_raw =
            get_setting(&self.pool, recommended_importers::cache_keys::DOCUMENT).await?;
        let etag = match cached_raw {
            Some(_) => get_setting(&self.pool, recommended_importers::cache_keys::ETAG).await?,
            None => None,
        };

        match recommended_importers::fetch(&client, url, etag.as_deref()).await {
            recommended_importers::Fetched::Document { raw, etag } => {
                match recommended_importers::parse(&raw) {
                    Ok(manifest) => {
                        self.store_recommended_manifest(&raw, etag.as_deref())
                            .await?;
                        Ok(recommended_importers::Refreshed::Replaced(manifest))
                    }
                    Err(e) => {
                        // The cache is untouched: authoritative until
                        // *replaced*, and an unreadable document
                        // replaces nothing.
                        tracing::warn!(
                            target: "gmm::recommendations",
                            error = %e,
                            "recommended-importers manifest is unusable by this build",
                        );
                        put_setting(
                            &self.pool,
                            recommended_importers::cache_keys::UNUSABLE_REASON,
                            Some(&e.to_string()),
                        )
                        .await?;
                        Ok(recommended_importers::Refreshed::Unusable(e))
                    }
                }
            }
            recommended_importers::Fetched::NotModified => {
                Ok(recommended_importers::Refreshed::NotModified)
            }
            recommended_importers::Fetched::Unreachable(message) => {
                tracing::warn!(
                    target: "gmm::recommendations",
                    error = %message,
                    "recommended-importers refresh failed; the cached manifest stays in force",
                );
                Ok(recommended_importers::Refreshed::Unreachable(message))
            }
        }
    }

    /// Replace the cached manifest with `raw`, which the caller has
    /// already parsed successfully.
    ///
    /// Writing the document before the ETag matters: a crash between the
    /// two costs one full download next launch, whereas the other order
    /// would leave an ETag describing a document GMM does not hold and
    /// a 304 it could not act on.
    async fn store_recommended_manifest(&self, raw: &str, etag: Option<&str>) -> Result<()> {
        put_setting(
            &self.pool,
            recommended_importers::cache_keys::DOCUMENT,
            Some(raw),
        )
        .await?;
        put_setting(&self.pool, recommended_importers::cache_keys::ETAG, etag).await?;
        // A readable document clears the "your build is too old" state:
        // whatever it was complaining about is no longer what GMM holds.
        put_setting(
            &self.pool,
            recommended_importers::cache_keys::UNUSABLE_REASON,
            None,
        )
        .await
    }

    /// The Importer Origin the current install was performed from.
    ///
    /// Absent reads back as [`importer_origin::InstalledOrigin::Unknown`]
    /// — a real, first-class state (#99), never backfilled to the
    /// compiled-in default and never treated as "not installed". A value
    /// that is present and unreadable comes back as
    /// [`importer_origin::InstalledOrigin::Unreadable`] instead (#124):
    /// it makes the opposite claim about the machine and must not be
    /// folded into `Unknown`.
    pub async fn installed_importer_origin(
        &self,
        game: GameCode,
    ) -> Result<importer_origin::InstalledOrigin> {
        Self::installed_importer_origin_in(&self.pool, game).await
    }

    /// [`Self::installed_importer_origin`] against an arbitrary
    /// executor, so the recording transaction reads the origin it is
    /// about to replace from inside that transaction rather than from a
    /// separate connection (#122).
    async fn installed_importer_origin_in<'e, E>(
        executor: E,
        game: GameCode,
    ) -> Result<importer_origin::InstalledOrigin>
    where
        E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
    {
        let raw = get_setting(executor, &importer_origin::keys::installed_origin(game)).await?;
        let installed = importer_origin::InstalledOrigin::decode(raw);
        if let importer_origin::InstalledOrigin::Unreadable { raw, error } = &installed {
            tracing::warn!(
                target: "gmm::importer_origin",
                game = game.as_str(),
                error = %error,
                raw = %raw,
                "recorded Importer Origin could not be read; treated as an \
                 unreadable record, never as a hand-install GMM never performed",
            );
        }
        Ok(installed)
    }

    /// Record a completed importer install: the version *and* the
    /// origin it came from.
    ///
    /// This is the only way an unknown origin becomes known (#99).
    /// Recording an origin without an actual install was explicitly
    /// rejected — it would assert both an origin and a version for
    /// files GMM has never seen.
    ///
    /// Being the only writer of the installed origin makes this the
    /// chokepoint for the other half of #110: accepting a recommended
    /// origin is an origin change, so any Importer Pin taken against
    /// the origin being replaced is cleared here. A version update from
    /// the *same* origin is not a change and leaves the pin alone.
    /// The pin reconciliation, the recorded origin and the recorded
    /// version are **one transaction** (#122). They describe a single
    /// install, and any subset of them is state no later decision can
    /// read correctly: pin clearing compares origins, the update badge
    /// compares versions, and the recommendation logic reads the
    /// recorded origin to decide whether it is proposing a change. A
    /// half-written install is worse than an unwritten one, because the
    /// unwritten one is still internally consistent.
    pub async fn record_importer_install(
        &self,
        game: GameCode,
        version: &str,
        origin: &importer_origin::ImporterOrigin,
    ) -> Result<()> {
        let encoded = serde_json::to_string(origin)
            .map_err(|e| Error::Importer(format!("could not encode Importer Origin: {e}")))?;

        let mut tx = self.pool.begin().await?;
        // Reconcile before writing: the comparison needs the origin the
        // install being replaced came from. Invalidating the record is
        // moot here — the two writes below replace it with a fresher one
        // either way — but the pin has to go before the new state lands.
        Self::reconcile_after_origin_change(&mut tx, game, origin).await?;
        put_setting(
            &mut *tx,
            &importer_origin::keys::installed_origin(game),
            Some(&encoded),
        )
        .await?;
        updates::set_importer_installed(&mut *tx, game, version).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Which Importer Origin an ordinary Install / Update acts on for
    /// `game` (#109).
    ///
    /// See [`importer_origin::origin_for_install`] for the rule. In
    /// short: a recommendation decides an install that does not exist
    /// yet, and never switches one that does.
    pub async fn importer_origin_for_install(
        &self,
        game: GameCode,
    ) -> Result<importer_origin::InstallOrigin> {
        let resolution = self.resolve_importer_origin(game).await?;
        let installed = self.installed_importer_origin(game).await?;
        Ok(importer_origin::origin_for_install(&installed, &resolution))
    }

    /// The Importer Origin an ordinary Install / Update acts on for
    /// `game`, plus its compiled asset pattern — or an error when there
    /// is none to act on.
    ///
    /// Lives here rather than in the Tauri command layer so the install
    /// path is one testable unit (#122). Both error arms are real
    /// errors: neither "nothing is in effect" nor "GMM cannot read what
    /// this install came from" may be answered by quietly installing
    /// something else.
    pub async fn resolved_importer_origin(
        &self,
        game: GameCode,
    ) -> Result<(importer_origin::ImporterOrigin, importer::AssetPattern)> {
        let origin = match self.importer_origin_for_install(game).await? {
            importer_origin::InstallOrigin::Installed(origin) => origin,
            importer_origin::InstallOrigin::Resolved { origin, .. } => origin,
            importer_origin::InstallOrigin::NoneInEffect { reason } => {
                return Err(Error::NoImporterOriginInEffect {
                    game: game.profile().display_name.to_string(),
                    reason: reason.map(|r| format!(" {r}")).unwrap_or_default(),
                })
            }
            importer_origin::InstallOrigin::InstalledUnreadable { error, .. } => {
                return Err(Error::InstalledImporterOriginUnreadable {
                    game: game.profile().display_name.to_string(),
                    message: error,
                })
            }
        };
        let pattern = importer::AssetPattern::new(origin.asset_pattern())?;
        Ok((origin, pattern))
    }

    /// The latest upstream release for the game's resolved Importer
    /// Origin.
    pub async fn latest_importer_release(
        &self,
        game: GameCode,
    ) -> Result<Option<importer::LatestRelease>> {
        let (origin, pattern) = self.resolved_importer_origin(game).await?;
        let client = self.http_client().await?;
        importer::fetch_latest_release(
            &client,
            &importer::Endpoints::default(),
            &origin.repo_slug(),
            &pattern,
            None,
        )
        .await
    }

    /// Download and install the latest Model Importer for `game` from
    /// its resolved Importer Origin, then record what was installed.
    ///
    /// The whole path — resolve, fetch, download, unpack, record — lives
    /// on `Core` so a test can drive the same code production runs.
    /// While it lived in the Tauri command the only thing a test could
    /// assert about it was that a function name appeared in the source
    /// file, which is how the discarded `record_importer_install` result
    /// survived review (#122).
    pub async fn install_importer(&self, game: GameCode) -> Result<importer::InstallReport> {
        self.install_importer_with_endpoints(game, &importer::Endpoints::default())
            .await
    }

    /// Test seam for [`Self::install_importer`] — production uses the
    /// `Endpoints::default()` overload.
    pub async fn install_importer_with_endpoints(
        &self,
        game: GameCode,
        endpoints: &importer::Endpoints,
    ) -> Result<importer::InstallReport> {
        let (origin, _) = self.resolved_importer_origin(game).await?;
        self.install_importer_from(game, &origin, endpoints).await
    }

    /// Install `game`'s Model Importer from an **explicitly chosen**
    /// Importer Origin.
    ///
    /// The origin is a parameter rather than something this function
    /// resolves, because the two callers choose it by different rules
    /// and that difference is the whole of #109. The ordinary Install /
    /// Update action passes what
    /// [`Core::importer_origin_for_install`] decided — the origin the
    /// install came from, never a substitute. Accepting a proposal
    /// passes the proposed origin, which is the one act that is allowed
    /// to move an existing install.
    async fn install_importer_from(
        &self,
        game: GameCode,
        origin: &importer_origin::ImporterOrigin,
        endpoints: &importer::Endpoints,
    ) -> Result<importer::InstallReport> {
        let origin = origin.clone();
        self.ensure_no_active_session().await?;
        let install = self.game_install_path(game).await?.ok_or_else(|| {
            Error::Importer(format!(
                "set {}'s install path in Settings before installing its Model Importer",
                game.profile().display_name,
            ))
        })?;
        let pattern = importer::AssetPattern::new(origin.asset_pattern())?;

        let client = self.http_client().await?;
        let release =
            importer::fetch_latest_release(&client, endpoints, &origin.repo_slug(), &pattern, None)
                .await?
                .ok_or_else(|| {
                    Error::ReleaseMetadata(format!(
                        "no release returned for {}",
                        origin.repo_slug()
                    ))
                })?;

        let data = self.data_dir();
        let backups_root = data.join("backups").join(game.as_str());
        let zip_path = data
            .join("downloads")
            .join(game.as_str())
            .join(&release.asset_name);
        importer::download_to(&client, &release.asset_url, &zip_path).await?;

        // What is about to be replaced, captured *before* the swap. A
        // backup is a pile of files and carries no provenance of its
        // own, so without this a rollback can only restore the files
        // and has to leave the record describing the install it just
        // undid (#126). Either field may legitimately be `None` — an
        // install over a hand-installed setup replaces files GMM never
        // recorded, and unknown is the honest answer for those.
        let replacing = importer::BackupProvenance {
            version: self.installed_importer_version(game).await?,
            origin: match self.installed_importer_origin(game).await? {
                importer_origin::InstalledOrigin::Known(origin) => Some(origin),
                importer_origin::InstalledOrigin::Unknown
                | importer_origin::InstalledOrigin::Unreadable { .. } => None,
            },
        };

        let report = tokio::task::spawn_blocking(move || {
            importer::install_from_local_zip(
                &zip_path,
                &install,
                &backups_root,
                importer::DEFAULT_LOADER_EXE,
            )
        })
        .await
        .map_err(|e| Error::Importer(format!("install task join error: {e}")))??;

        if let Some(backup_dir) = report.backup_dir.as_deref() {
            importer::write_backup_provenance(backup_dir, &replacing)?;
        }

        // Record the installed tag *and* the Importer Origin it came
        // from, so the update check can compare against it next launch
        // and so origin changes are detectable (ADR 0005). This is the
        // only way an unknown origin becomes known (#99).
        //
        // A failure here is **not** best-effort. The files are on disk
        // and GMM's record of them is not, which is a state the caller
        // has to be told about — reporting "Installed" over it is the
        // defect #122 fixed.
        self.record_importer_install(game, &release.tag_name, &origin)
            .await
            .map_err(|e| Error::ImporterInstallNotRecorded {
                game: game.profile().display_name.to_string(),
                version: release.tag_name.clone(),
                message: e.to_string(),
            })?;

        Ok(report)
    }

    /// Restore the game's Model Importer from its most recent backup,
    /// and bring GMM's record of what is installed back in line with it.
    ///
    /// Returns the backup that was restored, or `None` when there is
    /// nothing to roll back to.
    ///
    /// The record half is the point of #126. Restoring only the files
    /// left the database describing the install that had just been
    /// undone — and since the recorded origin drives pin clearing and
    /// the change-proposal logic, rolling back an origin switch left GMM
    /// convinced the switch had happened and with nothing to propose.
    pub async fn rollback_importer(&self, game: GameCode) -> Result<Option<PathBuf>> {
        self.ensure_no_active_session().await?;
        let install = self.game_install_path(game).await?.ok_or_else(|| {
            Error::Importer(format!(
                "set {}'s install path in Settings before rolling back its Model Importer",
                game.profile().display_name,
            ))
        })?;
        let backups_root = self.data_dir().join("backups").join(game.as_str());
        let Some(latest) = importer::latest_backup(&backups_root)? else {
            return Ok(None);
        };

        // Read the provenance before the restore: it sits beside the
        // backup rather than inside it, but reading first keeps the
        // ordering obvious.
        let provenance = importer::read_backup_provenance(&latest);

        let backup_for_blocking = latest.clone();
        tokio::task::spawn_blocking(move || importer::rollback_to(&backup_for_blocking, &install))
            .await
            .map_err(|e| Error::Importer(format!("rollback task join error: {e}")))??;

        self.restore_install_record(game, provenance.as_ref())
            .await
            .map_err(|e| Error::RollbackNotRecorded {
                game: game.profile().display_name.to_string(),
                backup: latest.display().to_string(),
                message: e.to_string(),
            })?;

        Ok(Some(latest))
    }

    /// Set the recorded install to what a backup's provenance says it
    /// was — or to unknown when there is no provenance to read.
    ///
    /// One transaction, for the same reason [`Self::record_importer_install`]
    /// is one: the version, the origin and the pin describe a single
    /// install and any subset of them is state no later decision can
    /// read correctly (#122).
    async fn restore_install_record(
        &self,
        game: GameCode,
        provenance: Option<&importer::BackupProvenance>,
    ) -> Result<()> {
        let version = provenance.and_then(|p| p.version.clone());
        let origin = provenance.and_then(|p| p.origin.clone());
        let encoded =
            match origin.as_ref() {
                Some(o) => Some(serde_json::to_string(o).map_err(|e| {
                    Error::Importer(format!("could not encode Importer Origin: {e}"))
                })?),
                None => None,
            };

        let mut tx = self.pool.begin().await?;
        match origin.as_ref() {
            // A rollback is an origin move like any other, so the same
            // reconciliation decides what the previous origin's state
            // meant. Rolling back a version within one origin leaves the
            // pin alone; rolling back across origins clears it.
            Some(o) => {
                Self::reconcile_after_origin_change(&mut tx, game, o).await?;
            }
            // Nothing to compare against, so the pin is a gate GMM can no
            // longer reason about — the same call `InstalledOrigin::Unknown`
            // makes in `change_effects`.
            None => updates::set_importer_pinned(&mut *tx, game, None).await?,
        }
        put_setting(
            &mut *tx,
            &importer_origin::keys::installed_origin(game),
            encoded.as_deref(),
        )
        .await?;
        put_setting(
            &mut *tx,
            &updates::keys::importer_installed(game),
            version.as_deref(),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// GMM's app-data directory — the Library root's parent, which is
    /// exactly how `build_core` lays it out. Derived rather than read
    /// from `crate::data_dir()` so an integration test's temp directory
    /// carries the downloads and backups too.
    fn data_dir(&self) -> PathBuf {
        self.default_library_root
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    // `set_loader_installed` lived here until #78. It never had a
    // caller — the Loader is embedded, not installed, so nothing
    // could ever have had a version to record.

    /// Resolve a GameBanana submission (URL or bare ID), download its
    /// first `.zip` asset, ingest it through the slice-1b zip path, and
    /// persist `source = gamebanana` plus the upstream metadata
    /// (author, version, screenshot URL, source URL) on the new mod
    /// row. The async HTTP path uses [`Core::http_client`] so the
    /// network goes through the user's proxy.
    pub async fn import_gamebanana(&self, game: GameCode, url_or_id: &str) -> Result<Mod> {
        self.import_gamebanana_with_endpoints(game, url_or_id, &gamebanana::Endpoints::default())
            .await
    }

    /// Test seam for [`Self::import_gamebanana`] — production uses the
    /// `Endpoints::default()` overload. Integration tests inject a
    /// mockito server URL through this entry point.
    pub async fn import_gamebanana_with_endpoints(
        &self,
        game: GameCode,
        url_or_id: &str,
        endpoints: &gamebanana::Endpoints,
    ) -> Result<Mod> {
        self.ensure_no_active_session().await?;
        let id = gamebanana::parse_url_or_id(url_or_id).ok_or_else(|| {
            Error::GameBanana(format!("could not parse GameBanana URL or ID: {url_or_id}"))
        })?;

        let client = self.http_client().await?;
        let submission = gamebanana::fetch_submission(&client, endpoints, id).await?;

        // Stash the download in a Library-adjacent cache (the same
        // data_dir tree the diagnostics + importer modules use) so
        // it's easy to inspect / wipe.
        let cache = self
            .default_library_root
            .parent()
            .map(|p| p.join("downloads").join("gamebanana"))
            .unwrap_or_else(|| std::path::PathBuf::from("./downloads/gamebanana"));
        std::fs::create_dir_all(&cache).map_err(|source| Error::Io {
            path: cache.clone(),
            source,
        })?;
        let zip_path = cache.join(format!("{}-{}", id, submission.file_name));
        gamebanana::download_to(&client, &submission.file_url, &zip_path).await?;

        // Reuse the slice-1b ingest path verbatim; that gives us
        // zip-slip protection, junk-file drop, single-root
        // normalisation, plus the variant detection from slice 5.
        let mut imported = self
            .import_zip(
                game,
                &zip_path,
                &submission.name,
                ImportZipOptions::default(),
            )
            .await?;

        // Rewrite the row to GameBanana provenance.
        sqlx::query(
            "UPDATE mods
               SET source = ?,
                   gamebanana_id = ?,
                   source_url = ?,
                   author = ?,
                   version = ?,
                   screenshot_url = ?
             WHERE id = ?",
        )
        .bind(Source::Gamebanana.as_str())
        .bind(id as i64)
        .bind(&submission.profile_url)
        .bind(&submission.author)
        .bind(&submission.version)
        .bind(&submission.screenshot_url)
        .bind(&imported.id)
        .execute(&self.pool)
        .await?;

        imported.source = Source::Gamebanana;
        imported.gamebanana_id = Some(id);
        imported.source_url = Some(submission.profile_url);
        imported.author = submission.author;
        imported.version = submission.version;
        imported.screenshot_url = submission.screenshot_url;

        Ok(imported)
    }

    /// Import a local ZIP into the Library as a Mod with `source = local`.
    ///
    /// Hardened against the dirty realities of GameBanana-style archives:
    /// zip-slip path traversal, `__MACOSX/` / `.DS_Store` / `Thumbs.db`
    /// junk files, single-root-directory shape, and size/entry caps. See
    /// [`crate::core::zip_import`] for the extraction details.
    ///
    /// On failure, cleanup removes the staged Library directory only after
    /// re-proving its identity and that no Mod row owns it. If either fact is
    /// uncertain, the bytes are retained for the orphan audit to surface.
    pub async fn import_zip(
        &self,
        game: GameCode,
        zip_path: &Path,
        display_name: &str,
        opts: ImportZipOptions,
    ) -> Result<Mod> {
        let id = Ulid::new().to_string();
        let (root, staged) = self
            .create_staged_library_directory(
                game,
                &id,
                library_mutation::LibraryMutation::ImportZip,
            )
            .await?;
        let library_path = staged.path().to_path_buf();

        if let Err(e) = zip_import::extract(zip_path, &library_path, opts) {
            // Best-effort cleanup. We swallow remove_dir_all errors so the
            // user sees the original extraction failure, not a noisy
            // cleanup follow-up.
            self.cleanup_staged_library_dir(
                &root,
                staged,
                library_mutation::LibraryMutation::ImportZip,
            )
            .await;
            return Err(e);
        }
        self.crash_point(crash_points::IMPORT_ZIP_AFTER_EXTRACT);

        // Keep recursive Variant detection outside the Library writer fence.
        // The Mod row, detected Variant rows, and active selection are staged
        // later in one bounded database transaction.
        let detected_variants = match variants::detect_variants(&library_path) {
            Ok(detected) => detected,
            Err(error) => {
                self.cleanup_staged_library_dir(
                    &root,
                    staged,
                    library_mutation::LibraryMutation::ImportZip,
                )
                .await;
                return Err(error);
            }
        };

        self.commit_staged_mod(
            root,
            staged,
            game,
            &id,
            display_name,
            Source::Local,
            &library_path,
            detected_variants,
            library_mutation::LibraryMutation::ImportZip,
            crash_points::IMPORT_ZIP_AFTER_ROW_INSERT,
            crash_points::IMPORT_ZIP_AFTER_FENCE_COMMIT,
        )
        .await?;

        Ok(Mod {
            id,
            game,
            name: display_name.to_string(),
            source: Source::Local,
            library_path,
            enabled: false,
            gamebanana_id: None,
            source_url: None,
            author: None,
            version: None,
            screenshot_url: None,
            reinstall_recovery: None,
        })
    }

    /// Revalidate a witnessed stage and commit its Mod/Variant state together
    /// with witness retirement. Every returned database error rolls the fence
    /// back before entering the same identity-checked cleanup used by copy and
    /// detection failures.
    #[allow(clippy::too_many_arguments)]
    async fn commit_staged_mod(
        &self,
        root: library_mutation::LibraryRootSnapshot,
        staged: library_mutation::StagedLibraryDirectory,
        game: GameCode,
        id: &str,
        display_name: &str,
        source: Source,
        library_path: &Path,
        detected_variants: Vec<variants::DetectedVariant>,
        mutation: library_mutation::LibraryMutation,
        after_row_insert: &'static str,
        after_commit: &'static str,
    ) -> Result<()> {
        let mut fence = match self
            .revalidate_library_root_for_mutation(&root, mutation)
            .await
        {
            Ok(fence) => fence,
            Err(error) => {
                self.cleanup_staged_library_dir(&root, staged, mutation)
                    .await;
                return Err(error);
            }
        };
        let staged_write = async {
            let base = sanitize_dir_name(display_name);
            let junction_dir_name =
                library_mutation::unique_junction_dir_name(&mut fence.transaction, game, &base)
                    .await?;
            sqlx::query(
                "INSERT INTO mods (
                    id, game_code, name, source, library_path,
                    junction_dir_name, enabled, created_at
                 )
                 VALUES (?, ?, ?, ?, ?, ?, 0, ?)",
            )
            .bind(id)
            .bind(game.as_str())
            .bind(display_name)
            .bind(source.as_str())
            .bind(library_path.to_string_lossy().as_ref())
            .bind(&junction_dir_name)
            .bind(Utc::now().to_rfc3339())
            .execute(&mut *fence.transaction)
            .await?;

            // This seam is deliberately inside the transaction: a process
            // death here must roll back the Mod row instead of exposing it
            // without its detected Variants and active selection.
            self.crash_point(after_row_insert);
            self.record_detected_variants(id, detected_variants, &mut fence.transaction)
                .await?;
            self.retire_staging_witness_for_commit(&staged, &mut fence)
                .await?;
            Ok(())
        }
        .await;
        if let Err(error) = staged_write {
            let _ = fence.transaction.rollback().await;
            self.cleanup_staged_library_dir(&root, staged, mutation)
                .await;
            return Err(error);
        }
        if let Err(error) = fence.commit().await {
            self.cleanup_staged_library_dir(&root, staged, mutation)
                .await;
            return Err(error);
        }
        self.crash_point(after_commit);
        Ok(())
    }

    /// Persist an already-detected Variant set and its initial active choice
    /// in the caller's transaction. Recovery shares this with ordinary adopt
    /// and import so all three paths record the same shape while recovery can
    /// include detection in its existing Mod-row transaction.
    async fn record_detected_variants(
        &self,
        mod_id: &str,
        detected: Vec<variants::DetectedVariant>,
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<()> {
        if detected.is_empty() {
            return Ok(());
        }

        let mut first_variant_id: Option<String> = None;
        for v in detected {
            let variant_id = Ulid::new().to_string();
            sqlx::query("INSERT INTO mod_variants (id, mod_id, name, subpath) VALUES (?, ?, ?, ?)")
                .bind(&variant_id)
                .bind(mod_id)
                .bind(&v.name)
                .bind(v.subpath.to_string_lossy().as_ref())
                .execute(&mut **transaction)
                .await?;
            if first_variant_id.is_none() {
                first_variant_id = Some(variant_id);
            }
        }

        sqlx::query("UPDATE mods SET active_variant_id = ? WHERE id = ?")
            .bind(&first_variant_id)
            .bind(mod_id)
            .execute(&mut **transaction)
            .await?;
        Ok(())
    }

    /// List the Variants stored for `mod_id` (empty when none).
    pub async fn list_variants(&self, mod_id: &str) -> Result<Vec<variants::Variant>> {
        let rows = sqlx::query(
            "SELECT id, mod_id, name, subpath FROM mod_variants WHERE mod_id = ? ORDER BY name ASC",
        )
        .bind(mod_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(variants::Variant {
                    id: row.try_get("id")?,
                    mod_id: row.try_get("mod_id")?,
                    name: row.try_get("name")?,
                    subpath: PathBuf::from(row.try_get::<String, _>("subpath")?),
                })
            })
            .collect()
    }

    /// Read the active variant ID for a mod (None if no variants or
    /// none active).
    pub async fn active_variant_id(&self, mod_id: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT active_variant_id FROM mods WHERE id = ?")
            .bind(mod_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get::<Option<String>, _>("active_variant_id")?)
    }

    /// Switch the active Variant for `mod_id`. Drops the existing
    /// junction (if any) and recreates it pointing at the new
    /// variant subpath. The Library copy is never touched.
    pub async fn set_active_variant(
        &self,
        mod_id: &str,
        variant_id: &str,
        game_mods_dir: &Path,
    ) -> Result<()> {
        self.set_active_variant_in_library_mutation(mod_id, variant_id, game_mods_dir)
            .await
    }

    /// Build a [`conflicts::ConflictReport`] for `game`. Walks the
    /// enabled Mods, resolves each one's effective directory (Library
    /// path joined with the active Variant's subpath when present),
    /// extracts `[TextureOverride*]` / `[ResourceOverride*]` hash
    /// bindings, and reports every hash bound by two or more Mods.
    pub async fn detect_conflicts(&self, game: GameCode) -> Result<conflicts::ConflictReport> {
        let quarantined: std::collections::HashSet<_> = self
            .reinstall_swap_witnesses()
            .await?
            .into_iter()
            .filter(|witness| witness.is_quarantined())
            .map(|witness| witness.mod_id().to_string())
            .collect();
        let rows = sqlx::query(
            "SELECT id, library_path, active_variant_id, enabled FROM mods
             WHERE game_code = ?",
        )
        .bind(game.as_str())
        .fetch_all(&self.pool)
        .await?;

        let mut per_mod_bindings: Vec<(String, Vec<conflicts::HashBinding>)> = Vec::new();
        for row in rows {
            let enabled: i64 = row.try_get("enabled")?;
            if enabled == 0 {
                continue;
            }
            let id: String = row.try_get("id")?;
            if quarantined.contains(&id) {
                continue;
            }
            let library_path: String = row.try_get("library_path")?;
            let library_path = PathBuf::from(library_path);
            let effective = self
                .junction_target_for(&id, &library_path, &self.pool)
                .await?;
            let bindings = conflicts::extract_hashes_from_dir(&effective)?;
            per_mod_bindings.push((id, bindings));
        }

        Ok(conflicts::build_report(&per_mod_bindings))
    }

    /// Resolve the one Junction target implied by a Mod's persisted active
    /// Variant. A dangling ID or a Variant owned by another Mod is corrupt
    /// state, not permission to silently deploy the Mod root instead.
    async fn junction_target_for<'e, E>(
        &self,
        mod_id: &str,
        library_path: &Path,
        executor: E,
    ) -> Result<PathBuf>
    where
        E: Executor<'e, Database = Sqlite>,
    {
        let row = sqlx::query(
            "SELECT m.name, m.active_variant_id, v.subpath
             FROM mods m
             LEFT JOIN mod_variants v
               ON v.id = m.active_variant_id AND v.mod_id = m.id
             WHERE m.id = ?",
        )
        .bind(mod_id)
        .fetch_one(executor)
        .await?;
        let mod_name: String = row.try_get("name")?;
        let active_variant_id: Option<String> = row.try_get("active_variant_id")?;
        let active_variant_subpath: Option<String> = row.try_get("subpath")?;
        match (active_variant_id, active_variant_subpath) {
            (None, _) => Ok(library_path.to_path_buf()),
            (Some(_), Some(subpath)) => Ok(library_path.join(subpath)),
            (Some(variant_id), None) => Err(Error::InvalidActiveVariant {
                mod_id: mod_id.to_string(),
                mod_name,
                variant_id,
            }),
        }
    }

    /// Read the persisted install path for a game (None until the user
    /// has picked one or slice 2 has auto-detected one).
    pub async fn game_install_path(&self, game: GameCode) -> Result<Option<PathBuf>> {
        let row = sqlx::query("SELECT install_path FROM games WHERE code = ?")
            .bind(game.as_str())
            .fetch_one(&self.pool)
            .await?;
        let install_path: Option<String> = row.try_get("install_path")?;
        Ok(install_path.map(PathBuf::from))
    }

    /// Persist a game's install path.
    pub async fn set_game_install_path(&self, game: GameCode, path: &Path) -> Result<()> {
        sqlx::query("UPDATE games SET install_path = ? WHERE code = ?")
            .bind(path.to_string_lossy().as_ref())
            .bind(game.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Run the startup reconcile pass across every game whose
    /// `install_path` is set. The per-game result is logged via tracing
    /// (NEW-LOG); the caller usually only cares about the aggregate
    /// vector for status reporting.
    pub async fn reconcile_all_set_games(&self) -> Result<reconcile::StartupReconcileReport> {
        let rows = sqlx::query("SELECT code, install_path FROM games")
            .fetch_all(&self.pool)
            .await?;
        let mut report = reconcile::StartupReconcileReport::default();
        for row in rows {
            let code: String = row.try_get("code")?;
            let install_path: Option<String> = row.try_get("install_path")?;
            let Some(install) = install_path else {
                continue;
            };
            let game = GameCode::from_str(&code)?;
            let mods_dir = PathBuf::from(install).join("Mods");
            match self.reconcile_junctions(game, &mods_dir).await {
                Ok(result) => {
                    tracing::info!(
                        target: "gmm::reconcile",
                        game = code.as_str(),
                        recreated = result.recreated.len(),
                        healthy = result.healthy.len(),
                        conflicting = result.conflicting.len(),
                        skipped = result.skipped.len(),
                        quarantined = result.quarantined.len(),
                        "startup reconcile completed",
                    );
                    report.reconciled.push((game, result));
                }
                Err(e) => {
                    tracing::warn!(
                        target: "gmm::reconcile",
                        game = code.as_str(),
                        error = %e,
                        "startup reconcile failed; falling back to lazy creation on enable",
                    );
                    report
                        .failures
                        .push(reconcile::StartupReconcileFailure::from_error(game, &e));
                }
            }
        }
        Ok(report)
    }

    /// Run the startup pass and publish every per-game failure to the
    /// in-memory bridge read by React. Keeping report production and
    /// publication together makes the user-visible handoff testable without a
    /// Tauri window or a timing-sensitive event listener.
    pub async fn reconcile_all_set_games_at_startup(
        &self,
        state: &reconcile::StartupReconcileState,
    ) -> Result<()> {
        let report = self.reconcile_all_set_games().await?;
        state.finish(report.failures);
        Ok(())
    }

    /// Walk every Mod row for `game` and reconcile its junction with
    /// reality. Recreates missing junctions for enabled mods. Surfaces
    /// (but does not auto-fix) junctions that resolve to an unexpected
    /// target — the UI prompts the user for those.
    ///
    /// See ADR 0003 — the Library is the source of truth, so we never
    /// rewrite Library files from a stale junction.
    pub async fn reconcile_junctions(
        &self,
        game: GameCode,
        game_mods_dir: &Path,
    ) -> Result<reconcile::ReconcileResult> {
        let quarantined: std::collections::HashMap<_, _> = self
            .reinstall_swap_witnesses()
            .await?
            .into_iter()
            .filter(|witness| witness.is_quarantined())
            .map(|witness| (witness.mod_id().to_string(), witness.token().to_string()))
            .collect();
        let rows = sqlx::query(
            "SELECT id, junction_dir_name, library_path, enabled
             FROM mods WHERE game_code = ?",
        )
        .bind(game.as_str())
        .fetch_all(&self.pool)
        .await?;

        // Non-fatal: if the game mods dir does not exist yet we'll just
        // recreate links into it; we ensure it exists first so the
        // junction crate can write into it.
        std::fs::create_dir_all(game_mods_dir).map_err(|source| Error::Io {
            path: game_mods_dir.to_path_buf(),
            source,
        })?;

        let mut result = reconcile::ReconcileResult::default();

        for row in rows {
            let id: String = row.try_get("id")?;
            let junction_dir_name: String = row.try_get("junction_dir_name")?;
            let library_path: String = row.try_get("library_path")?;
            let enabled: i64 = row.try_get("enabled")?;

            if let Some(token) = quarantined.get(&id) {
                let link = game_mods_dir.join(&junction_dir_name);
                if self
                    .withdraw_quarantined_reinstall_junction(token, Some(&link))
                    .await?
                    .is_some()
                {
                    result.quarantined.push(id);
                    continue;
                }
            }

            let link = game_mods_dir.join(&junction_dir_name);
            let library_path = PathBuf::from(&library_path);
            let expected_target = self
                .junction_target_for(&id, &library_path, &self.pool)
                .await?;

            // A disabled Mod should have no junction. If one is there,
            // something tore between `set_enabled`'s filesystem step and
            // its DB step — a crash (#59) or, on an unsupported second
            // instance, a race (#58). Left alone, the Model Importer
            // keeps loading a Mod the UI says is off.
            if enabled == 0 {
                if !link_exists(&link)? {
                    result.skipped.push(id);
                    continue;
                }
                match resolve_link(&link) {
                    // Ours: we put it there, the row says it should be
                    // gone, and deleting it cannot touch the Library.
                    Some(actual) if same_path(&actual, &expected_target) => {
                        junction::remove(&link)?;
                        result.removed.push(id);
                    }
                    // Points somewhere we never pointed it. Same rule as
                    // the enabled-but-drifted case: surface, don't
                    // clobber whatever the user intended.
                    _ => result.conflicting.push(reconcile::ConflictingJunction {
                        mod_id: id,
                        link,
                        expected_target,
                    }),
                }
                continue;
            }

            if !link_exists(&link)? {
                volume::require_ntfs_pair(game_mods_dir, &expected_target)?;
                junction::create(&link, &expected_target)?;
                result.recreated.push(id);
                continue;
            }

            match resolve_link(&link) {
                // A junction whose target directory has been deleted
                // still resolves to the expected path string, so
                // reporting on the string alone would leave the UI
                // showing an enabled mod the game cannot load.
                // Pointing at the right place is necessary but not
                // sufficient — the target directory has to actually be
                // there. `try_exists` rather than `exists` so a
                // permission error or disconnected drive doesn't
                // masquerade as "deleted": on an inconclusive answer we
                // leave the mod healthy rather than nagging the user
                // about a Library that is merely temporarily
                // unreachable. `is_dir` because a plain file at the
                // target path is not a usable junction target.
                Some(actual)
                    if same_path(&actual, &expected_target)
                        && matches!(expected_target.try_exists(), Ok(true) | Err(_))
                        && !expected_target.is_file() =>
                {
                    result.healthy.push(id);
                }
                // The Junction points somewhere other than the row
                // says. If that somewhere is still inside *this Mod's*
                // own Library directory, it is a stale Variant target
                // and nobody but GMM could have created it: the row is
                // the source of truth for which Variant is active, so
                // re-point it. This is the state a crash between
                // `set_active_variant`'s DB write and its junction swap
                // leaves behind (#59), and reporting it as conflicting
                // left the game loading a Variant the UI says is not
                // selected, with no way to fix it but a full rebuild.
                // `is_dir` matters: a Junction pointing at the right
                // place whose target the user deleted also lands here,
                // and there is nothing to relink it to. That case stays
                // conflicting (see
                // `a_junction_whose_target_was_deleted_is_not_healthy`).
                Some(actual) if path_within(&actual, &library_path) && expected_target.is_dir() => {
                    junction::remove(&link)?;
                    volume::require_ntfs_pair(game_mods_dir, &expected_target)?;
                    junction::create(&link, &expected_target)?;
                    result.recreated.push(id);
                }
                // Outside the Mod's Library entirely — the user aimed it
                // somewhere of their own. Surface, never clobber.
                _ => {
                    result.conflicting.push(reconcile::ConflictingJunction {
                        mod_id: id,
                        link,
                        expected_target,
                    });
                }
            }
        }

        Ok(result)
    }

    /// Drop every junction for `game` and recreate one per enabled Mod
    /// against the current Library. The hammer to use after a user
    /// relocates their Library directory (ADR 0003).
    pub async fn rebuild_junctions(
        &self,
        game: GameCode,
        game_mods_dir: &Path,
    ) -> Result<reconcile::ReconcileResult> {
        let quarantined: std::collections::HashMap<_, _> = self
            .reinstall_swap_witnesses()
            .await?
            .into_iter()
            .filter(|witness| witness.is_quarantined())
            .map(|witness| (witness.mod_id().to_string(), witness.token().to_string()))
            .collect();
        let rows = sqlx::query(
            "SELECT id, junction_dir_name, library_path, enabled
             FROM mods WHERE game_code = ?",
        )
        .bind(game.as_str())
        .fetch_all(&self.pool)
        .await?;

        std::fs::create_dir_all(game_mods_dir).map_err(|source| Error::Io {
            path: game_mods_dir.to_path_buf(),
            source,
        })?;

        // Resolve every usable enabled Mod before removing anything. A corrupt
        // active Variant must leave the user's existing deployment intact,
        // rather than turning Rebuild into a destructive partial operation.
        let mut prepared = Vec::with_capacity(rows.len());
        let mut result = reconcile::ReconcileResult::default();
        for row in rows {
            let id: String = row.try_get("id")?;
            let junction_dir_name: String = row.try_get("junction_dir_name")?;
            let library_path: String = row.try_get("library_path")?;
            if let Some(token) = quarantined.get(&id) {
                let link = game_mods_dir.join(&junction_dir_name);
                if self
                    .withdraw_quarantined_reinstall_junction(token, Some(&link))
                    .await?
                    .is_some()
                {
                    result.quarantined.push(id);
                    continue;
                }
            }
            let enabled = row.try_get::<i64, _>("enabled")? != 0;
            let target = if enabled {
                let target = self
                    .junction_target_for(&id, Path::new(&library_path), &self.pool)
                    .await?;
                volume::require_ntfs_pair(game_mods_dir, &target)?;
                Some(target)
            } else {
                None
            };
            prepared.push((id, junction_dir_name, enabled, target));
        }

        for (id, junction_dir_name, enabled, target) in prepared {
            let link = game_mods_dir.join(&junction_dir_name);

            // Always drop the existing link first; if the user relocated
            // the Library, the old link would resolve to thin air.
            let had_link = link_exists(&link)?;
            if had_link {
                junction::remove(&link)?;
            }

            if !enabled {
                // Rebuild already deletes stranded junctions as a side
                // effect of dropping every link. Report it the same way
                // reconcile does, so `removed` means one thing across
                // both passes rather than depending on which the user ran.
                if had_link {
                    result.removed.push(id);
                } else {
                    result.skipped.push(id);
                }
                continue;
            }
            let target = target.expect("enabled Mods have a preflighted Junction target");
            junction::create(&link, &target)?;
            result.recreated.push(id);
        }
        Ok(result)
    }

    /// Snapshot of the user-facing settings, for diagnostics bundles.
    /// Sensitive fields are NOT redacted here — call
    /// [`diagnostics::SettingsSnapshot::redacted`] before serialising.
    pub async fn settings_snapshot(&self) -> Result<diagnostics::SettingsSnapshot> {
        let rows = sqlx::query("SELECT code, install_path FROM games")
            .fetch_all(&self.pool)
            .await?;

        let mut game_install_paths = std::collections::HashMap::new();
        for row in rows {
            let code: String = row.try_get("code")?;
            let install_path: Option<String> = row.try_get("install_path")?;
            game_install_paths.insert(code, install_path.map(PathBuf::from));
        }

        Ok(diagnostics::SettingsSnapshot {
            library_root: Some(self.resolved_library_root().await?),
            game_install_paths,
            // Populated by slice 10 (proxy settings). Leaving blank here
            // means the bundle just shows `null` until then.
            proxy_url: None,
        })
    }

    /// Read the persisted active GameSession, if any.
    pub async fn session_info(&self) -> Result<Option<SessionInfo>> {
        let row = sqlx::query("SELECT game_code, pid, started_at FROM active_session WHERE id = 1")
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else { return Ok(None) };
        let game_code: String = row.try_get("game_code")?;
        let pid: i64 = row.try_get("pid")?;
        let started_at: String = row.try_get("started_at")?;
        Ok(Some(SessionInfo {
            game: GameCode::from_str(&game_code)?,
            pid: pid as u32,
            started_at: chrono::DateTime::parse_from_rfc3339(&started_at)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        }))
    }

    /// Atomically claim the singleton active_session row. Fails with the
    /// SQLite primary-key conflict if a session is already active —
    /// callers that see this error must abandon their launch (and kill
    /// any child process they may have already spawned). Pair with
    /// `ensure_no_active_session()` to surface a friendlier error before
    /// any side effects happen.
    pub async fn start_session(&self, info: &SessionInfo) -> Result<()> {
        sqlx::query(
            "INSERT INTO active_session (id, game_code, pid, started_at)
             VALUES (1, ?, ?, ?)",
        )
        .bind(info.game.as_str())
        .bind(info.pid as i64)
        .bind(info.started_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Clear the persisted active GameSession. Idempotent.
    pub async fn end_session(&self) -> Result<()> {
        sqlx::query("DELETE FROM active_session WHERE id = 1")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// If a session row points at a process that's no longer alive,
    /// delete the row and return the evicted info so the UI can surface
    /// "Genshin ended unexpectedly last time". Idempotent — returns
    /// `Ok(None)` when no stale row exists.
    pub async fn clean_stale_session(&self) -> Result<Option<SessionInfo>> {
        let Some(info) = self.session_info().await? else {
            return Ok(None);
        };
        if session::is_pid_alive(info.pid) {
            return Ok(None);
        }
        self.end_session().await?;
        Ok(Some(info))
    }

    async fn ensure_no_active_session(&self) -> Result<()> {
        if let Some(info) = self.session_info().await? {
            return Err(Error::SessionActive {
                game: info.game.as_str().to_string(),
                since: info.started_at.to_rfc3339(),
            });
        }
        Ok(())
    }

    /// Enable or disable a Mod. On enable, a Junction is created at
    /// `<game_mods_dir>/<mod-name>/` pointing at the Mod's Library path
    /// (joined with the active Variant's subpath when one is set).
    /// On disable, the Junction is removed (the Library copy is never touched).
    pub async fn set_enabled(&self, id: &str, enabled: bool, game_mods_dir: &Path) -> Result<()> {
        self.set_enabled_in_library_mutation(id, enabled, game_mods_dir)
            .await
    }

    /// List every Mod for a given game, ordered by creation time ascending.
    pub async fn list_mods(&self, game: GameCode) -> Result<Vec<Mod>> {
        let mut recoveries: std::collections::HashMap<_, _> = self
            .reinstall_swap_witnesses()
            .await?
            .into_iter()
            .filter_map(|witness| {
                let mod_id = witness.mod_id().to_string();
                witness.recovery().map(|recovery| (mod_id, recovery))
            })
            .collect();
        let rows = sqlx::query(
            "SELECT m.id, m.game_code, m.name, m.source, m.library_path, m.enabled,
                    m.gamebanana_id, m.source_url, m.author, m.version, m.screenshot_url
             FROM mods m
             WHERE m.game_code = ?
             ORDER BY m.created_at ASC",
        )
        .bind(game.as_str())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let id: String = row.try_get("id")?;
                let game_code: String = row.try_get("game_code")?;
                let name: String = row.try_get("name")?;
                let source: String = row.try_get("source")?;
                let library_path: String = row.try_get("library_path")?;
                let enabled: i64 = row.try_get("enabled")?;
                let reinstall_recovery = recoveries.remove(&id);

                Ok(Mod {
                    id,
                    game: GameCode::from_str(&game_code)?,
                    name,
                    source: Source::from_str(&source)?,
                    library_path: PathBuf::from(library_path),
                    enabled: enabled != 0,
                    gamebanana_id: row
                        .try_get::<Option<i64>, _>("gamebanana_id")?
                        .map(|v| v as u64),
                    source_url: row.try_get("source_url")?,
                    author: row.try_get("author")?,
                    version: row.try_get("version")?,
                    screenshot_url: row.try_get("screenshot_url")?,
                    reinstall_recovery,
                })
            })
            .collect()
    }
}

/// Convert a Mod's display name into a directory name that NTFS will
/// accept under `<Game>/Mods/`: strip reserved characters, trim trailing
/// dots/spaces, and prefix any DOS device name (CON, PRN, AUX, NUL,
/// COM1..9, LPT1..9) so it stops being reserved. Collision dedup happens
/// through the shared Library-mutation transaction.
pub(crate) fn sanitize_dir_name(display: &str) -> String {
    let stripped: String = display
        .chars()
        .filter(|c| {
            !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') && !c.is_control()
        })
        .collect();
    let trimmed = stripped.trim_end_matches(['.', ' ']);
    let capped: String = trimmed.chars().take(MAX_JUNCTION_DIR_CHARS).collect();
    let capped_trimmed = capped.trim_end_matches(['.', ' ']).to_string();

    if is_dos_reserved(&capped_trimmed) {
        format!("_{capped_trimmed}")
    } else {
        capped_trimmed
    }
}

/// Conservative cap that leaves headroom for `<Game>/Mods/` prefix and any
/// future suffix logic (e.g. ` (123)` dedup) while staying inside the
/// MAX_PATH-friendly window used by most Windows tooling.
const MAX_JUNCTION_DIR_CHARS: usize = 200;

pub(crate) fn is_dos_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    for prefix in ["COM", "LPT"] {
        if stem.len() == prefix.len() + 1 && stem.starts_with(prefix) {
            let last = stem.as_bytes()[prefix.len()];
            if last.is_ascii_digit() && last != b'0' {
                return true;
            }
        }
    }
    false
}

/// Summary of a Library-path move. Returned by
/// [`Core::set_library_root`] and
/// [`Core::set_library_path_for_game`].
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MoveReport {
    /// Mod IDs whose `library_path` was rewritten.
    pub relocated: Vec<String>,
    /// Top-level directories we moved (one per game, or a single entry
    /// for the per-game case).
    pub moved_directories: Vec<PathBuf>,
    /// Previously-enabled Mods whose Junction could not be recreated. The
    /// authoritative Library move still committed; Rebuild Junctions retries
    /// these reconstructible projections from the committed rows.
    pub failed_junction_restores: Vec<JunctionRestoreFailure>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct JunctionRestoreFailure {
    pub mod_id: String,
    pub game: GameCode,
    pub kind: error::SurfaceFailureKind,
    pub error: String,
}

/// Move `from` to `to`. Prefer atomic rename; fall back to recursive
/// copy + delete when rename fails (typically EXDEV, cross-volume).
fn move_subtree(from: &Path, to: &Path, report: &mut MoveReport) -> Result<()> {
    if !from.exists() {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    match std::fs::rename(from, to) {
        Ok(()) => {}
        Err(_) => {
            copy_dir_recursive(from, to, None)?;
            std::fs::remove_dir_all(from).map_err(|source| Error::Io {
                path: from.to_path_buf(),
                source,
            })?;
        }
    }
    report.moved_directories.push(to.to_path_buf());
    Ok(())
}

fn remove_reinstall_stage_if_identity_matches(path: &Path, expected: &str) -> Result<()> {
    let directory = match library_identity::IdentifiedDirectory::open(path) {
        Ok(directory) => directory,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(Error::Io {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    if directory.identity().durable_key() != expected {
        return Err(Error::ReinstallRecoveryUncertain {
            mod_id: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("<unknown>")
                .to_string(),
            reason: "the unwitnessed staging path changed identity before cleanup".to_string(),
        });
    }
    drop(directory);
    std::fs::remove_dir_all(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Does the path exist as a symlink/junction? `Path::exists` follows the
/// link; we want "the link entry itself is there", which is what
/// `symlink_metadata` returns. Only `NotFound` proves absence: an unreadable
/// deployment entry must not be treated as safe to replace or already gone.
fn link_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Resolve the target of a junction/symlink. Returns `None` if `path`
/// is not a link or the OS refuses to read it.
fn resolve_link(path: &Path) -> Option<PathBuf> {
    std::fs::read_link(path).ok()
}

/// Path equality that is tolerant of trailing separators and
/// case-insensitivity quirks on Windows. We canonicalise both sides;
/// on failure we fall back to a literal compare.
fn same_path(a: &Path, b: &Path) -> bool {
    let canon_a = std::fs::canonicalize(a).ok();
    let canon_b = std::fs::canonicalize(b).ok();
    match (canon_a, canon_b) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// Is `path` inside `ancestor`?
///
/// Canonicalises both sides for the same reason [`same_path`] does: the
/// Library path recorded in the DB and the target read back off a
/// Junction need not be spelled identically even when they name the same
/// directory. macOS resolves `/var` to `/private/var`; Windows has 8.3
/// short names and drive-letter casing. A raw `starts_with` answers
/// "no" for all of those, which would send a Junction GMM itself created
/// down the "the user aimed this somewhere" path.
///
/// Falls back to a literal comparison when either side cannot be
/// canonicalised (typically because it no longer exists).
fn path_within(path: &Path, ancestor: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(path).starts_with(canon(ancestor))
}

/// Recursive directory copy. Standard library has no built-in equivalent.
fn copy_dir_recursive(src: &Path, dst: &Path, after_file: Option<&dyn Fn()>) -> Result<()> {
    std::fs::create_dir_all(dst).map_err(|source| Error::Io {
        path: dst.to_path_buf(),
        source,
    })?;

    let entries = std::fs::read_dir(src).map_err(|source| Error::Io {
        path: src.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: src.to_path_buf(),
            source,
        })?;
        let entry_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        let metadata = entry.metadata().map_err(|source| Error::Io {
            path: entry_path.clone(),
            source,
        })?;

        if metadata.is_dir() {
            copy_dir_recursive(&entry_path, &dst_path, after_file)?;
        } else {
            std::fs::copy(&entry_path, &dst_path).map_err(|source| Error::Io {
                path: entry_path.clone(),
                source,
            })?;
            if let Some(after_file) = after_file {
                after_file();
            }
        }
    }

    Ok(())
}
