//! Key/value settings (the `settings` table).
//!
//! Slice 15 (Library path overrides) needs persistent settings; future
//! slices add more keys here. The schema is one row per logical key
//! (`library.root`, `library.gimi`, etc); `value = NULL` means the key
//! has been explicitly reset to the default.

use sqlx::{Row, SqlitePool};

use super::error::{Error, Result};
use super::games::GameCode;

/// Read a settings value. Returns `Ok(None)` if the key is absent or
/// stored as `NULL`.
pub async fn get(pool: &SqlitePool, key: &str) -> Result<Option<String>> {
    let row = sqlx::query("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    match row {
        Some(r) => Ok(r.try_get::<Option<String>, _>("value")?),
        None => Ok(None),
    }
}

/// Upsert a settings value. Pass `None` to clear (the row stays with a
/// NULL value so we don't have to special-case absence vs. cleared).
pub async fn put(pool: &SqlitePool, key: &str, value: Option<&str>) -> Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// Canonical key strings used by the rest of the codebase. Kept in one
/// place so a typo is impossible to silently introduce.
pub mod keys {
    use super::GameCode;

    pub fn library_root() -> &'static str {
        "library.root"
    }

    pub fn library_root_for_game(game: GameCode) -> String {
        format!("library.{}", game.as_str())
    }
}

/// Convenience: bring the keys into scope as `Error`-returning helpers
/// so the resolver can `?` straight onto them.
pub use keys::{library_root, library_root_for_game};

// Re-export so the parent module can `use super::settings::keys` and
// also have a typed `Error` alias if it wants one.
pub type SettingsError = Error;
