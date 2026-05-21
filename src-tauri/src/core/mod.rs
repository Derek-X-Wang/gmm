//! Pure-Rust core of GMM.
//!
//! Tauri commands are thin shells over the functions in this module; the
//! integration tests in `src-tauri/tests/` exercise this module directly so
//! they can run on macOS without spinning up the Tauri runtime.

pub mod error;
pub mod games;
pub mod junction;
pub mod mods;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::Utc;
use sqlx::{sqlite::SqliteConnectOptions, Row, SqlitePool};
use ulid::Ulid;

pub use error::{Error, Result};
pub use games::GameCode;
pub use mods::{Mod, Source};

/// The Core owns the SQLite pool and the Library root. Everything that
/// reads from or writes to the user's data goes through here.
#[derive(Clone)]
pub struct Core {
    pool: SqlitePool,
    library_root: PathBuf,
}

impl Core {
    /// Open (or create) the DB at `db_url`, run pending migrations, and
    /// ensure the Library root exists.
    pub async fn new(library_root: PathBuf, db_url: &str) -> Result<Self> {
        std::fs::create_dir_all(&library_root).map_err(|source| Error::Io {
            path: library_root.clone(),
            source,
        })?;

        let opts: SqliteConnectOptions = db_url
            .parse::<SqliteConnectOptions>()?
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool, library_root })
    }

    /// Adopt an already-extracted folder into the Library as a Mod with
    /// `source = manual`. Copies the source tree into
    /// `<library_root>/<game>/<ulid>/` and records the row.
    pub async fn adopt_folder(
        &self,
        game: GameCode,
        source_path: &Path,
        display_name: &str,
    ) -> Result<Mod> {
        let id = Ulid::new().to_string();
        let library_path = self.library_root.join(game.as_str()).join(&id);

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
            (false, true) => junction::create(&link, &target)?,
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
