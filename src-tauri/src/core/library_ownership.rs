//! The single database-to-filesystem ownership rule for Library directories.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sqlx::{Row, SqliteConnection};

use super::library_identity::{DirectoryIdentity, IdentifiedDirectory};
use super::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LibraryDirectoryOwner {
    Mod,
    ModWithPendingEnabledTransition,
    ActiveReinstall,
    ActiveStaging,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LibraryDirectoryDisposition {
    Owned(LibraryDirectoryOwner),
    IgnorableEmptyReinstallStage,
    Unreferenced,
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
    mods: HashMap<DirectoryIdentity, Vec<String>>,
    active_reinstall_directories: HashSet<DirectoryIdentity>,
    active_staging_directories: HashSet<DirectoryIdentity>,
    enabled_transition_mod_ids: HashSet<String>,
}

impl LibraryOwnershipSnapshot {
    #[cfg(test)]
    pub(super) fn empty_for_test() -> Self {
        Self {
            mods: HashMap::new(),
            active_reinstall_directories: HashSet::new(),
            active_staging_directories: HashSet::new(),
            enabled_transition_mod_ids: HashSet::new(),
        }
    }

    pub(super) async fn load(connection: &mut SqliteConnection) -> Result<Self> {
        let rows = sqlx::query("SELECT id AS mod_id, library_path FROM mods")
            .fetch_all(&mut *connection)
            .await?;
        let mut mods: HashMap<DirectoryIdentity, Vec<String>> = HashMap::new();
        for row in rows {
            let path = PathBuf::from(row.try_get::<String, _>("library_path")?);
            match IdentifiedDirectory::open(&path) {
                Ok(directory) => {
                    let mod_id: String = row.try_get("mod_id")?;
                    mods.entry(directory.identity().clone())
                        .or_default()
                        .push(mod_id);
                }
                // A missing spelling provides no filesystem identity to own.
                Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                Err(source) => return Err(Error::Io { path, source }),
            }
        }

        let mut active_reinstall_directories = HashSet::new();
        for witness in
            super::library_mutation::load_reinstall_swap_witnesses(&mut *connection).await?
        {
            if let Some(identity) = witnessed_identity_at_recorded_spelling(
                witness.staged_path(),
                witness.staged_identity(),
            )? {
                active_reinstall_directories.insert(identity);
            }
            if let Some(identity) = witnessed_identity_at_recorded_spelling(
                witness.quarantine_path(),
                witness.old_identity(),
            )? {
                active_reinstall_directories.insert(identity);
            }
        }

        let mut active_staging_directories = HashSet::new();
        for witness in
            super::library_mutation::load_staged_library_operation_witnesses(&mut *connection)
                .await?
        {
            if !witness.is_active() {
                continue;
            }
            if let Some(identity) = witnessed_identity_at_recorded_spelling(
                witness.staged_path(),
                witness.staged_identity(),
            )? {
                active_staging_directories.insert(identity);
            }
        }

        let enabled_transition_mod_ids = Self::enabled_transition_mod_ids(&mut *connection).await?;

        Ok(Self {
            mods,
            active_reinstall_directories,
            active_staging_directories,
            enabled_transition_mod_ids,
        })
    }

    pub(super) async fn enabled_transition_mod_ids(
        connection: &mut SqliteConnection,
    ) -> Result<HashSet<String>> {
        super::library_mutation::load_enabled_transition_witnesses(connection)
            .await
            .map(|witnesses| {
                witnesses
                    .into_iter()
                    .map(|witness| witness.mod_id().to_string())
                    .collect()
            })
    }

    pub(super) fn owner_of(&self, identity: &DirectoryIdentity) -> Option<LibraryDirectoryOwner> {
        if self.active_reinstall_directories.contains(identity) {
            return Some(LibraryDirectoryOwner::ActiveReinstall);
        }
        if self.active_staging_directories.contains(identity) {
            return Some(LibraryDirectoryOwner::ActiveStaging);
        }
        self.mods.get(identity).map(|mod_ids| {
            if mod_ids
                .iter()
                .any(|mod_id| self.enabled_transition_mod_ids.contains(mod_id))
            {
                LibraryDirectoryOwner::ModWithPendingEnabledTransition
            } else {
                LibraryDirectoryOwner::Mod
            }
        })
    }

    /// The one report/action rule for an immediate Library-root directory.
    ///
    /// A database witness owns an exact filesystem identity. Without one, a
    /// reserved reinstall name is ignorable only when GMM can prove it empty;
    /// a non-empty or uninspectable directory remains user-visible. Keeping
    /// that distinction here prevents audit, Reveal, Recover, and Delete from
    /// independently redefining what “unreferenced” means.
    pub(super) fn disposition_of(
        &self,
        directory: &IdentifiedDirectory,
    ) -> LibraryDirectoryDisposition {
        if let Some(owner) = self.owner_of(directory.identity()) {
            return LibraryDirectoryDisposition::Owned(owner);
        }
        let empty_reinstall_stage = directory.path().file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with(super::library_mutation::REINSTALL_STAGING_PREFIX)
        }) && fs::read_dir(directory.path())
            .is_ok_and(|mut entries| entries.next().is_none());
        if empty_reinstall_stage {
            LibraryDirectoryDisposition::IgnorableEmptyReinstallStage
        } else {
            LibraryDirectoryDisposition::Unreferenced
        }
    }

    /// Mod rows that currently resolve to `identity`.
    ///
    /// This is also the one uniqueness rule. There is deliberately no SQLite
    /// UNIQUE index on `mods.library_path`: path strings are not filesystem
    /// identity, and SQLite cannot express Windows volume serial plus file
    /// index. Audit and explicit duplicate resolution therefore use this same
    /// opened-directory evidence instead of presenting a string approximation
    /// as the invariant.
    pub(super) fn mod_ids_for(&self, identity: &DirectoryIdentity) -> &[String] {
        self.mods.get(identity).map(Vec::as_slice).unwrap_or(&[])
    }

    pub(super) fn duplicate_mod_ids(&self) -> HashSet<String> {
        self.mods
            .values()
            .filter(|ids| ids.len() > 1)
            .flatten()
            .cloned()
            .collect()
    }

    pub(super) fn duplicate_mod_groups(&self) -> Vec<Vec<String>> {
        self.mods
            .values()
            .filter(|ids| ids.len() > 1)
            .cloned()
            .collect()
    }
}

/// A durable identity owns bytes only while the recorded pathname still names
/// that exact filesystem object. `NotFound` contributes no owner; every other
/// inspection failure keeps audit and destructive actions fail-closed.
fn witnessed_identity_at_recorded_spelling(
    path: &Path,
    expected: &DirectoryIdentity,
) -> Result<Option<DirectoryIdentity>> {
    match IdentifiedDirectory::open(path) {
        Ok(directory) if directory.identity() == expected => Ok(Some(expected.clone())),
        Ok(_) => Ok(None),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}
