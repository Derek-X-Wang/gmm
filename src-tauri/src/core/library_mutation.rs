//! One writer-fence protocol for every mutation of Library-owned bytes.
//!
//! Root relocation is the exclusive form: it holds SQLite's writer claim
//! from the row snapshot through the filesystem move and the setting/row
//! commit. Potentially unbounded producers (`adopt_folder` and `import_zip`)
//! use the staged form: resolve and identify the root under a short claim,
//! perform their copy/extract without blocking SQLite, then reacquire the
//! claim and prove both the configured root name and its filesystem identity
//! are unchanged before inserting a row. Recovery/delete keep their bounded
//! filesystem ownership acts inside one claim.

use std::path::{Path, PathBuf};

use sqlx::{Row, Sqlite};

use super::library_identity::IdentifiedDirectory;
use super::settings::{get as get_setting, keys};
use super::{Core, Error, GameCode, Result};

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
        }
    }
}

/// Source marker for the architecture inventory in `tests/concurrency.rs`.
/// The assertions are only a debug-build sanity check; release builds get no
/// runtime enforcement from this function, and a new filesystem primitive or
/// module can escape the inventory. #166 owns staging-and-swap for the existing
/// destructive reinstall path; #164 must not partially implement it.
pub(super) fn record_library_mutation_exemption(mutation: LibraryMutation, issue: u32) {
    debug_assert_eq!(mutation, LibraryMutation::ReinstallGamebananaMod);
    debug_assert_eq!(issue, 166);
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
            let mut ownership_unknown = false;
            let referenced_paths = match sqlx::query("SELECT library_path FROM mods")
                .fetch_all(&mut *fence.transaction)
                .await
            {
                Ok(referenced_paths) => referenced_paths,
                Err(error) => {
                    tracing::warn!(
                        target: "gmm::library",
                        path = %path.display(),
                        error = %error,
                        "could not prove a staged Library directory is unowned; leaving it for orphan audit",
                    );
                    ownership_unknown = true;
                    Vec::new()
                }
            };
            let mut owned = false;
            for row in referenced_paths {
                let referenced = match row.try_get::<String, _>("library_path") {
                    Ok(referenced) => PathBuf::from(referenced),
                    Err(error) => {
                        tracing::warn!(
                            target: "gmm::library",
                            path = %path.display(),
                            error = %error,
                            "could not read a Mod's Library path during staged cleanup; leaving the candidate for orphan audit",
                        );
                        ownership_unknown = true;
                        break;
                    }
                };
                match IdentifiedDirectory::open(&referenced) {
                    Ok(referenced) if referenced.identity() == current.identity() => {
                        owned = true;
                        break;
                    }
                    Ok(_) => {}
                    Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
                    Err(source) => {
                        tracing::warn!(
                            target: "gmm::library",
                            path = %path.display(),
                            referenced_path = %referenced.display(),
                            error = %source,
                            "could not prove a Mod row does not own the staged cleanup candidate; leaving it for orphan audit",
                        );
                        ownership_unknown = true;
                        break;
                    }
                }
            }
            if ownership_unknown {
                // Uncertainty must preserve the bytes for orphan audit.
            } else if owned {
                tracing::warn!(
                    target: "gmm::library",
                    path = %path.display(),
                    "staged Library cleanup candidate is now owned by a Mod; leaving it intact",
                );
            } else {
                self.crash_point(super::crash_points::STAGED_CLEANUP_BEFORE_QUARANTINE_MOVE);
                match self.quarantine_library_directory(&path, &current, None, None) {
                    Ok(directory) => {
                        self.crash_point(super::crash_points::STAGED_CLEANUP_AFTER_QUARANTINE_MOVE);
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
