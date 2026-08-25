//! One writer-fence protocol for every mutation of Library-owned bytes or a
//! Mod's enabled deployment state.
//!
//! Root relocation is the exclusive form: it holds SQLite's writer claim
//! from the row snapshot through the filesystem move and the setting/row
//! commit. Potentially unbounded producers (`adopt_folder` and `import_zip`)
//! use the staged form: resolve and identify the root under a short claim,
//! perform their copy/extract without blocking SQLite, then reacquire the
//! claim and prove both the configured root name and its filesystem identity
//! are unchanged before inserting a row. Recovery/delete keep their bounded
//! filesystem ownership acts inside one claim. Enabling or disabling a Mod
//! likewise holds the claim across both its Junction mutation and `enabled`
//! update: creating or removing one reparse point is bounded, and the two
//! deployment-state changes must not be observed or overwritten separately.
//! Active-Variant retargeting uses the same fence so recovery quarantine and
//! Variant deployment cannot pass one another after either operation's guard.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::Utc;
use sqlx::{Column, Executor, Row, Sqlite};
use ulid::Ulid;

use super::library_audit::{load_duplicate_mod_records, DuplicateResolution, ReviewedDuplicateMod};
use super::library_identity::{DirectoryIdentity, IdentifiedDirectory};
use super::library_ownership::{LibraryDirectoryOwner, LibraryOwnershipSnapshot};
use super::mods::{ReinstallRecovery, ReinstallRecoveryOutcome};
#[cfg(not(any(windows, unix)))]
use super::same_path;
use super::settings::{get as get_setting, keys};
use super::{
    crash_points, junction, link_exists, path_within, resolve_link, volume, Core, Error, GameCode,
    Result,
};

pub(super) const REINSTALL_STAGING_PREFIX: &str = ".gmm-reinstall-";

const REINSTALL_SWAP_COLUMNS: [&str; 14] = [
    "token",
    "mod_id",
    "game_code",
    "library_path",
    "staged_path",
    "quarantine_path",
    "old_identity",
    "staged_identity",
    "created_at",
    "recovery_error",
    "recovery_attempted_at",
    "recovery_attempts",
    "junction_withdrawn",
    "junction_withdrawal_error",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LibraryMutation {
    FinishInterruptedDeletes,
    ResolveInterruptedStaging,
    RetryReinstallRecovery,
    SetLibraryRoot,
    SetLibraryPathForGame,
    AdoptFolder,
    ImportZip,
    RecoverUnreferencedLibraryDir,
    DeleteUnreferencedLibraryDir,
    ReinstallGamebananaMod,
    SetEnabled,
    SetActiveVariant,
    ResolveDuplicateMods,
}

impl LibraryMutation {
    pub(super) const fn function_name(self) -> &'static str {
        match self {
            Self::FinishInterruptedDeletes => "finish_interrupted_library_deletes",
            Self::ResolveInterruptedStaging => "resolve_interrupted_staging_at_startup",
            Self::RetryReinstallRecovery => "retry_reinstall_recovery",
            Self::SetLibraryRoot => "set_library_root",
            Self::SetLibraryPathForGame => "set_library_path_for_game",
            Self::AdoptFolder => "adopt_folder",
            Self::ImportZip => "import_zip",
            Self::RecoverUnreferencedLibraryDir => "recover_unreferenced_library_dir",
            Self::DeleteUnreferencedLibraryDir => "delete_unreferenced_library_dir",
            Self::ReinstallGamebananaMod => "reinstall_gamebanana_mod_with_endpoints",
            Self::SetEnabled => "set_enabled",
            Self::SetActiveVariant => "set_active_variant",
            Self::ResolveDuplicateMods => "resolve_duplicate_mods",
        }
    }
}

pub(super) struct LibraryMutationFence {
    pub(super) transaction: sqlx::Transaction<'static, Sqlite>,
}

impl LibraryMutationFence {
    pub(super) async fn commit(self) -> Result<()> {
        self.transaction.commit().await?;
        Ok(())
    }
}

/// The root name and filesystem object a staged row committer selected before
/// its unbounded filesystem work.
pub(super) struct LibraryRootSnapshot {
    game: GameCode,
    root: IdentifiedDirectory,
}

impl LibraryRootSnapshot {
    pub(super) fn path(&self) -> &Path {
        self.root.path()
    }
}

/// Identity evidence for bytes staged outside the writer fence.
pub(super) struct StagedLibraryDirectory {
    id: String,
    directory: IdentifiedDirectory,
}

#[derive(Debug, Clone)]
pub(super) struct ReinstallSwapWitness {
    token: Ulid,
    mod_id: String,
    game: GameCode,
    library_path: PathBuf,
    staged_path: PathBuf,
    quarantine_path: PathBuf,
    old_identity: DirectoryIdentity,
    staged_identity: DirectoryIdentity,
}

/// The database representation is deliberately a separate type from the
/// witness recovery is allowed to trust. Every loader selects the complete row
/// and checks its SQLite columns against `REINSTALL_SWAP_COLUMNS`; the
/// exhaustive destructure in `validate` then makes adding a ruled field a
/// compile error until its construction rule is decided. Typed identities keep
/// malformed durable keys out of every downstream filesystem classification by
/// construction.
struct UnvalidatedReinstallSwapWitness {
    token: String,
    mod_id: String,
    game_code: String,
    library_path: String,
    staged_path: String,
    quarantine_path: String,
    old_identity: String,
    staged_identity: String,
    created_at: String,
    recovery_error: Option<String>,
    recovery_attempted_at: Option<String>,
    recovery_attempts: i64,
    junction_withdrawn: i64,
    junction_withdrawal_error: Option<String>,
}

impl UnvalidatedReinstallSwapWitness {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self> {
        let mod_id: String = row.try_get("mod_id")?;
        let columns: Vec<_> = row.columns().iter().map(Column::name).collect();
        if columns.as_slice() != REINSTALL_SWAP_COLUMNS {
            return Err(Error::ReinstallWitnessCorrupt {
                mod_id,
                reason: format!(
                    "the reinstall_swaps schema columns changed from the ruled set: {columns:?}"
                ),
            });
        }
        Ok(Self {
            token: row.try_get("token")?,
            mod_id,
            game_code: row.try_get("game_code")?,
            library_path: row.try_get("library_path")?,
            staged_path: row.try_get("staged_path")?,
            quarantine_path: row.try_get("quarantine_path")?,
            old_identity: row.try_get("old_identity")?,
            staged_identity: row.try_get("staged_identity")?,
            created_at: row.try_get("created_at")?,
            recovery_error: row.try_get("recovery_error")?,
            recovery_attempted_at: row.try_get("recovery_attempted_at")?,
            recovery_attempts: row.try_get("recovery_attempts")?,
            junction_withdrawn: row.try_get("junction_withdrawn")?,
            junction_withdrawal_error: row.try_get("junction_withdrawal_error")?,
        })
    }

    fn validate(self) -> Result<ReinstallSwapWitness> {
        let Self {
            token,
            mod_id,
            game_code,
            library_path,
            staged_path,
            quarantine_path,
            old_identity,
            staged_identity,
            created_at: _created_at,
            recovery_error: _recovery_error,
            recovery_attempted_at: _recovery_attempted_at,
            recovery_attempts: _recovery_attempts,
            junction_withdrawn: _junction_withdrawn,
            junction_withdrawal_error: _junction_withdrawal_error,
        } = self;
        let corrupt = |reason| Error::ReinstallWitnessCorrupt {
            mod_id: mod_id.clone(),
            reason,
        };
        let token = Ulid::from_string(&token)
            .map_err(|_| corrupt(format!("the swap token {token:?} is not a ULID")))?;
        Ulid::from_string(&mod_id)
            .map_err(|_| corrupt(format!("the Mod ID {mod_id:?} is not a ULID")))?;
        let game = GameCode::from_str(&game_code).map_err(|_| {
            corrupt(format!(
                "the recorded value {game_code:?} is an invalid game code"
            ))
        })?;
        // These six fields order recovery and describe prior recovery attempts;
        // they are not filesystem identity evidence. Their rule at this trust
        // boundary is deliberate type decoding above, followed by exclusion
        // from the trusted witness. Dedicated recovery-state loaders consume
        // them where they affect user-visible retry and withdrawal status.
        let old_identity = DirectoryIdentity::from_durable_key(&old_identity).ok_or_else(|| {
            corrupt(format!(
                "the old directory identity {old_identity:?} is not a canonical durable identity"
            ))
        })?;
        let staged_identity = DirectoryIdentity::from_durable_key(&staged_identity).ok_or_else(|| {
            corrupt(format!(
                "the staged directory identity {staged_identity:?} is not a canonical durable identity"
            ))
        })?;
        let witness = ReinstallSwapWitness {
            token,
            mod_id,
            game,
            library_path: PathBuf::from(library_path),
            staged_path: PathBuf::from(staged_path),
            quarantine_path: PathBuf::from(quarantine_path),
            old_identity,
            staged_identity,
        };
        witness.validate_paths()?;
        Ok(witness)
    }
}

impl ReinstallSwapWitness {
    pub(super) fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self> {
        UnvalidatedReinstallSwapWitness::from_row(row)?.validate()
    }

    pub(super) fn mod_id(&self) -> &str {
        &self.mod_id
    }

    pub(super) fn library_path(&self) -> &Path {
        &self.library_path
    }

    pub(super) fn staged_path(&self) -> &Path {
        &self.staged_path
    }

    pub(super) fn old_identity(&self) -> &DirectoryIdentity {
        &self.old_identity
    }

    pub(super) fn staged_identity(&self) -> &DirectoryIdentity {
        &self.staged_identity
    }

    fn validate_paths(&self) -> Result<()> {
        let Some(root) = self.library_path.parent() else {
            return self.corrupt("the recorded live path has no Library root");
        };
        let expected_stage = root.join(format!("{REINSTALL_STAGING_PREFIX}{}", self.token));
        let expected_quarantine = root.join(format!(
            "{}{}",
            super::library_recovery::DELETE_QUARANTINE_PREFIX,
            self.token
        ));
        if self.library_path.file_name().and_then(|name| name.to_str()) != Some(&self.mod_id)
            || self.staged_path != expected_stage
            || self.quarantine_path != expected_quarantine
        {
            return self.corrupt("the recorded swap paths do not match the Mod ID and token");
        }
        Ok(())
    }

    fn corrupt<T>(&self, reason: impl Into<String>) -> Result<T> {
        Err(Error::ReinstallWitnessCorrupt {
            mod_id: self.mod_id.clone(),
            reason: reason.into(),
        })
    }

    fn uncertain<T>(&self, reason: impl Into<String>) -> Result<T> {
        Err(Error::ReinstallRecoveryUncertain {
            mod_id: self.mod_id.clone(),
            reason: reason.into(),
        })
    }
}

impl StagedLibraryDirectory {
    pub(super) fn path(&self) -> &Path {
        self.directory.path()
    }

    fn identity_key(&self) -> String {
        self.directory.identity().durable_key()
    }
}

impl Core {
    pub(super) async fn begin_library_mutation(
        &self,
        mutation: LibraryMutation,
    ) -> Result<LibraryMutationFence> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if !matches!(
            mutation,
            LibraryMutation::FinishInterruptedDeletes | LibraryMutation::ResolveInterruptedStaging
        ) {
            self.ensure_no_active_session_in_library_mutation(&mut transaction)
                .await?;
        }
        Ok(LibraryMutationFence { transaction })
    }

    /// Keep the one Mod record the user selected and discard only the other
    /// records they reviewed in the duplicate audit.
    ///
    /// SQLite cannot lock filesystem identity, so the BEGIN IMMEDIATE fence
    /// covers a fresh identity snapshot, Junction preflight/removal, and the
    /// row deletion. The caller supplies the exact reviewed IDs; any drift
    /// refuses the action instead of silently widening the user's choice.
    pub async fn resolve_duplicate_mods(
        &self,
        keeper_id: &str,
        reviewed_mods: &[ReviewedDuplicateMod],
    ) -> Result<DuplicateResolution> {
        let reviewed_fingerprints: HashMap<_, _> = reviewed_mods
            .iter()
            .map(|record| (record.id.clone(), record.fingerprint.clone()))
            .collect();
        let reviewed: HashSet<_> = reviewed_fingerprints.keys().cloned().collect();
        if reviewed.len() < 2 || reviewed.len() != reviewed_mods.len() {
            return Err(Error::DuplicateModResolutionChanged {
                reason: "the reviewed record list is incomplete or contains the same record twice"
                    .to_string(),
            });
        }
        if !reviewed.contains(keeper_id) {
            return Err(Error::DuplicateModResolutionChanged {
                reason: "the selected keeper is not one of the reviewed records".to_string(),
            });
        }

        let mut fence = self
            .begin_library_mutation(LibraryMutation::ResolveDuplicateMods)
            .await?;
        let ownership = LibraryOwnershipSnapshot::load(&mut *fence.transaction).await?;
        let keeper_path: Option<String> =
            sqlx::query_scalar("SELECT library_path FROM mods WHERE id = ?")
                .bind(keeper_id)
                .fetch_optional(&mut *fence.transaction)
                .await?;
        let keeper_path = keeper_path.ok_or_else(|| Error::DuplicateModResolutionChanged {
            reason: format!("the selected keeper {keeper_id} no longer exists"),
        })?;
        let keeper_path = PathBuf::from(keeper_path);
        let keeper_directory = IdentifiedDirectory::open(&keeper_path).map_err(|source| {
            Error::DuplicateModResolutionChanged {
                reason: format!(
                    "the selected keeper's Library directory could not be identified: {source}"
                ),
            }
        })?;
        let current: HashSet<_> = ownership
            .mod_ids_for(keeper_directory.identity())
            .iter()
            .cloned()
            .collect();
        if current != reviewed {
            return Err(Error::DuplicateModResolutionChanged {
                reason: "the set of Mod records naming this directory changed".to_string(),
            });
        }

        let witness_rows = sqlx::query("SELECT mod_id FROM reinstall_swaps")
            .fetch_all(&mut *fence.transaction)
            .await?;
        for row in witness_rows {
            let mod_id: String = row.try_get("mod_id")?;
            if reviewed.contains(&mod_id) {
                return Err(Error::DuplicateModResolutionBlockedByReinstall { mod_id });
            }
        }

        let current_records =
            load_duplicate_mod_records(&mut *fence.transaction, &reviewed).await?;
        if current_records.len() != reviewed.len()
            || current_records
                .iter()
                .any(|(id, record)| reviewed_fingerprints.get(id) != Some(&record.fingerprint))
        {
            return Err(Error::DuplicateModResolutionChanged {
                reason: "one or more reviewed records changed after the audit".to_string(),
            });
        }

        let rows = sqlx::query(
            "SELECT m.id, m.game_code, m.library_path, m.junction_dir_name,
                    m.enabled, g.install_path
             FROM mods m JOIN games g ON g.code = m.game_code",
        )
        .fetch_all(&mut *fence.transaction)
        .await?;
        let mut surviving_junctions = Vec::new();
        for row in &rows {
            let mod_id: String = row.try_get("id")?;
            if reviewed.contains(&mod_id) && mod_id != keeper_id {
                continue;
            }
            let Some(install_path) = row.try_get::<Option<String>, _>("install_path")? else {
                continue;
            };
            surviving_junctions.push((
                mod_id,
                PathBuf::from(install_path)
                    .join("Mods")
                    .join(row.try_get::<String, _>("junction_dir_name")?),
            ));
        }
        let mut seen = HashSet::new();
        let mut junctions = Vec::new();
        for row in rows {
            let mod_id: String = row.try_get("id")?;
            if !reviewed.contains(&mod_id) || mod_id == keeper_id {
                continue;
            }
            seen.insert(mod_id.clone());
            let game_code: String = row.try_get("game_code")?;
            let enabled = row.try_get::<i64, _>("enabled")? != 0;
            let library_path = PathBuf::from(row.try_get::<String, _>("library_path")?);
            if enabled {
                let target = self
                    .junction_target_for(&mod_id, &library_path, &mut *fence.transaction)
                    .await?;
                if !target.is_dir() {
                    return Err(Error::DuplicateModResolutionChanged {
                        reason: format!(
                            "duplicate Mod {mod_id}'s selected Library target is no longer a directory"
                        ),
                    });
                }
            }

            let install_path: Option<String> = row.try_get("install_path")?;
            let Some(install_path) = install_path else {
                if enabled {
                    return Err(Error::DuplicateModInstallPathMissing {
                        mod_id,
                        game: game_code,
                    });
                }
                continue;
            };
            let link = PathBuf::from(install_path)
                .join("Mods")
                .join(row.try_get::<String, _>("junction_dir_name")?);
            if !link_exists(&link)? {
                continue;
            }
            for (surviving_mod_id, surviving_link) in &surviving_junctions {
                if same_physical_link_path(&link, surviving_link)? {
                    return Err(Error::DuplicateModJunctionClaimedBySurvivor {
                        mod_id,
                        surviving_mod_id: surviving_mod_id.clone(),
                        path: link,
                    });
                }
            }
            let actual =
                resolve_link(&link).ok_or_else(|| Error::DuplicateModJunctionConflict {
                    mod_id: mod_id.clone(),
                    path: link.clone(),
                })?;
            if !path_within(&actual, &library_path) {
                return Err(Error::DuplicateModJunctionConflict { mod_id, path: link });
            }
            junctions.push((mod_id, link));
        }
        if seen.len() != reviewed.len() - 1 {
            return Err(Error::DuplicateModResolutionChanged {
                reason: "one or more reviewed duplicate records no longer exist".to_string(),
            });
        }

        // Every row, Variant target, witness, and Junction is validated before
        // the first filesystem act. If a later step fails, the rows remain and
        // ordinary reconcile can recreate any already-withdrawn enabled link.
        for (mod_id, link) in &junctions {
            withdraw_reinstall_junction(link)?;
            self.crash_point(crash_points::RESOLVE_DUPLICATES_AFTER_JUNCTION_WITHDRAWAL);
            if link_exists(link)? {
                return Err(Error::DuplicateModJunctionStillPresent {
                    mod_id: mod_id.clone(),
                    path: link.clone(),
                });
            }
        }

        let mut removed_mod_ids: Vec<_> = reviewed
            .into_iter()
            .filter(|mod_id| mod_id != keeper_id)
            .collect();
        removed_mod_ids.sort();
        for mod_id in &removed_mod_ids {
            // Break the mods.active_variant_id -> mod_variants cycle before
            // the chosen row's ON DELETE CASCADE removes its Variant set.
            sqlx::query("UPDATE mods SET active_variant_id = NULL WHERE id = ?")
                .bind(mod_id)
                .execute(&mut *fence.transaction)
                .await?;
            sqlx::query("DELETE FROM mods WHERE id = ?")
                .bind(mod_id)
                .execute(&mut *fence.transaction)
                .await?;
        }
        fence.commit().await?;
        drop(keeper_directory);

        Ok(DuplicateResolution {
            keeper_id: keeper_id.to_string(),
            removed_mod_ids,
        })
    }

    /// Create a staged adopt/import directory and commit its durable owner
    /// before returning it to code that can write source bytes.
    pub(super) async fn create_staged_library_directory(
        &self,
        game: GameCode,
        directory_name: &str,
        mutation: LibraryMutation,
    ) -> Result<(LibraryRootSnapshot, StagedLibraryDirectory)> {
        let operation = match mutation {
            LibraryMutation::AdoptFolder => "adopt",
            LibraryMutation::ImportZip => "import_zip",
            _ => unreachable!("only adopt/import create ordinary Library stages"),
        };
        let mut fence = self.begin_library_mutation(mutation).await?;
        let root_path = self
            .resolved_library_root_for_in_mutation(game, &mut fence)
            .await?;
        fs::create_dir_all(&root_path).map_err(|source| Error::Io {
            path: root_path.clone(),
            source,
        })?;
        let root = IdentifiedDirectory::open(&root_path).map_err(|source| Error::Io {
            path: root_path.clone(),
            source,
        })?;
        let snapshot = LibraryRootSnapshot { game, root };
        let staged_path = snapshot.path().join(directory_name);
        fs::create_dir(&staged_path).map_err(|source| Error::Io {
            path: staged_path.clone(),
            source,
        })?;
        let directory = match IdentifiedDirectory::open(&staged_path) {
            Ok(directory) => directory,
            Err(source) => {
                let _ = fs::remove_dir(&staged_path);
                return Err(Error::Io {
                    path: staged_path,
                    source,
                });
            }
        };
        let staged = StagedLibraryDirectory {
            id: directory_name.to_string(),
            directory,
        };
        let insert = sqlx::query(
            "INSERT INTO staged_library_operations (
                id, game_code, operation, staged_path, staged_identity, created_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&staged.id)
        .bind(game.as_str())
        .bind(operation)
        .bind(staged_path.to_string_lossy().as_ref())
        .bind(staged.identity_key())
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *fence.transaction)
        .await;
        if let Err(error) = insert {
            let _ = fence.transaction.rollback().await;
            drop(staged);
            let _ = fs::remove_dir(&staged_path);
            return Err(error.into());
        }
        if let Err(error) = fence.commit().await {
            drop(staged);
            let _ = fs::remove_dir(&staged_path);
            return Err(error);
        }
        self.crash_point(crash_points::STAGING_AFTER_WITNESS_COMMIT);
        Ok((snapshot, staged))
    }

    /// Retire the exact staging witness inside the transaction that makes its
    /// Mod row authoritative. A missing row fails closed rather than allowing
    /// an unwitnessed commit.
    pub(super) async fn retire_staging_witness_for_commit(
        &self,
        staged: &StagedLibraryDirectory,
        fence: &mut LibraryMutationFence,
    ) -> Result<()> {
        let removed = sqlx::query(
            "DELETE FROM staged_library_operations
             WHERE id = ? AND staged_path = ? AND staged_identity = ?",
        )
        .bind(&staged.id)
        .bind(staged.path().to_string_lossy().as_ref())
        .bind(staged.identity_key())
        .execute(&mut *fence.transaction)
        .await?;
        if removed.rows_affected() != 1 {
            return Err(Error::StagingWitnessChanged {
                path: staged.path().to_path_buf(),
            });
        }
        Ok(())
    }

    /// A crashed producer can no longer finish its staged bytes. Release its
    /// durable claim at startup but preserve the directory itself so the
    /// ordinary audit records it and the user can inspect, recover, or delete
    /// it explicitly. If witness deletion fails, record that obstruction on
    /// every surviving witness and stop treating those rows as active owners;
    /// otherwise the durable claim would permanently conceal the only copy of
    /// a partial import from every in-app recovery action.
    pub(super) async fn resolve_interrupted_staging_at_startup(&self) -> Result<usize> {
        let mut fence = self
            .begin_library_mutation(LibraryMutation::ResolveInterruptedStaging)
            .await?;
        let witnesses: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, staged_path
             FROM staged_library_operations ORDER BY created_at, id",
        )
        .fetch_all(&mut *fence.transaction)
        .await?;
        let removed = match sqlx::query("DELETE FROM staged_library_operations")
            .execute(&mut *fence.transaction)
            .await
        {
            Ok(removed) => removed,
            Err(error) => {
                let reason = error.to_string();
                fence.transaction.rollback().await?;
                self.record_interrupted_staging_resolution_failure(&witnesses, &reason)
                    .await?;
                return Ok(0);
            }
        };
        if let Err(error) = fence.commit().await {
            self.record_interrupted_staging_resolution_failure(&witnesses, &error.to_string())
                .await?;
            return Ok(0);
        }
        for (_, path) in witnesses {
            tracing::warn!(
                target: "gmm::library",
                path,
                "released an interrupted staging witness; preserved any staged bytes for the Library audit",
            );
        }
        Ok(removed.rows_affected() as usize)
    }

    async fn record_interrupted_staging_resolution_failure(
        &self,
        witnesses: &[(String, String)],
        reason: &str,
    ) -> Result<usize> {
        let attempted_at = Utc::now().to_rfc3339();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut marked_paths = Vec::new();
        for (id, path) in witnesses {
            let marked = sqlx::query(
                "UPDATE staged_library_operations
                 SET recovery_error = ?, recovery_attempted_at = ?,
                     recovery_attempts = recovery_attempts + 1
                 WHERE id = ?",
            )
            .bind(reason)
            .bind(&attempted_at)
            .bind(id)
            .execute(&mut *transaction)
            .await?;
            if marked.rows_affected() == 1 {
                marked_paths.push(path.clone());
            }
        }
        transaction.commit().await?;
        for path in &marked_paths {
            tracing::error!(
                target: "gmm::library",
                path,
                error = reason,
                "could not release an interrupted staging witness; preserved and exposed any staged bytes through the Library audit",
            );
        }
        Ok(marked_paths.len())
    }

    /// Change both halves of a Mod's enabled deployment state under the one
    /// Library mutation writer fence described by this module.
    pub(super) async fn set_enabled_in_library_mutation(
        &self,
        id: &str,
        enabled: bool,
        game_mods_dir: &Path,
    ) -> Result<()> {
        let mut fence = self
            .begin_library_mutation(LibraryMutation::SetEnabled)
            .await?;
        self.ensure_mod_reinstall_is_usable(
            id,
            &mut *fence.transaction,
            crash_points::SET_ENABLED_AFTER_REINSTALL_GUARD,
        )
        .await?;
        let row =
            sqlx::query("SELECT junction_dir_name, library_path, enabled FROM mods WHERE id = ?")
                .bind(id)
                .fetch_one(&mut *fence.transaction)
                .await?;

        let junction_dir_name: String = row.try_get("junction_dir_name")?;
        let library_path = PathBuf::from(row.try_get::<String, _>("library_path")?);
        let current_enabled: i64 = row.try_get("enabled")?;
        let link = game_mods_dir.join(junction_dir_name);

        match (current_enabled != 0, enabled) {
            (false, true) => {
                let target = self
                    .junction_target_for(id, &library_path, &mut *fence.transaction)
                    .await?;
                volume::require_ntfs_pair(game_mods_dir, &target)?;
                junction::create(&link, &target)?;
                self.crash_point(crash_points::SET_ENABLED_AFTER_JUNCTION_CREATE);
            }
            (true, false) => {
                junction::remove(&link)?;
                self.crash_point(crash_points::SET_ENABLED_AFTER_JUNCTION_REMOVE);
            }
            _ => {}
        }

        sqlx::query("UPDATE mods SET enabled = ? WHERE id = ?")
            .bind(if enabled { 1_i64 } else { 0_i64 })
            .bind(id)
            .execute(&mut *fence.transaction)
            .await?;
        self.crash_point(crash_points::SET_ENABLED_AFTER_DB_UPDATE);
        fence.commit().await
    }

    /// Change the selected Variant and its enabled Junction while holding the
    /// same writer fence used by reinstall recovery and other Library changes.
    pub(super) async fn set_active_variant_in_library_mutation(
        &self,
        mod_id: &str,
        variant_id: &str,
        game_mods_dir: &Path,
    ) -> Result<()> {
        let mut fence = self
            .begin_library_mutation(LibraryMutation::SetActiveVariant)
            .await?;
        self.ensure_mod_reinstall_is_usable(
            mod_id,
            &mut *fence.transaction,
            crash_points::SET_ACTIVE_VARIANT_AFTER_REINSTALL_GUARD,
        )
        .await?;

        // Validate the Variant belongs to this Mod before persisting it.
        sqlx::query("SELECT subpath FROM mod_variants WHERE id = ? AND mod_id = ?")
            .bind(variant_id)
            .bind(mod_id)
            .fetch_one(&mut *fence.transaction)
            .await?;

        let mod_row =
            sqlx::query("SELECT junction_dir_name, library_path, enabled FROM mods WHERE id = ?")
                .bind(mod_id)
                .fetch_one(&mut *fence.transaction)
                .await?;
        let junction_dir_name: String = mod_row.try_get("junction_dir_name")?;
        let enabled: i64 = mod_row.try_get("enabled")?;
        let library_path = PathBuf::from(mod_row.try_get::<String, _>("library_path")?);

        sqlx::query("UPDATE mods SET active_variant_id = ? WHERE id = ?")
            .bind(variant_id)
            .bind(mod_id)
            .execute(&mut *fence.transaction)
            .await?;
        self.crash_point(crash_points::SET_ACTIVE_VARIANT_AFTER_DB_UPDATE);

        if enabled != 0 {
            let link = game_mods_dir.join(&junction_dir_name);
            if link_exists(&link)? {
                junction::remove(&link)?;
                self.crash_point(crash_points::SET_ACTIVE_VARIANT_AFTER_JUNCTION_REMOVE);
            }
            let target = self
                .junction_target_for(mod_id, &library_path, &mut *fence.transaction)
                .await?;
            volume::require_ntfs_pair(game_mods_dir, &target)?;
            junction::create(&link, &target)?;
        }

        fence.commit().await
    }

    pub(super) async fn reinstall_swap_witness(
        &self,
        token: Ulid,
        fence: &mut LibraryMutationFence,
    ) -> Result<ReinstallSwapWitness> {
        let row = sqlx::query("SELECT * FROM reinstall_swaps WHERE token = ?")
            .bind(token.to_string())
            .fetch_one(&mut *fence.transaction)
            .await?;
        let witness = ReinstallSwapWitness::from_row(&row)?;
        self.rebase_reinstall_swap_witness(witness, fence).await
    }

    /// Attempt every durable reinstall witness independently at startup.
    /// Filesystem/identity failures quarantine only that Mod and remain
    /// retryable through the same witness. Database and schema failures still
    /// abort Core construction because they are not evidence about one Mod's
    /// bytes.
    pub(super) async fn recover_interrupted_reinstalls_at_startup(&self) -> Result<usize> {
        let rows = sqlx::query("SELECT token FROM reinstall_swaps ORDER BY created_at, token")
            .fetch_all(&self.pool)
            .await?;
        let mut rolled_back = 0;
        for row in rows {
            let token: String = row.try_get("token")?;
            match self
                .attempt_reinstall_recovery(&token, LibraryMutation::FinishInterruptedDeletes)
                .await?
            {
                ReinstallRecoveryOutcome::Recovered => rolled_back += 1,
                ReinstallRecoveryOutcome::AlreadyRecovered => {}
                ReinstallRecoveryOutcome::Quarantined { recovery } => tracing::error!(
                    target: "gmm::library",
                    token,
                    error = %recovery.reason,
                    "interrupted reinstall remains quarantined; the rest of GMM will start",
                ),
            }
        }
        Ok(rolled_back)
    }

    /// Retry the exact verified rollback that startup attempted for one Mod.
    /// A witness absent at the initial lookup means startup or an earlier
    /// completed retry already settled it. Concurrent calls that both observe
    /// the token are serialized, but the later call can report that the row
    /// vanished rather than claiming in-flight retries are idempotent.
    pub async fn retry_reinstall_recovery(&self, mod_id: &str) -> Result<ReinstallRecoveryOutcome> {
        let token: Option<String> =
            sqlx::query_scalar("SELECT token FROM reinstall_swaps WHERE mod_id = ?")
                .bind(mod_id)
                .fetch_optional(&self.pool)
                .await?;
        self.crash_point(super::crash_points::RETRY_REINSTALL_AFTER_WITNESS_LOOKUP);
        let Some(token) = token else {
            return Ok(ReinstallRecoveryOutcome::AlreadyRecovered);
        };
        let outcome = self
            .attempt_reinstall_recovery(&token, LibraryMutation::RetryReinstallRecovery)
            .await?;
        if matches!(outcome, ReinstallRecoveryOutcome::Recovered) {
            if let Err(error) = self.finish_interrupted_library_deletes().await {
                tracing::warn!(
                    target: "gmm::library",
                    mod_id,
                    error = %error,
                    "reinstall recovery succeeded but ordinary quarantine cleanup was deferred",
                );
            }
        }
        Ok(outcome)
    }

    async fn attempt_reinstall_recovery(
        &self,
        token_raw: &str,
        mutation: LibraryMutation,
    ) -> Result<ReinstallRecoveryOutcome> {
        let token = Ulid::from_string(token_raw).map_err(|_| Error::ReinstallWitnessCorrupt {
            mod_id: "<unknown>".to_string(),
            reason: format!("the swap token {token_raw:?} is not a ULID"),
        })?;
        let mut fence = self.begin_library_mutation(mutation).await?;
        let recovery = async {
            let Some(witness) = self
                .reinstall_swap_witness_if_present(token, &mut fence)
                .await?
            else {
                return Ok(None);
            };
            self.rollback_reinstall_swap_in_mutation(&witness, &mut fence)
                .await?;
            Ok(Some(()))
        }
        .await;
        match recovery {
            Ok(Some(())) => {
                fence.commit().await?;
                Ok(ReinstallRecoveryOutcome::Recovered)
            }
            Ok(None) => {
                fence.commit().await?;
                Ok(ReinstallRecoveryOutcome::AlreadyRecovered)
            }
            Err(error) if quarantinable_reinstall_failure(&error) => {
                fence.transaction.rollback().await?;
                let recovery = self
                    .record_reinstall_recovery_failure(token_raw, &error.to_string())
                    .await?;
                match recovery {
                    Some(recovery) => Ok(ReinstallRecoveryOutcome::Quarantined { recovery }),
                    None => Ok(ReinstallRecoveryOutcome::AlreadyRecovered),
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn reinstall_swap_witness_if_present(
        &self,
        token: Ulid,
        fence: &mut LibraryMutationFence,
    ) -> Result<Option<ReinstallSwapWitness>> {
        let row = sqlx::query("SELECT * FROM reinstall_swaps WHERE token = ?")
            .bind(token.to_string())
            .fetch_optional(&mut *fence.transaction)
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let witness = ReinstallSwapWitness::from_row(&row)?;
        self.rebase_reinstall_swap_witness(witness, fence)
            .await
            .map(Some)
    }

    async fn record_reinstall_recovery_failure(
        &self,
        token: &str,
        reason: &str,
    ) -> Result<Option<ReinstallRecovery>> {
        let attempted_at = chrono::Utc::now().to_rfc3339();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let updated = sqlx::query(
            "UPDATE reinstall_swaps
             SET recovery_error = ?, recovery_attempted_at = ?,
                 recovery_attempts = recovery_attempts + 1
             WHERE token = ?",
        )
        .bind(reason)
        .bind(&attempted_at)
        .bind(token)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() == 0 {
            transaction.commit().await?;
            return Ok(None);
        }

        let junction = sqlx::query(
            "SELECT m.junction_dir_name, g.install_path
             FROM reinstall_swaps rs
             JOIN mods m ON m.id = rs.mod_id
             JOIN games g ON g.code = m.game_code
             WHERE rs.token = ?",
        )
        .bind(token)
        .fetch_one(&mut *transaction)
        .await?;
        let link = junction
            .try_get::<Option<String>, _>("install_path")?
            .map(PathBuf::from)
            .map(|install_path| {
                Ok::<PathBuf, Error>(
                    install_path
                        .join("Mods")
                        .join(junction.try_get::<String, _>("junction_dir_name")?),
                )
            })
            .transpose()?;
        transaction.commit().await?;

        // The quarantine is durable before the fallible Junction operation.
        // A process death here leaves junction_withdrawn = 0, which startup,
        // retry, reconcile, and rebuild all retry without losing the witness.
        self.withdraw_quarantined_reinstall_junction(token, link.as_deref())
            .await?;
        self.reinstall_recovery_for_token(token).await
    }

    pub(super) async fn withdraw_quarantined_reinstall_junction(
        &self,
        token: &str,
        link: Option<&Path>,
    ) -> Result<Option<bool>> {
        let state: Option<i64> = sqlx::query_scalar(
            "SELECT junction_withdrawn FROM reinstall_swaps
             WHERE token = ? AND recovery_error IS NOT NULL",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await?;
        let Some(state) = state else {
            return Ok(None);
        };
        if state != 0 {
            let absent = match link {
                Some(path) => !super::link_exists(path)?,
                None => true,
            };
            if absent {
                return Ok(Some(true));
            }
        }

        let withdrawal = link.map_or(Ok(()), withdraw_reinstall_junction);
        let (withdrawn, withdrawal_error) = match withdrawal {
            Ok(()) => (1_i64, None),
            Err(error) => (0_i64, Some(error.to_string())),
        };
        let updated = sqlx::query(
            "UPDATE reinstall_swaps
             SET junction_withdrawn = ?, junction_withdrawal_error = ?
             WHERE token = ? AND recovery_error IS NOT NULL",
        )
        .bind(withdrawn)
        .bind(&withdrawal_error)
        .bind(token)
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        if let Some(error) = withdrawal_error {
            tracing::error!(
                target: "gmm::library",
                token,
                error,
                "quarantined reinstall may still be deployed because Junction withdrawal failed",
            );
        }
        Ok(Some(withdrawn != 0))
    }

    async fn reinstall_recovery_for_token(&self, token: &str) -> Result<Option<ReinstallRecovery>> {
        let row = sqlx::query(
            "SELECT recovery_error, recovery_attempted_at, recovery_attempts,
                    library_path, staged_path, quarantine_path,
                    junction_withdrawn, junction_withdrawal_error
             FROM reinstall_swaps WHERE token = ? AND recovery_error IS NOT NULL",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(reinstall_recovery_from_row).transpose()
    }

    /// Current relocation refuses to move a subtree with an active witness,
    /// because its cross-volume copy fallback cannot preserve identity. A
    /// recorded root different from the current effective root is therefore
    /// corrupt durable state, not evidence that relocation legitimately carried
    /// the witness elsewhere. Only after proving that root do we rebase the
    /// sibling spellings to the current Mod row.
    pub(super) async fn rebase_reinstall_swap_witness(
        &self,
        mut witness: ReinstallSwapWitness,
        fence: &mut LibraryMutationFence,
    ) -> Result<ReinstallSwapWitness> {
        let current_library_path: String =
            sqlx::query_scalar("SELECT library_path FROM mods WHERE id = ? AND game_code = ?")
                .bind(&witness.mod_id)
                .bind(witness.game.as_str())
                .fetch_one(&mut *fence.transaction)
                .await?;
        let current_library_path = PathBuf::from(current_library_path);
        let current_root = self
            .resolved_library_root_for_in_mutation(witness.game, fence)
            .await?;
        let recorded_root = witness
            .library_path
            .parent()
            .expect("validated witness paths always have a parent");
        if !super::same_path(recorded_root, &current_root) {
            return witness
                .corrupt("the recorded swap root is not the Mod's effective Library root");
        }
        let current_mod_root =
            current_library_path
                .parent()
                .ok_or_else(|| Error::ReinstallWitnessCorrupt {
                    mod_id: witness.mod_id.clone(),
                    reason: "the current Mod row's Library path has no parent".to_string(),
                })?;
        if !super::same_path(current_mod_root, &current_root)
            || current_library_path
                .file_name()
                .and_then(|name| name.to_str())
                != Some(&witness.mod_id)
        {
            return witness.corrupt(
                "the current Mod row is not a direct child of its effective Library root",
            );
        }
        witness.library_path = current_library_path;
        witness.staged_path =
            current_root.join(format!("{REINSTALL_STAGING_PREFIX}{}", witness.token));
        witness.quarantine_path = current_root.join(format!(
            "{}{}",
            super::library_recovery::DELETE_QUARANTINE_PREFIX,
            witness.token
        ));
        witness.validate_paths()?;
        Ok(witness)
    }

    pub(super) async fn rollback_reinstall_swap(&self, token: Ulid) -> Result<()> {
        let mut fence = self
            .begin_library_mutation(LibraryMutation::FinishInterruptedDeletes)
            .await?;
        let witness = self.reinstall_swap_witness(token, &mut fence).await?;
        self.rollback_reinstall_swap_in_mutation(&witness, &mut fence)
            .await?;
        fence.commit().await?;
        // The rollback moved the staged replacement into the ordinary shared
        // delete quarantine. Finish it now; startup repeats the same verified
        // purge if this process stops first.
        if let Err(error) = self.finish_interrupted_library_deletes().await {
            tracing::warn!(
                target: "gmm::library",
                mod_id = %witness.mod_id,
                error = %error,
                "reinstall rollback succeeded but ordinary quarantine cleanup was deferred"
            );
        }
        Ok(())
    }

    async fn rollback_reinstall_swap_in_mutation(
        &self,
        witness: &ReinstallSwapWitness,
        fence: &mut LibraryMutationFence,
    ) -> Result<()> {
        let live = identified_if_exists(&witness.library_path)?;
        let staged = identified_if_exists(&witness.staged_path)?;
        let quarantine = identified_if_exists(&witness.quarantine_path)?;

        reject_unexpected_identity(
            witness,
            "live",
            live.as_ref(),
            &[&witness.old_identity, &witness.staged_identity],
        )?;
        reject_unexpected_identity(
            witness,
            "staged",
            staged.as_ref(),
            &[&witness.staged_identity],
        )?;
        reject_unexpected_identity(
            witness,
            "quarantine",
            quarantine.as_ref(),
            &[&witness.old_identity],
        )?;

        let old_is_live = live
            .as_ref()
            .is_some_and(|directory| directory.identity() == &witness.old_identity);
        let old_is_quarantined = quarantine
            .as_ref()
            .is_some_and(|directory| directory.identity() == &witness.old_identity);
        if old_is_live == old_is_quarantined {
            return witness.uncertain(if old_is_live {
                "the old directory appears at both its live and quarantine names"
            } else {
                "the old directory is at neither its live nor quarantine name"
            });
        }

        // Release directory handles before Windows renames the names they
        // identify. The identities remain recorded in the witness.
        drop(live);
        drop(staged);
        drop(quarantine);

        if old_is_quarantined {
            if let Some(current_live) = identified_if_exists(&witness.library_path)? {
                move_live_replacement_back_to_stage(witness, current_live)?;
            }
            fs::rename(&witness.quarantine_path, &witness.library_path).map_err(|source| {
                Error::Io {
                    path: witness.quarantine_path.clone(),
                    source,
                }
            })?;
        }

        // Once the old object is back at the live name, the swap quarantine's
        // ownership intent is stranded and must not be allowed to classify a
        // later replacement at that reserved name.
        let swap_intent = witness.quarantine_path.with_file_name(format!(
            "{}{}.intent",
            super::library_recovery::DELETE_QUARANTINE_PREFIX,
            witness.token
        ));
        match fs::remove_file(&swap_intent) {
            Ok(()) => {}
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(Error::Io {
                    path: swap_intent,
                    source,
                })
            }
        }

        if let Some(replacement) = identified_if_exists(&witness.staged_path)? {
            if replacement.identity() != &witness.staged_identity {
                return witness.uncertain("the staging name changed identity during rollback");
            }
            let staged_quarantine =
                self.quarantine_library_directory(&witness.staged_path, &replacement, None, None)?;
            drop(replacement);
            drop(staged_quarantine);
        }

        self.restore_reinstall_junction_in_mutation(witness, fence)
            .await?;
        sqlx::query("DELETE FROM reinstall_swaps WHERE token = ?")
            .bind(witness.token.to_string())
            .execute(&mut *fence.transaction)
            .await?;
        Ok(())
    }

    async fn restore_reinstall_junction_in_mutation(
        &self,
        witness: &ReinstallSwapWitness,
        fence: &mut LibraryMutationFence,
    ) -> Result<()> {
        let row = sqlx::query(
            "SELECT m.enabled, m.junction_dir_name, g.install_path
             FROM mods m JOIN games g ON g.code = m.game_code
             WHERE m.id = ? AND m.game_code = ?",
        )
        .bind(&witness.mod_id)
        .bind(witness.game.as_str())
        .fetch_one(&mut *fence.transaction)
        .await?;
        let Some(install) = row
            .try_get::<Option<String>, _>("install_path")?
            .map(PathBuf::from)
        else {
            return Ok(());
        };
        let mods_dir = install.join("Mods");
        let link = mods_dir.join(row.try_get::<String, _>("junction_dir_name")?);
        if super::link_exists(&link)? {
            junction::remove(&link)?;
        }
        if row.try_get::<i64, _>("enabled")? == 0 {
            return Ok(());
        }
        let target = self
            .junction_target_for(
                &witness.mod_id,
                &witness.library_path,
                &mut *fence.transaction,
            )
            .await?;
        fs::create_dir_all(&mods_dir).map_err(|source| Error::Io {
            path: mods_dir.clone(),
            source,
        })?;
        volume::require_ntfs_pair(&mods_dir, &target)?;
        junction::create(&link, &target)
    }

    pub(super) async fn revalidate_library_root_for_mutation(
        &self,
        snapshot: &LibraryRootSnapshot,
        mutation: LibraryMutation,
    ) -> Result<LibraryMutationFence> {
        let mut fence = self.begin_library_mutation(mutation).await?;
        let current = self
            .resolved_library_root_for_in_mutation(snapshot.game, &mut fence)
            .await?;
        let current_directory =
            IdentifiedDirectory::open(&current).map_err(|source| Error::Io {
                path: current.clone(),
                source,
            })?;
        if current != snapshot.root.path()
            || current_directory.identity() != snapshot.root.identity()
        {
            return Err(Error::LibraryRootChangedDuringMutation {
                mutation: mutation.function_name(),
                previous: snapshot.root.path().to_path_buf(),
                current,
            });
        }
        Ok(fence)
    }

    pub(super) async fn cleanup_staged_library_dir(
        &self,
        snapshot: &LibraryRootSnapshot,
        staged: StagedLibraryDirectory,
        mutation: LibraryMutation,
    ) {
        let staged_path = staged.path().to_path_buf();
        let mut fence = match self.begin_library_mutation(mutation).await {
            Ok(fence) => fence,
            Err(error) => {
                tracing::warn!(
                    target: "gmm::library",
                    path = %staged_path.display(),
                    error = %error,
                    "could not fence staged Library cleanup; leaving bytes for orphan audit",
                );
                return;
            }
        };
        let retired = match sqlx::query(
            "DELETE FROM staged_library_operations
             WHERE id = ? AND staged_path = ? AND staged_identity = ?",
        )
        .bind(&staged.id)
        .bind(staged.path().to_string_lossy().as_ref())
        .bind(staged.identity_key())
        .execute(&mut *fence.transaction)
        .await
        {
            Ok(result) if result.rows_affected() == 1 => true,
            Ok(_) => {
                tracing::warn!(
                    target: "gmm::library",
                    path = %staged_path.display(),
                    "staged Library witness changed before cleanup; preserving every candidate",
                );
                false
            }
            Err(error) => {
                tracing::warn!(
                    target: "gmm::library",
                    path = %staged_path.display(),
                    error = %error,
                    "could not retire staged Library witness; preserving every candidate",
                );
                false
            }
        };
        if retired {
            self.crash_point(crash_points::STAGED_CLEANUP_AFTER_WITNESS_RETIRE);
        }
        let current_root = match self
            .resolved_library_root_for_in_mutation(snapshot.game, &mut fence)
            .await
        {
            Ok(root) => root,
            Err(error) => {
                tracing::warn!(
                    target: "gmm::library",
                    path = %staged_path.display(),
                    error = %error,
                    "could not resolve the current Library root during staged cleanup; leaving bytes for orphan audit",
                );
                return;
            }
        };
        let directory_name = staged_path
            .file_name()
            .expect("a staged Library directory has a final component");
        let current_path = current_root.join(directory_name);
        let mut candidates = vec![staged_path.clone()];
        if current_path != staged_path {
            candidates.push(current_path);
        }

        let mut deletion_candidate = None;
        for path in candidates {
            let current = match IdentifiedDirectory::open(&path) {
                Ok(directory) => directory,
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => {
                    tracing::warn!(
                        target: "gmm::library",
                        path = %path.display(),
                        error = %source,
                        "could not identify a staged Library cleanup candidate; leaving it for orphan audit",
                    );
                    continue;
                }
            };
            if current.identity() != staged.directory.identity() {
                tracing::warn!(
                    target: "gmm::library",
                    path = %path.display(),
                    "staged Library cleanup candidate changed identity; leaving it for orphan audit",
                );
                continue;
            }
            deletion_candidate = Some((path, current));
            break;
        }

        let mut quarantined = None;
        if retired {
            if let Some((path, current)) = deletion_candidate {
                let ownership = match LibraryOwnershipSnapshot::load(&mut *fence.transaction).await
                {
                    Ok(ownership) => Some(ownership),
                    Err(error) => {
                        tracing::warn!(
                            target: "gmm::library",
                            path = %path.display(),
                            error = %error,
                            "could not prove a staged Library directory is unowned; leaving it for orphan audit",
                        );
                        None
                    }
                };
                match ownership
                    .as_ref()
                    .and_then(|ownership| ownership.owner_of(current.identity()))
                {
                    Some(owner) => {
                        let owner = match owner {
                            LibraryDirectoryOwner::Mod => "a Mod",
                            LibraryDirectoryOwner::ActiveReinstall => "interrupted reinstall state",
                            LibraryDirectoryOwner::ActiveStaging => "another staging operation",
                        };
                        tracing::warn!(
                            target: "gmm::library",
                            path = %path.display(),
                            owner,
                            "staged Library cleanup candidate is now owned; leaving it intact",
                        );
                    }
                    None if ownership.is_some() => {
                        self.crash_point(
                            super::crash_points::STAGED_CLEANUP_BEFORE_QUARANTINE_MOVE,
                        );
                        match self.quarantine_library_directory(&path, &current, None, None) {
                            Ok(directory) => {
                                self.crash_point(
                                    super::crash_points::STAGED_CLEANUP_AFTER_QUARANTINE_MOVE,
                                );
                                // Windows keeps a removed directory name visible until
                                // every open handle closes, even when the handles share
                                // DELETE. The intent now records the object moved to
                                // GMM's reserved name, so release the old handles before
                                // the path-based purge; #172 tracks anchoring removal.
                                drop(current);
                                drop(staged);
                                quarantined = Some(directory);
                            }
                            Err(error) => tracing::warn!(
                                target: "gmm::library",
                                path = %path.display(),
                                error = %error,
                                "could not quarantine a staged Library cleanup candidate; leaving bytes for orphan audit",
                            ),
                        }
                    }
                    // Snapshot errors mean ownership is uncertain, so preserve the
                    // bytes for the read-only orphan audit.
                    None => {}
                }
            }
        }
        if let Err(error) = fence.commit().await {
            tracing::warn!(
                target: "gmm::library",
                path = %staged_path.display(),
                error = %error,
                "could not commit the staged Library cleanup fence",
            );
        }
        if let Some(quarantined) = quarantined {
            self.crash_point(super::crash_points::STAGED_CLEANUP_BEFORE_QUARANTINE_PURGE);
            match quarantined.purge(false) {
                Ok(super::library_recovery::QuarantinePurgeOutcome::Reclaimed(_)) => {}
                Ok(super::library_recovery::QuarantinePurgeOutcome::Deferred { path, error }) => {
                    tracing::warn!(
                        target: "gmm::library",
                        path = %staged_path.display(),
                        quarantine = %path.display(),
                        error = %error,
                        "GMM could not reclaim the staged Library cleanup bytes now; later startups will retry while the directory remains at its reserved name",
                    );
                }
                Ok(super::library_recovery::QuarantinePurgeOutcome::OwnershipLost) => {
                    tracing::error!(
                        target: "gmm::library",
                        path = %staged_path.display(),
                        "GMM cannot locate the staged Library cleanup quarantine or determine whether its bytes were reclaimed; a later startup will retry only if GMM can again verify the original directory at its reserved name",
                    );
                }
                Err(error) => tracing::warn!(
                    target: "gmm::library",
                    path = %staged_path.display(),
                    error = %error,
                    "could not inspect or purge a staged Library cleanup quarantine",
                ),
            }
        }
    }

    pub(super) async fn resolved_library_root_in_mutation(
        &self,
        fence: &mut LibraryMutationFence,
    ) -> Result<PathBuf> {
        let override_path = get_setting(&mut *fence.transaction, keys::library_root()).await?;
        Ok(override_path
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_library_root.clone()))
    }

    pub(super) async fn resolved_library_root_for_in_mutation(
        &self,
        game: GameCode,
        fence: &mut LibraryMutationFence,
    ) -> Result<PathBuf> {
        let per_game =
            get_setting(&mut *fence.transaction, &keys::library_root_for_game(game)).await?;
        if let Some(path) = per_game {
            return Ok(PathBuf::from(path));
        }
        Ok(self
            .resolved_library_root_in_mutation(fence)
            .await?
            .join(game.as_str()))
    }

    async fn ensure_no_active_session_in_library_mutation(
        &self,
        transaction: &mut sqlx::Transaction<'_, Sqlite>,
    ) -> Result<()> {
        let row = sqlx::query("SELECT game_code, started_at FROM active_session WHERE id = 1")
            .fetch_optional(&mut **transaction)
            .await?;
        if let Some(row) = row {
            return Err(Error::SessionActive {
                game: row.try_get("game_code")?,
                since: row.try_get("started_at")?,
            });
        }
        Ok(())
    }

    async fn ensure_mod_reinstall_is_usable<'e, E>(
        &self,
        mod_id: &str,
        executor: E,
        checked_at: &'static str,
    ) -> Result<()>
    where
        E: Executor<'e, Database = Sqlite>,
    {
        let quarantined = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM reinstall_swaps
             WHERE mod_id = ? AND recovery_error IS NOT NULL",
        )
        .bind(mod_id)
        .fetch_one(executor)
        .await?;
        self.crash_point(checked_at);
        if quarantined != 0 {
            return Err(Error::ReinstallRecoveryQuarantined {
                mod_id: mod_id.to_string(),
            });
        }
        Ok(())
    }
}

fn reinstall_recovery_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<ReinstallRecovery> {
    Ok(ReinstallRecovery {
        reason: row.try_get("recovery_error")?,
        attempted_at: row.try_get("recovery_attempted_at")?,
        attempts: row.try_get::<i64, _>("recovery_attempts")? as u32,
        library_path: PathBuf::from(row.try_get::<String, _>("library_path")?),
        staged_path: PathBuf::from(row.try_get::<String, _>("staged_path")?),
        quarantine_path: PathBuf::from(row.try_get::<String, _>("quarantine_path")?),
        junction_withdrawn: row.try_get::<i64, _>("junction_withdrawn")? != 0,
        junction_withdrawal_error: row.try_get("junction_withdrawal_error")?,
    })
}

fn quarantinable_reinstall_failure(error: &Error) -> bool {
    matches!(
        error,
        Error::Io { .. } | Error::ReinstallRecoveryUncertain { .. } | Error::NonNtfsVolume { .. }
    )
}

pub(super) fn withdraw_reinstall_junction(link: &Path) -> Result<()> {
    if !super::link_exists(link)? {
        return Ok(());
    }
    if super::resolve_link(link).is_none() {
        return Err(Error::Io {
            path: link.to_path_buf(),
            source: io::Error::other(
                "the Mod deployment path is not a Junction GMM can safely remove",
            ),
        });
    }
    junction::remove(link)
}

fn identified_if_exists(path: &Path) -> Result<Option<IdentifiedDirectory>> {
    match IdentifiedDirectory::open(path) {
        Ok(directory) => Ok(Some(directory)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn move_live_replacement_back_to_stage(
    witness: &ReinstallSwapWitness,
    current_live: IdentifiedDirectory,
) -> Result<()> {
    if current_live.identity() != &witness.staged_identity {
        return witness.uncertain("the live name no longer identifies the staged replacement");
    }
    if entry_exists(&witness.staged_path)? {
        return witness.uncertain("both the live and staging names contain replacement bytes");
    }
    drop(current_live);
    fs::rename(&witness.library_path, &witness.staged_path).map_err(|source| Error::Io {
        path: witness.library_path.clone(),
        source,
    })
}

/// Inspect the named entry itself. Only `NotFound` proves that a rename
/// destination is free; target-following existence checks collapse every
/// other metadata failure into the same false answer.
fn entry_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn reject_unexpected_identity(
    witness: &ReinstallSwapWitness,
    name: &str,
    directory: Option<&IdentifiedDirectory>,
    expected: &[&DirectoryIdentity],
) -> Result<()> {
    let Some(directory) = directory else {
        return Ok(());
    };
    if expected
        .iter()
        .any(|expected| directory.identity() == *expected)
    {
        return Ok(());
    }
    witness.uncertain(format!(
        "the recorded {name} path identifies an unrelated directory"
    ))
}

/// Compare deployment entry paths without resolving the final Junction.
/// Resolving the whole path would compare targets and falsely conflate two
/// distinct Junction names that happen to deploy the same duplicate bytes.
fn same_physical_link_path(left: &Path, right: &Path) -> Result<bool> {
    #[cfg(windows)]
    {
        // These handles deliberately retain FILE_FLAG_OPEN_REPARSE_POINT via
        // IdentifiedDirectory, so the identity belongs to each Junction entry
        // rather than to the duplicate Library directory both entries target.
        let Some(left) = identified_if_exists(left)? else {
            return Ok(false);
        };
        let Some(right) = identified_if_exists(right)? else {
            return Ok(false);
        };
        Ok(left.identity() == right.identity())
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        let left = match fs::symlink_metadata(left) {
            Ok(metadata) => Some(metadata),
            Err(source) if source.kind() == io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(Error::Io {
                    path: left.to_path_buf(),
                    source,
                })
            }
        };
        let Some(left) = left else {
            return Ok(false);
        };
        let right = match fs::symlink_metadata(right) {
            Ok(metadata) => Some(metadata),
            Err(source) if source.kind() == io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(Error::Io {
                    path: right.to_path_buf(),
                    source,
                })
            }
        };
        let Some(right) = right else {
            return Ok(false);
        };
        Ok(left.dev() == right.dev() && left.ino() == right.ino())
    }
    #[cfg(not(any(windows, unix)))]
    {
        let (Some(left_parent), Some(right_parent)) = (left.parent(), right.parent()) else {
            return Ok(false);
        };
        Ok(same_path(left_parent, right_parent) && left.file_name() == right.file_name())
    }
}

pub(super) async fn unique_junction_dir_name(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    game: GameCode,
    base: &str,
) -> Result<String> {
    let rows = sqlx::query("SELECT junction_dir_name FROM mods WHERE game_code = ?")
        .bind(game.as_str())
        .fetch_all(&mut **transaction)
        .await?;
    let existing: std::collections::HashSet<String> = rows
        .iter()
        .filter_map(|row| row.try_get("junction_dir_name").ok())
        .collect();
    if !existing.contains(base) {
        return Ok(base.to_string());
    }
    for n in 2..=u32::MAX {
        let candidate = format!("{base} ({n})");
        if !existing.contains(&candidate) {
            return Ok(candidate);
        }
    }
    unreachable!("u32::MAX collisions on one display name is not a real scenario")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A staging-name inspection error is uncertainty, never evidence that
    /// the name is free for a rename. Mutation oracle: restoring
    /// target-following `Path::exists` reaches the rename, attributes its
    /// failure to the live path, and fires the named pre-rename assertion.
    #[test]
    fn reinstall_rollback_propagates_staging_metadata_error_before_rename() {
        use std::os::unix::fs::PermissionsExt as _;

        let tmp = TempDir::new().expect("tmp");
        let token = Ulid::new();
        let mod_id = Ulid::new().to_string();
        let library_path = tmp.path().join(&mod_id);
        let staged_path = tmp
            .path()
            .join(format!("{REINSTALL_STAGING_PREFIX}{token}"));
        let quarantine_path = tmp.path().join(format!(
            "{}{}",
            super::super::library_recovery::DELETE_QUARANTINE_PREFIX,
            token
        ));
        std::fs::create_dir(&library_path).expect("live replacement");
        std::fs::write(library_path.join("replacement.ini"), b"replacement")
            .expect("live replacement bytes");
        std::fs::create_dir(&quarantine_path).expect("old quarantine");
        let current_live = IdentifiedDirectory::open(&library_path).expect("identify live");
        let old = IdentifiedDirectory::open(&quarantine_path).expect("identify old");
        let witness = ReinstallSwapWitness {
            token,
            mod_id,
            game: GameCode::Gimi,
            library_path: library_path.clone(),
            staged_path: staged_path.clone(),
            quarantine_path,
            old_identity: old.identity().clone(),
            staged_identity: current_live.identity().clone(),
        };

        let original_permissions = std::fs::metadata(tmp.path())
            .expect("temporary root metadata")
            .permissions();
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o0))
            .expect("make staging name unreadable");
        let result = move_live_replacement_back_to_stage(&witness, current_live);
        std::fs::set_permissions(tmp.path(), original_permissions)
            .expect("restore temporary root permissions");
        let error = result.expect_err("staging metadata errors must stop rollback before rename");

        assert!(
            matches!(
                error,
                Error::Io { ref path, ref source }
                    if path == &staged_path
                        && source.kind() == io::ErrorKind::PermissionDenied
            ),
            "staging metadata errors must stop rollback before rename: {error}",
        );
        assert_eq!(
            std::fs::read(library_path.join("replacement.ini"))
                .expect("live bytes remain before rename"),
            b"replacement",
            "a staging metadata error must not permit the live replacement rename",
        );
        assert!(
            matches!(
                std::fs::symlink_metadata(&staged_path),
                Err(error) if error.kind() == io::ErrorKind::NotFound
            ),
            "a staging metadata error must leave the reserved name untouched",
        );
    }
}
