//! Wuthering Waves (WWMI) install detection (slice 8 / #18).
//!
//! Unlike the Hoyoverse trio (GIMI/SRMI/ZZMI), Wuthering Waves ships
//! on Unreal Engine, not Unity. The game executable lives several
//! directories deep:
//!
//! ```text
//! <install>/Wuthering Waves Game/Client/Binaries/Win64/Client-Win64-Shipping.exe
//! ```
//!
//! The Model Importer (WWMI) installs `d3d11.dll` + `Mods/` alongside
//! the exe, so GMM stores the install path as the
//! `Client/Binaries/Win64/` directory (consistent with where mods
//! actually load from). That is **not** the path the launcher's
//! shortcut points at — the launcher root is two levels up. The
//! registry probe walks down to the playable directory before
//! returning, so both the launcher-recorded root and the playable
//! path resolve to the same canonical candidate.
//!
//! Per the issue body, WWMI has historically had a more aggressive
//! anti-cheat posture than the Hoyoverse importers. We do not pass
//! XXMI's `-SkipSplash` launch option (`use_launch_options: bool =
//! False` by default upstream); `launch_game` spawns the executable
//! with no extra args, matching that default.
//!
//! Validation: a candidate passes iff `Client-Win64-Shipping.exe` is
//! present AND the Unreal `Content/` directory exists two levels up
//! (i.e. `<path>/../../Content`). The UE content tree is the
//! discriminator that stops the detector from accepting a folder
//! someone dropped a renamed exe into.

use std::path::{Path, PathBuf};

/// WWMI executable names. The launcher root contains `launcher.exe`,
/// but that is not what gets injected into; GMM stores the playable
/// `Win64/` directory and launches the actual game binary.
pub const EXE_NAMES: &[&str] = &["Client-Win64-Shipping.exe"];

/// `true` iff the candidate looks like Wuthering Waves's playable
/// directory: contains the shipping exe AND the Unreal `Content/`
/// directory two levels up.
pub fn validate(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let exe_present = EXE_NAMES.iter().any(|name| path.join(name).is_file());
    if !exe_present {
        return false;
    }
    // Walk two ancestors back to land at `<install>/Wuthering Waves
    // Game/Client/`, where Unreal's `Content/` directory lives.
    let content_root = path
        .parent()
        .and_then(Path::parent)
        .map(|p| p.join("Content"));
    matches!(content_root, Some(p) if p.is_dir())
}

/// Try each candidate path in order, returning the first one that
/// passes [`validate`].
pub fn detect_from_paths<I>(candidates: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    candidates.into_iter().find(|c| validate(c))
}

/// "The installer probably dropped it here" paths. Kuro's launcher
/// has shipped at least three layouts; we cover the common installer
/// drop directly plus the standalone drive-root locations.
pub fn common_install_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let suffix = PathBuf::from("Wuthering Waves Game")
        .join("Client")
        .join("Binaries")
        .join("Win64");
    for program_files_var in ["ProgramFiles", "ProgramFiles(x86)"] {
        let pf = std::env::var_os(program_files_var)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(program_files_var_to_default(program_files_var)));
        out.push(pf.join("Wuthering Waves").join(&suffix));
        out.push(pf.join(&suffix));
    }
    out.push(PathBuf::from(r"C:\Wuthering Waves").join(&suffix));
    out.push(PathBuf::from(r"D:\Wuthering Waves").join(&suffix));
    out.push(PathBuf::from(r"C:\Games\Wuthering Waves").join(&suffix));
    out.push(PathBuf::from(r"D:\Games\Wuthering Waves").join(&suffix));
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

/// Read uninstall registry entries on Windows. Kuro's launcher writes
/// `InstallLocation` as the directory containing `launcher.exe`; we
/// push that plus the playable `Client/Binaries/Win64/` descent so
/// `validate` accepts the deepest match regardless of which level the
/// registry actually recorded.
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

    let suffix = PathBuf::from("Wuthering Waves Game")
        .join("Client")
        .join("Binaries")
        .join("Win64");

    for (root, path) in roots {
        let Ok(uninstall) = root.open_subkey(path) else {
            continue;
        };
        for name in uninstall.enum_keys().flatten() {
            let Ok(sub) = uninstall.open_subkey(&name) else {
                continue;
            };
            let display: String = sub.get_value("DisplayName").unwrap_or_default();
            if !is_wuthering_display_name(&display) {
                continue;
            }
            let install_location: String = sub.get_value("InstallLocation").unwrap_or_default();
            if install_location.is_empty() {
                continue;
            }
            let install = PathBuf::from(&install_location);
            out.push(install.join(&suffix));
            out.push(install);
        }
    }
    out
}

#[cfg(not(windows))]
pub fn detect_from_registry() -> Vec<PathBuf> {
    Vec::new()
}

/// Match Kuro's display-name conventions across global and CN
/// installer builds. Public so unit tests can exercise it without a
/// registry.
pub fn is_wuthering_display_name(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("wuthering waves") || lower.contains("鸣潮")
}

/// Production orchestrator: registry → common paths. Returns the
/// first candidate that passes [`validate`].
pub fn detect() -> Option<PathBuf> {
    let mut chain = detect_from_registry();
    chain.extend(common_install_candidates());
    detect_from_paths(chain)
}
