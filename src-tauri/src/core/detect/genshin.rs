//! Genshin (GIMI) install detection.
//!
//! The chain we walk, in order:
//!
//! 1. Windows uninstall registry (HKLM + HKCU under
//!    `Software\Microsoft\Windows\CurrentVersion\Uninstall`). Hoyoverse's
//!    installer writes `InstallLocation` here.
//! 2. Common install paths: `Program Files\Genshin Impact`,
//!    `Program Files (x86)\Genshin Impact`, `C:\Genshin`, `D:\Genshin`.
//! 3. The user-confirmed cached path lives in the `games` table; once
//!    set, GMM just uses it without re-running detection.
//! 4. Falls back to the manual picker in the UI.
//!
//! Validation: a candidate is accepted only if both
//! `GenshinImpact.exe` (or `YuanShen.exe`, the CN client) and the
//! `GenshinImpact_Data` directory exist directly inside the candidate.

use std::path::{Path, PathBuf};

/// Names of the Genshin client executable. The CN client uses
/// `YuanShen.exe`; the global client uses `GenshinImpact.exe`.
pub const EXE_NAMES: &[&str] = &["GenshinImpact.exe", "YuanShen.exe"];

/// The `*_Data` directory Unity puts alongside every Unity-engine game.
/// We use its presence to discriminate a real install from a folder
/// that happens to contain a renamed `.exe`.
pub const DATA_DIR_NAME: &str = "GenshinImpact_Data";

/// Returns `true` iff `path` is a directory containing one of the
/// supported executables AND the matching Unity data directory.
pub fn validate(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let exe_present = EXE_NAMES.iter().any(|name| path.join(name).is_file());
    if !exe_present {
        return false;
    }
    path.join(DATA_DIR_NAME).is_dir()
}

/// Try each candidate path in order, returning the first one that
/// passes [`validate`].
pub fn detect_from_paths<I>(candidates: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    candidates.into_iter().find(|c| validate(c))
}

/// The hardcoded list of "the installer probably dropped it here"
/// paths. Order matters — Program Files first because that's where the
/// official installer lands by default; drive-root paths last because
/// they're the user-relocated installs.
pub fn common_install_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for program_files_var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(pf) = std::env::var_os(program_files_var) {
            out.push(
                PathBuf::from(pf)
                    .join("Genshin Impact")
                    .join("Genshin Impact game"),
            );
            out.push(
                PathBuf::from(program_files_var_to_default(program_files_var))
                    .join("Genshin Impact")
                    .join("Genshin Impact game"),
            );
        } else {
            out.push(
                PathBuf::from(program_files_var_to_default(program_files_var))
                    .join("Genshin Impact")
                    .join("Genshin Impact game"),
            );
        }
    }
    // Common standalone-installer drive-root locations.
    out.push(PathBuf::from(r"C:\Genshin Impact\Genshin Impact game"));
    out.push(PathBuf::from(r"D:\Genshin Impact\Genshin Impact game"));
    out.push(PathBuf::from(r"C:\Genshin"));
    out.push(PathBuf::from(r"D:\Genshin"));
    dedup_preserve_order(out)
}

fn program_files_var_to_default(var: &str) -> &'static str {
    match var {
        "ProgramFiles" => r"C:\Program Files",
        "ProgramFiles(x86)" => r"C:\Program Files (x86)",
        _ => r"C:\Program Files",
    }
}

fn dedup_preserve_order(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    paths
        .into_iter()
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Read uninstall registry entries on Windows and return any
/// `InstallLocation` values whose display name matches Genshin. Empty on
/// non-Windows and on registry read errors.
#[cfg(windows)]
pub fn detect_from_registry() -> Vec<PathBuf> {
    use winreg::enums::*;
    use winreg::RegKey;

    let mut out = Vec::new();
    let roots = [
        (
            RegKey::predef(HKEY_LOCAL_MACHINE),
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            RegKey::predef(HKEY_LOCAL_MACHINE),
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            RegKey::predef(HKEY_CURRENT_USER),
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
    ];

    for (root, path) in roots {
        let Ok(uninstall) = root.open_subkey(path) else {
            continue;
        };
        for name in uninstall.enum_keys().flatten() {
            let Ok(sub) = uninstall.open_subkey(&name) else {
                continue;
            };
            let display: String = sub.get_value("DisplayName").unwrap_or_default();
            if !is_genshin_display_name(&display) {
                continue;
            }
            let install_location: String = sub.get_value("InstallLocation").unwrap_or_default();
            if !install_location.is_empty() {
                out.push(PathBuf::from(install_location));
            }
        }
    }
    out
}

#[cfg(not(windows))]
pub fn detect_from_registry() -> Vec<PathBuf> {
    Vec::new()
}

/// Match Hoyoverse's display-name conventions across the global and CN
/// installer builds. Public so it can be exercised in unit tests
/// without spinning up a registry.
pub fn is_genshin_display_name(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("genshin impact") || lower.contains("yuanshen") || lower.contains("原神")
}

/// Production orchestrator: registry → common paths. Returns the first
/// candidate that passes [`validate`].
pub fn detect() -> Option<PathBuf> {
    let mut chain = detect_from_registry();
    chain.extend(common_install_candidates());
    detect_from_paths(chain)
}
