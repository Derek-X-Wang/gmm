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
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use super::error::{Error, SurfaceFailureKind};
use super::games::GameCode;

/// Summary of a reconcile or rebuild pass. The numbers are not meant
/// to be authoritative; they're for the tracing log and the UI toast.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReconcileResult {
    /// Mod IDs whose junction we (re)created during this pass.
    pub recreated: Vec<String>,
    /// Mod IDs whose junction was already healthy.
    pub healthy: Vec<String>,
    /// Junctions needing user attention rather than an automatic fix.
    /// Two shapes land here:
    ///
    /// * the junction resolves somewhere other than the Library path
    ///   the DB records (something else re-pointed it), and
    /// * the junction resolves to the right path but that directory no
    ///   longer exists — the Library copy was deleted or moved out from
    ///   under us, so there is nothing to relink to.
    ///
    /// Neither is auto-fixed: the first would clobber whatever the user
    /// intended, and the second has no surviving source to restore.
    pub conflicting: Vec<ConflictingJunction>,
    /// Mod IDs whose junction we deleted because the Mod is disabled but
    /// a junction for it was still projected into `<Game>/Mods/`.
    ///
    /// This is the inverse of [`Self::recreated`] and the more dangerous
    /// of the two drifts: a missing junction means a Mod the user turned
    /// on isn't loading, which is self-evident, whereas a stranded
    /// junction means the Model Importer keeps loading a Mod that GMM's
    /// UI says is off — silently, with nothing in the app to explain it.
    ///
    /// Only junctions that resolve to the Library path the row records
    /// are removed. One pointing anywhere else is the user's and lands
    /// in [`Self::conflicting`] instead. Removing a junction never
    /// touches the Library copy (ADR 0003).
    pub removed: Vec<String>,
    /// Mod IDs we skipped: disabled Mods that had no junction to begin
    /// with, i.e. the ones that were already in the state they should be.
    pub skipped: Vec<String>,
    /// Mod IDs whose interrupted reinstall recovery is quarantined. Their
    /// Junction and Library state is intentionally left untouched until the
    /// witness can be settled; calling them disabled would misstate the row.
    pub quarantined: Vec<String>,
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

/// One game whose startup reconcile pass aborted. The operation stays
/// fail-closed for that game while the remaining games continue, and the
/// frontend receives the exact actionable error instead of inferring success.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StartupReconcileFailure {
    pub game: GameCode,
    pub kind: SurfaceFailureKind,
    pub error: String,
}

impl StartupReconcileFailure {
    pub fn from_error(game: GameCode, error: &Error) -> Self {
        Self {
            game,
            kind: error.surface_failure_kind(),
            error: error.to_string(),
        }
    }
}

/// Complete result of the best-effort startup pass. Failure is scoped to one
/// game: that game's pass aborts, but no failure is discarded and other games
/// are still reconciled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupReconcileReport {
    pub reconciled: Vec<(GameCode, ReconcileResult)>,
    pub failures: Vec<StartupReconcileFailure>,
}

/// In-memory bridge between the pre-window startup worker and React. It starts
/// unfinished so the frontend can poll without racing the background pass.
#[derive(Clone, Default)]
pub struct StartupReconcileState(Arc<RwLock<StartupReconcileStatus>>);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StartupReconcileStatus {
    pub finished: bool,
    pub failures: Vec<StartupReconcileFailure>,
}

impl StartupReconcileState {
    pub fn finish(&self, failures: Vec<StartupReconcileFailure>) {
        let mut status = self
            .0
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        status.finished = true;
        status.failures = failures;
    }

    pub fn snapshot(&self) -> StartupReconcileStatus {
        self.0
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}
