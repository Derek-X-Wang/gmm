//! Model Importer install + rollback (slice 3).
//!
//! GMM downloads each game's official Model Importer release ZIP from
//! GitHub (e.g. `SilentNightSound/GIMI-Package`), verifies it, lays it out
//! into `<Game>/` itself (not GMM's own directory) and rewrites the
//! `d3dx.ini`'s `loader:` line to point at GMM's own executable.
//!
//! Per ADR 0004 importer installs are high-risk because they touch the
//! game directory — a botched install during a ban-wave can lock a
//! user out of their account. The flow here is therefore:
//!
//! 1. Stage extraction into a temp directory inside `<Game>/.gmm-staging`.
//!    Failures during extraction never touch the user's game folder.
//! 2. Move any pre-existing importer files into a timestamped backup
//!    under `<backups_root>/<game>/<timestamp>/` *before* the swap.
//! 3. Atomically swap the staged files into the game directory. If any
//!    step from this point on fails, [`rollback_to`] restores the
//!    backed-up files byte-for-byte.
//! 4. Rewrite `d3dx.ini`'s `loader:` line to `gmm.exe` (GMM is the
//!    loader process per ADR 0001).
//!
//! Network fetch + checksum verification live in this module too; the
//! orchestrator accepts a local ZIP path so integration tests can
//! exercise the install/rollback flow without making HTTP calls.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::error::{Error, Result};
use super::zip_import;

/// Filenames the Model Importer drops at the root of the game directory.
/// Used by the backup-and-restore code path so we know what to move
/// even when extracting a fresh release for the first time.
pub const IMPORTER_ROOT_FILES: &[&str] = &["d3d11.dll", "d3dcompiler_46.dll", "d3dx.ini"];

/// Directories the Model Importer owns and may freely replace.
///
/// **`Mods` is deliberately absent.** It is the deployment target for
/// GMM's Junctions (ADR 0003) — user data, not importer state. Moving
/// or deleting it during an install would silently strip every enabled
/// mod out of the game while the DB still reported `enabled = 1`. See
/// [`USER_OWNED_DIRS`].
pub const IMPORTER_ROOT_DIRS: &[&str] = &["ShaderCache", "ShaderFixes"];

/// Directories inside the game folder that the importer subsystem must
/// never move, replace, or delete.
///
/// `Mods/` holds the Junctions GMM creates when a Mod is enabled. An
/// importer package may ship its own `Mods/` (usually examples); those
/// contents are merged in rather than swapped over the top, so a
/// reinstall or importer update can never orphan a user's enabled mods.
pub const USER_OWNED_DIRS: &[&str] = &["Mods"];

/// The executable name written into `d3dx.ini`'s `loader:` line. GMM
/// runs as the loader process per ADR 0001.
pub const DEFAULT_LOADER_EXE: &str = "gmm.exe";

/// Outcome of a single install attempt. Travels through tracing
/// (NEW-LOG) and back to the UI for the success toast.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallReport {
    /// Where any pre-existing files were stashed, if anything was
    /// backed up. `None` means a fresh install onto a clean game dir.
    pub backup_dir: Option<PathBuf>,
    /// Computed SHA-256 of the input ZIP, hex-encoded. Surfaced to
    /// the UI even when no published digest exists for the asset.
    pub sha256: String,
    /// Files that were rewritten (e.g. `d3dx.ini`).
    pub rewrote_files: Vec<PathBuf>,
}

/// Result of a successful HTTP fetch of the latest release metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LatestRelease {
    pub tag_name: String,
    /// The chosen asset's browser-download URL.
    pub asset_url: String,
    /// The asset's filename (e.g. `GIMI-Package-v0.7.1.zip`).
    pub asset_name: String,
    /// Hex-encoded SHA-256 digest if the release publishes one. Many
    /// importer authors don't yet; in that case we surface the
    /// computed digest to the user for visual confirmation.
    pub sha256_digest: Option<String>,
}

/// The release-asset selector for one Importer Origin (ADR 0005): an
/// **anchored** regular expression that must match exactly one asset in
/// the release.
///
/// Anchoring is applied here rather than trusted to the pattern's
/// author. A pattern is wrapped as `^(?:…)$` on construction, so
/// `GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip` cannot accidentally behave like
/// the `str::contains` rule it replaces. That matters because patterns
/// arrive from the recommended-importers manifest and from a user's own
/// origin, not only from the compiled-in defaults — and an unanchored
/// pattern there would silently reintroduce #79.
///
/// A bare substring is the rule this replaces: `"SRMI"` matched
/// `SRMI-TEST-PACKAGE-v2.4.2.zip`, so GMM would have installed a build
/// upstream explicitly labelled TEST. A denylist (`TEST`, `DEBUG`,
/// `-rc`, …) was rejected because it only catches the failure modes
/// already imagined; an anchored shape rejects everything that is not
/// the expected form without enumerating anything.
#[derive(Debug, Clone)]
pub struct AssetPattern {
    /// The pattern as written, for error messages. Reported without the
    /// `^(?:…)$` wrapper so the user sees what they configured.
    source: String,
    anchored: regex::Regex,
}

impl AssetPattern {
    /// Compile `pattern`, anchoring it to the whole asset name.
    pub fn new(pattern: &str) -> Result<Self> {
        let anchored = regex::Regex::new(&format!("^(?:{pattern})$")).map_err(|e| {
            Error::InvalidAssetPattern {
                pattern: pattern.to_string(),
                message: e.to_string(),
            }
        })?;
        Ok(Self {
            source: pattern.to_string(),
            anchored,
        })
    }

    /// The pattern as configured, without the anchors added by
    /// [`AssetPattern::new`].
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Whether `asset_name` is the whole-string shape this origin
    /// expects.
    pub fn matches(&self, asset_name: &str) -> bool {
        self.anchored.is_match(asset_name)
    }
}

/// Compute the hex-encoded SHA-256 of the bytes in `path`.
pub fn sha256_of_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Install a Model Importer into `game_dir` from a local ZIP file.
///
/// The orchestrator that the production path calls: stage in a temp
/// dir, back up existing files into a timestamped folder under
/// `backups_root`, swap into `game_dir`, rewrite `d3dx.ini`'s loader
/// line, and return the resulting [`InstallReport`].
///
/// Designed so the network fetch is *not* a prerequisite for testing
/// — integration tests pass a fixture ZIP from disk.
pub fn install_from_local_zip(
    zip_path: &Path,
    game_dir: &Path,
    backups_root: &Path,
    loader_exe: &str,
) -> Result<InstallReport> {
    let sha256 = sha256_of_file(zip_path)?;

    // 1. Stage extraction into a temp dir under the game directory.
    fs::create_dir_all(game_dir).map_err(|source| Error::Io {
        path: game_dir.to_path_buf(),
        source,
    })?;
    let staging = game_dir.join(".gmm-staging");
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|source| Error::Io {
            path: staging.clone(),
            source,
        })?;
    }
    zip_import::extract(zip_path, &staging, zip_import::ImportZipOptions::default())?;

    // 2. Back up pre-existing importer files.
    let backup_dir = backup_existing(game_dir, backups_root)?;

    // 3. Swap staged files into the game directory. From this point
    //    on, any failure triggers a rollback.
    if let Err(e) = swap_in(&staging, game_dir) {
        if let Some(bdir) = backup_dir.as_ref() {
            let _ = rollback_to(bdir, game_dir);
        }
        // Best-effort cleanup of the staging dir before surfacing the
        // failure.
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    fs::remove_dir_all(&staging).map_err(|source| Error::Io {
        path: staging,
        source,
    })?;

    // 4. Rewrite d3dx.ini's loader line.
    let d3dx = game_dir.join("d3dx.ini");
    let mut rewrote_files = Vec::new();
    if d3dx.is_file() {
        if let Err(e) = rewrite_d3dx_loader(&d3dx, loader_exe) {
            if let Some(bdir) = backup_dir.as_ref() {
                let _ = rollback_to(bdir, game_dir);
            }
            return Err(e);
        }
        rewrote_files.push(d3dx);
    }

    Ok(InstallReport {
        backup_dir,
        sha256,
        rewrote_files,
    })
}

/// Move pre-existing importer files (the known DLLs + dirs at
/// `game_dir`'s root) into a timestamped backup folder under
/// `backups_root`. Returns `None` if nothing was there to back up.
pub fn backup_existing(game_dir: &Path, backups_root: &Path) -> Result<Option<PathBuf>> {
    let mut found = false;
    for name in IMPORTER_ROOT_FILES
        .iter()
        .copied()
        .chain(IMPORTER_ROOT_DIRS.iter().copied())
    {
        if game_dir.join(name).exists() {
            found = true;
            break;
        }
    }
    if !found {
        return Ok(None);
    }

    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S").to_string();
    let dest = backups_root.join(&timestamp);
    fs::create_dir_all(&dest).map_err(|source| Error::Io {
        path: dest.clone(),
        source,
    })?;

    for name in IMPORTER_ROOT_FILES
        .iter()
        .copied()
        .chain(IMPORTER_ROOT_DIRS.iter().copied())
    {
        let from = game_dir.join(name);
        if !from.exists() {
            continue;
        }
        let to = dest.join(name);
        if let Err(_e) = fs::rename(&from, &to) {
            // Cross-volume; fall back to copy + delete.
            copy_any(&from, &to)?;
            remove_any(&from)?;
        }
    }
    Ok(Some(dest))
}

/// Swap files staged in `staging` into `game_dir`. Existing files are
/// already in the backup folder at this point; we just `rename` from
/// staging into the game directory.
fn swap_in(staging: &Path, game_dir: &Path) -> Result<()> {
    for entry in fs::read_dir(staging).map_err(|source| Error::Io {
        path: staging.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| Error::Io {
            path: staging.to_path_buf(),
            source,
        })?;
        let from = entry.path();
        let name = entry.file_name();
        let to = game_dir.join(&name);

        // User-owned directories are merged, never replaced — blowing
        // away `Mods/` here would take every Junction with it.
        // `is_dir()` follows reparse points, so a dangling junction at
        // `<game>/Mods` would look absent and we would try to rename
        // onto an occupied directory entry. Ask whether an entry exists
        // at all instead.
        let is_user_owned = USER_OWNED_DIRS.iter().any(|d| name.eq_ignore_ascii_case(d));
        if is_user_owned && fs::symlink_metadata(&to).is_ok() {
            merge_into(&from, &to)?;
            remove_any(&from)?;
            continue;
        }

        if to.exists() {
            remove_any(&to)?;
        }
        if let Err(_rename_err) = fs::rename(&from, &to) {
            copy_any(&from, &to)?;
            remove_any(&from)?;
        }
    }
    Ok(())
}

/// Copy everything under `from` into the existing directory `to`,
/// leaving entries already present in `to` untouched. Used for
/// [`USER_OWNED_DIRS`] so importer-shipped example mods can land
/// without disturbing the user's Junctions.
fn merge_into(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).map_err(|source| Error::Io {
        path: to.to_path_buf(),
        source,
    })?;

    for entry in fs::read_dir(from).map_err(|source| Error::Io {
        path: from.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| Error::Io {
            path: from.to_path_buf(),
            source,
        })?;
        let src = entry.path();
        let dest = to.join(entry.file_name());

        // `exists()` follows reparse points, so a *dangling* junction —
        // one whose Library target was deleted — reports false and we
        // would try to copy straight onto the existing directory entry.
        // `symlink_metadata` answers "is there an entry here" without
        // following, which is the question we actually have.
        let occupied = fs::symlink_metadata(&dest).is_ok();

        if occupied {
            // Recurse only when both sides are ordinary directories.
            // A junction/reparse point on the destination side belongs
            // to the user (it is an enabled mod) and is left alone.
            let dest_is_plain_dir = fs::symlink_metadata(&dest)
                .map(|m| m.is_dir() && !m.file_type().is_symlink())
                .unwrap_or(false);
            if src.is_dir() && dest_is_plain_dir {
                merge_into(&src, &dest)?;
            }
            // Otherwise keep whatever is already there — never clobber
            // something the user (or GMM) put in place.
            continue;
        }

        copy_any(&src, &dest)?;
    }
    Ok(())
}

/// Restore `game_dir` to the state captured in `backup_dir`. Files
/// currently in `game_dir` with the same name are removed first.
pub fn rollback_to(backup_dir: &Path, game_dir: &Path) -> Result<()> {
    for entry in fs::read_dir(backup_dir).map_err(|source| Error::Io {
        path: backup_dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| Error::Io {
            path: backup_dir.to_path_buf(),
            source,
        })?;
        let from = entry.path();
        let name = entry.file_name();
        let to = game_dir.join(&name);

        // Backups taken before `Mods` was reclassified as user-owned
        // still contain it — that is exactly the wreckage of the bug
        // this reclassification fixes, and rollback is the recovery
        // path for it. Skipping would strand those Junctions in the
        // backup forever; replacing wholesale would delete whatever
        // the user has enabled since. So merge, preferring live.
        if USER_OWNED_DIRS.iter().any(|d| name.eq_ignore_ascii_case(d)) {
            if fs::symlink_metadata(&to).is_ok() {
                merge_into(&from, &to)?;
                remove_any(&from)?;
            } else {
                // Nothing live to preserve — restore it outright.
                if let Err(_rename_err) = fs::rename(&from, &to) {
                    copy_any(&from, &to)?;
                    remove_any(&from)?;
                }
            }
            continue;
        }

        if fs::symlink_metadata(&to).is_ok() {
            remove_any(&to)?;
        }
        if let Err(_rename_err) = fs::rename(&from, &to) {
            copy_any(&from, &to)?;
            remove_any(&from)?;
        }
    }
    Ok(())
}

/// Rewrite `d3dx.ini` so the first `loader = …` line names
/// `loader_exe`. Idempotent: re-running with the same loader name
/// leaves the file unchanged. Preserves every other line + comments
/// + section headers.
///
/// Implementation note: 3dmigoto's INIs are case-insensitive on keys
/// and tolerate whitespace; we match the first key on the line.
pub fn rewrite_d3dx_loader(d3dx_path: &Path, loader_exe: &str) -> Result<()> {
    let contents = fs::read_to_string(d3dx_path).map_err(|source| Error::Io {
        path: d3dx_path.to_path_buf(),
        source,
    })?;

    let mut out = String::with_capacity(contents.len());
    let mut rewrote = false;
    for line in contents.lines() {
        // Don't touch comments or empty lines.
        let trimmed = line.trim_start();
        let stripped = trimmed.split_once(';').map(|(l, _)| l).unwrap_or(trimmed);
        if let Some((key, _value)) = stripped.split_once('=') {
            if key.trim().eq_ignore_ascii_case("loader") && !rewrote {
                out.push_str(&format!("loader = {loader_exe}"));
                out.push('\n');
                rewrote = true;
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }

    if !rewrote {
        // No loader line in this file — append one to the `[Loader]`
        // section if it exists, else append at end.
        out.push_str(&format!("\n[Loader]\nloader = {loader_exe}\n"));
    }

    fs::write(d3dx_path, out).map_err(|source| Error::Io {
        path: d3dx_path.to_path_buf(),
        source,
    })?;
    Ok(())
}

/// Copy a file or directory tree from `from` to `to`. Used in the
/// cross-volume fallback path where `fs::rename` fails with `EXDEV`.
fn copy_any(from: &Path, to: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(from).map_err(|source| Error::Io {
        path: from.to_path_buf(),
        source,
    })?;
    if meta.is_dir() {
        copy_dir_recursive(from, to)
    } else {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::copy(from, to).map_err(|source| Error::Io {
            path: from.to_path_buf(),
            source,
        })?;
        Ok(())
    }
}

fn remove_any(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if meta.is_dir() {
        fs::remove_dir_all(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
    } else {
        fs::remove_file(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).map_err(|source| Error::Io {
        path: dst.to_path_buf(),
        source,
    })?;
    for entry in fs::read_dir(src).map_err(|source| Error::Io {
        path: src.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| Error::Io {
            path: src.to_path_buf(),
            source,
        })?;
        let entry_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            copy_dir_recursive(&entry_path, &dst_path)?;
        } else {
            fs::copy(&entry_path, &dst_path).map_err(|source| Error::Io {
                path: entry_path.clone(),
                source,
            })?;
        }
    }
    Ok(())
}

/// Render a list of asset names for an error message. Empty lists get a
/// sentence rather than an empty pair of brackets, because "this release
/// has no assets at all" is a distinct diagnosis.
fn describe(names: &[&str]) -> String {
    if names.is_empty() {
        return "no assets at all".to_string();
    }
    names
        .iter()
        .map(|n| format!("{n:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Network fetch of the latest release metadata for `owner/repo` (e.g.
/// `SilentNightSound/GIMI-Package`), selecting the one asset matching
/// this origin's [`AssetPattern`]. Returns `Ok(None)` on a 304 Not
/// Modified when `etag` is supplied.
///
/// The caller must build the `client` via
/// [`crate::core::Core::http_client`] so the request honours any
/// configured proxy.
pub async fn fetch_latest_release(
    client: &reqwest::Client,
    owner_repo: &str,
    pattern: &AssetPattern,
    etag: Option<&str>,
) -> Result<Option<LatestRelease>> {
    let url = format!("https://api.github.com/repos/{owner_repo}/releases/latest");
    let mut req = client.get(&url);
    if let Some(tag) = etag {
        req = req.header("If-None-Match", tag);
    }
    let res = req
        .send()
        .await
        .map_err(|e| Error::ReleaseMetadata(format!("GET {url}: {e}")))?;

    if res.status().as_u16() == 304 {
        return Ok(None);
    }
    if !res.status().is_success() {
        return Err(Error::ReleaseMetadata(format!(
            "GitHub returned {} for {url}",
            res.status()
        )));
    }

    let json: serde_json::Value = res
        .json()
        .await
        .map_err(|e| Error::ReleaseMetadata(format!("parse JSON from {url}: {e}")))?;

    parse_latest_release(&json, pattern).map(Some)
}

/// Pure half of [`fetch_latest_release`]: turn a GitHub
/// `releases/latest` payload into a [`LatestRelease`], selecting the
/// **one** asset whose name matches `pattern`.
///
/// Split out from the network call so the pattern can be tested against
/// *recorded* copies of real upstream responses. Issue #78 existed
/// because nothing ever compared a filter to a real payload:
/// `check_loader_update` shipped the filter `"Libs"`, which matches no
/// asset any `XXMI-Libs-Package` release has ever published.
///
/// Both failure modes are errors, and distinct ones (#79):
///
/// - zero matches → [`Error::ReleaseAssetNoMatch`]
/// - two or more  → [`Error::ReleaseAssetAmbiguous`]
///
/// "First match wins" is deliberately not an option. Zero-match silence
/// is the #78 defect; ambiguity means the pattern is wrong or upstream
/// published something unexpected, and resolving it by release order is
/// how a TEST package gets installed.
pub fn parse_latest_release(
    json: &serde_json::Value,
    pattern: &AssetPattern,
) -> Result<LatestRelease> {
    let tag_name = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::ReleaseMetadata("release JSON missing tag_name".to_string()))?
        .to_string();

    let assets = json
        .get("assets")
        .and_then(|v| v.as_array())
        .ok_or_else(|| Error::ReleaseMetadata("release JSON missing assets".to_string()))?;

    // An asset with no `name` cannot be selected by name, but it is
    // still worth listing when nothing matched — "that release publishes
    // nothing at all" and "it publishes two things, neither shaped like
    // this" are different problems for the user.
    let named: Vec<(&serde_json::Value, &str)> = assets
        .iter()
        .map(|a| (a, a.get("name").and_then(|n| n.as_str()).unwrap_or("")))
        .collect();
    let matched: Vec<&(&serde_json::Value, &str)> = named
        .iter()
        .filter(|(_, name)| pattern.matches(name))
        .collect();

    let asset = match matched.as_slice() {
        [(asset, _)] => *asset,
        [] => {
            let candidates: Vec<&str> = named.iter().map(|(_, name)| *name).collect();
            return Err(Error::ReleaseAssetNoMatch {
                release: tag_name,
                pattern: pattern.as_str().to_string(),
                candidates: describe(&candidates),
            });
        }
        many => {
            let matches: Vec<&str> = many.iter().map(|(_, name)| *name).collect();
            return Err(Error::ReleaseAssetAmbiguous {
                release: tag_name,
                pattern: pattern.as_str().to_string(),
                matches: describe(&matches),
                count: many.len(),
            });
        }
    };

    let asset_url = asset
        .get("browser_download_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::ReleaseMetadata("asset missing browser_download_url".to_string()))?
        .to_string();
    let asset_name = asset
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // GitHub doesn't put SHA-256s in the release JSON directly. Some
    // upstream authors publish a `*.sha256` sibling asset, but
    // verifying it would require a second HTTP fetch and a parser
    // for the shasum text format. Deferred to a follow-up slice —
    // for now we surface the *computed* digest from the downloaded
    // bytes via [`InstallReport::sha256`] so the user can compare
    // visually.
    let sha256_digest = None;

    Ok(LatestRelease {
        tag_name,
        asset_url,
        asset_name,
        sha256_digest,
    })
}

/// Stream a release asset to `dest`. Returns the byte count written so
/// the caller can sanity-check Content-Length.
///
/// The caller must build the `client` via
/// [`crate::core::Core::http_client`].
pub async fn download_to(client: &reqwest::Client, url: &str, dest: &Path) -> Result<u64> {
    let bytes = client
        .get(url)
        .send()
        .await
        .map_err(|e| Error::Importer(format!("GET {url}: {e}")))?
        .error_for_status()
        .map_err(|e| Error::Importer(format!("download {url}: {e}")))?
        .bytes()
        .await
        .map_err(|e| Error::Importer(format!("read bytes from {url}: {e}")))?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(dest, &bytes).map_err(|source| Error::Io {
        path: dest.to_path_buf(),
        source,
    })?;
    Ok(bytes.len() as u64)
}
