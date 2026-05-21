//! Star Rail (SRMI) install detection (slice 6 / #16).
//!
//! Mirrors the GIMI detector — Hoyoverse's installer layout is the
//! same Unity-engine pattern across Genshin, Star Rail, and ZZZ:
//! `<install>/Game/<exe>` next to `<install>/Game/<exe-prefix>_Data/`.
//!
//! Chain walked in order:
//!
//! 1. Windows uninstall registry (HKLM + HKCU under
//!    `Software\Microsoft\Windows\CurrentVersion\Uninstall`). HoYoPlay
//!    writes `InstallLocation` here with a *Honkai: Star Rail* or
//!    CN-locale display name.
//! 2. Common install paths: HoYoPlay's `Program Files\Star Rail\Game\`,
//!    the legacy `Program Files\Honkai Star Rail\Game\`, and the
//!    drive-root standalone-installer paths users frequently pick.
//! 3. The user-confirmed cached path lives in the `games` table; once
//!    set, GMM just uses it without re-running detection (same flow as
//!    GIMI — see `core::detect::genshin`).
//! 4. Falls back to the manual picker in the UI.
//!
//! Validation: a candidate is accepted only if `StarRail.exe` exists
//! AND the matching `StarRail_Data` Unity directory is present —
//! exactly the discriminator that stops the detector from accepting a
//! folder where someone renamed an unrelated `.exe`.

use std::path::{Path, PathBuf};

/// Names of the Star Rail client executable. The global and CN
/// clients have been unified under a single `StarRail.exe`; we keep
/// the slice shaped like GIMI's so a future CN-only build can be
/// added without churning callers.
pub const EXE_NAMES: &[&str] = &["StarRail.exe"];

/// The Unity `*_Data` directory dropped next to the exe.
pub const DATA_DIR_NAME: &str = "StarRail_Data";

/// `true` iff `path` is a directory containing the executable AND the
/// matching Unity data directory.
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

/// "The installer probably dropped it here" paths. Order matters —
/// HoYoPlay's `Star Rail/Game/` first, then the legacy *Honkai Star
/// Rail/Game/* layout, then the drive-root user-relocated installs.
pub fn common_install_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for program_files_var in ["ProgramFiles", "ProgramFiles(x86)"] {
        let pf = std::env::var_os(program_files_var)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(program_files_var_to_default(program_files_var)));
        out.push(pf.join("Star Rail").join("Game"));
        out.push(pf.join("Honkai Star Rail").join("Game"));
        out.push(
            pf.join("HoYoPlay")
                .join("games")
                .join("Star Rail")
                .join("Game"),
        );
    }
    out.push(PathBuf::from(r"C:\Star Rail\Game"));
    out.push(PathBuf::from(r"D:\Star Rail\Game"));
    out.push(PathBuf::from(r"C:\Honkai Star Rail\Game"));
    out.push(PathBuf::from(r"D:\Honkai Star Rail\Game"));
    out.push(PathBuf::from(r"C:\StarRail"));
    out.push(PathBuf::from(r"D:\StarRail"));
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
/// `InstallLocation` values whose display name matches Honkai: Star
/// Rail (global English / CN locales). Empty on non-Windows and on
/// registry read errors. HoYoPlay's display name varies across
/// versions, so we tolerate "Star Rail" without the prefix as well.
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
            if !is_star_rail_display_name(&display) {
                continue;
            }
            let install_location: String = sub.get_value("InstallLocation").unwrap_or_default();
            if install_location.is_empty() {
                continue;
            }
            // HoYoPlay writes the launcher root; the playable game lives one
            // level deeper under `Game/`. Push both so `validate` can pick
            // whichever passes.
            let install = PathBuf::from(&install_location);
            out.push(install.join("Game"));
            out.push(install);
        }
    }
    out
}

#[cfg(not(windows))]
pub fn detect_from_registry() -> Vec<PathBuf> {
    Vec::new()
}

/// Match Hoyoverse's display-name conventions across global and CN
/// installer builds. Public so it can be unit-tested without spinning
/// up a registry. The matcher is intentionally generous — HoYoPlay
/// has shipped at least three different display strings over the
/// life of the game.
pub fn is_star_rail_display_name(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("star rail")
        || lower.contains("honkai: star rail")
        || lower.contains("honkaistarrail")
        || lower.contains("崩坏：星穹铁道")
        || lower.contains("崩坏星穹铁道")
        || lower.contains("崩壊：スターレイル")
}

/// Production orchestrator: registry → common paths. Returns the
/// first candidate that passes [`validate`].
pub fn detect() -> Option<PathBuf> {
    let mut chain = detect_from_registry();
    chain.extend(common_install_candidates());
    detect_from_paths(chain)
}
