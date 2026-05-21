//! Per-mod update detection (slice 13c).
//!
//! Walks every `source = 'gamebanana'` mod for a game, refreshes the
//! upstream version via the existing
//! [`crate::core::gamebanana::fetch_submission`] path, and surfaces a
//! per-mod badge when `upstream_version != version`. Apply re-runs
//! the slice-11 ingest in place (replacing bytes + bumping metadata),
//! preserving the existing mod ID and the user's enabled/junction
//! state.
//!
//! Per ADR 0004 the check is opt-out but never auto-applies. The
//! global toggle lives in settings (`mod_updates.enabled`); each mod
//! row has its own `update_check_enabled` flag for fine-grained
//! control.

use serde::{Deserialize, Serialize};

/// One row per polled mod. Returned to the UI so the badge can render
/// inline alongside the existing Mod list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModUpdateRow {
    /// GMM mod ID — matches `Mod.id`.
    pub mod_id: String,
    pub name: String,
    /// Version we installed (from the GameBanana fetch at adopt
    /// time). `None` if we never recorded one.
    pub installed_version: Option<String>,
    /// Most recent upstream version observed. `None` until the first
    /// poll lands.
    pub upstream_version: Option<String>,
    /// `installed_version` != `upstream_version`, ignoring nulls.
    pub upstream_ahead: bool,
    /// Per-mod opt-out: when `false`, this mod will be excluded from
    /// the next weekly check and the badge is hidden.
    pub update_check_enabled: bool,
}

/// Settings keys for the global toggle + last-check timestamp.
pub mod keys {
    pub const GLOBAL_ENABLED: &str = "mod_updates.enabled";
    pub const LAST_CHECK_AT: &str = "mod_updates.last_check_at";
}

/// Compute the `upstream_ahead` flag from the two version strings.
/// Null on either side → not ahead.
pub fn upstream_ahead(installed: Option<&str>, upstream: Option<&str>) -> bool {
    match (installed, upstream) {
        (Some(i), Some(u)) => i != u,
        _ => false,
    }
}
