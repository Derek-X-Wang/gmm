//! Read-only consistency checks for the on-disk Library.
//!
//! Imports copy bytes before inserting their Mod row. If GMM exits between
//! those durable steps, the immediate child directory remains valuable user
//! data but has no database reference. This module finds that shape without
//! mutating it; inspect/delete/recovery actions belong to issue #72.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sqlx::Row;

use super::library_identity::{DirectoryIdentity, IdentifiedDirectory};
use super::{Core, Error, GameCode, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnreferencedLibraryDir {
    pub directory_name: String,
    pub path: PathBuf,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryAuditReport {
    pub game: GameCode,
    pub unreferenced: Vec<UnreferencedLibraryDir>,
    pub total_bytes: u64,
}

impl Core {
    /// Report immediate child directories of this game's resolved Library
    /// root that no Mod row references. Filesystem traversal runs on the
    /// blocking pool and never follows links.
    pub async fn audit_library(&self, game: GameCode) -> Result<LibraryAuditReport> {
        let root = self.resolved_library_root_for(game).await?;
        let rows = sqlx::query("SELECT library_path FROM mods WHERE game_code = ?")
            .bind(game.as_str())
            .fetch_all(&self.pool)
            .await?;
        let mut referenced = HashSet::new();
        for row in rows {
            let path = PathBuf::from(row.try_get::<String, _>("library_path")?);
            match IdentifiedDirectory::open(&path) {
                Ok(directory) => {
                    referenced.insert(directory.identity().clone());
                }
                Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                Err(source) => return Err(Error::Io { path, source }),
            }
        }

        let join_error_path = root.clone();
        tokio::task::spawn_blocking(move || scan_game_root(game, &root, &referenced))
            .await
            .map_err(|join_error| Error::Io {
                path: join_error_path,
                source: io::Error::other(format!("Library audit worker failed: {join_error}")),
            })?
    }
}

fn scan_game_root(
    game: GameCode,
    root: &Path,
    referenced: &HashSet<DirectoryIdentity>,
) -> Result<LibraryAuditReport> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(LibraryAuditReport {
                game,
                unreferenced: Vec::new(),
                total_bytes: 0,
            });
        }
        Err(source) => {
            return Err(Error::Io {
                path: root.to_path_buf(),
                source,
            });
        }
    };

    let mut unreferenced = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if super::library_recovery::is_owned_delete_quarantine(&path)? {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        if is_link_or_reparse_point(&metadata) || !metadata.file_type().is_dir() {
            continue;
        }
        let directory = IdentifiedDirectory::open(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        if referenced.contains(directory.identity()) {
            continue;
        }

        unreferenced.push(UnreferencedLibraryDir {
            directory_name: entry.file_name().to_string_lossy().into_owned(),
            size_bytes: directory_size_without_links(&path).ok(),
            path,
        });
    }
    unreferenced.sort_by(|left, right| left.path.cmp(&right.path));
    let total_bytes = unreferenced.iter().filter_map(|dir| dir.size_bytes).sum();

    Ok(LibraryAuditReport {
        game,
        unreferenced,
        total_bytes,
    })
}

pub(super) fn directory_size_without_links(path: &Path) -> io::Result<u64> {
    let metadata = fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    if is_link_or_reparse_point(&metadata) {
        return Ok(0);
    }
    if file_type.is_file() {
        return Ok(metadata.len());
    }
    if !file_type.is_dir() {
        return Ok(0);
    }

    let mut total = 0u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        total = total.saturating_add(directory_size_without_links(&entry.path())?);
    }
    Ok(total)
}

pub(super) fn is_link_or_reparse_point(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;

        // Directory junctions are reparse points but are not guaranteed to
        // present as `FileType::is_symlink()`. Skipping every reparse point is
        // the conservative read-only choice and prevents traversal outside the
        // requested Library root.
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}
