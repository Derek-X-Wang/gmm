//! Key/value settings (the `settings` table).
//!
//! Slice 15 (Library path overrides) needs persistent settings; future
//! slices add more keys here. The schema is one row per logical key
//! (`library.root`, `library.gimi`, etc); `value = NULL` means the key
//! has been explicitly reset to the default.

use sqlx::{Executor, Row, Sqlite};

use super::error::{Error, Result};
use super::games::GameCode;

/// Read a settings value. Returns `Ok(None)` if the key is absent or
/// stored as `NULL`.
///
/// Generic over the executor rather than taking a `&SqlitePool` so a
/// caller that has to write several related keys as one unit can run
/// the whole group inside a transaction (#122). `&SqlitePool` still
/// satisfies it, so single-key callers read unchanged.
pub async fn get<'e, E>(executor: E, key: &str) -> Result<Option<String>>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(executor)
        .await?;
    match row {
        Some(r) => Ok(r.try_get::<Option<String>, _>("value")?),
        None => Ok(None),
    }
}

/// Upsert a settings value. Pass `None` to clear (the row stays with a
/// NULL value so we don't have to special-case absence vs. cleared).
pub async fn put<'e, E>(executor: E, key: &str, value: Option<&str>) -> Result<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(executor)
    .await?;
    Ok(())
}

/// The version shown to the user and the signed artifact identity they skipped.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SkippedUpdate {
    pub version: String,
    pub identity: String,
}

/// Canonical setting keys.
pub mod keys {
    use super::GameCode;

    pub fn skipped_update() -> &'static str {
        "updater.skippedBuild"
    }

    pub fn library_root() -> &'static str {
        "library.root"
    }

    pub fn library_root_for_game(game: GameCode) -> String {
        format!("library.{}", game.as_str())
    }

    /// Slice 16-b (#24) — first-run onboarding state. `true` once the
    /// wizard either finishes or is skipped; the App router uses it to
    /// route between `/onboarding` and the main view.
    pub fn onboarding_complete() -> &'static str {
        "onboarding.complete"
    }

    /// #327: requested Mod inspector width, in CSS pixels, not window geometry.
    pub fn mod_panel_width() -> &'static str {
        "mods.panelWidthCssPx"
    }

    /// #369 — the main window's remembered geometry, as the JSON of a
    /// [`crate::core::window_state::SavedWindowState`]. One row rather
    /// than four: the numbers are only ever meaningful together, and a
    /// half-written position is exactly the state that strands a window.
    pub fn window_geometry() -> &'static str {
        "window.mainGeometry"
    }

    /// Slice 16-b (#24) — `true` iff the user pressed *Skip setup*
    /// instead of completing the wizard. Drives the "Finish setup"
    /// banner the spec puts on Settings until the user resumes.
    pub fn onboarding_skipped() -> &'static str {
        "onboarding.skipped"
    }
}

/// Convenience: bring the keys into scope as `Error`-returning helpers
/// so the resolver can `?` straight onto them.
pub use keys::{library_root, library_root_for_game};

// Re-export so the parent module can `use super::settings::keys` and
// also have a typed `Error` alias if it wants one.
pub type SettingsError = Error;
