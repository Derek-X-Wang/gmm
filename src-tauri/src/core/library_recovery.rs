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
use std::io;
use std::path::{Component, Path, PathBuf};

use chrono::Utc;
use serde::Serialize;
use sqlx::Row;
use ulid::Ulid;

use super::library_audit::{directory_size_without_links, is_link_or_reparse_point};
use super::{crash_points, Core, Error, GameCode, Mod, Result, Source};

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

impl Core {
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
        self.validate_unreferenced_library_dir(game, path).await
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
        let path = self.validate_unreferenced_library_dir(game, path).await?;

        let id = self.recovered_mod_id(&path).await?;
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
        let junction_dir_name = self.pick_unique_junction_dir_name(game, &base).await?;
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
        .execute(&self.pool)
        .await?;

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
        let path = self.validate_unreferenced_library_dir(game, path).await?;
        let directory_name = path
            .file_name()
            .expect("a validated orphan has a file name")
            .to_string_lossy()
            .into_owned();

        let removed = path.clone();
        let size_bytes = tokio::task::spawn_blocking(move || {
            let size = directory_size_without_links(&removed).ok();
            fs::remove_dir_all(&removed)
                .map(|()| size)
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
    async fn recovered_mod_id(&self, path: &Path) -> Result<String> {
        let name = path
            .file_name()
            .expect("a validated orphan has a file name")
            .to_string_lossy()
            .into_owned();
        if Ulid::from_string(&name).is_ok() && !self.mod_id_exists(&name).await? {
            return Ok(name);
        }
        Ok(Ulid::new().to_string())
    }

    async fn mod_id_exists(&self, id: &str) -> Result<bool> {
        Ok(sqlx::query("SELECT 1 FROM mods WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .is_some())
    }

    /// The shared precondition for every action in this module.
    async fn validate_unreferenced_library_dir(
        &self,
        game: GameCode,
        path: &Path,
    ) -> Result<PathBuf> {
        let root = self.resolved_library_root_for(game).await?;
        let path =
            lexically_normalized(path).ok_or_else(|| Error::NotAnUnreferencedLibraryDir {
                path: path.to_path_buf(),
                reason: "the path is not absolute or contains `..`".to_string(),
            })?;

        let parent = path.parent();
        if parent
            != Some(
                lexically_normalized(&root)
                    .unwrap_or(root.clone())
                    .as_path(),
            )
        {
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

        // Across every game, not just this one: a per-game Library root
        // override can point two games at the same directory, and a row
        // from either one makes these bytes somebody's Mod.
        let rows = sqlx::query("SELECT library_path FROM mods")
            .fetch_all(&self.pool)
            .await?;
        for row in rows {
            let referenced = PathBuf::from(row.try_get::<String, _>("library_path")?);
            if lexically_normalized(&referenced).as_deref() == Some(path.as_path()) {
                return Err(Error::NotAnUnreferencedLibraryDir {
                    path,
                    reason: "a Mod now references it — refresh the report".to_string(),
                });
            }
        }

        Ok(path)
    }
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
