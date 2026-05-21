//! Arknights: Endfield (EFMI) install detection (slice 10 / #20).
//!
//! Endfield is Hypergryph's Unreal Engine 5 title — same general
//! shape as Kuro's Wuthering Waves (UE5, deeply nested binary, UE
//! `Content/` tree as the discriminator). The canonical layout is:
//!
//! ```text
//! <install>/Endfield Game/Endfield/Binaries/Win64/Endfield-Win64-Shipping.exe
//! ```
//!
//! Quirks called out in #20:
//!
//! - Upstream XXMI sets `custom_launch_inject_mode = 'Inject'` for
//!   EFMI (rather than the default `Hook` mode used for the
//!   Hoyoverse + Kuro games). We wire that into the per-game
//!   `GameProfile::inject_mode` field; `launch_game` branches on it.
//! - Mod scene is near-zero as of the Feb 2026 game launch. We
//!   keep the slice in proportion: zero per-game branches in
//!   `commands.rs`, no UI special-case, all dispatch via the
//!   `GameProfile` registry introduced in slice 6 (#16).

use std::path::{Path, PathBuf};

/// EFMI executable names. UE5's shipping binary is the canonical
/// target. Earlier closed-beta builds shipped `Endfield.exe` as a
/// thin launcher next to the shipping exe; accept it as a fallback so
/// the validator does not refuse a CBT-tested install on day 1.
pub const EXE_NAMES: &[&str] = &["Endfield-Win64-Shipping.exe", "Endfield.exe"];

/// Validate iff `Endfield-Win64-Shipping.exe` (or the closed-beta
/// `Endfield.exe`) is present AND the Unreal `Content/` directory
/// exists two levels up. Same UE discriminator we use for Wuthering
/// Waves (slice 8 / #18).
pub fn validate(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let exe_present = EXE_NAMES.iter().any(|name| path.join(name).is_file());
    if !exe_present {
        return false;
    }
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

/// "Probably installed here" paths. Endfield ships through the
/// Hypergryph launcher; we cover the common Program Files / drive-
/// root locations plus the deeply-nested UE descent.
pub fn common_install_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let suffix = PathBuf::from("Endfield Game")
        .join("Endfield")
        .join("Binaries")
        .join("Win64");
    for program_files_var in ["ProgramFiles", "ProgramFiles(x86)"] {
        let pf = std::env::var_os(program_files_var)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(program_files_var_to_default(program_files_var)));
        out.push(pf.join("Endfield").join(&suffix));
        out.push(pf.join(&suffix));
    }
    out.push(PathBuf::from(r"C:\Endfield").join(&suffix));
    out.push(PathBuf::from(r"D:\Endfield").join(&suffix));
    out.push(PathBuf::from(r"C:\Games\Endfield").join(&suffix));
    out.push(PathBuf::from(r"D:\Games\Endfield").join(&suffix));
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

/// Read uninstall registry entries on Windows. The Hypergryph
/// installer writes `InstallLocation` as the launcher root; we push
/// both that and the `Endfield Game/Endfield/Binaries/Win64/` descent
/// so the deepest valid match wins.
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

    let suffix = PathBuf::from("Endfield Game")
        .join("Endfield")
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
            if !is_endfield_display_name(&display) {
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

/// Match Hypergryph's display-name conventions across global / CN /
/// JP locales. Public so unit tests can hit it without a registry.
pub fn is_endfield_display_name(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("endfield")
        || lower.contains("明日方舟：终末地")
        || lower.contains("アークナイツ：エンドフィールド")
}

/// Production orchestrator: registry → common paths. Returns the
/// first candidate that passes [`validate`].
pub fn detect() -> Option<PathBuf> {
    let mut chain = detect_from_registry();
    chain.extend(common_install_candidates());
    detect_from_paths(chain)
}
