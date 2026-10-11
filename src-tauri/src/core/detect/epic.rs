//! Epic Games Launcher's `.item` install records, not folder-name guesses.
//!
//! The default Windows store is `%ProgramData%/Epic/EpicGamesLauncher/Data/Manifests`.
//! Callers can inject that directory on any OS. Without an override, other OSes
//! have no Epic source. Every record is an independent best-effort probe.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;

// Install records are small JSON documents; refuse oversized or truncated data.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InstallManifest {
    display_name: String,
    install_location: String,
    launch_executable: String,
    #[serde(default, rename = "bIsIncompleteInstall")]
    incomplete: bool,
}

/// Read Epic candidates for one Game, accepting only directories its optional
/// validator recognizes. `manifest_root` overrides the store, not an install
/// location. Missing/unreadable stores and individual bad records yield nothing.
pub fn install_candidates(
    manifest_root: Option<&Path>,
    matches_game: fn(&str) -> bool,
    validate: fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let root = manifest_root
        .map(Path::to_path_buf)
        .or_else(default_manifest_root);
    let Some(root) = root else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut manifests: Vec<_> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            #[allow(
                clippy::disallowed_methods,
                reason = "optional-install Epic detection intentionally skips unreadable manifest entries"
            )]
            let usable_manifest = matches!(entry.file_type(), Ok(kind) if kind.is_file());
            usable_manifest
        })
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("item"))
        })
        .collect();
    manifests.sort();

    let mut candidates = Vec::new();
    for path in manifests {
        let Some(manifest) = read_manifest(&path) else {
            continue;
        };
        if manifest.incomplete || !matches_game(&manifest.display_name) {
            continue;
        }
        let install = PathBuf::from(&manifest.install_location);
        if !install.is_absolute() {
            continue;
        }
        // Epic records launch paths relative to InstallLocation, using either
        // separator. Refuse traversal, drive prefixes and empty components on
        // every OS rather than letting a malformed record escape its install.
        let launch = manifest.launch_executable.replace('\\', "/");
        if launch
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".." || part.contains(':'))
        {
            continue;
        }
        let executable = install.join(launch);
        #[allow(
            clippy::disallowed_methods,
            reason = "optional-install Epic detection intentionally treats an unreadable launch executable as unusable"
        )]
        let usable_executable =
            matches!(fs::metadata(&executable), Ok(metadata) if metadata.is_file());
        if !usable_executable {
            continue;
        }
        if let Some(parent) = executable.parent() {
            candidates.push(parent.to_path_buf());
        }
        candidates.push(install);
    }
    super::validated_candidates(candidates, validate)
}

fn read_manifest(path: &Path) -> Option<InstallManifest> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return None;
    }
    let manifest: InstallManifest = serde_json::from_slice(&bytes).ok()?;
    if manifest.display_name.trim().is_empty()
        || manifest.install_location.trim().is_empty()
        || manifest.launch_executable.trim().is_empty()
    {
        return None;
    }
    Some(manifest)
}

#[cfg(windows)]
fn default_manifest_root() -> Option<PathBuf> {
    let program_data = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    Some(program_data.join("Epic/EpicGamesLauncher/Data/Manifests"))
}

#[cfg(not(windows))]
fn default_manifest_root() -> Option<PathBuf> {
    None
}
