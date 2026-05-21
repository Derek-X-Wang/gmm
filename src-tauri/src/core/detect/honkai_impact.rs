//! Honkai Impact 3rd (HIMI) install detection (slice 9 / #19).
//!
//! Same Unity-engine shape as Genshin / Star Rail / ZZZ. HoYoPlay's
//! canonical layout is `<install>/Honkai Impact 3rd Game/BH3.exe`
//! next to `BH3_Data/`. The CN client uses the same exe name; older
//! CN installs sometimes ship as `Bh3.exe` (lowercase `h`), which
//! Windows treats identically but a literal path-join does not.
//!
//! Quirks called out in the issue body (#19):
//!
//! - `process_start_method: str = 'Native'` in upstream XXMI — i.e.
//!   the loader spawns the game via `CreateProcess`, not
//!   `ShellExecute`. GMM's `launch_game` already uses
//!   `std::process::Command::new` (which underneath is
//!   `CreateProcessW`), so this is the default we want; no extra
//!   wiring required.
//! - XXMI also mutates a handful of `HKCU\Software\miHoYo\Honkai
//!   Impact 3\` registry keys before launch to force certain graphics
//!   settings. GMM intentionally does NOT do this in v1 — it is a
//!   user-account-touching side effect that belongs behind a
//!   per-game settings panel, not in the launch flow.
//! - HI3rd's mod scene is the smallest of the six; we keep the
//!   maintenance cost in proportion (no per-game branches in
//!   `commands.rs`; everything routes through `GameProfile`).
//!
//! Validation: a candidate passes iff `BH3.exe` is present AND the
//! Unity data directory `BH3_Data/` exists.

use std::path::{Path, PathBuf};

/// Honkai Impact 3rd executable names. We accept both casings of the
/// `H` since older builds shipped both `BH3.exe` and `Bh3.exe`; NTFS
/// is case-insensitive at lookup time but the candidate list is
/// queried by literal string before the FS gets a vote.
pub const EXE_NAMES: &[&str] = &["BH3.exe", "Bh3.exe"];

/// The Unity `*_Data` directory dropped next to the exe.
pub const DATA_DIR_NAME: &str = "BH3_Data";

/// `true` iff `path` is a directory containing the executable AND
/// the Unity data directory.
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

/// "The installer probably dropped it here" paths. HoYoPlay's
/// preferred `<root>/Honkai Impact 3rd Game/`, the legacy *Honkai
/// Impact 3rd* short-name path, and drive-root standalone-installer
/// locations.
pub fn common_install_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for program_files_var in ["ProgramFiles", "ProgramFiles(x86)"] {
        let pf = std::env::var_os(program_files_var)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(program_files_var_to_default(program_files_var)));
        out.push(pf.join("Honkai Impact 3rd").join("Game"));
        out.push(pf.join("Honkai Impact 3rd Game"));
        out.push(
            pf.join("HoYoPlay")
                .join("games")
                .join("Honkai Impact 3rd")
                .join("Game"),
        );
    }
    out.push(PathBuf::from(r"C:\Honkai Impact 3rd\Game"));
    out.push(PathBuf::from(r"D:\Honkai Impact 3rd\Game"));
    out.push(PathBuf::from(r"C:\BH3"));
    out.push(PathBuf::from(r"D:\BH3"));
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
/// `InstallLocation` paths whose display name matches HI3rd's global
/// or CN locale strings. HoYoPlay's display string is *Honkai Impact
/// 3rd*; the CN client uses *崩坏3*.
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
            if !is_honkai_impact_display_name(&display) {
                continue;
            }
            let install_location: String = sub.get_value("InstallLocation").unwrap_or_default();
            if install_location.is_empty() {
                continue;
            }
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

/// Match HI3rd's display-name conventions across global EN / CN / JP
/// HoYoPlay locales. Public so unit tests can hit it without spinning
/// up a registry. **Must not** match *Honkai: Star Rail* (SRMI) —
/// the substrings `honkai` would collide if we matched on that alone.
pub fn is_honkai_impact_display_name(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    // Reject Star Rail variants first so the substring match below
    // doesn't accidentally claim them.
    if lower.contains("star rail")
        || lower.contains("honkaistarrail")
        || lower.contains("崩坏：星穹铁道")
        || lower.contains("崩坏星穹铁道")
        || lower.contains("崩壊：スターレイル")
    {
        return false;
    }
    lower.contains("honkai impact 3rd")
        || lower.contains("honkai impact 3")
        || lower.contains("honkaiimpact3")
        || lower.contains("崩坏3")
        || lower.contains("崩坏3rd")
        || lower.contains("崩壊3rd")
}

/// Production orchestrator: registry → common paths. Returns the
/// first candidate that passes [`validate`].
pub fn detect() -> Option<PathBuf> {
    let mut chain = detect_from_registry();
    chain.extend(common_install_candidates());
    detect_from_paths(chain)
}
