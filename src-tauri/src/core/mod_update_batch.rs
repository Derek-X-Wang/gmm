//! Apply a snapshot of known updates through the ordinary, recoverable reinstall path.
use serde::Serialize;

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
    Started { mods: Vec<BatchMod> },
    Applying { item: BatchMod },
    Completed { result: BatchModResult },
    CheckingConflicts,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModUpdateBatchReport {
    pub results: Vec<BatchModResult>,
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

    /// Production-called seam: only the upstream origin changes in tests.
    /// No cancellation point is exposed inside or between reinstalls. Losing
    /// the progress receiver does not cancel the command or its recovery work.
    pub async fn apply_all_mod_updates_with_endpoints(
        &self,
        game: GameCode,
        endpoints: &Endpoints,
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
        for item in mods {
            let recovery = library_mods.iter().find(|row| row.id == item.mod_id);
            let skip_reason = recovery.and_then(|row| {
                if row.reinstall_recovery.is_some() {
                    Some("This Mod needs interrupted update recovery. Recover it in the Library before updating.")
                } else if row.enabled_transition_recovery.is_some() {
                    Some("This Mod needs enable/disable recovery. Recover it in the Library before updating.")
                } else {
                    None
                }
            });
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
                let incomplete = preexisting_incomplete
                    || results
                        .iter()
                        .any(|result| matches!(result.outcome, ModUpdateOutcome::Unknown { .. }));
                (Some(report), incomplete.then(|| "The full Conflict reading is unverified because a Mod update needs recovery.".to_string()))
            }
            Err(error) => (None, Some(error.to_string())),
        };
        Ok(ModUpdateBatchReport {
            results,
            conflicts,
            conflict_error,
        })
    }
}
