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

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ulid::Ulid;

use super::error::{Error, Result};
use super::filesystem::{metadata_if_exists, symlink_metadata_if_exists};
use super::library_identity::IdentifiedDirectory;
use super::zip_import;

/// Root-level filenames the backup-and-restore path must move out of
/// the way before an install, and put back on a rollback.
///
/// **This is a backup set, not a list of what a Model Importer ships**
/// (#127). Since #113 [`validate_importer_archive`] refuses any archive
/// containing a `.dll`, so an importer shipping these is false by
/// construction — the DLLs come with the Loader package (ADR 0001).
///
/// The DLLs stay in this list precisely *because* GMM did not put them
/// there: a user who hand-installed an XXMI setup before adopting GMM
/// has them sitting beside their `d3dx.ini`, and an install that swapped
/// over the top without capturing them would leave [`rollback_to`]
/// unable to restore the setup it replaced.
pub const IMPORTER_ROOT_FILES: &[&str] = &["d3d11.dll", "d3dcompiler_46.dll", "d3dx.ini"];

/// Directories the Model Importer owns and may freely replace.
///
/// **`Mods` is deliberately absent.** It is the deployment target for
/// GMM's Junctions (ADR 0003) — user data, not importer state. Moving
/// or deleting it during an install would silently strip every enabled
/// mod out of the game while the DB still reported `enabled = 1`. See
/// [`USER_OWNED_DIRS`].
///
/// `Core` was missing here until #113. It is the largest thing every
/// package ships, so `backup_existing` skipped it while `swap_in` deleted
/// it — a reinstall discarded the previous `Core/` outright and
/// `rollback_to` had nothing to restore. The omission came from the same
/// wrong picture of a package the install path's test fixtures encoded:
/// that an importer is `d3d11.dll` plus `ShaderFixes/`.
pub const IMPORTER_ROOT_DIRS: &[&str] = &["Core", "ShaderCache", "ShaderFixes"];

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

/// Test seam fired after one planned importer entry reaches its complete
/// backup location. A test fails the blocking task here to exercise a real
/// partial evacuation without relying on filesystem timing.
#[doc(hidden)]
pub const BACKUP_AFTER_ENTRY_TEST_SEAM: &str = "importer_backup.after_entry";

/// Pause-only test seam after recovery found equal live and backup contents,
/// before it revalidates that evidence and retires the backup. Kept outside
/// `crash_points::ALL` because this barrier models a concurrent filesystem
/// change, not a process death that startup must replay.
#[doc(hidden)]
pub const RECOVERY_AFTER_ENTRY_COMPARISON_TEST_SEAM: &str =
    "importer_evacuation.after_entry_comparison";

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

/// A Model Importer evacuation that startup could not safely settle yet.
/// The backup remains the authoritative rollback source until recovery
/// restores every planned entry and retires the durable witness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImporterEvacuationRecovery {
    pub reason: String,
    pub attempted_at: String,
    pub attempts: u32,
    pub game_path: PathBuf,
    pub backup_path: PathBuf,
    /// True when the numeric PID is live but its start identity is unknown.
    pub owner_uncertain: bool,
    pub action: ImporterEvacuationRecoveryAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImporterEvacuationRecoveryAction {
    Retry,
    RetireProducer,
    AcknowledgeAndRelease,
}

#[derive(Debug)]
pub(super) struct PreparedImporterInstall {
    game_dir: PathBuf,
    staging: PathBuf,
    backup: Option<PreparedImporterBackup>,
    sha256: String,
}

#[derive(Debug, Clone)]
struct PreparedImporterBackup {
    destination: PathBuf,
    entries: Vec<String>,
    game_identity: String,
    backup_identity: String,
    backup_root_identity: String,
}

impl PreparedImporterInstall {
    pub(super) fn backup_dir(&self) -> Option<&Path> {
        self.backup
            .as_ref()
            .map(|backup| backup.destination.as_path())
    }

    pub(super) fn backup_entries(&self) -> Option<&[String]> {
        self.backup.as_ref().map(|backup| backup.entries.as_slice())
    }

    pub(super) fn game_dir(&self) -> &Path {
        &self.game_dir
    }

    pub(super) fn game_identity(&self) -> Option<&str> {
        self.backup
            .as_ref()
            .map(|backup| backup.game_identity.as_str())
    }

    pub(super) fn backup_root_identity(&self) -> Option<&str> {
        self.backup
            .as_ref()
            .map(|backup| backup.backup_root_identity.as_str())
    }

    pub(super) fn backup_identity(&self) -> Option<&str> {
        self.backup
            .as_ref()
            .map(|backup| backup.backup_identity.as_str())
    }

    pub(super) fn cleanup_unstarted_backup(&self) -> Result<()> {
        let Some(backup) = self.backup.as_ref() else {
            return Ok(());
        };
        fs::remove_dir(&backup.destination).map_err(|source| Error::Io {
            path: backup.destination.clone(),
            source,
        })
    }
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
///
/// SRMI's compiled-in pattern was later widened to accept that TEST
/// name explicitly (#116), because it is the only package upstream
/// publishes and Star Rail could otherwise not be installed at all.
/// That is one origin naming one extra shape it accepts — the rule
/// here is unchanged: still anchored, still exactly-one-match, still
/// distinct errors for zero and for ambiguity.
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

/// The root file every Model Importer package ships. Also the one file
/// the install path must rewrite, so its absence is fatal rather than
/// skippable (#113).
pub const IMPORTER_REQUIRED_FILE: &str = "d3dx.ini";

/// Directories every Model Importer package ships at its root.
///
/// Derived by inspecting the live packages of all six games across their
/// three maintainers, recorded in
/// `tests/fixtures/importer_package_layouts.json`. Two findings shaped
/// this list, and neither was guessable:
///
/// - **`Mods/` is not here.** `ZZMI-PACKAGE-v1.4.5.zip` ships no `Mods/`
///   entry at all, so requiring it would reject the real ZZMI package.
///   GMM creates the directory when a Mod is enabled anyway.
/// - **These are checked as directories, not as directories with
///   content.** `EFMI-PACKAGE-v1.3.0.zip` and `WWMI-PACKAGE-v1.0.0.zip`
///   ship `ShaderFixes/` empty.
pub const IMPORTER_REQUIRED_DIRS: &[&str] = &["Core", "ShaderFixes"];

/// Human-readable summary of the required shape, for the rejection
/// message.
const IMPORTER_EXPECTED_SHAPE: &str =
    "d3dx.ini at its root plus Core/ and ShaderFixes/ directories, and no compiled binaries";

/// File extensions that name a Windows executable image. A Model
/// Importer is configuration and HLSL (ADR 0001, ADR 0005), so one of
/// these in the archive means it is not an importer.
///
/// Deliberately narrow rather than a general denylist of suspicious
/// names: the positive requirements above do the real work, and this
/// catches the specific case that matters — an archive that would drop an
/// executable image into the game's own directory.
const EXECUTABLE_IMAGE_EXTENSIONS: &[&str] = &["dll", "exe", "sys"];

/// Reject `zip_path` unless it has the structural shape of a Model
/// Importer package.
///
/// Runs against the entry names [`zip_import::extract`] *would* produce,
/// which means a package zipped inside a wrapper folder validates the
/// same way it extracts, and means the check needs no filesystem access
/// at all. That is the point: a rejection has to be a no-op on the game
/// directory rather than something [`rollback_to`] has to undo (#113).
///
/// This is a check of *shape*, not of authenticity. Verifying a
/// downloaded asset against upstream's signed `Manifest.json` is a real
/// gap and a different concern, tracked separately.
pub fn validate_importer_archive(zip_path: &Path) -> Result<()> {
    let entries = zip_import::entry_names(zip_path, zip_import::ImportZipOptions::default())?;

    let mut missing: Vec<String> = Vec::new();

    let has_required_file = entries.iter().any(|entry| {
        !entry.is_dir
            && entry
                .path
                .to_str()
                .is_some_and(|p| p.eq_ignore_ascii_case(IMPORTER_REQUIRED_FILE))
    });
    if !has_required_file {
        missing.push(format!("no {IMPORTER_REQUIRED_FILE} at the archive root"));
    }

    for dir in IMPORTER_REQUIRED_DIRS {
        // Satisfied by an explicit directory entry *or* by anything
        // living under it — plenty of zip writers omit directory
        // entries, and an importer whose `Core/` arrives only as path
        // prefixes is still an importer.
        let present = entries.iter().any(|entry| {
            let mut components = entry.path.components();
            let Some(std::path::Component::Normal(first)) = components.next() else {
                return false;
            };
            if !first.to_string_lossy().eq_ignore_ascii_case(dir) {
                return false;
            }
            entry.is_dir || components.next().is_some()
        });
        if !present {
            missing.push(format!("no {dir}/ directory"));
        }
    }

    if !missing.is_empty() {
        return Err(Error::NotAModelImporter {
            missing: missing.join("; "),
            expected: IMPORTER_EXPECTED_SHAPE,
        });
    }

    let binaries: Vec<String> = entries
        .iter()
        .filter(|entry| !entry.is_dir)
        .filter(|entry| {
            entry
                .path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| {
                    EXECUTABLE_IMAGE_EXTENSIONS
                        .iter()
                        .any(|bad| ext.eq_ignore_ascii_case(bad))
                })
        })
        .map(|entry| entry.path.to_string_lossy().to_string())
        .collect();
    if !binaries.is_empty() {
        return Err(Error::ImporterArchiveHasBinaries {
            entries: binaries.join(", "),
        });
    }

    Ok(())
}

/// Locate the installed `d3dx.ini` in `game_dir`, comparing the filename
/// case-insensitively as NTFS does.
///
/// An error rather than an `Option`, because by the time this runs
/// [`validate_importer_archive`] has already guaranteed the archive
/// carried one: its absence is a contradiction, not a condition. The
/// rewrite it feeds is the single most importer-specific action in the
/// whole install, and it used to be guarded by `if d3dx.is_file()` — so
/// the one step that proved the input was an importer was also the one
/// that quietly did nothing when it wasn't (#113).
///
/// Matching case-insensitively also fixes a real gap: a package shipping
/// `D3DX.INI` worked on Windows and skipped the rewrite on a
/// case-sensitive filesystem, leaving an install that still pointed at
/// XXMI's loader.
pub fn find_d3dx_ini(game_dir: &Path) -> Result<PathBuf> {
    let entries = fs::read_dir(game_dir).map_err(|source| Error::Io {
        path: game_dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: game_dir.to_path_buf(),
            source,
        })?;
        if !entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case(IMPORTER_REQUIRED_FILE)
        {
            continue;
        }
        let path = entry.path();
        let metadata = fs::metadata(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        // Safe: `metadata()` above propagated I/O uncertainty; this only classifies known metadata.
        if metadata.is_file() {
            return Ok(path);
        }
    }
    Err(Error::Importer(format!(
        "the archive validated as a Model Importer but no {IMPORTER_REQUIRED_FILE} \
         is present in {} after the swap; the install is incomplete",
        game_dir.display()
    )))
}

/// Test-only low-level install seam that intentionally has no durable
/// evacuation witness. Production and crash probes must call
/// [`super::Core::install_importer_from_local_zip`] instead.
///
/// This symbol is absent from release builds so an ordinary app caller cannot
/// accidentally reintroduce the process-abort gap from #227.
#[cfg(test)]
#[doc(hidden)]
pub fn install_from_local_zip_unwitnessed_for_test(
    zip_path: &Path,
    game_dir: &Path,
    backups_root: &Path,
    loader_exe: &str,
) -> Result<InstallReport> {
    install_from_local_zip_with_staging_probe(
        zip_path,
        game_dir,
        backups_root,
        loader_exe,
        symlink_metadata_if_exists,
    )
}

#[cfg(test)]
fn install_from_local_zip_with_staging_probe<F>(
    zip_path: &Path,
    game_dir: &Path,
    backups_root: &Path,
    loader_exe: &str,
    mut staging_probe: F,
) -> Result<InstallReport>
where
    F: FnMut(&Path) -> std::io::Result<Option<fs::Metadata>>,
{
    let prepared = prepare_install_from_local_zip_with_staging_probe(
        zip_path,
        game_dir,
        backups_root,
        Ulid::new(),
        &mut staging_probe,
    )?;
    let recovery = prepared.backup.as_ref().map(|backup| {
        (
            prepared.game_dir.clone(),
            backup.destination.clone(),
            backup.entries.clone(),
        )
    });
    match execute_prepared_importer_install(prepared, loader_exe, None) {
        Ok(report) => Ok(report),
        Err(error) => {
            if let Some((game_dir, backup_dir, entries)) = recovery {
                recover_evacuated_importer(&game_dir, &backup_dir, &entries, None).map_err(
                    |recovery_error| {
                        Error::Importer(format!(
                            "{error}; restoring the partially evacuated importer also failed: {recovery_error}"
                        ))
                    },
                )?;
            }
            Err(error)
        }
    }
}

/// Validate and stage an importer archive, then resolve the complete backup
/// plan without evacuating any importer entry. The caller commits the durable
/// witness described by the returned plan before calling
/// [`execute_prepared_importer_install`].
pub(super) fn prepare_install_from_local_zip(
    zip_path: &Path,
    game_dir: &Path,
    backups_root: &Path,
    token: Ulid,
) -> Result<PreparedImporterInstall> {
    prepare_install_from_local_zip_with_staging_probe(
        zip_path,
        game_dir,
        backups_root,
        token,
        &mut symlink_metadata_if_exists,
    )
}

fn prepare_install_from_local_zip_with_staging_probe<F>(
    zip_path: &Path,
    game_dir: &Path,
    backups_root: &Path,
    token: Ulid,
    staging_probe: &mut F,
) -> Result<PreparedImporterInstall>
where
    F: FnMut(&Path) -> std::io::Result<Option<fs::Metadata>>,
{
    // 0. Refuse an archive that is not a Model Importer, before anything
    //    in the game directory is touched — not before the swap, and not
    //    before the backup, but before the first `create_dir_all`. A
    //    rejection has to be a no-op, because the alternative is a
    //    destroyed working setup that `rollback_to` can only repair if
    //    the user realises what happened (#113).
    validate_importer_archive(zip_path)?;

    let sha256 = sha256_of_file(zip_path)?;

    // 1. Stage extraction into a temp dir under the game directory.
    fs::create_dir_all(game_dir).map_err(|source| Error::Io {
        path: game_dir.to_path_buf(),
        source,
    })?;
    let staging = game_dir.join(".gmm-staging");
    if staging_probe(&staging)
        .map_err(|source| Error::Io {
            path: staging.clone(),
            source,
        })?
        .is_some()
    {
        fs::remove_dir_all(&staging).map_err(|source| Error::Io {
            path: staging.clone(),
            source,
        })?;
    }
    zip_import::extract(zip_path, &staging, zip_import::ImportZipOptions::default())?;

    // 2. Resolve the whole backup plan. No importer entry moves until the
    //    caller commits the durable witness for this exact plan.
    let backup = plan_existing_backup_with(
        game_dir,
        backups_root,
        token,
        &mut symlink_metadata_if_exists,
    )?;

    Ok(PreparedImporterInstall {
        game_dir: game_dir.to_path_buf(),
        staging,
        backup,
        sha256,
    })
}

pub(super) fn execute_prepared_importer_install(
    prepared: PreparedImporterInstall,
    loader_exe: &str,
    crash_hook: Option<&super::CrashHook>,
) -> Result<InstallReport> {
    let PreparedImporterInstall {
        game_dir,
        staging,
        backup,
        sha256,
    } = prepared;
    let backup_dir = backup.as_ref().map(|backup| backup.destination.clone());
    if let Some(backup) = backup.as_ref() {
        execute_backup_plan(backup, &game_dir, crash_hook)?;
    }

    // 3. Swap staged files into the game directory. From this point
    //    on, any failure triggers a rollback.
    if let Err(e) = swap_in(&staging, &game_dir) {
        if let Some(bdir) = backup_dir.as_ref() {
            let _ = rollback_to(bdir, &game_dir);
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

    // 4. Rewrite d3dx.ini's loader line. Step 0 established that the
    //    archive carries one, so a miss here is a failed install, never a
    //    step to skip.
    let mut rewrote_files = Vec::new();
    let rewrite = find_d3dx_ini(&game_dir).and_then(|d3dx| {
        rewrite_d3dx_loader(&d3dx, loader_exe)?;
        Ok(d3dx)
    });
    match rewrite {
        Ok(d3dx) => rewrote_files.push(d3dx),
        Err(e) => {
            if let Some(bdir) = backup_dir.as_ref() {
                let _ = rollback_to(bdir, &game_dir);
            }
            return Err(e);
        }
    }

    Ok(InstallReport {
        backup_dir,
        sha256,
        rewrote_files,
    })
}

/// Test-only low-level backup seam that intentionally has no durable witness.
/// The release app cannot construct this operation.
#[cfg(test)]
#[doc(hidden)]
pub fn backup_existing_unwitnessed_for_test(
    game_dir: &Path,
    backups_root: &Path,
) -> Result<Option<PathBuf>> {
    backup_existing_with(game_dir, backups_root, symlink_metadata_if_exists)
}

#[cfg(test)]
fn backup_existing_with<F>(
    game_dir: &Path,
    backups_root: &Path,
    mut probe: F,
) -> Result<Option<PathBuf>>
where
    F: FnMut(&Path) -> std::io::Result<Option<fs::Metadata>>,
{
    let Some(plan) = plan_existing_backup_with(game_dir, backups_root, Ulid::new(), &mut probe)?
    else {
        return Ok(None);
    };
    execute_backup_plan(&plan, game_dir, None)?;
    Ok(Some(plan.destination))
}

#[cfg(test)]
#[path = "../test_support/importer_tests.rs"]
mod importer_tests;

#[cfg(test)]
#[path = "../test_support/importer_archive_validation_tests.rs"]
mod importer_archive_validation_tests;

fn plan_existing_backup_with<F>(
    game_dir: &Path,
    backups_root: &Path,
    token: Ulid,
    probe: &mut F,
) -> Result<Option<PreparedImporterBackup>>
where
    F: FnMut(&Path) -> std::io::Result<Option<fs::Metadata>>,
{
    // Resolve the complete move set before creating a backup directory or
    // moving anything. An uncertain later entry must not leave an earlier
    // entry evacuated with no usable backup result.
    let mut entries = Vec::new();
    for name in IMPORTER_ROOT_FILES
        .iter()
        .copied()
        .chain(IMPORTER_ROOT_DIRS.iter().copied())
    {
        let path = game_dir.join(name);
        if probe(&path)
            .map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?
            .is_some()
        {
            entries.push(name.to_string());
        }
    }
    if entries.is_empty() {
        return Ok(None);
    }

    fs::create_dir_all(backups_root).map_err(|source| Error::Io {
        path: backups_root.to_path_buf(),
        source,
    })?;
    let game = IdentifiedDirectory::open(game_dir).map_err(|source| Error::Io {
        path: game_dir.to_path_buf(),
        source,
    })?;
    let backup_root = IdentifiedDirectory::open(backups_root).map_err(|source| Error::Io {
        path: backups_root.to_path_buf(),
        source,
    })?;
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S").to_string();
    let destination = backups_root.join(format!("{timestamp}-{token}"));
    fs::create_dir(&destination).map_err(|source| Error::Io {
        path: destination.clone(),
        source,
    })?;
    let backup = IdentifiedDirectory::open(&destination).map_err(|source| Error::Io {
        path: destination.clone(),
        source,
    })?;
    Ok(Some(PreparedImporterBackup {
        destination,
        entries,
        game_identity: game.identity().durable_key(),
        backup_identity: backup.identity().durable_key(),
        backup_root_identity: backup_root.identity().durable_key(),
    }))
}

fn backup_copy_staging_path(backup_dir: &Path, entry: &str) -> PathBuf {
    backup_dir.join(format!(".gmm-copy-{entry}"))
}

fn execute_backup_plan(
    plan: &PreparedImporterBackup,
    game_dir: &Path,
    crash_hook: Option<&super::CrashHook>,
) -> Result<()> {
    for name in &plan.entries {
        let from = game_dir.join(name);
        let to = plan.destination.join(name);
        if let Err(_e) = fs::rename(&from, &to) {
            // Cross-volume fallback. Publish a complete backup entry before
            // deleting the source: a failed or partial delete then still has
            // one authoritative copy for durable startup rollback.
            let copy_staging = backup_copy_staging_path(&plan.destination, name);
            copy_any(&from, &copy_staging)?;
            fs::rename(&copy_staging, &to).map_err(|source| Error::Io {
                path: copy_staging,
                source,
            })?;
            remove_any(&from)?;
        }
        if let Some(hook) = crash_hook {
            hook(BACKUP_AFTER_ENTRY_TEST_SEAM);
        }
    }
    Ok(())
}

pub(super) fn is_importer_backup_entry(name: &str) -> bool {
    IMPORTER_ROOT_FILES
        .iter()
        .chain(IMPORTER_ROOT_DIRS.iter())
        .any(|candidate| *candidate == name)
}

fn restore_copy_staging_path(game_dir: &Path, entry: &str) -> PathBuf {
    game_dir.join(format!(".gmm-restore-{entry}"))
}

#[derive(Debug, PartialEq, Eq)]
enum ImporterEntryContentSnapshot {
    Link(PathBuf),
    File { len: u64, sha256: [u8; 32] },
    Directory(Vec<(OsString, ImporterEntryContentSnapshot)>),
}

fn importer_entry_content_snapshot(path: &Path) -> Result<ImporterEntryContentSnapshot> {
    let metadata = fs::symlink_metadata(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let file_type = metadata.file_type();

    if file_type.is_symlink() {
        return fs::read_link(path)
            .map(ImporterEntryContentSnapshot::Link)
            .map_err(|source| Error::Io {
                path: path.to_path_buf(),
                source,
            });
    }

    if metadata.is_file() {
        let mut file = fs::File::open(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut len = 0_u64;
        loop {
            let read = file.read(&mut buffer).map_err(|source| Error::Io {
                path: path.to_path_buf(),
                source,
            })?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            len = len.saturating_add(read as u64);
        }
        if len != metadata.len() {
            return Err(Error::Importer(format!(
                "the importer entry {} changed while GMM was snapshotting it; GMM preserved both copies",
                path.display(),
            )));
        }
        return Ok(ImporterEntryContentSnapshot::File {
            len,
            sha256: hasher.finalize().into(),
        });
    }

    if !metadata.is_dir() {
        return Err(Error::Importer(format!(
            "the importer entry {} has an unsupported filesystem type",
            path.display(),
        )));
    }
    let mut names = fs::read_dir(path)
        .map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name())
                .map_err(|source| Error::Io {
                    path: path.to_path_buf(),
                    source,
                })
        })
        .collect::<Result<Vec<_>>>()?;
    names.sort();
    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        let snapshot = importer_entry_content_snapshot(&path.join(&name))?;
        entries.push((name, snapshot));
    }
    Ok(ImporterEntryContentSnapshot::Directory(entries))
}

/// Restore every entry named by a validated durable evacuation witness.
///
/// A published backup entry is always a complete copy: the cross-volume
/// fallback promotes its private copy stage before attempting source removal.
/// Recovery therefore keeps that backup until a complete replacement is in
/// the game directory. A private copy stage without a published backup is
/// only removable when the original source still exists; otherwise its
/// completeness is unknowable and recovery refuses to guess.
pub(super) fn recover_evacuated_importer(
    game_dir: &Path,
    backup_dir: &Path,
    entries: &[String],
    crash_hook: Option<&super::CrashHook>,
) -> Result<()> {
    for name in entries {
        let source = game_dir.join(name);
        let backup = backup_dir.join(name);
        let backup_copy = backup_copy_staging_path(backup_dir, name);
        let restore_copy = restore_copy_staging_path(game_dir, name);
        let source_present =
            symlink_metadata_if_exists(&source).map_err(|source_error| Error::Io {
                path: source.clone(),
                source: source_error,
            })?;
        let backup_present =
            symlink_metadata_if_exists(&backup).map_err(|source_error| Error::Io {
                path: backup.clone(),
                source: source_error,
            })?;

        if backup_present.is_some() {
            if source_present.is_some() {
                let source_snapshot = importer_entry_content_snapshot(&source)?;
                let backup_snapshot = importer_entry_content_snapshot(&backup)?;
                if source_snapshot != backup_snapshot {
                    return Err(Error::Importer(format!(
                        "the live importer entry {} differs from its recorded backup; GMM preserved both because it cannot safely choose which one is authoritative",
                        source.display(),
                    )));
                }
                if let Some(hook) = crash_hook {
                    hook(RECOVERY_AFTER_ENTRY_COMPARISON_TEST_SEAM);
                }
                if symlink_metadata_if_exists(&restore_copy)
                    .map_err(|source_error| Error::Io {
                        path: restore_copy.clone(),
                        source: source_error,
                    })?
                    .is_some()
                {
                    remove_any(&restore_copy)?;
                }
                let revalidated_source = importer_entry_content_snapshot(&source)?;
                let revalidated_backup = importer_entry_content_snapshot(&backup)?;
                if revalidated_source != source_snapshot || revalidated_backup != backup_snapshot {
                    return Err(Error::Importer(format!(
                        "the live importer entry {} or its recorded backup changed after GMM compared them; GMM preserved both because the evidence was no longer current",
                        source.display(),
                    )));
                }
                remove_any(&backup)?;
                continue;
            }
            if symlink_metadata_if_exists(&restore_copy)
                .map_err(|source_error| Error::Io {
                    path: restore_copy.clone(),
                    source: source_error,
                })?
                .is_some()
            {
                remove_any(&restore_copy)?;
            }
            copy_any(&backup, &restore_copy)?;
            fs::rename(&restore_copy, &source).map_err(|source_error| Error::Io {
                path: restore_copy.clone(),
                source: source_error,
            })?;
            // The game now has a complete restored copy. Retaining the backup
            // until this point makes any earlier failure safely retryable.
            remove_any(&backup)?;
            continue;
        }

        let backup_copy_present =
            symlink_metadata_if_exists(&backup_copy).map_err(|source| Error::Io {
                path: backup_copy.clone(),
                source,
            })?;
        if backup_copy_present.is_some() {
            if source_present.is_none() {
                return Err(Error::Importer(format!(
                    "the original importer entry {} is absent while its unpublished backup copy remains; GMM cannot prove that copy completed",
                    source.display(),
                )));
            }
            remove_any(&backup_copy)?;
        }
        if source_present.is_none() {
            return Err(Error::Importer(format!(
                "the interrupted evacuation recorded {}, but neither the game directory nor the backup contains it",
                source.display(),
            )));
        }
        if symlink_metadata_if_exists(&restore_copy)
            .map_err(|source_error| Error::Io {
                path: restore_copy.clone(),
                source: source_error,
            })?
            .is_some()
        {
            remove_any(&restore_copy)?;
        }
    }

    match fs::remove_dir(backup_dir) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::Io {
            path: backup_dir.to_path_buf(),
            source,
        }),
    }
}

/// Swap files staged in `staging` into `game_dir`. Existing files are
/// already in the backup folder at this point; we just `rename` from
/// staging into the game directory.
fn swap_in(staging: &Path, game_dir: &Path) -> Result<()> {
    swap_in_with_destination_probe(staging, game_dir, symlink_metadata_if_exists)
}

fn swap_in_with_destination_probe<F>(
    staging: &Path,
    game_dir: &Path,
    mut destination_probe: F,
) -> Result<()>
where
    F: FnMut(&Path) -> std::io::Result<Option<fs::Metadata>>,
{
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
        let destination_present = destination_probe(&to).map_err(|source| Error::Io {
            path: to.clone(),
            source,
        })?;
        if is_user_owned && destination_present.is_some() {
            merge_into(&from, &to)?;
            remove_any(&from)?;
            continue;
        }

        if destination_present.is_some() {
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
    merge_into_with_destination_probe(from, to, &mut symlink_metadata_if_exists)
}

fn merge_into_with_destination_probe<F>(
    from: &Path,
    to: &Path,
    destination_probe: &mut F,
) -> Result<()>
where
    F: FnMut(&Path) -> std::io::Result<Option<fs::Metadata>>,
{
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
        let destination = destination_probe(&dest).map_err(|source| Error::Io {
            path: dest.clone(),
            source,
        })?;

        if let Some(destination) = destination {
            // Recurse only when both sides are ordinary directories.
            // A junction/reparse point on the destination side belongs
            // to the user (it is an enabled mod) and is left alone.
            // Safe: the fallible lookup above succeeded; these calls only classify known metadata.
            let dest_is_plain_dir = destination.is_dir() && !destination.file_type().is_symlink();
            let source_metadata = fs::metadata(&src).map_err(|source| Error::Io {
                path: src.clone(),
                source,
            })?;
            // Safe: `metadata()` above propagated I/O uncertainty.
            if source_metadata.is_dir() && dest_is_plain_dir {
                merge_into_with_destination_probe(&src, &dest, destination_probe)?;
            }
            // Otherwise keep whatever is already there — never clobber
            // something the user (or GMM) put in place.
            continue;
        }

        copy_any(&src, &dest)?;
    }
    Ok(())
}

/// What GMM knew about the install a backup replaced.
///
/// A backup is a pile of files and carries no provenance of its own, so
/// rolling one back left GMM's record describing the install that had
/// just been undone (#126). This is written *beside* the backup at
/// install time and read back at rollback time.
///
/// Both fields are optional because GMM may genuinely not have known:
/// an install performed over a hand-installed setup replaces files GMM
/// never recorded. `None` here means unknown, which is a first-class
/// state (#99) and strictly better than a confident wrong answer.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupProvenance {
    /// The importer version GMM had recorded for the backed-up files.
    pub version: Option<String>,
    /// The Importer Origin GMM had recorded for them.
    pub origin: Option<super::importer_origin::ImporterOrigin>,
}

/// Where [`BackupProvenance`] is stored for `backup_dir`.
///
/// A **sibling** of the backup directory, never a file inside it:
/// [`rollback_to`] moves every entry of the backup directory into the
/// game directory, so a sidecar stored inside would be deposited next to
/// the user's `d3dx.ini`. The rollback picker only considers
/// directories, so the sibling is invisible to it.
pub fn provenance_path(backup_dir: &Path) -> PathBuf {
    let mut name = backup_dir.file_name().unwrap_or_default().to_os_string();
    name.push(".gmm-provenance.json");
    backup_dir.with_file_name(name)
}

/// Record what the files in `backup_dir` were, so a later rollback can
/// restore GMM's bookkeeping along with the files.
pub fn write_backup_provenance(backup_dir: &Path, provenance: &BackupProvenance) -> Result<()> {
    let path = provenance_path(backup_dir);
    let json = serde_json::to_string_pretty(provenance)
        .map_err(|e| Error::Importer(format!("could not encode backup provenance: {e}")))?;
    fs::write(&path, json).map_err(|source| Error::Io { path, source })
}

/// Read back what [`write_backup_provenance`] recorded.
///
/// `None` for every reason: no sidecar (a backup taken by a GMM that
/// predates this, or one taken over files GMM never installed) and an
/// unreadable one alike. All of them mean the same thing to the caller —
/// GMM cannot say what these files were — and the caller's response is
/// to record unknown rather than to guess.
pub fn read_backup_provenance(backup_dir: &Path) -> Option<BackupProvenance> {
    let path = provenance_path(backup_dir);
    #[allow(
        clippy::disallowed_methods,
        reason = "backup provenance is optional metadata; this read failure deliberately produces the explicit unknown-origin state"
    )]
    let raw = fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&raw) {
        Ok(provenance) => Some(provenance),
        Err(e) => {
            tracing::warn!(
                target: "gmm::importer",
                path = %path.display(),
                error = %e,
                "backup provenance could not be read; the rollback will record an \
                 unknown Importer Origin rather than guess",
            );
            None
        }
    }
}

/// The most recent backup under `backups_root`, or `None` when there is
/// nothing to roll back to.
///
/// Backups are named with an ISO-8601 timestamp, so lexicographic order
/// is chronological order.
pub fn latest_backup(backups_root: &Path) -> Result<Option<PathBuf>> {
    let listed = match fs::read_dir(backups_root) {
        Ok(listed) => listed,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::Io {
                path: backups_root.to_path_buf(),
                source,
            })
        }
    };
    let mut entries = Vec::new();
    for entry in listed {
        let entry = entry.map_err(|source| Error::Io {
            path: backups_root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let Some(metadata) = metadata_if_exists(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?
        else {
            continue;
        };
        // Safe: `metadata_if_exists` propagated I/O uncertainty.
        if metadata.is_dir() {
            entries.push(path);
        }
    }
    entries.sort();
    Ok(entries.pop())
}

/// Restore `game_dir` to the state captured in `backup_dir`. Files
/// currently in `game_dir` with the same name are removed first.
pub fn rollback_to(backup_dir: &Path, game_dir: &Path) -> Result<()> {
    rollback_to_with_destination_probe(backup_dir, game_dir, &mut symlink_metadata_if_exists)
}

fn rollback_to_with_destination_probe<F>(
    backup_dir: &Path,
    game_dir: &Path,
    destination_probe: &mut F,
) -> Result<()>
where
    F: FnMut(&Path) -> std::io::Result<Option<fs::Metadata>>,
{
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
            if destination_probe(&to)
                .map_err(|source| Error::Io {
                    path: to.clone(),
                    source,
                })?
                .is_some()
            {
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

        if destination_probe(&to)
            .map_err(|source| Error::Io {
                path: to.clone(),
                source,
            })?
            .is_some()
        {
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
    // Safe: `symlink_metadata()` above propagated I/O uncertainty.
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
    // Safe: `symlink_metadata()` above propagated I/O uncertainty.
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
    copy_dir_recursive_with_file_type(src, dst, &mut |entry| entry.file_type())
}

fn copy_dir_recursive_with_file_type<F>(
    src: &Path,
    dst: &Path,
    classify_file_type: &mut F,
) -> Result<()>
where
    F: FnMut(&fs::DirEntry) -> std::io::Result<fs::FileType>,
{
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
        let file_type = classify_file_type(&entry).map_err(|source| Error::Io {
            path: entry_path.clone(),
            source,
        })?;
        // Safe: `file_type()` above propagated I/O uncertainty.
        if file_type.is_dir() {
            copy_dir_recursive_with_file_type(&entry_path, &dst_path, classify_file_type)?;
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
/// Where release metadata is read from.
///
/// The same test seam as [`super::gamebanana::Endpoints`], and for the
/// same reason: the install path is only worth testing end-to-end if a
/// test can stand in for upstream. Production always uses
/// [`Endpoints::default`]; nothing in the shipped code constructs any
/// other value.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub api_base: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            api_base: "https://api.github.com".to_string(),
        }
    }
}

/// The caller must build the `client` via
/// [`crate::core::Core::http_client`] so the request honours any
/// configured proxy.
pub async fn fetch_latest_release(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    owner_repo: &str,
    pattern: &AssetPattern,
    etag: Option<&str>,
) -> Result<Option<LatestRelease>> {
    let url = format!(
        "{}/repos/{owner_repo}/releases/latest",
        endpoints.api_base.trim_end_matches('/')
    );
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

#[cfg(test)]
mod tests {
    use std::io::{self, Write as _};

    use super::*;

    fn importer_zip(path: &Path) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).expect("create importer zip"));
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("d3dx.ini", options).expect("start d3dx.ini");
        zip.write_all(b"[Loader]\nloader = old.exe\n")
            .expect("write d3dx.ini");
        zip.add_directory("Core/", options).expect("add Core");
        zip.start_file("Core/library.ini", options)
            .expect("start Core file");
        zip.write_all(b"core").expect("write Core file");
        zip.add_directory("ShaderFixes/", options)
            .expect("add ShaderFixes");
        zip.finish().expect("finish importer zip");
    }

    fn names(path: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(path)
            .expect("read test directory")
            .map(|entry| {
                entry
                    .expect("read test entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    fn denied(message: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, message)
    }

    #[test]
    fn staging_lookup_uncertainty_leaves_game_directory_unchanged() {
        let temp = tempfile::tempdir().expect("temporary importer roots");
        let zip = temp.path().join("importer.zip");
        importer_zip(&zip);
        let game = temp.path().join("game");
        fs::create_dir_all(&game).expect("create game");
        fs::write(game.join("sentinel.txt"), b"user bytes").expect("write sentinel");
        let backups = temp.path().join("backups");
        let staging = game.join(".gmm-staging");

        let result = install_from_local_zip_with_staging_probe(
            &zip,
            &game,
            &backups,
            DEFAULT_LOADER_EXE,
            |_| Err(denied("test staging obstruction")),
        );

        assert_eq!(
            names(&game),
            vec!["sentinel.txt"],
            "staging lookup uncertainty must not mutate the game directory",
        );
        assert_eq!(
            fs::read(game.join("sentinel.txt")).expect("read sentinel"),
            b"user bytes",
        );
        assert!(
            matches!(result, Err(Error::Io { ref path, ref source })
                if path == &staging && source.kind() == io::ErrorKind::PermissionDenied),
            "staging lookup uncertainty must stop installation, got {result:?}",
        );
    }

    #[test]
    fn swap_destination_uncertainty_leaves_both_trees_unchanged() {
        let temp = tempfile::tempdir().expect("temporary swap roots");
        let staging = temp.path().join("staging");
        let game = temp.path().join("game");
        fs::create_dir_all(&staging).expect("create staging");
        fs::create_dir_all(&game).expect("create game");
        fs::write(staging.join("d3dx.ini"), b"staged bytes").expect("write staged file");
        fs::write(game.join("d3dx.ini"), b"live bytes").expect("write live file");

        let result = swap_in_with_destination_probe(&staging, &game, |_| {
            Err(denied("test swap destination obstruction"))
        });

        assert_eq!(
            fs::read(game.join("d3dx.ini")).expect("read live file"),
            b"live bytes",
            "destination uncertainty must not overwrite the live game file",
        );
        assert_eq!(
            fs::read(staging.join("d3dx.ini")).expect("read staged file"),
            b"staged bytes",
            "destination uncertainty must not evacuate staging",
        );
        assert!(
            matches!(result, Err(Error::Io { ref source, .. })
                if source.kind() == io::ErrorKind::PermissionDenied),
            "swap destination uncertainty must stop before mutation, got {result:?}",
        );
    }

    #[test]
    fn merge_destination_uncertainty_leaves_both_trees_unchanged() {
        let temp = tempfile::tempdir().expect("temporary merge roots");
        let from = temp.path().join("from");
        let to = temp.path().join("to");
        fs::create_dir_all(&from).expect("create source");
        fs::create_dir_all(&to).expect("create destination");
        fs::write(from.join("example.ini"), b"shipped bytes").expect("write source");
        fs::write(to.join("example.ini"), b"user bytes").expect("write destination");
        let mut probe = |_: &Path| Err(denied("test merge destination obstruction"));

        let result = merge_into_with_destination_probe(&from, &to, &mut probe);

        assert_eq!(
            fs::read(to.join("example.ini")).expect("read destination"),
            b"user bytes",
            "destination uncertainty must not overwrite the user's file",
        );
        assert_eq!(
            fs::read(from.join("example.ini")).expect("read source"),
            b"shipped bytes",
            "destination uncertainty must not remove the source file",
        );
        assert!(
            matches!(result, Err(Error::Io { ref source, .. })
                if source.kind() == io::ErrorKind::PermissionDenied),
            "merge destination uncertainty must stop before mutation, got {result:?}",
        );
    }

    #[test]
    fn rollback_mods_destination_uncertainty_leaves_both_trees_unchanged() {
        let temp = tempfile::tempdir().expect("temporary rollback roots");
        let backup = temp.path().join("backup");
        let game = temp.path().join("game");
        fs::create_dir_all(backup.join("Mods")).expect("create backup Mods");
        fs::create_dir_all(game.join("Mods")).expect("create live Mods");
        fs::write(backup.join("Mods/shipped.ini"), b"backup bytes").expect("write backup");
        fs::write(game.join("Mods/user.ini"), b"user bytes").expect("write live mod");
        let mut probe = |_: &Path| Err(denied("test rollback Mods obstruction"));

        let result = rollback_to_with_destination_probe(&backup, &game, &mut probe);

        assert_eq!(
            fs::read(game.join("Mods/user.ini")).expect("read live mod"),
            b"user bytes",
            "Mods destination uncertainty must preserve the live file bytes",
        );
        assert_eq!(
            fs::read(backup.join("Mods/shipped.ini")).expect("read backup mod"),
            b"backup bytes",
            "Mods destination uncertainty must preserve the backup file bytes",
        );
        assert_eq!(
            names(&game.join("Mods")),
            vec!["user.ini"],
            "Mods destination uncertainty must not merge backup bytes into the live tree",
        );
        assert_eq!(
            names(&backup.join("Mods")),
            vec!["shipped.ini"],
            "Mods destination uncertainty must not evacuate the backup",
        );
        assert!(
            matches!(result, Err(Error::Io { ref source, .. })
                if source.kind() == io::ErrorKind::PermissionDenied),
            "rollback Mods destination uncertainty must stop before mutation, got {result:?}",
        );
    }

    #[test]
    fn rollback_file_destination_uncertainty_leaves_both_trees_unchanged() {
        let temp = tempfile::tempdir().expect("temporary rollback roots");
        let backup = temp.path().join("backup");
        let game = temp.path().join("game");
        fs::create_dir_all(&backup).expect("create backup");
        fs::create_dir_all(&game).expect("create game");
        fs::write(backup.join("d3dx.ini"), b"backup bytes").expect("write backup");
        fs::write(game.join("d3dx.ini"), b"live bytes").expect("write live file");
        let mut probe = |_: &Path| Err(denied("test rollback file obstruction"));

        let result = rollback_to_with_destination_probe(&backup, &game, &mut probe);

        assert_eq!(
            fs::read(game.join("d3dx.ini")).expect("read live file"),
            b"live bytes",
            "file destination uncertainty must not overwrite the live file",
        );
        assert_eq!(
            fs::read(backup.join("d3dx.ini")).expect("read backup file"),
            b"backup bytes",
            "file destination uncertainty must not evacuate the backup",
        );
        assert!(
            matches!(result, Err(Error::Io { ref source, .. })
                if source.kind() == io::ErrorKind::PermissionDenied),
            "rollback file destination uncertainty must stop before mutation, got {result:?}",
        );
    }

    #[test]
    fn recursive_copy_entry_type_uncertainty_leaves_destination_unchanged() {
        let temp = tempfile::tempdir().expect("temporary copy roots");
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir_all(&source).expect("create source");
        fs::create_dir_all(&destination).expect("create destination");
        fs::write(source.join("file.ini"), b"source bytes").expect("write source file");
        let mut classify = |_: &fs::DirEntry| Err(denied("test entry type obstruction"));

        let result = copy_dir_recursive_with_file_type(&source, &destination, &mut classify);

        assert_eq!(
            names(&destination),
            Vec::<String>::new(),
            "entry type uncertainty must not copy anything into the destination",
        );
        assert_eq!(
            fs::read(source.join("file.ini")).expect("read source file"),
            b"source bytes",
            "entry type uncertainty must not remove source bytes",
        );
        assert!(
            matches!(result, Err(Error::Io { ref source, .. })
                if source.kind() == io::ErrorKind::PermissionDenied),
            "entry type uncertainty must stop recursive copy, got {result:?}",
        );
    }

    #[test]
    fn backup_preflight_uncertainty_leaves_game_directory_intact() {
        let temp = tempfile::tempdir().expect("temporary importer roots");
        let game_dir = temp.path().join("game");
        let backups_root = temp.path().join("backups");
        fs::create_dir_all(game_dir.join("Core")).expect("create game Core");
        fs::write(game_dir.join("d3d11.dll"), b"working loader").expect("write loader");

        let result = backup_existing_with(&game_dir, &backups_root, |path| {
            if path.file_name().is_some_and(|name| name == "Core") {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "test obstruction on later importer entry",
                ));
            }
            symlink_metadata_if_exists(path)
        });

        assert_eq!(
            fs::read(game_dir.join("d3d11.dll")).expect(
                "preflight uncertainty must leave earlier importer entries in the game directory",
            ),
            b"working loader",
            "preflight uncertainty must leave earlier importer entries in the game directory",
        );
        fs::symlink_metadata(game_dir.join("Core"))
            .expect("preflight uncertainty must leave the obstructed entry in the game directory");
        assert!(
            matches!(result, Err(Error::Io { ref path, ref source })
                if path == &game_dir.join("Core")
                    && source.kind() == io::ErrorKind::PermissionDenied),
            "a later uncertain entry must abort before evacuation, got {result:?}",
        );
    }

    #[cfg(unix)]
    #[test]
    fn merge_into_propagates_uncertain_source_metadata_at_occupied_destination() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary merge roots");
        let from = temp.path().join("from");
        let to = temp.path().join("to");
        fs::create_dir_all(&from).expect("create source");
        fs::create_dir_all(&to).expect("create destination");
        let source = from.join("loop");
        symlink(&source, &source).expect("create self-referential source symlink");
        fs::write(to.join("loop"), b"occupied").expect("occupy destination");

        let result = merge_into(&from, &to);
        assert!(
            matches!(result, Err(Error::Io { ref path, .. }) if path == &source),
            "uncertain source metadata at an occupied destination must not be silently skipped, got {result:?}",
        );
    }
}
