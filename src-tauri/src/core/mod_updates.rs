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
//! control. [`crate::core::mod_update_schedule`] runs the weekly check.

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

/// A Mod whose upstream fetch did not complete during a check.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModCheckFailure {
    pub mod_id: String,
    pub name: String,
    /// User-facing reason. A transport failure names the transport (for
    /// example an unreachable proxy), never the Mod.
    pub reason: String,
}

/// What a check did, as opposed to what the database holds afterwards.
///
/// `unchecked` lives here and not on [`ModUpdateRow`] because
/// `list_mod_updates` returns the same rows without fetching anything, so
/// a per-row field would read as "this Mod is fine" when nothing was
/// checked. A Mod whose per-Mod check is off is skipped by design and is
/// not listed. Empty `unchecked` means every Mod with its check on was
/// reached.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModUpdateCheckReport {
    pub rows: Vec<ModUpdateRow>,
    pub unchecked: Vec<ModCheckFailure>,
}

/// Settings keys for the global toggle + per-Game last-check time.
pub mod keys {
    use crate::core::games::GameCode;

    pub const GLOBAL_ENABLED: &str = "mod_updates.enabled";

    /// When a check of `game`'s Mods last reached upstream, as RFC 3339.
    ///
    /// Per Game because a check covers one Game: "GIMI was checked, ZZMI
    /// was not" has to be expressible. It means *reached*, not *ran* (see
    /// `Core::check_mod_updates_at`). The pre-scheduler global key
    /// `mod_updates.last_check_at` was never read and is not migrated.
    pub fn last_check_at(game: GameCode) -> String {
        format!("mod_updates.last_check_at.{}", game.as_str())
    }
}

/// Compute the `upstream_ahead` flag from the two version strings.
/// Null on either side → not ahead.
pub fn upstream_ahead(installed: Option<&str>, upstream: Option<&str>) -> bool {
    match (installed, upstream) {
        (Some(i), Some(u)) => i != u,
        _ => false,
    }
}
