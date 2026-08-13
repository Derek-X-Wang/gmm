//! Single-instance enforcement, scoped to the GMM data directory.
//!
//! # Why GMM is single-instance
//!
//! Two GMM processes sharing one `gmm.db` and one Library is not a
//! configuration we support. Making it safe would mean making *every*
//! mutation path concurrency-safe across processes, and the paths that
//! would need it are the ones where being wrong is expensive:
//!
//! * [`Core::set_enabled`] reads a Mod's `enabled` column, touches the
//!   filesystem, then writes the column back. Two processes interleaving
//!   there can leave the DB saying one thing and `<Game>/Mods/` saying
//!   another, with no error raised on either side.
//! * `sqlx`'s SQLite migrator takes no cross-process lock — its `lock`
//!   implementation is a no-op for SQLite. Two cold instances against an
//!   old schema race to apply the same migration.
//! * Importer install rewrites files inside the game directory. The
//!   Library is the source of truth (ADR 0003), but the game directory
//!   is not, and there is nothing to reconcile it against.
//!
//! Refusing the second instance costs one file handle and removes all of
//! that at once. This is also what every comparable tool does (XXMI,
//! Mod Organizer 2, Vortex).
//!
//! # Why the lock is scoped to the data directory
//!
//! The hazard is not "two GMM windows" — it is two writers against one
//! `gmm.db` and one Library. Locking the *data directory* names exactly
//! that resource. An executable-identity lock (what
//! `tauri-plugin-single-instance` provides) would wave through the case
//! most likely to bite a developer or a user with a portable copy: two
//! different GMM builds pointed at the same `%APPDATA%\GMM`.
//!
//! # Why a file lock and not a PID file
//!
//! Both backing primitives are released by the kernel when the process
//! dies, however it dies. A `SIGKILL`, a power cut, or a panic leaves a
//! stale lock *file* but never a stale *lock*, so there is no recovery
//! path to get wrong and no "delete this file to fix it" support burden.
//! A PID file has both problems, plus PID reuse.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Name of the lock file inside the data directory. Public so tests and
/// diagnostics can name it without hardcoding the string twice.
pub const LOCK_FILE_NAME: &str = "instance.lock";

/// A held single-instance lock. The lock lives as long as this value:
/// dropping it (or exiting the process, however abruptly) releases it.
///
/// Hold it for the lifetime of the process — binding it to `_` releases
/// it immediately, which is worse than not taking it at all.
#[derive(Debug)]
#[must_use = "the lock is released as soon as this value is dropped"]
pub struct InstanceLock {
    /// Kept solely for its `Drop`: closing the handle is what releases
    /// the underlying `flock` / share-mode lock.
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    /// Path of the lock file being held.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Error)]
pub enum InstanceLockError {
    #[error(
        "another GMM instance is already using this data directory (lock held on {path:?}). \
         Close the running GMM and try again."
    )]
    AlreadyRunning { path: PathBuf },

    #[error("could not open the instance lock at {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Try to become the single GMM instance for `data_dir`.
///
/// Creates `data_dir` if it does not exist — startup calls this before
/// anything else, so it cannot assume the directory is already there.
///
/// Returns [`InstanceLockError::AlreadyRunning`] if another live process
/// holds the lock. Never blocks.
pub fn acquire(data_dir: &Path) -> Result<InstanceLock, InstanceLockError> {
    std::fs::create_dir_all(data_dir).map_err(|source| InstanceLockError::Io {
        path: data_dir.to_path_buf(),
        source,
    })?;

    let path = data_dir.join(LOCK_FILE_NAME);
    let file = open_exclusive(&path)?;

    Ok(InstanceLock { _file: file, path })
}

#[cfg(unix)]
fn open_exclusive(path: &Path) -> Result<File, InstanceLockError> {
    use std::os::unix::io::AsRawFd;

    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(|source| InstanceLockError::Io {
            path: path.to_path_buf(),
            source,
        })?;

    // `flock` locks the open file description, not the path, so a second
    // `open` in *this* process is refused too — which is what makes the
    // in-process tests meaningful rather than a special case.
    //
    // SAFETY: `fd` is owned by `file` and outlives the call.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let source = io::Error::last_os_error();
        return match source.raw_os_error() {
            Some(libc::EWOULDBLOCK) => Err(InstanceLockError::AlreadyRunning {
                path: path.to_path_buf(),
            }),
            _ => Err(InstanceLockError::Io {
                path: path.to_path_buf(),
                source,
            }),
        };
    }

    Ok(file)
}

#[cfg(windows)]
fn open_exclusive(path: &Path) -> Result<File, InstanceLockError> {
    use std::os::windows::fs::OpenOptionsExt;

    // `share_mode(0)` asks the kernel for exclusive access: while this
    // handle is open, no other handle to the file can be opened at all.
    // That is the lock. No `LockFileEx` call is needed, and the kernel
    // closes the handle — releasing the lock — however the process dies.
    let result = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .share_mode(0)
        .open(path);

    match result {
        Ok(file) => Ok(file),
        // ERROR_SHARING_VIOLATION (32): someone else holds the handle.
        Err(source) if source.raw_os_error() == Some(32) => {
            Err(InstanceLockError::AlreadyRunning {
                path: path.to_path_buf(),
            })
        }
        Err(source) => Err(InstanceLockError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}
