//! Mod Variants (slice 5).
//!
//! A Mod has zero, two, or more Variants — mutually-exclusive subfolders
//! that hold alternative looks (hair colours, costume variants, etc).
//! Exactly one Variant is active at any time when there are 2+; a Mod
//! with zero Variants behaves like a single-folder Mod.
//!
//! The detection heuristic in this module runs against an extracted
//! Library subtree right after import (slice 1a `adopt_folder`,
//! slice 1b `import_zip`). The shape we accept as multi-variant:
//!
//! * The Mod root contains no `.ini` files. A root-level `.ini` means
//!   the archive describes a single Mod, even if it has sibling
//!   directories beside it (typical "preview images + a single set
//!   of files" layout).
//! * Two or more first-level subdirectories each contain at least one
//!   `.ini` somewhere inside them.
//!
//! Anything else is treated as a single-folder Mod (empty Variant
//! list).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::{Error, Result};

/// One detected Variant. `name` is the subdirectory name as it
/// appeared on disk; `subpath` is the path relative to the Mod root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedVariant {
    pub name: String,
    pub subpath: PathBuf,
}

/// A persisted Variant row, surfaced to the UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Variant {
    pub id: String,
    pub mod_id: String,
    pub name: String,
    pub subpath: PathBuf,
}

/// Detect Variants inside `mod_root`. Returns the list sorted by
/// name (lexicographic, case-sensitive). An empty list means the Mod
/// has no Variants — callers should treat it as a single-folder Mod.
pub fn detect_variants(mod_root: &Path) -> Result<Vec<DetectedVariant>> {
    if !mod_root.is_dir() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(mod_root).map_err(|source| Error::Io {
        path: mod_root.to_path_buf(),
        source,
    })?;

    let mut root_has_ini = false;
    let mut candidate_dirs: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: mod_root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_file() {
            if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("ini"))
            {
                root_has_ini = true;
            }
        } else if file_type.is_dir() {
            candidate_dirs.push(path);
        }
    }

    if root_has_ini {
        return Ok(Vec::new());
    }

    let mut variants: Vec<DetectedVariant> = candidate_dirs
        .into_iter()
        .filter_map(|dir| {
            if has_ini_anywhere(&dir) {
                let name = dir.file_name()?.to_string_lossy().to_string();
                let subpath = PathBuf::from(&name);
                Some(DetectedVariant { name, subpath })
            } else {
                None
            }
        })
        .collect();

    variants.sort_by(|a, b| a.name.cmp(&b.name));

    if variants.len() >= 2 {
        Ok(variants)
    } else {
        Ok(Vec::new())
    }
}

/// Does `dir` contain at least one `.ini` file at any depth?
fn has_ini_anywhere(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_file() {
            if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("ini"))
            {
                return true;
            }
        } else if file_type.is_dir() && has_ini_anywhere(&path) {
            return true;
        }
    }
    false
}
