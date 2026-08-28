//! Read-only consistency checks for the on-disk Library.
//!
//! Imports copy bytes before inserting their Mod row. If GMM exits between
//! those durable steps, the immediate child directory remains valuable user
//! data but has no database reference. This module finds that shape without
//! mutating it; inspect/delete/recovery actions belong to issue #72.
//!
//! A second consistency failure is multiple Mod rows that resolve to one
//! filesystem directory. Those rows can carry different user state, so this
//! audit reports every field needed for an informed, explicit resolution and
//! never selects or deletes a row itself.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqliteConnection};

use super::library_identity::{DirectoryIdentity, IdentifiedDirectory};
use super::library_mutation::LibraryMutation;
use super::library_ownership::{LibraryDirectoryDisposition, LibraryOwnershipSnapshot};
use super::{Core, Error, GameCode, Result, Source};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnreferencedLibraryDir {
    pub directory_name: String,
    pub path: PathBuf,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateModVariant {
    pub id: String,
    pub name: String,
    pub subpath: PathBuf,
    pub active: bool,
}

/// One of multiple Mod rows that resolve to the same filesystem directory.
/// Every field here is state the user would discard by rejecting this row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateModRecord {
    pub id: String,
    pub game: GameCode,
    pub name: String,
    pub source: Source,
    pub library_path: PathBuf,
    pub junction_dir_name: String,
    pub enabled: bool,
    pub created_at: String,
    pub gamebanana_id: Option<u64>,
    pub source_url: Option<String>,
    pub author: Option<String>,
    pub version: Option<String>,
    pub upstream_version: Option<String>,
    pub update_check_enabled: bool,
    pub screenshot_url: Option<String>,
    pub variants: Vec<DuplicateModVariant>,
    pub reinstall_in_progress: bool,
    /// Digest of every field rendered by the duplicate-review UI. Resolution
    /// recomputes it under the Library writer fence so stable IDs cannot
    /// authorize discarding records whose reviewed contents have changed.
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewedDuplicateMod {
    pub id: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateModGroup {
    /// The spelling found while walking the selected Library root.
    pub path: PathBuf,
    pub mods: Vec<DuplicateModRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateResolution {
    pub keeper_id: String,
    pub removed_mod_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryAuditReport {
    pub game: GameCode,
    pub unreferenced: Vec<UnreferencedLibraryDir>,
    pub duplicates: Vec<DuplicateModGroup>,
    pub total_bytes: u64,
}

struct ScannedUnreferencedLibraryDir {
    reported: UnreferencedLibraryDir,
    identity: Option<DirectoryIdentity>,
}

impl Core {
    /// Report immediate child directories of this game's resolved Library
    /// root that no ownership witness references. Two short writer-fenced
    /// snapshots bracket the unbounded filesystem traversal, which runs on the
    /// blocking pool and never follows links. The second snapshot is required
    /// so the report and its Reveal/Recover/Delete guard agree when a directory
    /// gains an owner during the scan; only candidates unreferenced in both
    /// snapshots are returned. Identity-open uncertainty remains visible with
    /// an unknown size so later actions continue to fail closed.
    pub async fn audit_library(&self, game: GameCode) -> Result<LibraryAuditReport> {
        let mut fence = self
            .begin_library_mutation(LibraryMutation::AuditLibrary)
            .await?;
        let root = self
            .resolved_library_root_for_in_mutation(game, &mut fence)
            .await?;
        let ownership = LibraryOwnershipSnapshot::load(&mut fence.transaction).await?;
        fence.commit().await?;
        self.crash_point(super::crash_points::AUDIT_AFTER_OWNERSHIP_SNAPSHOT);

        let join_error_path = root.clone();
        let scan_ownership = ownership.clone();
        let candidates =
            tokio::task::spawn_blocking(move || scan_game_root(&root, &scan_ownership))
                .await
                .map_err(|join_error| Error::Io {
                    path: join_error_path.clone(),
                    source: io::Error::other(format!("Library audit worker failed: {join_error}")),
                })??;

        let mut recheck_fence = self
            .begin_library_mutation(LibraryMutation::AuditLibrary)
            .await?;
        let current_root = self
            .resolved_library_root_for_in_mutation(game, &mut recheck_fence)
            .await?;
        let current_ownership =
            LibraryOwnershipSnapshot::load(&mut recheck_fence.transaction).await?;
        let duplicate_ids = current_ownership.duplicate_mod_ids();
        let duplicate_records =
            load_duplicate_mod_records(&mut recheck_fence.transaction, &duplicate_ids).await?;
        recheck_fence.commit().await?;

        let mut duplicates: Vec<_> = current_ownership
            .duplicate_mod_groups()
            .into_iter()
            .filter_map(|ids| {
                let mut mods: Vec<_> = ids
                    .iter()
                    .filter_map(|id| duplicate_records.get(id).cloned())
                    .collect();
                if mods.len() != ids.len() || !mods.iter().any(|record| record.game == game) {
                    return None;
                }
                mods.sort_by(|left, right| {
                    left.created_at
                        .cmp(&right.created_at)
                        .then_with(|| left.id.cmp(&right.id))
                });
                Some(DuplicateModGroup {
                    path: mods[0].library_path.clone(),
                    mods,
                })
            })
            .collect();
        duplicates.sort_by(|left, right| left.path.cmp(&right.path));

        // Relocation changed which root the action guard would accept. Do not
        // report candidates scanned under the previous spelling; a refresh
        // will scan the current root instead.
        let unreferenced = if current_root == join_error_path {
            tokio::task::spawn_blocking(move || {
                recheck_unreferenced_candidates(candidates, &current_ownership)
            })
            .await
            .map_err(|join_error| Error::Io {
                path: current_root,
                source: io::Error::other(format!(
                    "Library audit recheck worker failed: {join_error}"
                )),
            })??
        } else {
            Vec::new()
        };
        let total_bytes = unreferenced
            .iter()
            .filter_map(|directory| directory.size_bytes)
            .sum();
        Ok(LibraryAuditReport {
            game,
            unreferenced,
            duplicates,
            total_bytes,
        })
    }
}

fn scan_game_root(
    root: &Path,
    ownership: &LibraryOwnershipSnapshot,
) -> Result<Vec<ScannedUnreferencedLibraryDir>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
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
        // Safe: `symlink_metadata()` above propagated I/O uncertainty.
        if is_link_or_reparse_point(&metadata) || !metadata.file_type().is_dir() {
            continue;
        }
        let directory = match IdentifiedDirectory::open(&path) {
            Ok(directory) => directory,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                // Identity uncertainty is not evidence that user bytes are
                // absent. Keep the directory visible with an unknown size;
                // the action guard will still fail closed until it can open
                // and identify the object.
                unreferenced.push(ScannedUnreferencedLibraryDir {
                    reported: UnreferencedLibraryDir {
                        directory_name: entry.file_name().to_string_lossy().into_owned(),
                        path,
                        size_bytes: None,
                    },
                    identity: None,
                });
                continue;
            }
        };
        match ownership.disposition_of(&directory) {
            LibraryDirectoryDisposition::Owned(_)
            | LibraryDirectoryDisposition::IgnorableEmptyReinstallStage => continue,
            LibraryDirectoryDisposition::Unreferenced => {}
        }
        unreferenced.push(ScannedUnreferencedLibraryDir {
            reported: UnreferencedLibraryDir {
                directory_name: entry.file_name().to_string_lossy().into_owned(),
                size_bytes: directory_size_without_links(&path).ok(),
                path,
            },
            identity: Some(directory.identity().clone()),
        });
    }
    Ok(unreferenced)
}

fn recheck_unreferenced_candidates(
    candidates: Vec<ScannedUnreferencedLibraryDir>,
    ownership: &LibraryOwnershipSnapshot,
) -> Result<Vec<UnreferencedLibraryDir>> {
    let mut unreferenced = Vec::new();
    for mut candidate in candidates {
        let path = &candidate.reported.path;
        if super::library_recovery::is_owned_delete_quarantine(path)? {
            continue;
        }
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                candidate.reported.size_bytes = None;
                unreferenced.push(candidate.reported);
                continue;
            }
        };
        // Safe: `symlink_metadata()` above propagated I/O uncertainty.
        if is_link_or_reparse_point(&metadata) || !metadata.file_type().is_dir() {
            continue;
        }
        let directory = match IdentifiedDirectory::open(path) {
            Ok(directory) => directory,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                candidate.reported.size_bytes = None;
                unreferenced.push(candidate.reported);
                continue;
            }
        };
        if candidate
            .identity
            .as_ref()
            .is_some_and(|identity| identity != directory.identity())
        {
            // This spelling now names an object the first snapshot and scan
            // never classified. Leave it for a fresh audit rather than attach
            // stale size or ownership evidence to the replacement.
            continue;
        }
        match ownership.disposition_of(&directory) {
            LibraryDirectoryDisposition::Owned(_)
            | LibraryDirectoryDisposition::IgnorableEmptyReinstallStage => continue,
            LibraryDirectoryDisposition::Unreferenced => unreferenced.push(candidate.reported),
        }
    }
    unreferenced.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(unreferenced)
}

pub(super) async fn load_duplicate_mod_records(
    connection: &mut SqliteConnection,
    duplicate_ids: &HashSet<String>,
) -> Result<HashMap<String, DuplicateModRecord>> {
    if duplicate_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let reinstalling: HashSet<_> =
        super::library_mutation::load_reinstall_swap_witnesses(&mut *connection)
            .await?
            .into_iter()
            .map(|witness| witness.mod_id().to_string())
            .collect();
    let rows = sqlx::query(
        "SELECT m.id, m.game_code, m.name, m.source, m.library_path,
                m.junction_dir_name, m.enabled, m.created_at, m.gamebanana_id,
                m.source_url, m.author, m.version, m.upstream_version,
                m.update_check_enabled, m.screenshot_url,
                m.active_variant_id,
                v.id AS variant_id, v.name AS variant_name, v.subpath AS variant_subpath
         FROM mods m
         LEFT JOIN mod_variants v ON v.mod_id = m.id
         ORDER BY m.created_at, m.id, v.name",
    )
    .fetch_all(&mut *connection)
    .await?;

    let mut records = HashMap::new();
    for row in rows {
        let id: String = row.try_get("id")?;
        if !duplicate_ids.contains(&id) {
            continue;
        }
        if !records.contains_key(&id) {
            records.insert(
                id.clone(),
                DuplicateModRecord {
                    id: id.clone(),
                    game: row.try_get::<String, _>("game_code")?.parse()?,
                    name: row.try_get("name")?,
                    source: row.try_get::<String, _>("source")?.parse()?,
                    library_path: PathBuf::from(row.try_get::<String, _>("library_path")?),
                    junction_dir_name: row.try_get("junction_dir_name")?,
                    enabled: row.try_get::<i64, _>("enabled")? != 0,
                    created_at: row.try_get("created_at")?,
                    gamebanana_id: row
                        .try_get::<Option<i64>, _>("gamebanana_id")?
                        .map(|value| value as u64),
                    source_url: row.try_get("source_url")?,
                    author: row.try_get("author")?,
                    version: row.try_get("version")?,
                    upstream_version: row.try_get("upstream_version")?,
                    update_check_enabled: row.try_get::<i64, _>("update_check_enabled")? != 0,
                    screenshot_url: row.try_get("screenshot_url")?,
                    variants: Vec::new(),
                    reinstall_in_progress: reinstalling.contains(&id),
                    fingerprint: String::new(),
                },
            );
        }
        if let Some(variant_id) = row.try_get::<Option<String>, _>("variant_id")? {
            let active_variant_id: Option<String> = row.try_get("active_variant_id")?;
            records
                .get_mut(&id)
                .expect("duplicate Mod record was inserted above")
                .variants
                .push(DuplicateModVariant {
                    active: active_variant_id.as_deref() == Some(variant_id.as_str()),
                    id: variant_id,
                    name: row.try_get("variant_name")?,
                    subpath: PathBuf::from(row.try_get::<String, _>("variant_subpath")?),
                });
        }
    }
    for record in records.values_mut() {
        record.fingerprint = duplicate_mod_fingerprint(record);
    }
    Ok(records)
}

fn duplicate_mod_fingerprint(record: &DuplicateModRecord) -> String {
    // Hash the same serialised definition the review UI receives, less the
    // opaque digest itself. A newly rendered field therefore joins the digest
    // automatically instead of requiring a second hand-maintained schema.
    // Vec order is stable because the query orders Variants by name and their
    // IDs break otherwise indistinguishable entries.
    let mut reviewed =
        serde_json::to_value(record).expect("duplicate review fields are always JSON serialisable");
    reviewed
        .as_object_mut()
        .expect("DuplicateModRecord serialises as an object")
        .remove("fingerprint")
        .expect("DuplicateModRecord always serialises its fingerprint");
    let encoded = serde_json::to_vec(&reviewed)
        .expect("duplicate review fields are always JSON serialisable");
    hex::encode(Sha256::digest(encoded))
}

pub(super) fn directory_size_without_links(path: &Path) -> io::Result<u64> {
    let metadata = fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    if is_link_or_reparse_point(&metadata) {
        return Ok(0);
    }
    // Safe: `symlink_metadata()` above propagated I/O uncertainty.
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
