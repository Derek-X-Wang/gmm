//! Apply complete per-Game snapshots through the existing per-Mod actions.
use serde::Serialize;
use sqlx::Row;
use std::collections::BTreeMap;

use super::{conflicts::ConflictReport, Core, Error, GameCode, Result};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum PresetApplyOutcome {
    Applied { reason: String },
    Skipped { reason: String },
    Failed { reason: String },
    Unknown { reason: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetApplyMod {
    pub mod_id: String,
    pub name: String,
    pub target_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetApplyResult {
    #[serde(flatten)]
    pub item: PresetApplyMod,
    #[serde(flatten)]
    pub outcome: PresetApplyOutcome,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub enum PresetApplyProgress {
    Started { mods: Vec<PresetApplyMod> },
    Applying { item: PresetApplyMod },
    Completed { result: PresetApplyResult },
    CheckingConflicts,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetApplyReport {
    pub results: Vec<PresetApplyResult>,
    pub conflicts: Option<ConflictReport>,
    pub conflict_error: Option<String>,
}

impl Core {
    pub async fn apply_preset(
        &self,
        game: GameCode,
        preset_id: &str,
        mut progress: impl FnMut(PresetApplyProgress),
    ) -> Result<PresetApplyReport> {
        if !self
            .list_presets(game)
            .await?
            .iter()
            .any(|preset| preset.id == preset_id)
        {
            return Err(Error::PresetNotFound);
        }
        self.ensure_no_active_session().await?;
        self.ensure_no_pending_enabled_transition().await?;
        let install = self
            .game_install_path(game)
            .await?
            .ok_or(Error::PresetGamePathMissing)?;
        let mods_dir = install.join("Mods");
        let metadata = super::filesystem::metadata_if_exists(&mods_dir)
            .map_err(|source| Error::Io {
                path: mods_dir.clone(),
                source,
            })?
            .ok_or(Error::PresetGamePathMissing)?;
        if !metadata.is_dir() {
            return Err(Error::PresetModsPathNotDirectory);
        }
        let mut entries: BTreeMap<String, Option<String>> = sqlx::query(
            "SELECT mod_id, active_variant_id FROM preset_entries WHERE preset_id = ? ORDER BY mod_id",
        )
        .bind(preset_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| Ok((row.try_get("mod_id")?, row.try_get("active_variant_id")?)))
        .collect::<Result<_>>()?;
        let library = self.list_mods(game).await?;
        let mut plan = Vec::new();
        for row in library {
            // Outer Option is membership; inner Option is the captured selection.
            let selection = entries.remove(&row.id);
            let item = PresetApplyMod {
                mod_id: row.id.clone(),
                name: row.name.clone(),
                target_enabled: selection.is_some(),
            };
            plan.push((item, Some(row), selection));
        }
        for (id, selection) in entries {
            // A deleted Mod has no retained name in the identity-only snapshot.
            let name: Option<String> = sqlx::query_scalar("SELECT name FROM mods WHERE id = ?")
                .bind(&id)
                .fetch_optional(&self.pool)
                .await?;
            plan.push((
                PresetApplyMod {
                    name: name.unwrap_or_else(|| format!("Mod with recorded identifier {id}")),
                    mod_id: id,
                    target_enabled: true,
                },
                None,
                Some(selection),
            ));
        }
        progress(PresetApplyProgress::Started {
            mods: plan.iter().map(|(item, _, _)| item.clone()).collect(),
        });
        let mut results = Vec::with_capacity(plan.len());
        for (item, row, selection) in plan {
            let outcome = match row {
                None => PresetApplyOutcome::Skipped { reason: format!("{} was skipped because it is no longer in this Game's Library.", item.name) },
                Some(row) if row.reinstall_recovery.is_some() || row.enabled_transition_recovery.is_some() =>
                    PresetApplyOutcome::Skipped { reason: format!("{} needs recovery. Recover it in the Library before applying a preset.", item.name) },
                Some(row) => {
                    progress(PresetApplyProgress::Applying { item: item.clone() });
                    self.apply_preset_mod(&item, row.enabled, selection, &mods_dir).await
                }
            };
            let result = PresetApplyResult { item, outcome };
            progress(PresetApplyProgress::Completed {
                result: result.clone(),
            });
            results.push(result);
        }
        progress(PresetApplyProgress::CheckingConflicts);
        let (conflicts, conflict_error) = match self.detect_conflicts(game).await {
            Ok(report) => {
                let unverified = !report.omitted.is_empty() || results.iter().any(|result| matches!(result.outcome, PresetApplyOutcome::Unknown { .. }));
                (Some(report), unverified.then(|| "The full Conflict reading is unverified because Mods were omitted or a Mod's final state is unknown. Review the affected Mods in the Library.".to_string()))
            }
            Err(_) => (None, Some("The Conflict scan could not finish. Review the Library and check Conflicts again.".to_string())),
        };
        Ok(PresetApplyReport {
            results,
            conflicts,
            conflict_error,
        })
    }

    async fn apply_preset_mod(
        &self,
        item: &PresetApplyMod,
        enabled: bool,
        selection: Option<Option<String>>,
        mods_dir: &std::path::Path,
    ) -> PresetApplyOutcome {
        let desired = match selection.as_ref() {
            Some(variant) => {
                if let Some(id) = variant {
                    match self.list_variants(&item.mod_id).await {
                        Ok(variants) if !variants.iter().any(|variant| &variant.id == id) => {
                            return PresetApplyOutcome::Failed { reason: format!("{} could not be applied because its recorded Variant is gone. Save a new preset with a valid Variant.", item.name) };
                        }
                        Err(_) => return PresetApplyOutcome::Failed { reason: format!("{} could not be read. Its preset change was not started. Try again.", item.name) },
                        _ => {}
                    }
                }
                variant.as_deref()
            }
            None => None,
        };
        let change = async {
            if selection.is_some() {
                // Trust the recorded state; filesystem drift belongs to audit_library.
                if enabled && self.active_variant_id(&item.mod_id).await?.as_deref() == desired {
                    return Ok(true);
                }
                // Select while disabled, so an interrupted selection cannot leave
                // an enabled Mod between its stored and deployed appearances.
                self.set_enabled(&item.mod_id, false, mods_dir).await?;
                self.set_active_variant(&item.mod_id, desired, mods_dir)
                    .await?;
                self.set_enabled(&item.mod_id, true, mods_dir).await?;
            } else {
                self.set_enabled(&item.mod_id, false, mods_dir).await?;
            }
            Ok(false)
        }
        .await;
        match change {
            Ok(already_matched) => PresetApplyOutcome::Applied { reason: if already_matched {
                format!("{} already matched the preset; no change was needed.", item.name)
            } else if item.target_enabled {
                format!("{} is enabled with its recorded Variant selection.", item.name)
            } else {
                format!("{} is disabled because it is not in this preset, including Mods added after it was saved.", item.name)
            } },
            Err(Error::ReinstallRecoveryQuarantined { .. } | Error::EnabledTransitionPending { .. }) => PresetApplyOutcome::Skipped { reason: format!("{} could not continue because Library recovery is pending. Check the Library for the Mod that needs recovery; it may be a different Mod. Complete that recovery, then retry.", item.name) },
            Err(Error::SessionActive { .. } | Error::SessionLaunchInProgress { .. } | Error::SessionLaunchInterrupted { .. }) => PresetApplyOutcome::Failed { reason: format!("{} could not finish because a Game Session or launch now blocks Library changes. End it, then retry the preset.", item.name) },
            // The shared actions may have committed an intermediate state before
            // returning an error. Never infer success or rollback from that error.
            Err(_) => PresetApplyOutcome::Unknown { reason: format!("GMM could not confirm the final state of {}. Check its state in the Library, then retry the preset.", item.name) },
        }
    }
}
