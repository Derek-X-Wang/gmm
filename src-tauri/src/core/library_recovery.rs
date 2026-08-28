//! Acting on the unreferenced Library directories that [`super::library_audit`]
//! finds (#72).
//!
//! An orphan exists because `adopt_folder` and `import_zip` copy bytes into
//! the Library *before* inserting the Mod row: a crash in that window leaves
//! a directory nothing references. `reconcile_junctions` walks Mod rows, so
//! it structurally cannot see one.
//!
//! # Recovery is a fresh adopt, not a restore
//!
//! The display name and the Source only ever existed on the row the crash
//! prevented from being written; nothing on disk records either.
//! Reconstructing them would mean inventing values and presenting them as
//! recovered facts. So the user supplies the name, the Source is `manual`
//! (a human did point GMM at a folder), and the *only* thing that makes
//! this different from an ordinary adopt is that it copies nothing — the
//! bytes are already where the Library wants them.
//!
//! # The directory keeps its ULID
//!
//! An orphan's directory name is already the ULID the crashed adopt
//! generated, so the recovered Mod takes that ULID as its ID and the
//! directory never moves. That preserves the invariant the codebase leans
//! on implicitly: a Library path's final component *is* the Mod ID.
//! A directory whose name is not a usable ULID — a folder a user dropped
//! into the Library root by hand — gets a fresh ULID and a rename, because
//! there is no way to keep both the name and the invariant.
//!
//! # Why both actions revalidate
//!
//! The report the user acted from may be stale: a Mod row can be created
//! between rendering it and clicking. Both actions therefore re-resolve the
//! *per-game* Library root — which is overridable globally and per game —
//! and re-check that the directory is still a direct child of it and still
//! unreferenced. Deleting is the first place GMM destroys Library bytes on
//! a user's say-so (ADR 0003 otherwise keeps the Library untouched), so it
//! refuses on anything it cannot re-prove.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};

use chrono::Utc;
use serde::Serialize;
use sqlx::{Executor, Sqlite};
use ulid::Ulid;

#[cfg(not(windows))]
use super::library_audit::directory_size_without_links;
use super::library_audit::is_link_or_reparse_point;
use super::library_identity::IdentifiedDirectory;
use super::library_mutation::{unique_junction_dir_name, LibraryMutation, LibraryMutationFence};
use super::{crash_points, variants, Core, CrashHook, Error, GameCode, Mod, Result, Source};

pub(super) const DELETE_QUARANTINE_PREFIX: &str = ".gmm-delete-";
const DELETE_INTENT_SUFFIX: &str = ".intent";

/// What an accepted delete removed from the visible Library. `size_bytes` is
/// measured only after the quarantine identity is proved and is omitted when
/// byte reclamation is deferred or ownership is lost. Only a deferred outcome
/// names a reclamation path, because only then has GMM proved its bytes remain
/// there. On Windows, both traversal and measurement are derived from the
/// verified quarantine handle, so the count describes the object removed
/// even if its reserved pathname is replaced after proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedLibraryDir {
    pub directory_name: String,
    pub path: PathBuf,
    pub size_bytes: Option<u64>,
    pub reclamation: LibraryReclamationOutcome,
}

/// Whether an accepted Library delete reclaimed the quarantine's bytes.
///
/// The tagged wire shape keeps deferred and ownership-lost mutually exclusive.
/// Ownership loss carries no path because the reserved pathname no longer
/// identifies the directory GMM quarantined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum LibraryReclamationOutcome {
    Reclaimed,
    Deferred { path: PathBuf },
    OwnershipLost,
}

/// The indivisible proof for one destructive Library claim: the SQLite
/// writer lock excludes competing recovery/delete calls, while the open
/// directory handle fixes which filesystem object the caller proved safe.
struct GuardedLibraryMutation {
    fence: LibraryMutationFence,
    directory: IdentifiedDirectory,
}

/// A directory GMM atomically renamed into its reserved, intent-backed delete
/// quarantine. Startup cleanup can finish the same purge if this process stops
/// before removing it.
pub(super) struct QuarantinedLibraryDirectory {
    pub(super) path: PathBuf,
    pub(super) intent: PathBuf,
    after_root_handle_open: Option<CrashHook>,
}

pub(super) enum QuarantinePurgeOutcome {
    Reclaimed(Option<u64>),
    Deferred { path: PathBuf, error: String },
    OwnershipLost,
}

impl QuarantinedLibraryDirectory {
    pub(super) fn purge(self, measure_size: bool) -> Result<QuarantinePurgeOutcome> {
        let Some(verified) = open_owned_delete_quarantine(&self.path)? else {
            return Ok(QuarantinePurgeOutcome::OwnershipLost);
        };

        #[cfg(windows)]
        let removal =
            match super::windows_directory_delete::HandleAnchoredDirectoryRemoval::new(verified) {
                Ok(removal) => {
                    if let Some(hook) = &self.after_root_handle_open {
                        hook(crash_points::QUARANTINE_PURGE_AFTER_ROOT_HANDLE_OPEN);
                    }
                    removal.remove(measure_size, self.after_root_handle_open.as_ref())
                }
                Err(source) => Err(source),
            };

        #[cfg(not(windows))]
        let removal = {
            let size = measure_size
                .then(|| directory_size_without_links(&self.path).ok())
                .flatten();
            let Some(verified_after_measurement) = open_owned_delete_quarantine(&self.path)? else {
                return Ok(QuarantinePurgeOutcome::OwnershipLost);
            };
            if let Some(hook) = &self.after_root_handle_open {
                hook(crash_points::QUARANTINE_PURGE_AFTER_ROOT_HANDLE_OPEN);
                // The per-entry boundary exists only in the Windows walker.
                // Fire its registry observer at the equivalent last
                // pre-removal point on non-Windows so the cross-platform
                // crash-point inventory still proves the seam is reachable.
                hook(crash_points::QUARANTINE_PURGE_AFTER_ENTRY_ENUMERATION);
            }
            drop(verified);
            drop(verified_after_measurement);
            fs::remove_dir_all(&self.path).map(|()| size)
        };

        match removal {
            Ok(size) => {
                if let Err(source) = fs::remove_file(&self.intent) {
                    if source.kind() != io::ErrorKind::NotFound {
                        tracing::warn!(
                            target: "gmm::library",
                            intent = %self.intent.display(),
                            error = %source,
                            "Library bytes were reclaimed but delete-intent cleanup will wait for startup",
                        );
                    }
                }
                Ok(QuarantinePurgeOutcome::Reclaimed(size))
            }
            Err(source) => {
                let error = source.to_string();
                if open_owned_delete_quarantine(&self.path)?.is_some() {
                    Ok(QuarantinePurgeOutcome::Deferred {
                        path: self.path,
                        error,
                    })
                } else {
                    Ok(QuarantinePurgeOutcome::OwnershipLost)
                }
            }
        }
    }
}

impl Core {
    /// Finish ordinary delete quarantines left by a process that stopped after
    /// the atomic rename and before recursive purge. Reinstall rollback is a
    /// separate, earlier startup phase: any witness that remains after that
    /// phase is a quarantined Mod, and both of its recorded identities must be
    /// preserved rather than reclaimed here.
    pub async fn finish_interrupted_library_deletes(&self) -> Result<usize> {
        // Serialize cleanup with the pre-commit half of delete. In particular,
        // an intent without a quarantine is known to be stranded only while no
        // delete can be between writing that intent and performing its rename.
        // Resolve roots under this second fence too: an old root snapshot is
        // not safe evidence once another mutation can relocate the Library.
        let mut fence = self
            .begin_library_mutation(LibraryMutation::FinishInterruptedDeletes)
            .await?;
        let mut roots = Vec::new();
        for profile in super::games::GAME_PROFILES {
            roots.push(
                self.resolved_library_root_for_in_mutation(profile.code, &mut fence)
                    .await?,
            );
        }
        let ownership =
            super::library_ownership::LibraryOwnershipSnapshot::load(&mut fence.transaction)
                .await?;
        let removed =
            tokio::task::spawn_blocking(move || purge_delete_quarantines(&roots, &ownership))
                .await
                .map_err(|join_error| Error::Io {
                    path: PathBuf::from("<Library delete quarantines>"),
                    source: io::Error::other(format!(
                        "Library quarantine cleanup worker failed: {join_error}"
                    )),
                })??;
        fence.commit().await?;
        Ok(removed)
    }

    /// Re-prove that `path` is a directory this game's Library owns and no
    /// Mod row references, and hand back the path to show the user.
    ///
    /// Revealing mutates nothing, so it does not gate on a Game Session;
    /// it validates anyway so a stale report cannot open an arbitrary
    /// directory through GMM.
    pub async fn unreferenced_library_dir_for_reveal(
        &self,
        game: GameCode,
        path: &Path,
    ) -> Result<PathBuf> {
        let mut connection = self.pool.acquire().await?;
        Ok(self
            .validate_unreferenced_library_dir(game, path, &mut connection)
            .await?
            .path()
            .to_path_buf())
    }

    /// Adopt an orphaned Library directory as a Mod without copying it.
    ///
    /// Equivalent to [`Core::adopt_folder`] in every respect except that
    /// the bytes are already in place. Gated on there being no active Game
    /// Session for consistency with other Library mutations. An orphan has
    /// no Mod row or Junction, so junction safety is not the reason for this
    /// deliberately consistent gate.
    pub async fn recover_unreferenced_library_dir(
        &self,
        game: GameCode,
        path: &Path,
        display_name: &str,
    ) -> Result<Mod> {
        self.ensure_no_active_session().await?;
        let GuardedLibraryMutation {
            mut fence,
            directory: validated_directory,
        } = self
            .begin_guarded_library_mutation(
                game,
                path,
                LibraryMutation::RecoverUnreferencedLibraryDir,
            )
            .await?;
        let path = validated_directory.path().to_path_buf();

        let id = self.recovered_mod_id(&path, &mut fence.transaction).await?;
        let library_path = path
            .parent()
            .expect("a validated orphan is a direct child of the Library root")
            .join(&id);

        // Move before insert, for the same reason imports copy before
        // insert: a row pointing at a directory that is not there is worse
        // than a directory no row points at, and the second shape is one
        // this very feature can recover a second time.
        if library_path != path {
            fs::rename(&path, &library_path).map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?;
        }
        let staged_directory =
            IdentifiedDirectory::open(&library_path).map_err(|source| Error::Io {
                path: library_path.clone(),
                source,
            })?;
        if staged_directory.identity() != validated_directory.identity() {
            return Err(Error::NotAnUnreferencedLibraryDir {
                path: library_path,
                reason: "the directory changed after it was validated".to_string(),
            });
        }

        // The rename and identity proof are bounded. Release the global writer
        // fence before recursively traversing an unbounded Variant tree, while
        // retaining the directory identity that the final transaction must
        // re-prove.
        fence.commit().await?;
        self.crash_point(crash_points::RECOVER_AFTER_LIBRARY_MOVE);
        let detected_variants = variants::detect_variants(&library_path)?;

        // Reacquire the fence and repeat the full orphan validation. This
        // re-resolves the effective Library root, proves the path is still its
        // direct child, rejects any Mod or reinstall ownership acquired while
        // detection ran, and opens the current filesystem object for an
        // identity comparison with the staged object above.
        let GuardedLibraryMutation {
            mut fence,
            directory: revalidated_directory,
        } = self
            .begin_guarded_library_mutation(
                game,
                &library_path,
                LibraryMutation::RecoverUnreferencedLibraryDir,
            )
            .await?;
        if revalidated_directory.identity() != staged_directory.identity() {
            return Err(Error::NotAnUnreferencedLibraryDir {
                path: library_path,
                reason: "the directory changed while Variant detection ran".to_string(),
            });
        }
        self.recheck_recovered_mod_id_is_available_ignoring_ascii_case(
            &id,
            &library_path,
            &mut fence.transaction,
        )
        .await?;

        let base = super::sanitize_dir_name(display_name);
        let junction_dir_name =
            unique_junction_dir_name(&mut fence.transaction, game, &base).await?;
        let created_at = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO mods (
                id, game_code, name, source, library_path,
                junction_dir_name, enabled, created_at
             )
             VALUES (?, ?, ?, ?, ?, ?, 0, ?)",
        )
        .bind(&id)
        .bind(game.as_str())
        .bind(display_name)
        .bind(Source::Manual.as_str())
        .bind(library_path.to_string_lossy().as_ref())
        .bind(&junction_dir_name)
        .bind(&created_at)
        .execute(&mut *fence.transaction)
        .await?;

        // A process can die at this exact seam. Keep the row, pre-detected
        // Variant rows, and active selection in one transaction so process
        // death leaves the directory unreferenced and visible to recovery.
        self.crash_point(crash_points::RECOVER_AFTER_ROW_INSERT);
        self.record_detected_variants(&id, detected_variants, &mut fence.transaction)
            .await?;
        fence.commit().await?;

        Ok(Mod {
            id,
            game,
            name: display_name.to_string(),
            source: Source::Manual,
            library_path,
            enabled: false,
            gamebanana_id: None,
            source_url: None,
            author: None,
            version: None,
            screenshot_url: None,
            reinstall_recovery: None,
        })
    }

    /// Permanently remove one orphaned Library directory.
    ///
    /// One explicitly named directory, never a set: the caller passes the
    /// single path the user confirmed. Gated on there being no active Game
    /// Session for consistency with other Library mutations. An orphan has
    /// no Mod row or Junction, so junction safety is not the reason for this
    /// deliberately consistent gate.
    pub async fn delete_unreferenced_library_dir(
        &self,
        game: GameCode,
        path: &Path,
    ) -> Result<DeletedLibraryDir> {
        self.ensure_no_active_session().await?;
        let guarded = self
            .begin_guarded_library_mutation(
                game,
                path,
                LibraryMutation::DeleteUnreferencedLibraryDir,
            )
            .await?;
        let path = guarded.directory.path().to_path_buf();
        let directory_name = path
            .file_name()
            .expect("a validated orphan has a file name")
            .to_string_lossy()
            .into_owned();

        let quarantine = self.quarantine_library_directory(
            &path,
            &guarded.directory,
            Some(crash_points::DELETE_AFTER_INTENT_WRITE),
            Some(crash_points::DELETE_AFTER_QUARANTINE_MOVE),
        )?;

        if let Err(error) = guarded.fence.commit().await {
            let _ = fs::rename(&quarantine.path, &path);
            let _ = fs::remove_file(&quarantine.intent);
            return Err(error);
        }
        drop(guarded.directory);
        self.crash_point(crash_points::DELETE_BEFORE_QUARANTINE_PURGE);

        let purge_result = tokio::task::spawn_blocking(move || quarantine.purge(true))
            .await
            .map_err(|join_error| Error::Io {
                path: path.clone(),
                source: io::Error::other(format!("Library delete worker failed: {join_error}")),
            })?;
        let (size_bytes, reclamation) = match purge_result {
            Ok(QuarantinePurgeOutcome::Reclaimed(size_bytes)) => {
                (size_bytes, LibraryReclamationOutcome::Reclaimed)
            }
            Ok(QuarantinePurgeOutcome::Deferred {
                path: quarantine_path,
                error,
            }) => {
                tracing::warn!(
                    target: "gmm::library",
                    path = %path.display(),
                    quarantine = %quarantine_path.display(),
                    error = %error,
                    "Library delete succeeded but GMM could not reclaim the owned bytes now; later startups will retry while GMM can still verify that directory at its reserved name",
                );
                (
                    None,
                    LibraryReclamationOutcome::Deferred {
                        path: quarantine_path,
                    },
                )
            }
            Ok(QuarantinePurgeOutcome::OwnershipLost) => {
                tracing::error!(
                    target: "gmm::library",
                    path = %path.display(),
                    "Library delete succeeded but GMM cannot locate the quarantined directory or determine whether its bytes were reclaimed; a later startup will retry only if GMM can again verify the original directory at its reserved name",
                );
                (None, LibraryReclamationOutcome::OwnershipLost)
            }
            Err(error) => return Err(error),
        };

        Ok(DeletedLibraryDir {
            directory_name,
            path,
            size_bytes,
            reclamation,
        })
    }

    /// Move a proven-unowned directory into the same durable quarantine used
    /// by explicit orphan deletion. The two optional crash points preserve the
    /// delete path's existing instrumentation; staged cleanup uses the same
    /// protocol without pretending it is an explicit delete operation.
    pub(super) fn quarantine_library_directory(
        &self,
        path: &Path,
        directory: &IdentifiedDirectory,
        after_intent_write: Option<&str>,
        after_quarantine_move: Option<&str>,
    ) -> Result<QuarantinedLibraryDirectory> {
        self.quarantine_library_directory_with_token(
            path,
            directory,
            Ulid::new(),
            after_intent_write,
            after_quarantine_move,
        )
    }

    /// Quarantine one identified directory under a caller-selected token.
    /// Reinstall records the derived reserved path in SQLite before its first
    /// filesystem rename, so startup can distinguish rollback from committed
    /// byte reclamation without inventing a parallel ownership protocol.
    pub(super) fn quarantine_library_directory_with_token(
        &self,
        path: &Path,
        directory: &IdentifiedDirectory,
        token: Ulid,
        after_intent_write: Option<&str>,
        after_quarantine_move: Option<&str>,
    ) -> Result<QuarantinedLibraryDirectory> {
        let root = path
            .parent()
            .expect("a validated Library directory is a direct child of its root");
        let quarantine = root.join(format!("{DELETE_QUARANTINE_PREFIX}{token}"));
        let intent = root.join(format!(
            "{DELETE_QUARANTINE_PREFIX}{token}{DELETE_INTENT_SUFFIX}"
        ));
        let intent_tmp = root.join(format!(
            "{DELETE_QUARANTINE_PREFIX}{token}{DELETE_INTENT_SUFFIX}.tmp"
        ));
        write_delete_intent(
            &intent_tmp,
            &intent,
            directory.identity().durable_key().as_bytes(),
        )?;
        if let Some(point) = after_intent_write {
            self.crash_point(point);
        }
        if let Err(source) = fs::rename(path, &quarantine) {
            let _ = fs::remove_file(&intent);
            return Err(Error::Io {
                path: path.to_path_buf(),
                source,
            });
        }
        let quarantined = IdentifiedDirectory::open(&quarantine).map_err(|source| Error::Io {
            path: quarantine.clone(),
            source,
        })?;
        // The open handle is an identity snapshot, not a lock. On Windows it
        // deliberately shares read, write, and delete access, so another actor
        // can replace `path` before this path-based rename. Re-open the object
        // that actually moved and prove it is still the one validation saw.
        if quarantined.identity() != directory.identity() {
            let _ = fs::rename(&quarantine, path);
            let _ = fs::remove_file(&intent);
            return Err(Error::NotAnUnreferencedLibraryDir {
                path: path.to_path_buf(),
                reason: "the directory changed while it was being quarantined".to_string(),
            });
        }
        if let Some(point) = after_quarantine_move {
            self.crash_point(point);
        }
        Ok(QuarantinedLibraryDirectory {
            path: quarantine,
            intent,
            after_root_handle_open: self.crash_hook.clone(),
        })
    }

    /// The ID a recovered Mod takes.
    ///
    /// The directory's own name when it is a ULID that no row already
    /// claims — that is what lets the directory stay put. Otherwise a
    /// fresh one, which forces the rename.
    async fn recovered_mod_id(
        &self,
        path: &Path,
        mutation: &mut sqlx::Transaction<'_, Sqlite>,
    ) -> Result<String> {
        let name = path
            .file_name()
            .expect("a validated orphan has a file name")
            .to_string_lossy()
            .into_owned();
        // Safe: ULID parsing is a pure format check, not a filesystem observation.
        if Ulid::from_string(&name).is_ok() {
            if self.mod_id_exists(&name, &mut **mutation).await? {
                return Err(Error::NotAnUnreferencedLibraryDir {
                    path: path.to_path_buf(),
                    reason: "a Mod ID already claims this ULID, ignoring ASCII case".to_string(),
                });
            }
            return Ok(name);
        }
        Ok(Ulid::new().to_string())
    }

    async fn recheck_recovered_mod_id_is_available_ignoring_ascii_case(
        &self,
        id: &str,
        path: &Path,
        mutation: &mut sqlx::Transaction<'_, Sqlite>,
    ) -> Result<()> {
        if self.mod_id_exists(id, &mut **mutation).await? {
            return Err(Error::NotAnUnreferencedLibraryDir {
                path: path.to_path_buf(),
                reason:
                    "a Mod ID claimed this ULID, ignoring ASCII case while Variant detection ran"
                        .to_string(),
            });
        }
        Ok(())
    }

    async fn mod_id_exists<'e, E>(&self, id: &str, executor: E) -> Result<bool>
    where
        E: Executor<'e, Database = Sqlite>,
    {
        Ok(
            sqlx::query("SELECT 1 FROM mods WHERE id = ? COLLATE NOCASE")
                .bind(id)
                .fetch_optional(executor)
                .await?
                .is_some(),
        )
    }

    async fn begin_guarded_library_mutation(
        &self,
        game: GameCode,
        path: &Path,
        mutation: LibraryMutation,
    ) -> Result<GuardedLibraryMutation> {
        let mut fence = self.begin_library_mutation(mutation).await?;
        let directory = self
            .validate_unreferenced_library_dir(game, path, &mut fence.transaction)
            .await?;
        Ok(GuardedLibraryMutation { fence, directory })
    }

    /// The shared precondition for every action in this module.
    async fn validate_unreferenced_library_dir(
        &self,
        game: GameCode,
        path: &Path,
        connection: &mut sqlx::SqliteConnection,
    ) -> Result<IdentifiedDirectory> {
        let root = self.resolved_library_root_for(game).await?;
        let path =
            lexically_normalized(path).ok_or_else(|| Error::NotAnUnreferencedLibraryDir {
                path: path.to_path_buf(),
                reason: "the path is not absolute or contains `..`".to_string(),
            })?;

        if is_owned_delete_quarantine(&path)? {
            return Err(Error::NotAnUnreferencedLibraryDir {
                path,
                reason: "it is an interrupted-delete quarantine owned by GMM".to_string(),
            });
        }

        let parent = path
            .parent()
            .expect("an absolute normalized child has a parent");
        let root_directory = IdentifiedDirectory::open(&root).map_err(|source| Error::Io {
            path: root.clone(),
            source,
        })?;
        let parent_directory = IdentifiedDirectory::open(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        if root_directory.identity() != parent_directory.identity() {
            return Err(Error::NotAnUnreferencedLibraryDir {
                path,
                reason: format!(
                    "it is not a direct child of {}'s Library root ({})",
                    game.as_str(),
                    root.display()
                ),
            });
        }

        let metadata = fs::symlink_metadata(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        if is_link_or_reparse_point(&metadata) {
            return Err(Error::NotAnUnreferencedLibraryDir {
                path,
                reason: "it is a link, and acting on it would act on its target".to_string(),
            });
        }
        // Safe: `symlink_metadata()` above propagated I/O uncertainty.
        if !metadata.file_type().is_dir() {
            return Err(Error::NotAnUnreferencedLibraryDir {
                path,
                reason: "it is not a directory".to_string(),
            });
        }

        let directory = IdentifiedDirectory::open(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;

        let ownership =
            super::library_ownership::LibraryOwnershipSnapshot::load(connection).await?;
        match ownership.disposition_of(&directory) {
            super::library_ownership::LibraryDirectoryDisposition::Owned(owner) => {
                let reason = match owner {
                    super::library_ownership::LibraryDirectoryOwner::Mod => {
                        "a Mod now references it — refresh the report"
                    }
                    super::library_ownership::LibraryDirectoryOwner::ActiveReinstall => {
                        "it is interrupted reinstall state owned by GMM"
                    }
                    super::library_ownership::LibraryDirectoryOwner::ActiveStaging => {
                        "it is an active staging operation owned by GMM"
                    }
                };
                Err(Error::NotAnUnreferencedLibraryDir {
                    path,
                    reason: reason.to_string(),
                })
            }
            super::library_ownership::LibraryDirectoryDisposition::IgnorableEmptyReinstallStage => {
                Err(Error::NotAnUnreferencedLibraryDir {
                    path,
                    reason: "it is an empty interrupted reinstall stage with no user bytes"
                        .to_string(),
                })
            }
            super::library_ownership::LibraryDirectoryDisposition::Unreferenced => Ok(directory),
        }
    }
}

fn purge_delete_quarantines(
    roots: &[PathBuf],
    ownership: &super::library_ownership::LibraryOwnershipSnapshot,
) -> Result<usize> {
    purge_delete_quarantines_with(roots, ownership, |_| {})
}

fn purge_delete_quarantines_with(
    roots: &[PathBuf],
    ownership: &super::library_ownership::LibraryOwnershipSnapshot,
    mut before_open: impl FnMut(&Path),
) -> Result<usize> {
    let mut removed = 0;
    for root in roots {
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(Error::Io {
                    path: root.clone(),
                    source,
                })
            }
        };
        for entry in entries {
            let entry = entry.map_err(|source| Error::Io {
                path: root.clone(),
                source,
            })?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(quarantine_name) = name
                .strip_prefix(DELETE_QUARANTINE_PREFIX)
                .and_then(|name| name.strip_suffix(DELETE_INTENT_SUFFIX))
            else {
                continue;
            };
            let intent = entry.path();
            let quarantine = root.join(format!("{DELETE_QUARANTINE_PREFIX}{quarantine_name}"));
            match fs::symlink_metadata(&quarantine) {
                Ok(_) => {}
                Err(source) if source.kind() == io::ErrorKind::NotFound => {
                    match fs::remove_file(&intent) {
                        Ok(()) => {}
                        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                        Err(source) => {
                            return Err(Error::Io {
                                path: intent,
                                source,
                            })
                        }
                    }
                    continue;
                }
                Err(source) => {
                    return Err(Error::Io {
                        path: quarantine,
                        source,
                    })
                }
            }
            before_open(&quarantine);
            let directory = match IdentifiedDirectory::open(&quarantine) {
                Ok(directory) => directory,
                Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
                Err(source) => {
                    return Err(Error::Io {
                        path: quarantine.clone(),
                        source,
                    })
                }
            };
            if matches!(
                ownership.owner_of(directory.identity()),
                Some(super::library_ownership::LibraryDirectoryOwner::ActiveReinstall)
            ) {
                continue;
            }
            drop(directory);
            match (QuarantinedLibraryDirectory {
                path: quarantine,
                intent,
                after_root_handle_open: None,
            })
            .purge(false)
            {
                Ok(QuarantinePurgeOutcome::Reclaimed(_)) => removed += 1,
                Ok(QuarantinePurgeOutcome::Deferred { path, error }) => tracing::warn!(
                    target: "gmm::library",
                    quarantine = %path.display(),
                    error = %error,
                    "owned Library delete quarantine remains; a later startup will retry reclamation",
                ),
                Ok(QuarantinePurgeOutcome::OwnershipLost) => tracing::error!(
                    target: "gmm::library",
                    library_root = %root.display(),
                    "GMM cannot locate an intent-backed Library delete quarantine or determine whether its bytes were reclaimed on this startup; a later startup will retry only if GMM can again verify the original directory at its reserved name",
                ),
                Err(Error::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(removed)
}

fn write_delete_intent(tmp: &Path, intent: &Path, contents: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp)
        .map_err(|source| Error::Io {
            path: tmp.to_path_buf(),
            source,
        })?;
    file.write_all(contents).map_err(|source| Error::Io {
        path: tmp.to_path_buf(),
        source,
    })?;
    file.sync_all().map_err(|source| Error::Io {
        path: tmp.to_path_buf(),
        source,
    })?;
    fs::rename(tmp, intent).map_err(|source| Error::Io {
        path: intent.to_path_buf(),
        source,
    })
}

pub(super) fn is_owned_delete_quarantine(path: &Path) -> Result<bool> {
    Ok(open_owned_delete_quarantine(path)?.is_some())
}

/// Whether relocation would carry an identity-backed delete quarantine.
/// Cross-volume fallback copies directory contents and therefore changes the
/// quarantine's filesystem identity while preserving its old intent marker.
/// Callers hold the Library writer fence so cleanup cannot create or retire a
/// quarantine between this check and the relocation decision.
pub(super) fn has_owned_delete_quarantine(roots: &[PathBuf]) -> Result<bool> {
    for root in roots {
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(Error::Io {
                    path: root.clone(),
                    source,
                })
            }
        };
        for entry in entries {
            let entry = entry.map_err(|source| Error::Io {
                path: root.clone(),
                source,
            })?;
            if is_owned_delete_quarantine(&entry.path())? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn open_owned_delete_quarantine(path: &Path) -> Result<Option<IdentifiedDirectory>> {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    let Some(token) = name.strip_prefix(DELETE_QUARANTINE_PREFIX) else {
        return Ok(None);
    };
    if token.ends_with(DELETE_INTENT_SUFFIX) || Ulid::from_string(token).is_err() {
        return Ok(None);
    }
    let intent = path.with_file_name(format!(
        "{DELETE_QUARANTINE_PREFIX}{token}{DELETE_INTENT_SUFFIX}"
    ));
    let expected = match fs::read_to_string(&intent) {
        Ok(expected) => expected,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::Io {
                path: intent,
                source,
            })
        }
    };
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::Io {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    // Safe: `symlink_metadata()` above propagated I/O uncertainty.
    if is_link_or_reparse_point(&metadata) || !metadata.file_type().is_dir() {
        return Ok(None);
    }
    let directory = match IdentifiedDirectory::open(path) {
        Ok(directory) => directory,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::Io {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    Ok((directory.identity().durable_key() == expected).then_some(directory))
}

/// Drop `.` components and reject anything relative or containing `..`.
///
/// Purely textual: the path may be about to be deleted, so this must not
/// depend on it existing, and `canonicalize` would also resolve links —
/// exactly the thing the caller needs to detect rather than follow.
fn lexically_normalized(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => return None,
            other => out.push(other.as_os_str()),
        }
    }
    Some(out)
}

#[cfg(test)]
// Test-only postcondition probes may collapse an assertion failure to `false`; no production decision follows.
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn owned_quarantine(root: &Path) -> (PathBuf, PathBuf) {
        fs::create_dir_all(root).expect("create test Library root");
        let token = Ulid::new();
        let quarantine = root.join(format!("{DELETE_QUARANTINE_PREFIX}{token}"));
        let intent = root.join(format!(
            "{DELETE_QUARANTINE_PREFIX}{token}{DELETE_INTENT_SUFFIX}"
        ));
        fs::create_dir(&quarantine).expect("create owned quarantine");
        fs::write(quarantine.join("marker"), b"owned bytes").expect("write quarantine bytes");
        let identity = IdentifiedDirectory::open(&quarantine)
            .expect("identify owned quarantine")
            .identity()
            .durable_key();
        fs::write(&intent, identity).expect("write ownership intent");
        (quarantine, intent)
    }

    #[cfg(windows)]
    #[test]
    fn handle_anchored_purge_reclaims_an_owned_quarantine() {
        let temp = tempfile::tempdir().expect("temporary Library");
        let (quarantine, intent) = owned_quarantine(&temp.path().join("gimi"));

        match (QuarantinedLibraryDirectory {
            path: quarantine.clone(),
            intent: intent.clone(),
            after_root_handle_open: None,
        })
        .purge(false)
        .expect("purge owned quarantine")
        {
            QuarantinePurgeOutcome::Reclaimed(None) => {}
            QuarantinePurgeOutcome::Reclaimed(Some(size)) => {
                panic!("unmeasured purge unexpectedly reported {size} bytes")
            }
            QuarantinePurgeOutcome::Deferred { error, .. } => {
                panic!("handle-anchored purge was deferred: {error}")
            }
            QuarantinePurgeOutcome::OwnershipLost => {
                panic!("handle-anchored purge unexpectedly lost ownership")
            }
        }

        assert!(
            super::super::filesystem::symlink_metadata_if_exists(&quarantine)
                .expect("inspect reclaimed quarantine")
                .is_none(),
            "the quarantine must be removed"
        );
        assert!(
            super::super::filesystem::symlink_metadata_if_exists(&intent)
                .expect("inspect retired intent")
                .is_none(),
            "the durable intent must be retired"
        );
    }

    #[test]
    fn vanished_quarantine_does_not_abort_later_cleanup() {
        let temp = tempfile::tempdir().expect("temporary Library");
        let first_root = temp.path().join("gimi");
        let later_root = temp.path().join("srmi");
        let (vanished, vanished_intent) = owned_quarantine(&first_root);
        let (later, later_intent) = owned_quarantine(&later_root);
        let ownership = super::super::library_ownership::LibraryOwnershipSnapshot::empty_for_test();

        let result =
            purge_delete_quarantines_with(&[first_root, later_root], &ownership, |about_to_open| {
                if about_to_open == vanished {
                    fs::remove_dir_all(&vanished)
                        .expect("make first quarantine vanish after its existence check");
                }
            });
        let removed = match result {
            Ok(removed) => removed,
            Err(error) => panic!(
                "a quarantine that vanished after its existence check must not abort later cleanup: {error}"
            ),
        };

        assert_eq!(
            removed, 1,
            "the later owned quarantine must still be reclaimed"
        );
        assert!(
            vanished_intent.is_file(),
            "identity uncertainty keeps the vanished quarantine's intent for a later pass"
        );
        assert!(!later.exists(), "the later quarantine must be removed");
        assert!(!later_intent.exists(), "the later intent must be removed");
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_quarantine_keeps_its_durable_intent() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary Library");
        let root = temp.path().join("gimi");
        fs::create_dir_all(&root).expect("create test Library root");
        let token = Ulid::new();
        let quarantine = root.join(format!("{DELETE_QUARANTINE_PREFIX}{token}"));
        let intent = root.join(format!(
            "{DELETE_QUARANTINE_PREFIX}{token}{DELETE_INTENT_SUFFIX}"
        ));
        symlink(&quarantine, &quarantine)
            .expect("create a quarantine whose followed metadata is unreadable");
        fs::write(&intent, b"durable ownership evidence").expect("write ownership intent");
        let ownership = super::super::library_ownership::LibraryOwnershipSnapshot::empty_for_test();

        let result = purge_delete_quarantines(&[root], &ownership);

        assert!(
            intent.is_file(),
            "metadata uncertainty must retain the quarantine's durable ownership intent",
        );
        assert!(
            matches!(result, Err(Error::Io { ref path, .. }) if path == &quarantine),
            "an unreadable quarantine must propagate its filesystem error, got {result:?}",
        );
    }
}
