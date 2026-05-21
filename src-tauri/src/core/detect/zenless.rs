//! Zenless Zone Zero (ZZMI) install detection (slice 7 / #17).
//!
//! Same Unity-engine layout as Genshin and Star Rail: HoYoPlay drops
//! `<install>/Zenless Zone Zero/Game/ZenlessZoneZero.exe` next to a
//! `ZenlessZoneZero_Data/` directory. The standalone installer puts
//! it under the user-picked drive root.
//!
//! Chain:
//!
//! 1. Windows uninstall registry (HKLM + WOW6432Node + HKCU) for
//!    HoYoPlay's `InstallLocation` whose display name matches the
//!    *Zenless Zone Zero* / CN / JP locale strings.
//! 2. Common HoYoPlay + drive-root paths.
//! 3. The cached path in the `games` table; once set, no detection
//!    re-runs.
//! 4. Manual picker fallback in the UI.
//!
//! Validation: `ZenlessZoneZero.exe` AND `ZenlessZoneZero_Data/`. The
//! Unity Data directory is the discriminator that stops the detector
//! from accepting a folder that happens to contain a renamed `.exe`.

use std::path::{Path, PathBuf};

/// ZZZ executable names. HoYoPlay shipped a single binary for both
/// global and CN clients; we keep the slice shaped like GIMI's so a
/// future per-locale binary can be added without churn.
pub const EXE_NAMES: &[&str] = &["ZenlessZoneZero.exe"];

/// The Unity `*_Data` directory dropped next to the exe.
pub const DATA_DIR_NAME: &str = "ZenlessZoneZero_Data";

/// `true` iff `path` is a directory with the executable AND the
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

/// "Probably installed here" paths. HoYoPlay's layout first, then the
/// legacy / drive-root standalone-installer locations.
pub fn common_install_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for program_files_var in ["ProgramFiles", "ProgramFiles(x86)"] {
        let pf = std::env::var_os(program_files_var)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(program_files_var_to_default(program_files_var)));
        out.push(pf.join("Zenless Zone Zero").join("Game"));
        out.push(pf.join("ZenlessZoneZero").join("Game"));
        out.push(
            pf.join("HoYoPlay")
                .join("games")
                .join("Zenless Zone Zero")
                .join("Game"),
        );
    }
    out.push(PathBuf::from(r"C:\Zenless Zone Zero\Game"));
    out.push(PathBuf::from(r"D:\Zenless Zone Zero\Game"));
    out.push(PathBuf::from(r"C:\ZenlessZoneZero\Game"));
    out.push(PathBuf::from(r"D:\ZenlessZoneZero\Game"));
    out.push(PathBuf::from(r"C:\ZZZ"));
    out.push(PathBuf::from(r"D:\ZZZ"));
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

/// Walk uninstall keys on Windows; return `InstallLocation` paths whose
/// display name matches a ZZZ variant. Empty on non-Windows or read
/// errors. Pushes both the launcher root and `<root>/Game/` so HoYoPlay
/// installs that record the launcher root still validate.
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
            if !is_zenless_display_name(&display) {
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

/// Display-name matcher across HoYoPlay locales. Public so unit tests
/// can hit it without a registry.
pub fn is_zenless_display_name(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("zenless zone zero")
        || lower.contains("zenlesszonezero")
        || lower.contains("绝区零")
        || lower.contains("ゼンレスゾーンゼロ")
}

/// Production orchestrator: registry → common paths. Returns the
/// first candidate that passes [`validate`].
pub fn detect() -> Option<PathBuf> {
    let mut chain = detect_from_registry();
    chain.extend(common_install_candidates());
    detect_from_paths(chain)
}
