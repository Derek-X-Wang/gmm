//! ZIP ingest for the Library (slice 1b).
//!
//! Given a `.zip` and a target directory, extract its contents into the
//! target. The function is hardened against the dirty realities of
//! GameBanana-style archives:
//!
//! * zip-slip path traversal (`../etc/passwd`) is rejected before any I/O.
//! * Junk files from creators' platforms (`__MACOSX/`, `.DS_Store`,
//!   `Thumbs.db`) are silently dropped on import.
//! * Single-root archives — common GameBanana shape — collapse the
//!   redundant outer directory so the Mod's Library tree begins at the
//!   real content.
//! * Hard size and entry-count caps stop oversize / zip-bomb archives.
//!
//! See `CONTEXT.md` § Mod and ADR 0003 for why the Library is the source
//! of truth and junctions are the overlay mechanism.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};

use super::error::{Error, Result};

/// Caps and options for [`extract`]. Defaults mirror the values cited in the
/// slice 1b acceptance criteria (2 GiB / 10000 entries).
#[derive(Debug, Clone, Copy)]
pub struct ImportZipOptions {
    /// Hard cap on the sum of declared uncompressed sizes in the archive.
    /// `0` disables the check.
    pub max_uncompressed_bytes: u64,
    /// Hard cap on the number of entries (files + directories).
    /// `0` disables the check.
    pub max_entries: u32,
}

impl Default for ImportZipOptions {
    fn default() -> Self {
        Self {
            max_uncompressed_bytes: 2 * 1024 * 1024 * 1024, // 2 GiB
            max_entries: 10_000,
        }
    }
}

/// Extract `zip_path` into `target_dir`. `target_dir` must not already
/// exist (this function creates it). On any error the caller is
/// responsible for removing `target_dir`; we leave it in whatever state we
/// reached. See [`Core::import_zip`](crate::core::Core::import_zip) for
/// the cleanup-on-failure orchestration.
pub fn extract(zip_path: &Path, target_dir: &Path, opts: ImportZipOptions) -> Result<()> {
    let file = File::open(zip_path).map_err(|source| Error::Io {
        path: zip_path.to_path_buf(),
        source,
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(Error::from_zip_with_path(zip_path))?;

    let normalised = plan_extraction(&mut archive, opts)?;

    fs::create_dir_all(target_dir).map_err(|source| Error::Io {
        path: target_dir.to_path_buf(),
        source,
    })?;

    for entry in normalised {
        let dest = target_dir.join(&entry.relative_path);

        match entry.kind {
            EntryKind::Dir => {
                fs::create_dir_all(&dest).map_err(|source| Error::Io {
                    path: dest.clone(),
                    source,
                })?;
            }
            EntryKind::File { index } => {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).map_err(|source| Error::Io {
                        path: parent.to_path_buf(),
                        source,
                    })?;
                }
                let mut zfile = archive
                    .by_index(index)
                    .map_err(Error::from_zip_with_path(zip_path))?;
                let mut out = File::create(&dest).map_err(|source| Error::Io {
                    path: dest.clone(),
                    source,
                })?;
                io::copy(&mut zfile, &mut out).map_err(|source| Error::Io {
                    path: dest.clone(),
                    source,
                })?;
            }
        }
    }

    Ok(())
}

/// Result of walking the archive header without touching disk. We can
/// reject zip-slip, oversize, and entry-count violations before any
/// extraction starts.
#[derive(Debug)]
struct PlannedEntry {
    relative_path: PathBuf,
    kind: EntryKind,
}

#[derive(Debug)]
enum EntryKind {
    Dir,
    File { index: usize },
}

fn plan_extraction<R: io::Read + io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    opts: ImportZipOptions,
) -> Result<Vec<PlannedEntry>> {
    let total = archive.len();
    if opts.max_entries != 0 && total as u32 > opts.max_entries {
        return Err(Error::ZipEntryCap {
            cap: opts.max_entries,
            actual: total,
        });
    }

    // First pass: collect entries (skipping junk) and reject zip-slip.
    let mut entries: Vec<PlannedEntry> = Vec::with_capacity(total);
    let mut top_level_dirs: HashSet<String> = HashSet::new();
    let mut top_level_files: HashSet<String> = HashSet::new();
    let mut declared_bytes: u64 = 0;

    for i in 0..total {
        let zfile = archive.by_index(i).map_err(Error::from_zip)?;
        let raw_name = zfile.name().to_string();
        let enclosed = match zfile.enclosed_name() {
            Some(p) => p,
            None => return Err(Error::ZipSlip(raw_name)),
        };
        let relative = sanitize_relative(&enclosed).ok_or(Error::ZipSlip(raw_name.clone()))?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        if is_junk(&relative) {
            continue;
        }

        let is_dir = zfile.is_dir() || raw_name.ends_with('/');
        let kind = if is_dir {
            EntryKind::Dir
        } else {
            declared_bytes = declared_bytes.saturating_add(zfile.size());
            EntryKind::File { index: i }
        };

        if let Some(Component::Normal(first)) = relative.components().next() {
            let s = first.to_string_lossy().to_string();
            if relative.components().count() == 1 {
                if matches!(kind, EntryKind::Dir) {
                    top_level_dirs.insert(s);
                } else {
                    top_level_files.insert(s);
                }
            } else {
                top_level_dirs.insert(s);
            }
        }

        entries.push(PlannedEntry {
            relative_path: relative,
            kind,
        });
    }

    if opts.max_uncompressed_bytes != 0 && declared_bytes > opts.max_uncompressed_bytes {
        return Err(Error::ZipSizeCap {
            cap: opts.max_uncompressed_bytes,
            actual: declared_bytes,
        });
    }

    // Single-root normalisation: if every top-level entry sits under one
    // directory (and there are no stray top-level files), drop that
    // directory prefix so the Mod root starts at the real content.
    let strip_prefix = if top_level_dirs.len() == 1 && top_level_files.is_empty() {
        top_level_dirs.iter().next().cloned()
    } else {
        None
    };

    if let Some(prefix) = strip_prefix {
        let prefix = PathBuf::from(&prefix);
        let mut normalised = Vec::with_capacity(entries.len());
        for entry in entries {
            let new_rel = match entry.relative_path.strip_prefix(&prefix) {
                Ok(r) => r.to_path_buf(),
                Err(_) => entry.relative_path.clone(),
            };
            if new_rel.as_os_str().is_empty() {
                // Skip the prefix directory itself.
                continue;
            }
            normalised.push(PlannedEntry {
                relative_path: new_rel,
                kind: entry.kind,
            });
        }
        Ok(normalised)
    } else {
        Ok(entries)
    }
}

/// Reject anything that escapes the target (`..`, drive letters, absolute
/// paths). Returns `None` if the path is unsafe.
fn sanitize_relative(p: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => out.push(s),
            Component::CurDir => continue,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => return None,
        }
    }
    Some(out)
}

/// Junk files we never want in the Library, regardless of where in the
/// archive they appear. Match against any path component to catch
/// `nested/__MACOSX/foo` shapes too.
fn is_junk(rel: &Path) -> bool {
    for c in rel.components() {
        if let Component::Normal(part) = c {
            let s = part.to_string_lossy();
            if s == "__MACOSX" || s == ".DS_Store" || s == "Thumbs.db" {
                return true;
            }
        }
    }
    false
}

impl Error {
    fn from_zip(err: zip::result::ZipError) -> Error {
        Error::Zip {
            path: PathBuf::new(),
            message: err.to_string(),
        }
    }

    fn from_zip_with_path(path: &Path) -> impl Fn(zip::result::ZipError) -> Error + '_ {
        move |err| Error::Zip {
            path: path.to_path_buf(),
            message: err.to_string(),
        }
    }
}
