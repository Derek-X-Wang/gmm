//! Fallible filesystem presence checks.
//!
//! `None` has one meaning here: the filesystem returned `NotFound`. Every
//! other error remains an error so callers cannot turn uncertainty into a
//! confident claim that an entry is absent.

use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};

use super::error::{Error, Result};

/// Resolve a lookup on an entry already yielded by `read_dir`.
///
/// `NotFound` means the enumerated entry vanished before the follow-up lookup,
/// so its caller should skip that entry. Every other failure remains
/// uncertainty and must abort the operation.
///
/// Pass only one immediate lookup on that entry. A composite result, loop, or
/// recursive traversal widens this contract and can swallow `NotFound` from
/// unrelated work performed after the lookup boundary.
pub(super) fn resolve_enumerated_entry<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(Error::Io { ref source, .. }) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Read target-following metadata, returning `None` only for `NotFound`.
pub(super) fn metadata_if_exists(path: &Path) -> io::Result<Option<Metadata>> {
    optional_metadata(fs::metadata(path))
}

/// Read entry metadata without following links, returning `None` only for
/// `NotFound`.
pub(super) fn symlink_metadata_if_exists(path: &Path) -> io::Result<Option<Metadata>> {
    optional_metadata(fs::symlink_metadata(path))
}

/// Test whether `path` is inside `ancestor` without hiding canonicalization
/// failures.
///
/// `NotFound` permits the same component-wise lexical fallback used by the
/// legacy comparison helpers: an active witness can legitimately name a path
/// that has not been created yet or has already vanished. Every other error is
/// uncertainty and must stop the caller before it moves Library bytes.
pub(super) fn path_within(path: &Path, ancestor: &Path) -> Result<bool> {
    let canonical_path = canonicalize_if_exists(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let canonical_ancestor = canonicalize_if_exists(ancestor).map_err(|source| Error::Io {
        path: ancestor.to_path_buf(),
        source,
    })?;
    Ok(match (canonical_path, canonical_ancestor) {
        (Some(path), Some(ancestor)) => path.starts_with(ancestor),
        _ => path.starts_with(ancestor),
    })
}

fn optional_metadata(result: io::Result<Metadata>) -> io::Result<Option<Metadata>> {
    match result {
        Ok(metadata) => Ok(Some(metadata)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(source),
    }
}

fn canonicalize_if_exists(path: &Path) -> io::Result<Option<PathBuf>> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(Some(path)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_path_is_proven_absent() {
        let temp = tempfile::tempdir().expect("temporary directory");

        assert!(
            metadata_if_exists(&temp.path().join("missing"))
                .expect("NotFound is a known answer")
                .is_none(),
            "NotFound must be represented as proven absence",
        );
    }

    #[cfg(unix)]
    #[test]
    fn metadata_uncertainty_is_not_absence() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary directory");
        let looped = temp.path().join("looped");
        symlink(&looped, &looped).expect("self-referential symlink");

        let error = metadata_if_exists(&looped).expect_err("a link loop is uncertainty");
        assert_ne!(
            error.kind(),
            io::ErrorKind::NotFound,
            "a metadata error other than NotFound must remain distinguishable from absence",
        );
    }

    #[cfg(unix)]
    #[test]
    fn path_containment_propagates_canonicalization_uncertainty() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary directory");
        let looped = temp.path().join("looped");
        symlink(&looped, &looped).expect("self-referential symlink");

        let error = path_within(&looped, temp.path())
            .expect_err("a link loop cannot prove path containment");
        assert!(
            matches!(error, Error::Io { ref path, ref source }
                if path == &looped && source.kind() != io::ErrorKind::NotFound),
            "canonicalization uncertainty must remain an I/O error, got {error:?}",
        );
    }
}
