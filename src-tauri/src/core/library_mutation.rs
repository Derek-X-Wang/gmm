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

/// A deliberate exception is executable documentation, not a comment that a
/// later refactor can accidentally erase. #166 owns staging-and-swap for the
/// existing destructive reinstall path; #164 must not partially implement it.
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
        directory_name: &str,
    ) {
        let old_path = snapshot.path().join(directory_name);
        remove_staged_dir(&old_path);

        // A relocation may have carried the staged ULID into its new root.
        // The ULID belongs to this failed operation and no row was committed,
        // so removing that exact sibling is cleanup, not deletion of a Mod.
        if let Ok(current_root) = self.resolved_library_root_for(snapshot.game).await {
            let current_path = current_root.join(directory_name);
            if current_path != old_path {
                remove_staged_dir(&current_path);
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

fn remove_staged_dir(path: &Path) {
    match std::fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => tracing::warn!(
            target: "gmm::library",
            path = %path.display(),
            error = %source,
            "could not remove a staged Library directory after root revalidation failed",
        ),
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
