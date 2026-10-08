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

/// Positive namespace ownership for a Library writer, not proof of presence.
///
/// Resolve aliases on both sides, including existing parents of missing
/// children. Only a proven absent, ordinary suffix may be appended to a
/// resolved directory. Unreadable paths and dangling aliases remain errors;
/// lexical containment alone cannot authorize a Junction mutation. Callers
/// creating a deployment must additionally identify the existing directories.
pub(super) fn library_path_within(path: &Path, ancestor: &Path) -> Result<bool> {
    let resolve = |path: &Path| {
        resolve_library_namespace(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
    };
    Ok(resolve(path)?.starts_with(resolve(ancestor)?))
}

fn resolve_library_namespace(path: &Path) -> io::Result<PathBuf> {
    let mut candidate = path.to_path_buf();
    let mut missing = Vec::new();
    loop {
        match fs::canonicalize(&candidate) {
            Ok(mut resolved) => {
                if !missing.is_empty() {
                    // A missing suffix can only descend from a directory.
                    if !fs::metadata(&resolved)?.is_dir() {
                        return Err(io::Error::from(io::ErrorKind::NotADirectory));
                    }
                    for name in missing.iter().rev() {
                        resolved.push(name);
                    }
                }
                return Ok(resolved);
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                // A dangling alias exists but its destination is unresolved;
                // do not mistake that for an ordinary absent child name.
                if symlink_metadata_if_exists(&candidate)?.is_some()
                    || path
                        .components()
                        .any(|part| part == std::path::Component::ParentDir)
                {
                    return Err(source);
                }
                let Some(name) = candidate.file_name() else {
                    return Err(source);
                };
                missing.push(name.to_os_string());
                let Some(parent) = candidate.parent() else {
                    return Err(source);
                };
                candidate = if parent.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    parent.to_path_buf()
                };
            }
            Err(source) => return Err(source),
        }
    }
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

    #[cfg(unix)]
    #[test]
    fn library_containment_preserves_uncertainty_on_either_side() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = fs::canonicalize(temp.path()).expect("canonical root");
        let looped = root.join("looped");
        symlink(&looped, &looped).expect("self-referential symlink");
        for (path, ancestor) in [(&looped, &root), (&root, &looped)] {
            assert!(
                matches!(library_path_within(path, ancestor), Err(Error::Io { ref source, .. })
                    if source.kind() != io::ErrorKind::NotFound),
                "unreadable paths must never provide positive Library containment",
            );
        }
    }

    #[test]
    fn library_containment_resolves_aliases_before_appending_missing_children() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = fs::canonicalize(temp.path()).expect("canonical root");
        let library = root.join("library");
        fs::create_dir(&library).expect("Library");
        let alias = root.join("alias");
        super::super::junction::create(&alias, &library).expect("Library alias");
        assert!(
            library_path_within(&alias.join("missing"), &library).expect("known missing child"),
            "a missing target through a resolved alias remains Library-owned",
        );
        assert!(
            library_path_within(&library.join("missing"), &alias).expect("aliased ancestor"),
            "ancestor aliases must also resolve before missing-path comparison",
        );
    }

    #[test]
    fn library_containment_does_not_trust_missing_children_through_an_outside_alias() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = fs::canonicalize(temp.path()).expect("canonical root");
        let library = root.join("library");
        let outside = root.join("outside");
        fs::create_dir(&library).expect("Library");
        fs::create_dir(&outside).expect("outside");
        let alias = library.join("escape");
        super::super::junction::create(&alias, &outside).expect("outside alias");
        assert!(
            !library_path_within(&alias.join("missing"), &library).expect("known missing child"),
            "a lexical child through an outside alias is not Library-owned",
        );
    }

    #[test]
    fn library_containment_keeps_ordinary_missing_namespace_ownership() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = fs::canonicalize(temp.path()).expect("canonical root");
        let missing_library = root.join("missing-library");
        assert!(library_path_within(
            &missing_library.join("Variant").join("missing"),
            &missing_library,
        )
        .expect("both paths have proven missing suffixes"));
        assert!(!library_path_within(&root.join("other"), &missing_library)
            .expect("disjoint missing namespaces"));
        assert!(
            library_path_within(
                &missing_library.join("missing").join("..").join("other"),
                &root
            )
            .is_err(),
            "a missing prefix cannot prove the resolution of parent traversal",
        );
    }

    #[cfg(unix)]
    #[test]
    fn library_containment_does_not_treat_dangling_aliases_as_absent_names() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = fs::canonicalize(temp.path()).expect("canonical root");
        let dangling = root.join("dangling");
        symlink(root.join("missing"), &dangling).expect("dangling alias");
        for path in [&dangling, &dangling.join("child")] {
            assert!(
                library_path_within(path, &root).is_err(),
                "a dangling alias does not establish a resolved Library namespace",
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn library_containment_resolves_drive_letter_case_with_missing_children() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let temp = tempfile::tempdir().expect("temporary directory");
        let root = fs::canonicalize(temp.path()).expect("canonical root");
        let mut spelling: Vec<u16> = root.as_os_str().encode_wide().collect();
        let colon = spelling
            .iter()
            .position(|&ch| ch == b':' as u16)
            .expect("drive path");
        let drive = &mut spelling[colon - 1];
        *drive = if (b'A' as u16..=b'Z' as u16).contains(drive) {
            *drive + 32
        } else {
            *drive - 32
        };
        let alias = PathBuf::from(std::ffi::OsString::from_wide(&spelling));
        assert!(library_path_within(&alias.join("missing"), &root).expect("drive alias"));
        assert!(library_path_within(&root.join("missing"), &alias).expect("ancestor drive alias"));
    }

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
