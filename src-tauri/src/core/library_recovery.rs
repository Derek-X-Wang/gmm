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
use sqlx::{Executor, Row, Sqlite};
use ulid::Ulid;

use super::library_audit::{directory_size_without_links, is_link_or_reparse_point};
use super::library_identity::IdentifiedDirectory;
use super::{crash_points, Core, Error, GameCode, Mod, Result, Source};

pub(super) const DELETE_QUARANTINE_PREFIX: &str = ".gmm-delete-";
const DELETE_INTENT_SUFFIX: &str = ".intent";

/// What a delete actually removed. `size_bytes` is measured immediately
/// before the removal rather than taken from the report the user clicked,
/// so the number GMM reports back is the number it really freed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedLibraryDir {
    pub directory_name: String,
    pub path: PathBuf,
    pub size_bytes: Option<u64>,
}

/// The indivisible proof for one destructive Library claim: the SQLite
/// writer lock excludes competing recovery/delete calls, while the open
/// directory handle fixes which filesystem object the caller proved safe.
struct GuardedLibraryMutation {
    transaction: sqlx::Transaction<'static, Sqlite>,
    directory: IdentifiedDirectory,
}

impl Core {
    /// Finish delete quarantines left by a process that stopped after the
    /// atomic rename and before recursive purge. `Core::new` runs this on
    /// every startup; failures are retryable because the reserved directory
    /// remains hidden from audit and cannot be recovered as a Mod.
    pub async fn finish_interrupted_library_deletes(&self) -> Result<usize> {
        let mut roots = Vec::new();
        for profile in super::games::GAME_PROFILES {
            roots.push(self.resolved_library_root_for(profile.code).await?);
        }

        // Serialize cleanup with the pre-commit half of delete. In particular,
        // an intent without a quarantine is known to be stranded only while no
        // delete can be between writing that intent and performing its rename.
        let transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let removed = tokio::task::spawn_blocking(move || purge_delete_quarantines(&roots))
            .await
            .map_err(|join_error| Error::Io {
                path: PathBuf::from("<Library delete quarantines>"),
                source: io::Error::other(format!(
                    "Library quarantine cleanup worker failed: {join_error}"
                )),
            })??;
        transaction.commit().await?;
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
        Ok(self
            .validate_unreferenced_library_dir(game, path, &self.pool)
            .await?
            .path()
            .to_path_buf())
    }

    /// Adopt an orphaned Library directory as a Mod without copying it.
    ///
    /// Equivalent to [`Core::adopt_folder`] in every respect except that
    /// the bytes are already in place. Gated on there being no active Game
    /// Session, like every other Library mutation.
    pub async fn recover_unreferenced_library_dir(
        &self,
        game: GameCode,
        path: &Path,
        display_name: &str,
    ) -> Result<Mod> {
        self.ensure_no_active_session().await?;
        let mut guarded = self.begin_guarded_library_mutation(game, path).await?;
        let path = guarded.directory.path().to_path_buf();

        let id = self
            .recovered_mod_id(&path, &mut guarded.transaction)
            .await?;
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
        self.crash_point(crash_points::RECOVER_AFTER_LIBRARY_MOVE);

        let base = super::sanitize_dir_name(display_name);
        let junction_dir_name =
            unique_junction_dir_name(&mut guarded.transaction, game, &base).await?;
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
        .execute(&mut *guarded.transaction)
        .await?;

        guarded.transaction.commit().await?;

        self.detect_and_record_variants(&id, &library_path).await?;

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
        })
    }

    /// Permanently remove one orphaned Library directory.
    ///
    /// One explicitly named directory, never a set: the caller passes the
    /// single path the user confirmed. Gated on there being no active Game
    /// Session, like every other Library mutation.
    pub async fn delete_unreferenced_library_dir(
        &self,
        game: GameCode,
        path: &Path,
    ) -> Result<DeletedLibraryDir> {
        self.ensure_no_active_session().await?;
        let guarded = self.begin_guarded_library_mutation(game, path).await?;
        let path = guarded.directory.path().to_path_buf();
        let directory_name = path
            .file_name()
            .expect("a validated orphan has a file name")
            .to_string_lossy()
            .into_owned();

        let root = path
            .parent()
            .expect("a validated orphan is a direct child of the Library root");
        let token = Ulid::new();
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
            guarded.directory.identity().durable_key().as_bytes(),
        )?;
        self.crash_point(crash_points::DELETE_AFTER_INTENT_WRITE);
        if let Err(source) = fs::rename(&path, &quarantine) {
            let _ = fs::remove_file(&intent);
            return Err(Error::Io {
                path: path.clone(),
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
        if quarantined.identity() != guarded.directory.identity() {
            let _ = fs::rename(&quarantine, &path);
            let _ = fs::remove_file(&intent);
            return Err(Error::NotAnUnreferencedLibraryDir {
                path,
                reason: "the directory changed while it was being quarantined".to_string(),
            });
        }
        self.crash_point(crash_points::DELETE_AFTER_QUARANTINE_MOVE);

        if let Err(error) = guarded.transaction.commit().await {
            let _ = fs::rename(&quarantine, &path);
            let _ = fs::remove_file(&intent);
            return Err(error.into());
        }

        let removed = quarantine;
        let removed_intent = intent;
        let size_bytes = tokio::task::spawn_blocking(move || {
            let size = directory_size_without_links(&removed).ok();
            fs::remove_dir_all(&removed)
                .and_then(|()| fs::remove_file(&removed_intent).map(|()| size))
                .map_err(|source| Error::Io {
                    path: removed,
                    source,
                })
        })
        .await
        .map_err(|join_error| Error::Io {
            path: path.clone(),
            source: io::Error::other(format!("Library delete worker failed: {join_error}")),
        })??;

        Ok(DeletedLibraryDir {
            directory_name,
            path,
            size_bytes,
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

    async fn ensure_no_active_session_in_mutation(
        &self,
        mutation: &mut sqlx::Transaction<'_, Sqlite>,
    ) -> Result<()> {
        let row = sqlx::query("SELECT game_code, started_at FROM active_session WHERE id = 1")
            .fetch_optional(&mut **mutation)
            .await?;
        if let Some(row) = row {
            return Err(Error::SessionActive {
                game: row.try_get("game_code")?,
                since: row.try_get("started_at")?,
            });
        }
        Ok(())
    }

    async fn begin_guarded_library_mutation(
        &self,
        game: GameCode,
        path: &Path,
    ) -> Result<GuardedLibraryMutation> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        self.ensure_no_active_session_in_mutation(&mut transaction)
            .await?;
        let directory = self
            .validate_unreferenced_library_dir(game, path, &mut *transaction)
            .await?;
        Ok(GuardedLibraryMutation {
            transaction,
            directory,
        })
    }

    /// The shared precondition for every action in this module.
    async fn validate_unreferenced_library_dir<'e, E>(
        &self,
        game: GameCode,
        path: &Path,
        executor: E,
    ) -> Result<IdentifiedDirectory>
    where
        E: Executor<'e, Database = Sqlite>,
    {
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

        // Across every game, not just this one: a per-game Library root
        // override can point two games at the same directory, and a row
        // from either one makes these bytes somebody's Mod.
        let rows = sqlx::query("SELECT library_path FROM mods")
            .fetch_all(executor)
            .await?;
        for row in rows {
            let referenced = PathBuf::from(row.try_get::<String, _>("library_path")?);
            let referenced = match IdentifiedDirectory::open(&referenced) {
                Ok(referenced) => referenced,
                Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
                Err(source) => {
                    return Err(Error::Io {
                        path: referenced,
                        source,
                    })
                }
            };
            if referenced.identity() == directory.identity() {
                return Err(Error::NotAnUnreferencedLibraryDir {
                    path,
                    reason: "a Mod now references it — refresh the report".to_string(),
                });
            }
        }

        Ok(directory)
    }
}

async fn unique_junction_dir_name(
    mutation: &mut sqlx::Transaction<'_, Sqlite>,
    game: GameCode,
    base: &str,
) -> Result<String> {
    let rows = sqlx::query("SELECT junction_dir_name FROM mods WHERE game_code = ?")
        .bind(game.as_str())
        .fetch_all(&mut **mutation)
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

fn purge_delete_quarantines(roots: &[PathBuf]) -> Result<usize> {
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
            if !quarantine.exists() {
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
            if !is_owned_delete_quarantine(&quarantine)? {
                continue;
            }
            let metadata = match fs::symlink_metadata(&quarantine) {
                Ok(metadata) => metadata,
                Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
                Err(source) => {
                    return Err(Error::Io {
                        path: quarantine,
                        source,
                    })
                }
            };
            if is_link_or_reparse_point(&metadata) || !metadata.file_type().is_dir() {
                continue;
            }
            match fs::remove_dir_all(&quarantine) {
                Ok(()) => match fs::remove_file(&intent) {
                    Ok(()) => removed += 1,
                    Err(source) if source.kind() == io::ErrorKind::NotFound => removed += 1,
                    Err(source) => {
                        return Err(Error::Io {
                            path: intent,
                            source,
                        })
                    }
                },
                Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(Error::Io {
                        path: quarantine,
                        source,
                    })
                }
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
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(false);
    };
    let Some(token) = name.strip_prefix(DELETE_QUARANTINE_PREFIX) else {
        return Ok(false);
    };
    if token.ends_with(DELETE_INTENT_SUFFIX) || Ulid::from_string(token).is_err() {
        return Ok(false);
    }
    let intent = path.with_file_name(format!(
        "{DELETE_QUARANTINE_PREFIX}{token}{DELETE_INTENT_SUFFIX}"
    ));
    let expected = match fs::read_to_string(&intent) {
        Ok(expected) => expected,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(Error::Io {
                path: intent,
                source,
            })
        }
    };
    let directory = match IdentifiedDirectory::open(path) {
        Ok(directory) => directory,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(Error::Io {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    Ok(directory.identity().durable_key() == expected)
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
