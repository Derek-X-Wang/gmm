//! Apply a snapshot of known updates through the ordinary, recoverable reinstall path.
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub const STOP_REASON: &str =
    "Not started because you stopped the batch. The Library was not touched.";

/// An explicit request, independent of the best-effort progress receiver.
#[derive(Clone, Default)]
pub struct BatchStop(Arc<AtomicBool>);

impl BatchStop {
    pub fn request(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_requested(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

use super::{conflicts::ConflictReport, gamebanana::Endpoints, Core, Error, GameCode, Result};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchMod {
    pub mod_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ModUpdateOutcome {
    Updated,
    Failed { reason: String },
    Skipped { reason: String },
    Unknown { reason: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchModResult {
    #[serde(flatten)]
    pub item: BatchMod,
    #[serde(flatten)]
    pub outcome: ModUpdateOutcome,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub enum ModUpdateProgress {
    Started {
        mods: Vec<BatchMod>,
    },
    Applying {
        item: BatchMod,
    },
    Completed {
        result: BatchModResult,
    },
    CheckingConflicts,
    StopReady {
        #[serde(rename = "batchId")]
        batch_id: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModUpdateBatchReport {
    pub results: Vec<BatchModResult>,
    pub stopped: bool,
    pub conflicts: Option<ConflictReport>,
    pub conflict_error: Option<String>,
}

impl Core {
    pub async fn apply_all_mod_updates(
        &self,
        game: GameCode,
        progress: impl FnMut(ModUpdateProgress),
    ) -> Result<ModUpdateBatchReport> {
        self.apply_all_mod_updates_with_endpoints(game, &Endpoints::default(), progress)
            .await
    }

    /// Existing callers run the full batch without a stop request.
    pub async fn apply_all_mod_updates_with_endpoints(
        &self,
        game: GameCode,
        endpoints: &Endpoints,
        progress: impl FnMut(ModUpdateProgress),
    ) -> Result<ModUpdateBatchReport> {
        self.apply_all_mod_updates_with_endpoints_and_stop(
            game,
            endpoints,
            &BatchStop::default(),
            progress,
        )
        .await
    }

    /// Production-called seam. A stop is read only between complete reinstalls.
    /// Losing the progress receiver never requests a stop or drops a reinstall.
    pub async fn apply_all_mod_updates_with_endpoints_and_stop(
        &self,
        game: GameCode,
        endpoints: &Endpoints,
        stop: &BatchStop,
        mut progress: impl FnMut(ModUpdateProgress),
    ) -> Result<ModUpdateBatchReport> {
        self.ensure_no_active_session().await?;
        self.ensure_no_pending_enabled_transition().await?;
        // Match the per-Mod action's recovery gate before spending bandwidth.
        // Include all Sources, including Mods without an available update:
        // the Conflict scanner can omit those quarantined Mods too.
        let library_mods = self.list_mods(game).await?;
        let preexisting_incomplete = library_mods
            .iter()
            .any(|item| item.enabled && item.reinstall_recovery.is_some());
        let mods: Vec<_> = self
            .list_mod_updates(game)
            .await?
            .into_iter()
            .filter(|row| row.upstream_ahead)
            .map(|row| BatchMod {
                mod_id: row.mod_id,
                name: row.name,
            })
            .collect();
        progress(ModUpdateProgress::Started { mods: mods.clone() });
        let mut results = Vec::with_capacity(mods.len());
        let mut stopped = false;
        for item in mods {
            let recovery = library_mods.iter().find(|row| row.id == item.mod_id);
            // The previous Mod's reinstall has fully returned at this boundary.
            let skip_reason = if stop.is_requested() {
                stopped = true;
                Some(STOP_REASON)
            } else {
                recovery.and_then(|row| {
                if row.reinstall_recovery.is_some() {
                    Some("This Mod needs interrupted update recovery. Recover it in the Library before updating.")
                } else if row.enabled_transition_recovery.is_some() {
                    Some("This Mod needs enable/disable recovery. Recover it in the Library before updating.")
                } else {
                    None
                }
            })
            };
            let outcome = if let Some(reason) = skip_reason {
                ModUpdateOutcome::Skipped {
                    reason: reason.to_string(),
                }
            } else {
                progress(ModUpdateProgress::Applying { item: item.clone() });
                // This is exactly the path used by apply_mod_update, including
                // session fences, durable swap witnesses, rollback and quarantine.
                match self
                    .reinstall_gamebanana_mod_with_endpoints(&item.mod_id, endpoints)
                    .await
                {
                    Ok(()) => ModUpdateOutcome::Updated,
                    Err(error @ Error::ReinstallRollbackFailed { .. }) => {
                        ModUpdateOutcome::Unknown {
                            reason: error.to_string(),
                        }
                    }
                    Err(error) => ModUpdateOutcome::Failed {
                        reason: error.to_string(),
                    },
                }
            };
            let result = BatchModResult { item, outcome };
            progress(ModUpdateProgress::Completed {
                result: result.clone(),
            });
            results.push(result);
        }
        progress(ModUpdateProgress::CheckingConflicts);
        let (conflicts, conflict_error) = match self.detect_conflicts(game).await {
            Ok(report) => {
                // The scanner omits quarantined Mods. Preserve the known
                // findings without presenting that partial scan as complete.
                let incomplete = stopped
                    || preexisting_incomplete
                    || results
                        .iter()
                        .any(|result| matches!(result.outcome, ModUpdateOutcome::Unknown { .. }));
                (Some(report), incomplete.then(|| if stopped {
                    "The full Conflict reading is unverified because the batch stopped with Mods not attempted.".to_string()
                } else {
                    "The full Conflict reading is unverified because a Mod update needs recovery.".to_string()
                }))
            }
            Err(error) => (None, Some(error.to_string())),
        };
        Ok(ModUpdateBatchReport {
            results,
            stopped,
            conflicts,
            conflict_error,
        })
    }
}
