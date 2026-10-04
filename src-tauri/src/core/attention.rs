//! A single, conservative view of the recovery surfaces an agent cannot see.

use super::games::GAME_PROFILES;
use super::importer::ImporterEvacuationRecovery;
use super::mods::{EnabledTransitionRecovery, ReinstallRecovery};
use super::{
    Core, Error, GameCode, InterruptedSessionLaunch, LibraryAuditReport, Result, SessionInfo,
};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttentionReport {
    pub safe_to_proceed: bool,
    pub reinstalls: Vec<ReinstallAttention>,
    pub enabled_transitions: Vec<EnabledTransitionAttention>,
    pub importer_evacuations: Vec<ImporterEvacuationAttention>,
    pub staged_library_operations: Vec<StagedLibraryOperationAttention>,
    pub session_launches: Vec<InterruptedSessionLaunch>,
    pub active_session: Option<SessionInfo>,
    pub library_audits: Vec<LibraryAuditReport>,
    pub library_root_overlaps: Vec<LibraryRootOverlap>,
    pub mod_path_overlaps: Vec<super::ModLibraryPathOverlap>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReinstallAttention {
    pub mod_id: String,
    pub game: GameCode,
    pub library_path: PathBuf,
    pub recovery: Option<ReinstallRecovery>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnabledTransitionAttention {
    pub mod_id: String,
    pub game: GameCode,
    pub intended_enabled: bool,
    pub junction_path: PathBuf,
    pub recovery: Option<EnabledTransitionRecovery>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImporterEvacuationAttention {
    pub game: GameCode,
    pub game_path: PathBuf,
    pub backup_path: PathBuf,
    pub recovery: Option<ImporterEvacuationRecovery>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRootOverlap {
    pub game: Option<GameCode>,
    pub path: PathBuf,
    pub backups: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedLibraryOperationAttention {
    pub id: String,
    pub game: GameCode,
    pub operation: &'static str,
    pub staged_path: PathBuf,
    pub recovery_error: Option<String>,
}

impl Core {
    /// Read existing reports without repairing or dismissing evidence.
    /// An unavailable report is an error, never an apparently healthy status.
    pub async fn attention_status(&self) -> Result<AttentionReport> {
        let (reinstalls, enabled_transitions, importer_evacuations, staged_library_operations) =
            self.attention_witnesses().await?;
        let session_launches = self.interrupted_session_launches().await?;
        let active_session = self.session_info().await?;
        let mod_path_overlaps = self.mod_library_path_overlaps().await?;
        let mut library_root_overlaps = Vec::new();
        let mut library_audits = Vec::new();
        if let Err(error) = self.resolved_library_root().await {
            record_overlap(error, None, &mut library_root_overlaps)?;
        }
        for profile in GAME_PROFILES {
            match self.resolved_library_root_for(profile.code).await {
                Ok(_) => library_audits.push(self.audit_library(profile.code).await?),
                Err(error) => {
                    record_overlap(error, Some(profile.code), &mut library_root_overlaps)?
                }
            }
        }
        let safe_to_proceed = reinstalls.is_empty()
            && enabled_transitions.is_empty()
            && importer_evacuations.is_empty()
            && staged_library_operations.is_empty()
            && session_launches.is_empty()
            && active_session.is_none()
            && library_root_overlaps.is_empty()
            && mod_path_overlaps.is_empty()
            && library_audits
                .iter()
                .all(|audit| audit.unreferenced.is_empty() && audit.duplicates.is_empty());
        Ok(AttentionReport {
            safe_to_proceed,
            reinstalls,
            enabled_transitions,
            importer_evacuations,
            staged_library_operations,
            session_launches,
            active_session,
            library_audits,
            library_root_overlaps,
            mod_path_overlaps,
        })
    }
}

fn record_overlap(
    error: Error,
    game: Option<GameCode>,
    overlaps: &mut Vec<LibraryRootOverlap>,
) -> Result<()> {
    match error {
        Error::LibraryRootOverlapsBackups { path, backups } => {
            overlaps.push(LibraryRootOverlap {
                game,
                path,
                backups,
            });
            Ok(())
        }
        error => Err(error),
    }
}
