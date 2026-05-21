//! Pure-Rust core of GMM.
//!
//! Tauri commands are thin shells over the functions in this module; the
//! integration tests in `src-tauri/tests/` exercise this module directly so
//! they can run on macOS without spinning up the Tauri runtime.

pub mod detect;
pub mod diagnostics;
pub mod error;
pub mod games;
pub mod importer;
pub mod junction;
pub mod mods;
pub mod network;
pub mod reconcile;
pub mod settings;
pub mod volume;
pub mod zip_import;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::Utc;
use sqlx::{sqlite::SqliteConnectOptions, Row, SqlitePool};
use ulid::Ulid;

pub use error::{Error, Result};
pub use games::GameCode;
pub use mods::{Mod, Source};
pub use zip_import::ImportZipOptions;

use settings::{get as get_setting, keys, put as put_setting};

/// The Core owns the SQLite pool and the Library root. Everything that
/// reads from or writes to the user's data goes through here.
#[derive(Clone)]
pub struct Core {
    pool: SqlitePool,
    default_library_root: PathBuf,
}

impl Core {
    /// Open (or create) the DB at `db_url`, run pending migrations, and
    /// ensure the Library root exists.
    pub async fn new(default_library_root: PathBuf, db_url: &str) -> Result<Self> {
        std::fs::create_dir_all(&default_library_root).map_err(|source| Error::Io {
            path: default_library_root.clone(),
            source,
        })?;

        let opts: SqliteConnectOptions = db_url
            .parse::<SqliteConnectOptions>()?
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self {
            pool,
            default_library_root,
        })
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
        let previous = self.resolved_library_root().await?;
        let next = new_root
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.default_library_root.clone());

        if previous == next {
            put_setting(
                &self.pool,
                keys::library_root(),
                new_root.map(|p| p.to_string_lossy().to_string()).as_deref(),
            )
            .await?;
            return Ok(MoveReport::default());
        }

        volume::require_ntfs(&next)?;

        // Move every game's subtree from previous to next. Per-game
        // overrides are unaffected — they're absolute and live elsewhere.
        let report = self
            .move_root(&previous, &next, /* per_game */ None)
            .await?;
        put_setting(
            &self.pool,
            keys::library_root(),
            new_root.map(|p| p.to_string_lossy().to_string()).as_deref(),
        )
        .await?;
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
        let previous = self.resolved_library_root_for(game).await?;
        let next = new_path.map(Path::to_path_buf).unwrap_or_else(|| {
            // When clearing, the effective path becomes
            // `resolved_root().join(game)`. We compute it eagerly
            // so the move flow knows where files go.
            // (`resolved_library_root_for(game)` would still hit
            // the now-cleared override, so we mirror its fallback
            // here.)
            PathBuf::new()
        });

        let fallback = self.resolved_library_root().await?.join(game.as_str());
        let next_effective = if next.as_os_str().is_empty() {
            fallback
        } else {
            next.clone()
        };

        if previous == next_effective {
            put_setting(
                &self.pool,
                &keys::library_root_for_game(game),
                new_path.map(|p| p.to_string_lossy().to_string()).as_deref(),
            )
            .await?;
            return Ok(MoveReport::default());
        }

        volume::require_ntfs(&next_effective)?;

        let report = self
            .move_root(&previous, &next_effective, Some(game))
            .await?;
        put_setting(
            &self.pool,
            &keys::library_root_for_game(game),
            new_path.map(|p| p.to_string_lossy().to_string()).as_deref(),
        )
        .await?;
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
    ) -> Result<MoveReport> {
        // Snapshot mods that need their library_path rewritten. For the
        // global case we include every mod across every game; for the
        // per-game case only that game.
        let rows = match per_game {
            Some(game) => {
                sqlx::query(
                    "SELECT id, game_code, library_path, enabled FROM mods WHERE game_code = ?",
                )
                .bind(game.as_str())
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query("SELECT id, game_code, library_path, enabled FROM mods")
                    .fetch_all(&self.pool)
                    .await?
            }
        };

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
            let install = self.game_install_path(game).await?;
            if let Some(install) = install {
                let mods_dir = install.join("Mods");
                let junction_row = sqlx::query("SELECT junction_dir_name FROM mods WHERE id = ?")
                    .bind(&id)
                    .fetch_one(&self.pool)
                    .await?;
                let junction_dir_name: String = junction_row.try_get("junction_dir_name")?;
                let link = mods_dir.join(junction_dir_name);
                if link_exists(&link) {
                    let _ = junction::remove(&link);
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
                .execute(&self.pool)
                .await?;
            report.relocated.push(id);
        }

        // Re-enable previously-enabled mods (recreates junctions
        // against the rewritten library_path).
        for (id, game) in previously_enabled {
            let install = self.game_install_path(game).await?;
            if let Some(install) = install {
                let mods_dir = install.join("Mods");
                std::fs::create_dir_all(&mods_dir).map_err(|source| Error::Io {
                    path: mods_dir.clone(),
                    source,
                })?;
                // `set_enabled(false)` was effectively done by the
                // junction::remove above without persisting; flip the
                // bit through the proper path now so junctions land.
                sqlx::query("UPDATE mods SET enabled = 0 WHERE id = ?")
                    .bind(&id)
                    .execute(&self.pool)
                    .await?;
                self.set_enabled(&id, true, &mods_dir).await?;
            }
        }

        Ok(report)
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
        let library_path = self.resolved_library_root_for(game).await?.join(&id);

        copy_dir_recursive(source_path, &library_path)?;

        let base = sanitize_dir_name(display_name);
        let junction_dir_name = self.pick_unique_junction_dir_name(game, &base).await?;

        let created_at = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO mods (
                id, game_code, name, source, library_path,
                junction_dir_name, enabled, created_at
             )
             VALUES (?, ?, ?, ?, ?, ?, 0, ?)",
        )
        .bind(&id)
        .bind(game.as_str())
        .bind(display_name)
        .bind(Source::Manual.as_str())
        .bind(library_path.to_string_lossy().as_ref())
        .bind(&junction_dir_name)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;

        Ok(Mod {
            id,
            game,
            name: display_name.to_string(),
            source: Source::Manual,
            library_path,
            enabled: false,
        })
    }

    /// Import a local ZIP into the Library as a Mod with `source = local`.
    ///
    /// Hardened against the dirty realities of GameBanana-style archives:
    /// zip-slip path traversal, `__MACOSX/` / `.DS_Store` / `Thumbs.db`
    /// junk files, single-root-directory shape, and size/entry caps. See
    /// [`crate::core::zip_import`] for the extraction details.
    ///
    /// On any failure the partially-extracted Library path is removed so
    /// the user is never left with a half-imported Mod row pointing at
    /// half-extracted bytes.
    pub async fn import_zip(
        &self,
        game: GameCode,
        zip_path: &Path,
        display_name: &str,
        opts: ImportZipOptions,
    ) -> Result<Mod> {
        let id = Ulid::new().to_string();
        let library_path = self.resolved_library_root_for(game).await?.join(&id);

        if let Err(e) = zip_import::extract(zip_path, &library_path, opts) {
            // Best-effort cleanup. We swallow remove_dir_all errors so the
            // user sees the original extraction failure, not a noisy
            // cleanup follow-up.
            let _ = std::fs::remove_dir_all(&library_path);
            return Err(e);
        }

        let base = sanitize_dir_name(display_name);
        let junction_dir_name = self.pick_unique_junction_dir_name(game, &base).await?;

        let created_at = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO mods (
                id, game_code, name, source, library_path,
                junction_dir_name, enabled, created_at
             )
             VALUES (?, ?, ?, ?, ?, ?, 0, ?)",
        )
        .bind(&id)
        .bind(game.as_str())
        .bind(display_name)
        .bind(Source::Local.as_str())
        .bind(library_path.to_string_lossy().as_ref())
        .bind(&junction_dir_name)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;

        Ok(Mod {
            id,
            game,
            name: display_name.to_string(),
            source: Source::Local,
            library_path,
            enabled: false,
        })
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
    pub async fn reconcile_all_set_games(
        &self,
    ) -> Result<Vec<(GameCode, reconcile::ReconcileResult)>> {
        let rows = sqlx::query("SELECT code, install_path FROM games")
            .fetch_all(&self.pool)
            .await?;
        let mut out = Vec::new();
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
                        "startup reconcile completed",
                    );
                    out.push((game, result));
                }
                Err(e) => {
                    tracing::warn!(
                        target: "gmm::reconcile",
                        game = code.as_str(),
                        error = %e,
                        "startup reconcile failed; falling back to lazy creation on enable",
                    );
                }
            }
        }
        Ok(out)
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
        let rows = sqlx::query(
            "SELECT id, junction_dir_name, library_path, enabled FROM mods WHERE game_code = ?",
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

            if enabled == 0 {
                result.skipped.push(id);
                continue;
            }

            let link = game_mods_dir.join(&junction_dir_name);
            let expected_target = PathBuf::from(&library_path);

            if !link_exists(&link) {
                volume::require_ntfs_pair(game_mods_dir, &expected_target)?;
                junction::create(&link, &expected_target)?;
                result.recreated.push(id);
                continue;
            }

            match resolve_link(&link) {
                Some(actual) if same_path(&actual, &expected_target) => {
                    result.healthy.push(id);
                }
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
        let rows = sqlx::query(
            "SELECT id, junction_dir_name, library_path, enabled FROM mods WHERE game_code = ?",
        )
        .bind(game.as_str())
        .fetch_all(&self.pool)
        .await?;

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
            let link = game_mods_dir.join(&junction_dir_name);
            let target = PathBuf::from(library_path);

            // Always drop the existing link first; if the user relocated
            // the Library, the old link would resolve to thin air.
            if link_exists(&link) {
                let _ = junction::remove(&link);
            }

            if enabled == 0 {
                result.skipped.push(id);
                continue;
            }
            volume::require_ntfs_pair(game_mods_dir, &target)?;
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

    /// Find an unused junction directory name for the given game, deduping
    /// collisions by appending ` (2)`, ` (3)`, ... If `base` is already
    /// unique we return it unchanged.
    async fn pick_unique_junction_dir_name(&self, game: GameCode, base: &str) -> Result<String> {
        let rows = sqlx::query("SELECT junction_dir_name FROM mods WHERE game_code = ?")
            .bind(game.as_str())
            .fetch_all(&self.pool)
            .await?;

        let existing: HashSet<String> = rows
            .iter()
            .filter_map(|r| r.try_get::<String, _>("junction_dir_name").ok())
            .collect();

        if !existing.contains(base) {
            return Ok(base.to_string());
        }

        for n in 2..=u32::MAX {
            let candidate = format!("{base} ({n})");
            if !existing.contains(&candidate) {
                return Ok(candidate);
            }
        }
        unreachable!("u32::MAX collisions on one display name is not a real scenario")
    }

    /// Enable or disable a Mod. On enable, a Junction is created at
    /// `<game_mods_dir>/<mod-name>/` pointing at the Mod's Library path.
    /// On disable, the Junction is removed (the Library copy is never touched).
    pub async fn set_enabled(&self, id: &str, enabled: bool, game_mods_dir: &Path) -> Result<()> {
        let row =
            sqlx::query("SELECT junction_dir_name, library_path, enabled FROM mods WHERE id = ?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;

        let junction_dir_name: String = row.try_get("junction_dir_name")?;
        let library_path: String = row.try_get("library_path")?;
        let current_enabled: i64 = row.try_get("enabled")?;

        let link = game_mods_dir.join(&junction_dir_name);
        let target = PathBuf::from(library_path);

        match (current_enabled != 0, enabled) {
            (false, true) => {
                volume::require_ntfs_pair(game_mods_dir, &target)?;
                junction::create(&link, &target)?;
            }
            (true, false) => junction::remove(&link)?,
            _ => {}
        }

        sqlx::query("UPDATE mods SET enabled = ? WHERE id = ?")
            .bind(if enabled { 1_i64 } else { 0_i64 })
            .bind(id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// List every Mod for a given game, ordered by creation time ascending.
    pub async fn list_mods(&self, game: GameCode) -> Result<Vec<Mod>> {
        let rows = sqlx::query(
            "SELECT id, game_code, name, source, library_path, enabled
             FROM mods
             WHERE game_code = ?
             ORDER BY created_at ASC",
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

                Ok(Mod {
                    id,
                    game: GameCode::from_str(&game_code)?,
                    name,
                    source: Source::from_str(&source)?,
                    library_path: PathBuf::from(library_path),
                    enabled: enabled != 0,
                })
            })
            .collect()
    }
}

/// Convert a Mod's display name into a directory name that NTFS will
/// accept under `<Game>/Mods/`: strip reserved characters, trim trailing
/// dots/spaces, and prefix any DOS device name (CON, PRN, AUX, NUL,
/// COM1..9, LPT1..9) so it stops being reserved. Collision dedup happens
/// at the Core layer (see `pick_unique_junction_dir_name`).
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

fn is_dos_reserved(name: &str) -> bool {
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
            copy_dir_recursive(from, to)?;
            std::fs::remove_dir_all(from).map_err(|source| Error::Io {
                path: from.to_path_buf(),
                source,
            })?;
        }
    }
    report.moved_directories.push(to.to_path_buf());
    Ok(())
}

/// Does the path exist as a symlink/junction? `Path::exists` follows
/// the link; we want "the link entry itself is there", which is what
/// `symlink_metadata` returns.
fn link_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
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

/// Recursive directory copy. Standard library has no built-in equivalent.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
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
            copy_dir_recursive(&entry_path, &dst_path)?;
        } else {
            std::fs::copy(&entry_path, &dst_path).map_err(|source| Error::Io {
                path: entry_path.clone(),
                source,
            })?;
        }
    }

    Ok(())
}
