//! Registry-backed game detection.
//!
//! `detect::<game>::detect_from_registry()` walks the Windows uninstall
//! keys looking for a matching `DisplayName` and returns its
//! `InstallLocation`. Every game ships one of these and until now none
//! were exercised — the path-scan half was covered by fixtures, the
//! registry half was dead code as far as tests were concerned.
//!
//! These tests write to **HKCU** (`HKEY_CURRENT_USER`), which needs no
//! elevation and is per-user, so a CI runner and a developer machine
//! behave identically. Keys are removed in a guard that runs even if the
//! test panics.
//!
//! Windows-only; the whole file compiles away elsewhere.

#![cfg(windows)]

use std::fs;
use std::path::Path;

use gmm_lib::core::detect;
use tempfile::TempDir;
use winreg::enums::*;
use winreg::RegKey;

const UNINSTALL_PATH: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

/// Open the per-user uninstall root, **creating it if absent**.
///
/// Only the HKLM copy of this key is guaranteed to exist. A fresh
/// Windows image — a CI runner, or any machine where no per-user
/// installer has ever run — has no HKCU copy, and opening it fails with
/// ERROR_FILE_NOT_FOUND. The product handles that correctly
/// (`detect_from_registry` skips roots it cannot open); it was only
/// this test that assumed the key was already there.
fn uninstall_root() -> RegKey {
    RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(UNINSTALL_PATH)
        .expect("open-or-create HKCU uninstall root")
        .0
}

/// Creates an uninstall entry under HKCU and deletes it on drop, so a
/// panicking assertion can't leave residue in the runner's registry.
struct UninstallEntry {
    key_name: String,
}

impl UninstallEntry {
    fn new(key_name: &str, display_name: &str, install_location: &Path) -> Self {
        let uninstall = uninstall_root();
        let (key, _) = uninstall
            .create_subkey(key_name)
            .expect("create fake uninstall entry");
        key.set_value("DisplayName", &display_name)
            .expect("set DisplayName");
        key.set_value(
            "InstallLocation",
            &install_location.to_string_lossy().to_string(),
        )
        .expect("set InstallLocation");
        Self {
            key_name: key_name.to_string(),
        }
    }
}

impl Drop for UninstallEntry {
    fn drop(&mut self) {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(uninstall) = hkcu.open_subkey_with_flags(UNINSTALL_PATH, KEY_ALL_ACCESS) {
            let _ = uninstall.delete_subkey_all(&self.key_name);
        }
    }
}

fn make_game_dir(tmp: &TempDir, folder: &str, exe: &str, data_dir: &str) -> std::path::PathBuf {
    let dir = tmp.path().join(folder);
    fs::create_dir_all(dir.join(data_dir)).expect("data dir");
    fs::write(dir.join(exe), b"MZ").expect("exe stub");
    dir
}

#[test]
fn genshin_detect_from_registry_finds_a_matching_uninstall_entry() {
    let tmp = TempDir::new().expect("tmp");
    let game = make_game_dir(
        &tmp,
        "Genshin Impact Game",
        "GenshinImpact.exe",
        "GenshinImpact_Data",
    );

    let _guard = UninstallEntry::new("GMM-TEST-Genshin", "Genshin Impact", &game);

    let found = detect::genshin::detect_from_registry();
    assert!(
        found.iter().any(|p| p == &game),
        "registry scan should surface {game:?}, got {found:?}",
    );
}

#[test]
fn genshin_registry_entry_with_a_foreign_display_name_is_ignored() {
    let tmp = TempDir::new().expect("tmp");
    let game = make_game_dir(
        &tmp,
        "Some Other Game",
        "GenshinImpact.exe",
        "GenshinImpact_Data",
    );

    // Right shape on disk, wrong DisplayName — must not match.
    let _guard = UninstallEntry::new("GMM-TEST-NotGenshin", "Microsoft Edge Update", &game);

    let found = detect::genshin::detect_from_registry();
    assert!(
        !found.iter().any(|p| p == &game),
        "a non-Genshin DisplayName must not be returned, got {found:?}",
    );
}

#[test]
fn genshin_registry_entry_with_empty_install_location_is_skipped() {
    let uninstall = uninstall_root();
    let (key, _) = uninstall
        .create_subkey("GMM-TEST-GenshinEmptyLocation")
        .expect("create entry");
    key.set_value("DisplayName", &"Genshin Impact")
        .expect("set DisplayName");
    key.set_value("InstallLocation", &"")
        .expect("empty location");

    let found = detect::genshin::detect_from_registry();

    let _ = uninstall.delete_subkey_all("GMM-TEST-GenshinEmptyLocation");

    assert!(
        !found.iter().any(|p| p.as_os_str().is_empty()),
        "an empty InstallLocation must never be returned as a candidate, got {found:?}",
    );
}

/// The end-to-end registry path: an entry exists, points at a directory
/// that passes validation, and the top-level `detect()` returns it.
#[test]
fn genshin_detect_prefers_a_valid_registry_hit() {
    let tmp = TempDir::new().expect("tmp");
    let game = make_game_dir(
        &tmp,
        "Genshin Impact Game",
        "GenshinImpact.exe",
        "GenshinImpact_Data",
    );

    let _guard = UninstallEntry::new("GMM-TEST-GenshinValid", "Genshin Impact", &game);

    // Sanity: the fixture is a shape the validator accepts, otherwise
    // this test would pass for the wrong reason.
    assert!(detect::genshin::validate(&game));

    match detect::genshin::detect() {
        Some(found) => assert_eq!(
            found, game,
            "detect() should return the registry hit when no real install is present",
        ),
        None => panic!("detect() returned None despite a valid registry entry"),
    }
}

/// Registry scanning must not panic or hang when the uninstall tree
/// contains entries with missing values — real machines are full of
/// half-written keys.
#[test]
fn registry_scan_tolerates_entries_missing_values() {
    let uninstall = uninstall_root();
    let (_key, _) = uninstall
        .create_subkey("GMM-TEST-Malformed")
        .expect("create empty entry");

    // No DisplayName, no InstallLocation — every detector must survive.
    let _ = detect::genshin::detect_from_registry();
    let _ = detect::star_rail::detect_from_registry();
    let _ = detect::zenless::detect_from_registry();
    let _ = detect::wuthering::detect_from_registry();
    let _ = detect::honkai_impact::detect_from_registry();
    let _ = detect::endfield::detect_from_registry();

    let _ = uninstall.delete_subkey_all("GMM-TEST-Malformed");
}
