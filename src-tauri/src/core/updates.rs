//! Update detection (slice 13b).
//!
//! Per ADR 0004, GMM never auto-applies importer or loader updates —
//! it only checks the upstream release tag against the persisted
//! `installed_version` and exposes a badge. The user must click Apply
//! to actually reinstall. A per-game "Importer Pin" setting suppresses
//! the prompt entirely (the ban-wave escape hatch the ADR calls out).
//!
//! This module is the small, pure orchestration that translates "I
//! know a latest tag and an installed tag" into a typed
//! [`UpdateStatus`]. Tests drive it directly; production wires it to
//! [`crate::core::importer::fetch_latest_release`].

use serde::{Deserialize, Serialize};

use super::error::Result;
use super::games::GameCode;
use super::settings::{get as get_setting, put as put_setting};

/// Settings keys for the update subsystem.
pub mod keys {
    use super::GameCode;

    pub fn importer_installed(game: GameCode) -> String {
        format!("importer.{}.installed_version", game.as_str())
    }

    pub fn importer_pinned(game: GameCode) -> String {
        format!("importer.{}.pinned_version", game.as_str())
    }

    pub fn loader_installed() -> &'static str {
        "loader.installed_version"
    }
}

/// What [`compute_status`] decided. Travels through the Tauri command
/// boundary so the UI can render the badge + dialog directly off the
/// returned shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// `true` when there is a newer release than installed AND the
    /// user has not pinned. False clears the badge.
    pub available: bool,
    /// Tag of the version currently installed (or `None` if we've
    /// never recorded one).
    pub installed_version: Option<String>,
    /// Latest upstream tag (or `None` if the fetch failed silently).
    pub latest_version: Option<String>,
    /// `true` when the user has pinned the importer for this game.
    pub pinned: bool,
    /// `true` when latest is non-None and not equal to installed,
    /// **before** pin suppression. The UI uses it to show "An update
    /// is available but pinned" copy.
    pub upstream_ahead: bool,
}

/// Pure decision: given the strings we read from settings + the
/// upstream tag, produce the typed status. No I/O, no network — easy
/// to drive from unit tests.
pub fn compute_status(
    installed_version: Option<String>,
    latest_version: Option<String>,
    pinned: bool,
) -> UpdateStatus {
    let upstream_ahead = match (installed_version.as_deref(), latest_version.as_deref()) {
        (Some(installed), Some(latest)) => installed != latest,
        // No installed_version: treat as "fresh install" — there's
        // nothing to upgrade.
        (None, Some(_)) => false,
        _ => false,
    };
    UpdateStatus {
        available: upstream_ahead && !pinned,
        installed_version,
        latest_version,
        pinned,
        upstream_ahead,
    }
}

/// Read the per-game installed importer version (or `None` if never
/// recorded).
pub async fn importer_installed(pool: &sqlx::SqlitePool, game: GameCode) -> Result<Option<String>> {
    get_setting(pool, &keys::importer_installed(game)).await
}

/// Persist the per-game installed importer version. Called by
/// [`crate::core::Core::install_importer`] on a successful apply.
pub async fn set_importer_installed(
    pool: &sqlx::SqlitePool,
    game: GameCode,
    version: &str,
) -> Result<()> {
    put_setting(pool, &keys::importer_installed(game), Some(version)).await
}

/// Read the per-game pin (or `None` when unpinned).
pub async fn importer_pinned(pool: &sqlx::SqlitePool, game: GameCode) -> Result<Option<String>> {
    get_setting(pool, &keys::importer_pinned(game)).await
}

/// Pin (or clear) the per-game importer version. Passing `None`
/// clears the pin. The stored value is a free-form string — usually
/// the tag the user is comfortable on.
pub async fn set_importer_pinned(
    pool: &sqlx::SqlitePool,
    game: GameCode,
    version: Option<&str>,
) -> Result<()> {
    put_setting(pool, &keys::importer_pinned(game), version).await
}

/// Read the installed Loader (`3dmloader.dll`) version.
pub async fn loader_installed(pool: &sqlx::SqlitePool) -> Result<Option<String>> {
    get_setting(pool, keys::loader_installed()).await
}

/// Persist the installed Loader version.
pub async fn set_loader_installed(pool: &sqlx::SqlitePool, version: &str) -> Result<()> {
    put_setting(pool, keys::loader_installed(), Some(version)).await
}
