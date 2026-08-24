//! The single database-to-filesystem ownership rule for Library directories.

use std::collections::HashSet;
use std::io;
use std::path::PathBuf;

use sqlx::{Executor, Row, Sqlite};

use super::library_identity::{DirectoryIdentity, IdentifiedDirectory};
use super::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LibraryDirectoryOwner {
    Mod,
    ActiveReinstall,
}

/// A point-in-time view of every filesystem object GMM currently owns.
///
/// Ownership is deliberately global rather than scoped to a Game. Per-game
/// Library root overrides may legitimately resolve two Games to the same
/// directory, so a Mod or active reinstall from either Game owns its bytes for
/// every audit and every recovery/delete guard that examines that root.
///
/// A referenced `library_path` that no longer exists contributes no identity:
/// it cannot own an object through a different spelling that still resolves.
/// This means an audit can report a real Mod directory as unreferenced when the
/// stored spelling returns `NotFound` but another spelling reaches the same
/// object. Every other open failure aborts the snapshot so all callers fail
/// closed together instead of deleting bytes while ownership is uncertain.
#[derive(Debug, Clone)]
pub(super) struct LibraryOwnershipSnapshot {
    mods: HashSet<DirectoryIdentity>,
    active_reinstall_directories: HashSet<String>,
}

impl LibraryOwnershipSnapshot {
    pub(super) async fn load<'e, E>(executor: E) -> Result<Self>
    where
        E: Executor<'e, Database = Sqlite>,
    {
        let rows = sqlx::query(
            "SELECT library_path, NULL AS reinstall_identity FROM mods
             UNION ALL
             SELECT staged_path AS library_path, staged_identity AS reinstall_identity
             FROM reinstall_swaps
             UNION ALL
             SELECT quarantine_path AS library_path, old_identity AS reinstall_identity
             FROM reinstall_swaps",
        )
        .fetch_all(executor)
        .await?;
        let mut mods = HashSet::new();
        let mut active_reinstall_directories = HashSet::new();
        for row in rows {
            if let Some(identity) = row.try_get::<Option<String>, _>("reinstall_identity")? {
                active_reinstall_directories.insert(identity);
                continue;
            }

            let path = PathBuf::from(row.try_get::<String, _>("library_path")?);
            match IdentifiedDirectory::open(&path) {
                Ok(directory) => {
                    mods.insert(directory.identity().clone());
                }
                // A missing spelling provides no filesystem identity to own.
                Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                Err(source) => return Err(Error::Io { path, source }),
            }
        }

        Ok(Self {
            mods,
            active_reinstall_directories,
        })
    }

    pub(super) fn owner_of(&self, identity: &DirectoryIdentity) -> Option<LibraryDirectoryOwner> {
        if self
            .active_reinstall_directories
            .contains(&identity.durable_key())
        {
            return Some(LibraryDirectoryOwner::ActiveReinstall);
        }
        self.mods
            .contains(identity)
            .then_some(LibraryDirectoryOwner::Mod)
    }
}
