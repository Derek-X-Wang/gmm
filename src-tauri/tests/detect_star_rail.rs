//! Slice 6 (#16): Auto-detect SRMI (Honkai: Star Rail) install path.
//!
//! Same shape as `tests/detect_genshin.rs`. The registry probe is
//! exercised manually on a real Windows machine; here we test the
//! pure-Rust seams that the production detector composes.

use std::fs;

use gmm_lib::core::detect::star_rail;
use tempfile::TempDir;

fn make_install(root: &std::path::Path, exe_name: &str, include_data: bool) {
    fs::write(root.join(exe_name), b"MZ\x00\x00fakeexe").expect("write exe");
    if include_data {
        fs::create_dir_all(root.join(star_rail::DATA_DIR_NAME)).expect("data dir");
        fs::write(
            root.join(star_rail::DATA_DIR_NAME)
                .join("globalgamemanagers"),
            b"unity gunk",
        )
        .expect("write data marker");
    }
}

#[test]
fn validate_accepts_a_real_looking_install() {
    let tmp = TempDir::new().expect("tmp");
    make_install(tmp.path(), "StarRail.exe", true);
    assert!(star_rail::validate(tmp.path()));
}

#[test]
fn validate_rejects_missing_exe() {
    let tmp = TempDir::new().expect("tmp");
    // Data dir present but no exe — common shape if the user pointed at
    // a screenshots folder by mistake.
    fs::create_dir_all(tmp.path().join(star_rail::DATA_DIR_NAME)).expect("data dir");
    assert!(!star_rail::validate(tmp.path()));
}

#[test]
fn validate_rejects_missing_data_dir() {
    let tmp = TempDir::new().expect("tmp");
    fs::write(tmp.path().join("StarRail.exe"), b"MZ\x00\x00").expect("write exe");
    assert!(!star_rail::validate(tmp.path()));
}

#[test]
fn validate_rejects_nonexistent_path() {
    let tmp = TempDir::new().expect("tmp");
    assert!(!star_rail::validate(&tmp.path().join("does-not-exist")));
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
    make_install(&good, "StarRail.exe", true);
    make_install(&also_good, "StarRail.exe", true);

    let found = star_rail::detect_from_paths([bad.clone(), good.clone(), also_good.clone()]);
    assert_eq!(found.as_deref(), Some(good.as_path()));
}

#[test]
fn detect_from_paths_returns_none_when_no_candidates_match() {
    let tmp = TempDir::new().expect("tmp");
    let only_one = tmp.path().join("only_one");
    fs::create_dir_all(&only_one).expect("dir");

    assert_eq!(star_rail::detect_from_paths([only_one]), None);
}

#[test]
fn common_candidates_lists_expected_program_files_paths() {
    let candidates = star_rail::common_install_candidates();
    let names: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();
    assert!(
        names.iter().any(|n| n.contains("Program Files")),
        "must include a Program Files candidate, got {names:?}",
    );
    assert!(
        names
            .iter()
            .any(|n| n.contains("Star Rail") || n.contains("StarRail")),
        "every candidate ends in a Star Rail-flavoured dir name: {names:?}",
    );
    // HoYoPlay (the multi-game launcher) writes installs under a
    // `HoYoPlay/games/Star Rail/Game/` layout; assert at least one
    // candidate covers it.
    assert!(
        names.iter().any(|n| n.contains("HoYoPlay")),
        "must include a HoYoPlay candidate, got {names:?}",
    );
}

#[test]
fn display_name_matcher_handles_global_and_cn_strings() {
    // Global English (HoYoPlay's canonical string)
    assert!(star_rail::is_star_rail_display_name("Honkai: Star Rail"));
    // Older HoYoPlay variant
    assert!(star_rail::is_star_rail_display_name("Star Rail"));
    // CN locale
    assert!(star_rail::is_star_rail_display_name("崩坏：星穹铁道"));
    // JP locale (some HoYoPlay regional installs)
    assert!(star_rail::is_star_rail_display_name("崩壊：スターレイル"));
    // Negative: must NOT match Genshin display name
    assert!(!star_rail::is_star_rail_display_name("Genshin Impact"));
    // Negative: must NOT match Honkai Impact 3rd (different game; HIMI)
    assert!(!star_rail::is_star_rail_display_name("Honkai Impact 3rd"));
}
