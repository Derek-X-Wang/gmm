//! Per-game install-path detection.
//!
//! Each supported Game lives in its own submodule with a single
//! responsibility: produce an `Option<PathBuf>` pointing at the
//! directory that contains the game's executable. Detection is a
//! best-effort heuristic — when it fails the UI falls back to a
//! manual picker, exactly as it did before this slice landed.
//!
//! GMM never copies XXMI Launcher's per-game detection code (ADR 0002),
//! but the heuristics (registry uninstall keys, exe + Data-folder
//! validation) are public.

pub mod endfield;
pub mod epic;
pub mod genshin;
pub mod honkai_impact;
pub mod star_rail;
pub mod wuthering;
pub mod zenless;

use std::fs;
use std::path::{Path, PathBuf};

use super::error::{Error, Result};
use super::filesystem::metadata_if_exists;

/// Maximum directory entries inspected below each non-validating candidate.
/// This bounds work even when a launcher records a large drive-root directory.
pub const MAX_CANDIDATE_CHILDREN: usize = 256;

/// Return validated install directories in source order, without duplicates.
/// A valid candidate wins over its children. Otherwise inspect immediate real
/// directories only (never follow directory symlinks or recurse), in sorted
/// order within the bounded set. Unreadable entries are unusable optional probes.
pub fn validated_candidates<I>(candidates: I, validate: fn(&Path) -> bool) -> Vec<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut seen = std::collections::HashSet::new();
    let mut emitted = std::collections::HashSet::new();
    let mut out = Vec::new();
    for candidate in candidates {
        if !seen.insert(candidate.clone()) {
            continue;
        }
        if validate(&candidate) {
            if emitted.insert(candidate.clone()) {
                out.push(candidate);
            }
            continue;
        }
        let Ok(entries) = fs::read_dir(&candidate) else {
            continue;
        };
        let mut children: Vec<_> = entries
            .take(MAX_CANDIDATE_CHILDREN)
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                #[allow(
                    clippy::disallowed_methods,
                    reason = "optional-install expansion intentionally skips unreadable directory entries"
                )]
                let usable_directory = matches!(entry.file_type(), Ok(kind) if kind.is_dir());
                usable_directory
            })
            .map(|entry| entry.path())
            .collect();
        children.sort();
        for child in children {
            if !emitted.contains(&child) && validate(&child) && emitted.insert(child.clone()) {
                out.push(child);
            }
        }
    }
    out
}

/// Saved paths require a readable install. Optional auto-detection keeps its
/// existing best-effort validators; a manual choice must preserve uncertainty.
fn validate_install(path: &Path, exe_names: &[&str], data: Option<&Path>) -> Result<bool> {
    if !readable_directory(path)? {
        return Ok(false);
    }
    let mut executable_present = false;
    for name in exe_names {
        let executable = path.join(name);
        let metadata = metadata_if_exists(&executable).map_err(|source| Error::Io {
            path: executable.clone(),
            source,
        })?;
        if let Some(metadata) = metadata {
            if metadata.is_file() {
                fs::File::open(&executable).map_err(|source| Error::Io {
                    path: executable,
                    source,
                })?;
                executable_present = true;
            }
        }
    }
    let data_present = match data {
        Some(data) => readable_directory(data)?,
        None => false,
    };
    Ok(executable_present && data_present)
}

fn readable_directory(path: &Path) -> Result<bool> {
    let metadata = metadata_if_exists(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let Some(metadata) = metadata else {
        return Ok(false);
    };
    if !metadata.is_dir() {
        return Ok(false);
    }
    // Opening metadata does not prove that the directory can be read. Consume
    // the iterator too, because enumeration may fail after read_dir succeeds.
    for entry in fs::read_dir(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })? {
        entry.map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(true)
}
