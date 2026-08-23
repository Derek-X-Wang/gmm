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
//! * Hard size and entry-count caps stop oversize / zip-bomb archives —
//!   enforced against the bytes that actually arrive, not the sizes the
//!   archive claims, since an attacker writes those too.
//! * Names that are safe on one filesystem and dangerous on another are
//!   refused outright: backslash traversal, drive letters and UNC paths,
//!   NTFS alternate-data-stream names, trailing dots and spaces,
//!   reserved DOS device names in any component, entries that collide
//!   case-insensitively, and symlink entries. See [`check_entry_name`]
//!   and issue #60.
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

/// Extract `zip_path` into `target_dir`. `target_dir` must be absent or an
/// empty directory reserved by the caller (this function creates it when
/// needed). On any error the caller is responsible for removing `target_dir`;
/// we leave it in whatever state we reached. See
/// [`Core::import_zip`](crate::core::Core::import_zip) for the
/// cleanup-on-failure orchestration.
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

    let mut written: u64 = 0;
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
                // The planner's cap runs on the sizes the archive
                // *declares*, and an attacker writes those. Cap the
                // bytes that actually arrive as well: read one byte past
                // the budget so an entry that lies is caught rather than
                // silently truncated.
                let budget = remaining_bytes(opts.max_uncompressed_bytes, written);
                let mut limited = io::Read::take(&mut zfile, budget.saturating_add(1));
                let copied = io::copy(&mut limited, &mut out).map_err(|source| Error::Io {
                    path: dest.clone(),
                    source,
                })?;
                written = written.saturating_add(copied);
                if opts.max_uncompressed_bytes != 0 && written > opts.max_uncompressed_bytes {
                    return Err(Error::ZipSizeCap {
                        cap: opts.max_uncompressed_bytes,
                        actual: written,
                    });
                }
            }
        }
    }

    Ok(())
}

/// One entry as [`extract`] would write it: junk dropped, unsafe names
/// already rejected, and any redundant single root directory collapsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryName {
    /// Path relative to the extraction target.
    pub path: PathBuf,
    /// Whether the archive declares this entry a directory.
    pub is_dir: bool,
}

/// The entry names [`extract`] would produce for `zip_path`, without
/// writing anything.
///
/// A header-only walk, so callers can inspect an archive's shape *before*
/// touching the filesystem — which is what lets the Model Importer check
/// (#113) reject a wrong archive without a backup to roll back. It shares
/// [`plan_extraction`] with `extract` on purpose: a validator that
/// reimplemented single-root collapse or junk filtering would eventually
/// disagree with what actually lands on disk.
pub fn entry_names(zip_path: &Path, opts: ImportZipOptions) -> Result<Vec<EntryName>> {
    let file = File::open(zip_path).map_err(|source| Error::Io {
        path: zip_path.to_path_buf(),
        source,
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(Error::from_zip_with_path(zip_path))?;
    Ok(plan_extraction(&mut archive, opts)?
        .into_iter()
        .map(|entry| EntryName {
            path: entry.relative_path,
            is_dir: matches!(entry.kind, EntryKind::Dir),
        })
        .collect())
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

    let mut seen_lowercase: HashSet<String> = HashSet::new();

    for i in 0..total {
        let zfile = archive.by_index(i).map_err(Error::from_zip)?;
        let raw_name = zfile.name().to_string();

        // A zip carrying Unix mode bits can declare an entry a symlink,
        // in which case its "contents" are the target path. Extracting
        // that as an ordinary file produces a Mod that lies about what
        // it holds; honouring it would let an archive point anywhere.
        if is_symlink(&zfile) {
            return Err(Error::ZipUnsafeEntry {
                name: raw_name,
                reason: "the entry is a symlink, which the Library never stores",
            });
        }

        // The zip spec says names use `/`. A `\` is either a Windows
        // authoring bug or an attack, and reading it with the host's
        // path rules means `..\..\x` escapes on Windows and becomes a
        // silly filename everywhere else. Normalise first so every host
        // reaches the same verdict.
        let normalised_name = raw_name.replace('\\', "/");
        // `sanitize_relative` is the whole check: it rejects `..`,
        // absolute roots and drive prefixes, which is a superset of what
        // `ZipFile::enclosed_name` would have caught — and it does so
        // identically on every host now that separators are normalised.
        let relative = sanitize_relative(Path::new(&normalised_name))
            .ok_or(Error::ZipSlip(raw_name.clone()))?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        if is_junk(&relative) {
            continue;
        }
        check_entry_name(&relative, &raw_name)?;

        // NTFS compares filenames case-insensitively, so two entries
        // that differ only in case are two files in the archive and one
        // on disk — whichever came last wins, invisibly.
        let lowered = relative.to_string_lossy().to_lowercase();
        if !seen_lowercase.insert(lowered) {
            return Err(Error::ZipUnsafeEntry {
                name: raw_name,
                reason: "two entries collide when filenames are compared case-insensitively, \
                         as they are on NTFS",
            });
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

/// Remaining budget before [`ImportZipOptions::max_uncompressed_bytes`]
/// is exhausted. A cap of `0` disables the check, which is represented
/// as an effectively infinite budget.
fn remaining_bytes(cap: u64, written: u64) -> u64 {
    if cap == 0 {
        u64::MAX
    } else {
        cap.saturating_sub(written)
    }
}

/// True when the archive declares this entry a symlink through its Unix
/// mode bits (`S_IFLNK`), which is what `zip -y` writes.
fn is_symlink(zfile: &zip::read::ZipFile<'_>) -> bool {
    const S_IFMT: u32 = 0o170_000;
    const S_IFLNK: u32 = 0o120_000;
    zfile
        .unix_mode()
        .is_some_and(|mode| mode & S_IFMT == S_IFLNK)
}

/// Refuse names that are safe on the host we happen to be running on and
/// dangerous on the one GMM ships to. All of these are refusals rather
/// than rewrites: silently renaming an entry can collide with another
/// one, and a Mod whose files were quietly renamed is worse than a
/// clear error at import time.
fn check_entry_name(relative: &Path, raw_name: &str) -> Result<()> {
    for component in relative.components() {
        let Component::Normal(part) = component else {
            continue;
        };
        let s = part.to_string_lossy();

        // `C:` only parses as a drive prefix when it leads a path the
        // host recognises; on a non-Windows host it arrives as an
        // ordinary component and would otherwise be created as a
        // directory literally named `C:`.
        if s.len() == 2 && s.ends_with(':') && s.starts_with(|c: char| c.is_ascii_alphabetic()) {
            return Err(Error::ZipUnsafeEntry {
                name: raw_name.to_string(),
                reason: "the name is drive-qualified, so it points outside the import target",
            });
        }

        // `merged.ini:payload` writes an NTFS alternate data stream on
        // `merged.ini` — content that never appears in a listing.
        if s.contains(':') {
            return Err(Error::ZipUnsafeEntry {
                name: raw_name.to_string(),
                reason: "the name contains ':', which NTFS reads as an alternate data stream",
            });
        }

        // NTFS strips these silently, so `merged.ini.` and `merged.ini`
        // are one file with two names in the archive.
        if s.ends_with('.') || s.ends_with(' ') {
            return Err(Error::ZipUnsafeEntry {
                name: raw_name.to_string(),
                reason: "a path component ends with a dot or space, which Windows silently strips",
            });
        }

        // Reserved as *any* component, not just the last: `CON/body.dds`
        // cannot be created on Windows, and the OS error explains
        // nothing.
        if crate::core::is_dos_reserved(&s) {
            return Err(Error::ZipUnsafeEntry {
                name: raw_name.to_string(),
                reason: "a path component is a reserved DOS device name (CON, PRN, AUX, NUL, COM1-9, LPT1-9)",
            });
        }
    }
    Ok(())
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
