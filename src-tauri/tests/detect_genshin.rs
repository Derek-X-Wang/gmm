//! Slice 2: Auto-detect Genshin install path.
//!
//! Covers the validation rules (`GenshinImpact.exe`/`YuanShen.exe` plus
//! `GenshinImpact_Data` directory) and the orchestration of candidate
//! paths into [`detect_from_paths`]. The registry probe is exercised
//! manually on a real Windows machine; here we test the pure-Rust
//! seams that the production detector composes.

use std::fs;

use gmm_lib::core::detect::genshin;
use tempfile::TempDir;

fn make_install(root: &std::path::Path, exe_name: &str, include_data: bool) {
    fs::write(root.join(exe_name), b"MZ\x00\x00fakeexe").expect("write exe");
    if include_data {
        fs::create_dir_all(root.join("GenshinImpact_Data")).expect("data dir");
        fs::write(
            root.join("GenshinImpact_Data").join("globalgamemanagers"),
            b"unity gunk",
        )
        .expect("write data marker");
    }
}

#[test]
fn validate_accepts_a_real_looking_install() {
    let tmp = TempDir::new().expect("tmp");
    make_install(tmp.path(), "GenshinImpact.exe", true);
    assert!(genshin::validate(tmp.path()));
}

#[test]
fn validate_also_accepts_yuanshen_exe() {
    let tmp = TempDir::new().expect("tmp");
    make_install(tmp.path(), "YuanShen.exe", true);
    assert!(genshin::validate(tmp.path()));
}

#[test]
fn validate_rejects_missing_exe() {
    let tmp = TempDir::new().expect("tmp");
    // Data dir present but no exe — common shape if the user pointed at
    // a screenshots folder by mistake.
    fs::create_dir_all(tmp.path().join("GenshinImpact_Data")).expect("data dir");
    assert!(!genshin::validate(tmp.path()));
}

#[test]
fn validate_rejects_missing_data_dir() {
    let tmp = TempDir::new().expect("tmp");
    fs::write(tmp.path().join("GenshinImpact.exe"), b"MZ\x00\x00").expect("write exe");
    assert!(!genshin::validate(tmp.path()));
}

#[test]
fn validate_rejects_nonexistent_path() {
    let tmp = TempDir::new().expect("tmp");
    assert!(!genshin::validate(&tmp.path().join("does-not-exist")));
}

#[test]
fn detect_from_paths_returns_first_valid_candidate() {
    let tmp = TempDir::new().expect("tmp");
    let bad = tmp.path().join("bad");
    let good = tmp.path().join("good");
    let also_good = tmp.path().join("also_good");
    fs::create_dir_all(&bad).expect("bad dir");
    fs::create_dir_all(&good).expect("good dir");
    fs::create_dir_all(&also_good).expect("also_good dir");
    make_install(&good, "GenshinImpact.exe", true);
    make_install(&also_good, "YuanShen.exe", true);

    let found = genshin::detect_from_paths([bad.clone(), good.clone(), also_good.clone()]);
    assert_eq!(found.as_deref(), Some(good.as_path()));
}

#[test]
fn detect_from_paths_returns_none_when_no_candidates_match() {
    let tmp = TempDir::new().expect("tmp");
    let only_one = tmp.path().join("only_one");
    fs::create_dir_all(&only_one).expect("dir");

    assert_eq!(genshin::detect_from_paths([only_one]), None);
}

#[test]
fn common_candidates_lists_expected_program_files_paths() {
    let candidates = genshin::common_install_candidates();
    // We deliberately include the Program Files install location used by
    // HoYoPlay/HoYoLab installs as well as the standalone-installer
    // C:\Genshin and D:\Genshin paths called out in the acceptance criteria.
    let names: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();
    assert!(
        names.iter().any(|n| n.contains("Program Files")),
        "must include a Program Files candidate, got {names:?}",
    );
    assert!(
        names.iter().any(|n| n.contains("Genshin")),
        "every candidate ends in a Genshin-flavoured dir name: {names:?}",
    );
}
