//! Library → Game junction reconciliation.
//!
//! The Library is the source of truth (ADR 0003); junctions in
//! `<Game>/Mods/` are projections. They can drift — the user deletes a
//! junction by accident, moves their Library directory, or the
//! filesystem changes the resolution target. This module makes that
//! drift recoverable.
//!
//! The interesting public values live here:
//!
//! * [`ReconcileResult`] — the report we emit after a pass. Cheap to
//!   move through tracing as JSON.
//! * [`ConflictingJunction`] — one entry per junction that exists but
//!   resolves somewhere unexpected. The UI surfaces these as warnings.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Summary of a reconcile or rebuild pass. The numbers are not meant
/// to be authoritative; they're for the tracing log and the UI toast.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReconcileResult {
    /// Mod IDs whose junction we (re)created during this pass.
    pub recreated: Vec<String>,
    /// Mod IDs whose junction was already healthy.
    pub healthy: Vec<String>,
    /// Junctions that exist but resolve to an unexpected target. We do
    /// not auto-fix these — the UI prompts the user.
    pub conflicting: Vec<ConflictingJunction>,
    /// Mod IDs we skipped (e.g. disabled mods don't need a junction).
    pub skipped: Vec<String>,
}

/// One entry per drifted junction. `mod_id` is the GMM Mod ID; `link`
/// is the junction path under `<Game>/Mods/`; `expected_target` is the
/// Library subpath the row says should be on the other end.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConflictingJunction {
    pub mod_id: String,
    pub link: PathBuf,
    pub expected_target: PathBuf,
}
