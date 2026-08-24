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

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use sqlx::{Row, Sqlite};
use ulid::Ulid;

use super::library_identity::IdentifiedDirectory;
use super::library_ownership::{LibraryDirectoryOwner, LibraryOwnershipSnapshot};
use super::settings::{get as get_setting, keys};
use super::{crash_points, junction, volume, Core, Error, GameCode, Result};

pub(super) const REINSTALL_STAGING_PREFIX: &str = ".gmm-reinstall-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LibraryMutation {
    FinishInterruptedDeletes,
    SetLibraryRoot,
    SetLibraryPathForGame,
    AdoptFolder,
    ImportZip,
    RecoverUnreferencedLibraryDir,
    DeleteUnreferencedLibraryDir,
    ReinstallGamebananaMod,
    SetEnabled,
}

impl LibraryMutation {
    pub(super) const fn function_name(self) -> &'static str {
        match self {
            Self::FinishInterruptedDeletes => "finish_interrupted_library_deletes",
            Self::SetLibraryRoot => "set_library_root",
            Self::SetLibraryPathForGame => "set_library_path_for_game",
            Self::AdoptFolder => "adopt_folder",
            Self::ImportZip => "import_zip",
            Self::RecoverUnreferencedLibraryDir => "recover_unreferenced_library_dir",
            Self::DeleteUnreferencedLibraryDir => "delete_unreferenced_library_dir",
            Self::ReinstallGamebananaMod => "reinstall_gamebanana_mod_with_endpoints",
            Self::SetEnabled => "set_enabled",
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

    /// Create the ULID directory before unbounded copy/extract work and retain
    /// its filesystem identity until either its row commits or guarded cleanup
    /// proves the same object is still unowned.
    pub(super) fn create_staged_directory(
        &self,
        directory_name: &str,
    ) -> Result<StagedLibraryDirectory> {
        let path = self.path().join(directory_name);
        std::fs::create_dir(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        let directory = IdentifiedDirectory::open(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        Ok(StagedLibraryDirectory { directory })
    }
}

/// Identity evidence for bytes staged outside the writer fence.
pub(super) struct StagedLibraryDirectory {
    directory: IdentifiedDirectory,
}

#[derive(Debug, Clone)]
pub(super) struct ReinstallSwapWitness {
    pub(super) token: Ulid,
    pub(super) mod_id: String,
    pub(super) game: GameCode,
    pub(super) library_path: PathBuf,
    pub(super) staged_path: PathBuf,
    pub(super) quarantine_path: PathBuf,
    pub(super) old_identity: String,
    pub(super) staged_identity: String,
}

impl ReinstallSwapWitness {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self> {
        let token_raw: String = row.try_get("token")?;
        let token =
            Ulid::from_string(&token_raw).map_err(|_| Error::ReinstallRecoveryUncertain {
                mod_id: row
                    .try_get("mod_id")
                    .unwrap_or_else(|_| "<unknown>".to_string()),
                reason: format!("the swap token {token_raw:?} is not a ULID"),
            })?;
        let game_raw: String = row.try_get("game_code")?;
        Ok(Self {
            token,
            mod_id: row.try_get("mod_id")?,
            game: GameCode::from_str(&game_raw)?,
            library_path: PathBuf::from(row.try_get::<String, _>("library_path")?),
            staged_path: PathBuf::from(row.try_get::<String, _>("staged_path")?),
            quarantine_path: PathBuf::from(row.try_get::<String, _>("quarantine_path")?),
            old_identity: row.try_get("old_identity")?,
            staged_identity: row.try_get("staged_identity")?,
        })
    }

    fn validate_paths(&self) -> Result<()> {
        let Some(root) = self.library_path.parent() else {
            return self.uncertain("the recorded live path has no Library root");
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
            return self.uncertain("the recorded swap paths do not match the Mod ID and token");
        }
        Ok(())
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
}

impl Core {
    pub(super) async fn begin_library_mutation(
        &self,
        mutation: LibraryMutation,
    ) -> Result<LibraryMutationFence> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if mutation != LibraryMutation::FinishInterruptedDeletes {
            self.ensure_no_active_session_in_library_mutation(&mut transaction)
                .await?;
        }
        Ok(LibraryMutationFence { transaction })
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
        fence.commit().await
    }

    pub(super) async fn reinstall_swap_witness(
        &self,
        token: Ulid,
        fence: &mut LibraryMutationFence,
    ) -> Result<ReinstallSwapWitness> {
        let row = sqlx::query(
            "SELECT token, mod_id, game_code, library_path, staged_path,
                    quarantine_path, old_identity, staged_identity
             FROM reinstall_swaps WHERE token = ?",
        )
        .bind(token.to_string())
        .fetch_one(&mut *fence.transaction)
        .await?;
        let witness = ReinstallSwapWitness::from_row(&row)?;
        witness.validate_paths()?;
        self.rebase_reinstall_swap_witness(witness, fence).await
    }

    /// Roll back every reinstall whose durable witness is still present.
    /// Witness presence is the only decision: even if the complete new tree
    /// already occupies the live name, startup restores the old tree. A
    /// successful reinstall deletes the witness in the same SQLite transaction
    /// that commits its metadata and Variant rows, so an absent witness means
    /// the live replacement wins and the old delete quarantine can be purged.
    pub(super) async fn rollback_interrupted_reinstall_swaps(
        &self,
        fence: &mut LibraryMutationFence,
    ) -> Result<usize> {
        let rows = sqlx::query(
            "SELECT token, mod_id, game_code, library_path, staged_path,
                    quarantine_path, old_identity, staged_identity
             FROM reinstall_swaps ORDER BY created_at, token",
        )
        .fetch_all(&mut *fence.transaction)
        .await?;
        let mut rolled_back = 0;
        for row in rows {
            let witness = ReinstallSwapWitness::from_row(&row)?;
            witness.validate_paths()?;
            let witness = self.rebase_reinstall_swap_witness(witness, fence).await?;
            self.rollback_reinstall_swap_in_mutation(&witness, fence)
                .await?;
            rolled_back += 1;
        }
        Ok(rolled_back)
    }

    /// Current relocation refuses to move a subtree with an active witness,
    /// because its cross-volume copy fallback cannot preserve identity. Keep
    /// rebasing as a recovery boundary for a witness already carried to a new
    /// root: only the three sibling names change, while the recorded identities
    /// remain the ownership proof and still fail closed if a copy changed them.
    async fn rebase_reinstall_swap_witness(
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
        if current_library_path.parent() != Some(current_root.as_path())
            || current_library_path
                .file_name()
                .and_then(|name| name.to_str())
                != Some(&witness.mod_id)
        {
            return witness.uncertain(
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
        self.finish_interrupted_library_deletes().await?;
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
            .is_some_and(|directory| directory.identity().durable_key() == witness.old_identity);
        let old_is_quarantined = quarantine
            .as_ref()
            .is_some_and(|directory| directory.identity().durable_key() == witness.old_identity);
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
                if current_live.identity().durable_key() != witness.staged_identity {
                    return witness
                        .uncertain("the live name no longer identifies the staged replacement");
                }
                if witness.staged_path.exists() {
                    return witness
                        .uncertain("both the live and staging names contain replacement bytes");
                }
                drop(current_live);
                fs::rename(&witness.library_path, &witness.staged_path).map_err(|source| {
                    Error::Io {
                        path: witness.library_path.clone(),
                        source,
                    }
                })?;
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
            if replacement.identity().durable_key() != witness.staged_identity {
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
        if super::link_exists(&link) {
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

    pub(super) async fn snapshot_library_root_for_mutation(
        &self,
        game: GameCode,
        mutation: LibraryMutation,
    ) -> Result<LibraryRootSnapshot> {
        let mut fence = self.begin_library_mutation(mutation).await?;
        let root = self
            .resolved_library_root_for_in_mutation(game, &mut fence)
            .await?;
        std::fs::create_dir_all(&root).map_err(|source| Error::Io {
            path: root.clone(),
            source,
        })?;
        let root = IdentifiedDirectory::open(&root).map_err(|source| Error::Io {
            path: root.clone(),
            source,
        })?;
        fence.commit().await?;
        Ok(LibraryRootSnapshot { game, root })
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
        if let Some((path, current)) = deletion_candidate {
            let ownership = match LibraryOwnershipSnapshot::load(&mut *fence.transaction).await {
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
                        LibraryDirectoryOwner::ActiveReinstallStage => "an active reinstall stage",
                    };
                    tracing::warn!(
                        target: "gmm::library",
                        path = %path.display(),
                        owner,
                        "staged Library cleanup candidate is now owned; leaving it intact",
                    );
                }
                None if ownership.is_some() => {
                    self.crash_point(super::crash_points::STAGED_CLEANUP_BEFORE_QUARANTINE_MOVE);
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

fn reject_unexpected_identity(
    witness: &ReinstallSwapWitness,
    name: &str,
    directory: Option<&IdentifiedDirectory>,
    expected: &[&String],
) -> Result<()> {
    let Some(directory) = directory else {
        return Ok(());
    };
    let actual = directory.identity().durable_key();
    if expected.iter().any(|expected| actual == expected.as_str()) {
        return Ok(());
    }
    witness.uncertain(format!(
        "the recorded {name} path identifies an unrelated directory"
    ))
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
