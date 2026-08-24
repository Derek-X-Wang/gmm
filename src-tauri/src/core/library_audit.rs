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
use sqlx::{Executor, Row, Sqlite};

use super::library_identity::IdentifiedDirectory;
use super::library_ownership::LibraryOwnershipSnapshot;
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

impl Core {
    /// Report immediate child directories of this game's resolved Library
    /// root that no Mod row references. Filesystem traversal runs on the
    /// blocking pool and never follows links.
    pub async fn audit_library(&self, game: GameCode) -> Result<LibraryAuditReport> {
        let root = self.resolved_library_root_for(game).await?;
        let mut transaction = self.pool.begin().await?;
        let ownership = LibraryOwnershipSnapshot::load(&mut *transaction).await?;
        let duplicate_ids = ownership.duplicate_mod_ids();
        let duplicate_records =
            load_duplicate_mod_records(&mut *transaction, &duplicate_ids).await?;
        transaction.commit().await?;
        let mut duplicates: Vec<_> = ownership
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

        let join_error_path = root.clone();
        tokio::task::spawn_blocking(move || scan_game_root(game, &root, &ownership, duplicates))
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
    ownership: &LibraryOwnershipSnapshot,
    duplicates: Vec<DuplicateModGroup>,
) -> Result<LibraryAuditReport> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(LibraryAuditReport {
                game,
                unreferenced: Vec::new(),
                duplicates,
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
        if ownership.owner_of(directory.identity()).is_some() {
            continue;
        }
        // A process can die after creating reinstall's reserved stage but
        // before its witness transaction commits. Only a directory proven
        // empty is harmless internal residue. A reserved name containing any
        // entry (or one we cannot inspect) remains visible because names alone
        // are not ownership evidence for user bytes.
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(super::library_mutation::REINSTALL_STAGING_PREFIX)
            && fs::read_dir(&path).is_ok_and(|mut entries| entries.next().is_none())
        {
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
        duplicates,
        total_bytes,
    })
}

pub(super) async fn load_duplicate_mod_records<'e, E>(
    executor: E,
    duplicate_ids: &HashSet<String>,
) -> Result<HashMap<String, DuplicateModRecord>>
where
    E: Executor<'e, Database = Sqlite>,
{
    if duplicate_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        "SELECT m.id, m.game_code, m.name, m.source, m.library_path,
                m.junction_dir_name, m.enabled, m.created_at, m.gamebanana_id,
                m.source_url, m.author, m.version, m.upstream_version,
                m.update_check_enabled, m.screenshot_url,
                m.active_variant_id,
                EXISTS(SELECT 1 FROM reinstall_swaps rs WHERE rs.mod_id = m.id)
                    AS reinstall_in_progress,
                v.id AS variant_id, v.name AS variant_name, v.subpath AS variant_subpath
         FROM mods m
         LEFT JOIN mod_variants v ON v.mod_id = m.id
         ORDER BY m.created_at, m.id, v.name",
    )
    .fetch_all(executor)
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
                    reinstall_in_progress: row.try_get::<i64, _>("reinstall_in_progress")? != 0,
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
