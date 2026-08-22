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

/// The Loader version this GMM build ships, in upstream tag form
/// (e.g. `v0.8.8`).
///
/// Baked in by `build.rs` from the `Manifest.json` vendored beside
/// `3dmloader.dll`. The Loader is embedded via FFI, not installed
/// (ADR 0001), so "what is installed" is a property of the build, not
/// of the user's machine — there is nothing for a settings row to
/// record and nothing that could write one.
pub const SHIPPED_LOADER_VERSION: &str = env!("GMM_LOADER_VERSION");

/// Upstream repository that publishes the Loader.
pub const LOADER_REPO: &str = "SpectrumQT/XXMI-Libs-Package";

/// Anchored pattern that selects the Loader package asset out of a
/// [`LOADER_REPO`] release. Releases there publish
/// `XXMI-PACKAGE-v<version>.zip` alongside a `Manifest.json`; the filter
/// GMM shipped until #78 was `"Libs"`, which matches neither.
///
/// The same rule [`crate::core::importer::parse_latest_release`] applies
/// to Model Importer origins — #79 settled one matching rule rather than
/// two. Anchoring is what makes `Manifest.json` a non-match instead of a
/// coin toss: exactly one asset must match.
pub const LOADER_ASSET_PATTERN: &str = r"XXMI-PACKAGE-v\d+\.\d+\.\d+\.zip";

/// Settings keys for the update subsystem.
pub mod keys {
    use super::GameCode;

    pub fn importer_installed(game: GameCode) -> String {
        format!("importer.{}.installed_version", game.as_str())
    }

    pub fn importer_pinned(game: GameCode) -> String {
        format!("importer.{}.pinned_version", game.as_str())
    }

    // There is deliberately no `loader_installed` key. One existed
    // until #78 and nothing ever wrote it, so the Loader check had no
    // left-hand side. The shipped Loader version is a build-time
    // constant ([`super::SHIPPED_LOADER_VERSION`]), not user state.
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
    /// User-facing reason the check could not complete — an unreachable
    /// origin, or a release whose assets did not yield exactly one match
    /// for the origin's pattern (#79).
    ///
    /// `Some` here means "we don't know", which is a different statement
    /// from `available: false` ("we checked, nothing to apply"). Until
    /// #79 the importer path ran `.ok().flatten()` and the two were
    /// indistinguishable — the defect #78 fixed for the Loader and left
    /// standing here.
    pub check_error: Option<String>,
}

/// Pure decision: given the tag we read from settings and the outcome of
/// the upstream lookup, produce the typed status. No I/O, no network —
/// easy to drive from unit tests.
///
/// `latest` is a `Result` rather than an `Option` on purpose: a caller
/// cannot build an `UpdateStatus` without saying whether a missing
/// latest version means "upstream is current" or "we could not find
/// out". The error string is rendered verbatim in the UI.
pub fn compute_status(
    installed_version: Option<String>,
    latest: std::result::Result<String, String>,
    pinned: bool,
) -> UpdateStatus {
    let (latest_version, check_error) = match latest {
        Ok(tag) => (Some(tag), None),
        Err(message) => (None, Some(message)),
    };
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
        check_error,
    }
}

/// Read the per-game installed importer version (or `None` if never
/// recorded).
pub async fn importer_installed<'e, E>(executor: E, game: GameCode) -> Result<Option<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    get_setting(executor, &keys::importer_installed(game)).await
}

/// Persist the per-game installed importer version. Called by
/// [`crate::core::Core::install_importer`] on a successful apply.
pub async fn set_importer_installed<'e, E>(executor: E, game: GameCode, version: &str) -> Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    put_setting(executor, &keys::importer_installed(game), Some(version)).await
}

/// Read the per-game pin (or `None` when unpinned).
pub async fn importer_pinned<'e, E>(executor: E, game: GameCode) -> Result<Option<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    get_setting(executor, &keys::importer_pinned(game)).await
}

/// Pin (or clear) the per-game importer version. Passing `None`
/// clears the pin. The stored value is a free-form string — usually
/// the tag the user is comfortable on.
pub async fn set_importer_pinned<'e, E>(
    executor: E,
    game: GameCode,
    version: Option<&str>,
) -> Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    put_setting(executor, &keys::importer_pinned(game), version).await
}

/// What GMM knows about the Loader it ships versus what upstream has
/// published. Purely informational: the Loader is embedded via FFI
/// and ships inside the GMM binary (ADR 0001), so a newer upstream
/// Loader reaches users through a GMM release, not through an action
/// the user can take here.
///
/// Deliberately *not* an [`UpdateStatus`]: that type carries
/// `available` and `pinned`, which promise an Apply button and a pin
/// escape hatch. Neither exists for the Loader, and pretending
/// otherwise is what the #78 UI did.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LoaderVersionStatus {
    /// Loader version baked into this build ([`SHIPPED_LOADER_VERSION`]).
    pub shipped_version: String,
    /// Latest upstream release tag. `None` when the check failed —
    /// always read alongside `check_error`.
    pub latest_version: Option<String>,
    /// `true` only when the check succeeded *and* upstream differs
    /// from what we ship.
    pub upstream_ahead: bool,
    /// User-facing reason the check could not complete. `Some` here
    /// means "we don't know", which is a different statement from
    /// `upstream_ahead: false` ("we checked, we're current").
    pub check_error: Option<String>,
}

/// Pure decision: fold the outcome of the upstream fetch into a
/// [`LoaderVersionStatus`]. The caller renders the error to a
/// user-facing string first, so this stays free of I/O and of the
/// crate error type.
pub fn loader_status(latest: std::result::Result<String, String>) -> LoaderVersionStatus {
    match latest {
        Ok(tag) => LoaderVersionStatus {
            upstream_ahead: tag != SHIPPED_LOADER_VERSION,
            shipped_version: SHIPPED_LOADER_VERSION.to_string(),
            latest_version: Some(tag),
            check_error: None,
        },
        Err(message) => LoaderVersionStatus {
            shipped_version: SHIPPED_LOADER_VERSION.to_string(),
            latest_version: None,
            upstream_ahead: false,
            check_error: Some(message),
        },
    }
}
