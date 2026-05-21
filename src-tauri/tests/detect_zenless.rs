//! Slice 7 (#17): Auto-detect ZZMI (Zenless Zone Zero) install path.
//!
//! Same shape as `tests/detect_genshin.rs` and `tests/detect_star_rail.rs`.
//! The registry probe is exercised manually on a real Windows machine;
//! here we test the pure-Rust seams that the production detector
//! composes.

use std::fs;

use gmm_lib::core::detect::zenless;
use tempfile::TempDir;

fn make_install(root: &std::path::Path, exe_name: &str, include_data: bool) {
    fs::write(root.join(exe_name), b"MZ\x00\x00fakeexe").expect("write exe");
    if include_data {
        fs::create_dir_all(root.join(zenless::DATA_DIR_NAME)).expect("data dir");
        fs::write(
            root.join(zenless::DATA_DIR_NAME).join("globalgamemanagers"),
            b"unity gunk",
        )
        .expect("write data marker");
    }
}

#[test]
fn validate_accepts_a_real_looking_install() {
    let tmp = TempDir::new().expect("tmp");
    make_install(tmp.path(), "ZenlessZoneZero.exe", true);
    assert!(zenless::validate(tmp.path()));
}

#[test]
fn validate_rejects_missing_exe() {
    let tmp = TempDir::new().expect("tmp");
    fs::create_dir_all(tmp.path().join(zenless::DATA_DIR_NAME)).expect("data dir");
    assert!(!zenless::validate(tmp.path()));
}

#[test]
fn validate_rejects_missing_data_dir() {
    let tmp = TempDir::new().expect("tmp");
    fs::write(tmp.path().join("ZenlessZoneZero.exe"), b"MZ\x00\x00").expect("write exe");
    assert!(!zenless::validate(tmp.path()));
}

#[test]
fn validate_rejects_nonexistent_path() {
    let tmp = TempDir::new().expect("tmp");
    assert!(!zenless::validate(&tmp.path().join("does-not-exist")));
}

#[test]
fn detect_from_paths_returns_first_valid_candidate() {
    let tmp = TempDir::new().expect("tmp");
    let bad = tmp.path().join("bad");
    let good = tmp.path().join("good");
    fs::create_dir_all(&bad).expect("bad dir");
    fs::create_dir_all(&good).expect("good dir");
    make_install(&good, "ZenlessZoneZero.exe", true);

    let found = zenless::detect_from_paths([bad, good.clone()]);
    assert_eq!(found.as_deref(), Some(good.as_path()));
}

#[test]
fn common_candidates_lists_expected_hoyoplay_paths() {
    let candidates = zenless::common_install_candidates();
    let names: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();
    assert!(
        names.iter().any(|n| n.contains("Program Files")),
        "must include a Program Files candidate, got {names:?}",
    );
    assert!(
        names
            .iter()
            .any(|n| n.contains("Zenless Zone Zero") || n.contains("ZenlessZoneZero")),
        "every candidate ends in a ZZZ-flavoured dir name: {names:?}",
    );
    assert!(
        names.iter().any(|n| n.contains("HoYoPlay")),
        "must include a HoYoPlay candidate, got {names:?}",
    );
}

#[test]
fn display_name_matcher_handles_global_and_cn_strings() {
    assert!(zenless::is_zenless_display_name("Zenless Zone Zero"));
    assert!(zenless::is_zenless_display_name("ZenlessZoneZero"));
    assert!(zenless::is_zenless_display_name("绝区零"));
    assert!(zenless::is_zenless_display_name("ゼンレスゾーンゼロ"));
    // Negative: must NOT collide with other Hoyoverse titles.
    assert!(!zenless::is_zenless_display_name("Genshin Impact"));
    assert!(!zenless::is_zenless_display_name("Honkai: Star Rail"));
    assert!(!zenless::is_zenless_display_name("Honkai Impact 3rd"));
}
