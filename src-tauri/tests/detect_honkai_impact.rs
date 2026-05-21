//! Slice 9 (#19): Auto-detect HIMI (Honkai Impact 3rd) install path.
//!
//! Same shape as the other Hoyoverse detectors. Notable: the
//! display-name matcher must NOT collide with Star Rail (SRMI),
//! which shares the *Honkai* prefix in CN locales.

use std::fs;

use gmm_lib::core::detect::honkai_impact;
use tempfile::TempDir;

fn make_install(root: &std::path::Path, exe_name: &str, include_data: bool) {
    fs::write(root.join(exe_name), b"MZ\x00\x00fakeexe").expect("write exe");
    if include_data {
        fs::create_dir_all(root.join(honkai_impact::DATA_DIR_NAME)).expect("data dir");
        fs::write(
            root.join(honkai_impact::DATA_DIR_NAME)
                .join("globalgamemanagers"),
            b"unity gunk",
        )
        .expect("write data marker");
    }
}

#[test]
fn validate_accepts_a_real_looking_install() {
    let tmp = TempDir::new().expect("tmp");
    make_install(tmp.path(), "BH3.exe", true);
    assert!(honkai_impact::validate(tmp.path()));
}

#[test]
fn validate_also_accepts_lowercase_bh3_exe() {
    let tmp = TempDir::new().expect("tmp");
    make_install(tmp.path(), "Bh3.exe", true);
    assert!(honkai_impact::validate(tmp.path()));
}

#[test]
fn validate_rejects_missing_exe() {
    let tmp = TempDir::new().expect("tmp");
    fs::create_dir_all(tmp.path().join(honkai_impact::DATA_DIR_NAME)).expect("data dir");
    assert!(!honkai_impact::validate(tmp.path()));
}

#[test]
fn validate_rejects_missing_data_dir() {
    let tmp = TempDir::new().expect("tmp");
    fs::write(tmp.path().join("BH3.exe"), b"MZ\x00\x00").expect("write exe");
    assert!(!honkai_impact::validate(tmp.path()));
}

#[test]
fn validate_rejects_nonexistent_path() {
    let tmp = TempDir::new().expect("tmp");
    assert!(!honkai_impact::validate(&tmp.path().join("does-not-exist")));
}

#[test]
fn detect_from_paths_returns_first_valid_candidate() {
    let tmp = TempDir::new().expect("tmp");
    let bad = tmp.path().join("bad");
    let good = tmp.path().join("good");
    fs::create_dir_all(&bad).expect("bad dir");
    fs::create_dir_all(&good).expect("good dir");
    make_install(&good, "BH3.exe", true);

    let found = honkai_impact::detect_from_paths([bad, good.clone()]);
    assert_eq!(found.as_deref(), Some(good.as_path()));
}

#[test]
fn common_candidates_lists_expected_program_files_paths() {
    let candidates = honkai_impact::common_install_candidates();
    let names: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();
    assert!(
        names.iter().any(|n| n.contains("Program Files")),
        "must include a Program Files candidate, got {names:?}",
    );
    assert!(
        names.iter().any(|n| n.contains("Honkai Impact 3rd")),
        "every candidate ends in a HI3rd-flavoured dir name: {names:?}",
    );
    assert!(
        names.iter().any(|n| n.contains("HoYoPlay")),
        "must include a HoYoPlay candidate, got {names:?}",
    );
}

#[test]
fn display_name_matcher_handles_global_and_cn_strings() {
    assert!(honkai_impact::is_honkai_impact_display_name(
        "Honkai Impact 3rd"
    ));
    assert!(honkai_impact::is_honkai_impact_display_name(
        "HonkaiImpact3"
    ));
    assert!(honkai_impact::is_honkai_impact_display_name("崩坏3"));
    assert!(honkai_impact::is_honkai_impact_display_name("崩壊3rd"));
    // Negative: must NOT match Star Rail (closest collision risk —
    // both contain the *Honkai* prefix in CN).
    assert!(!honkai_impact::is_honkai_impact_display_name(
        "Honkai: Star Rail"
    ));
    assert!(!honkai_impact::is_honkai_impact_display_name(
        "崩坏：星穹铁道"
    ));
    // Negative: other Hoyoverse + Kuro titles.
    assert!(!honkai_impact::is_honkai_impact_display_name(
        "Genshin Impact"
    ));
    assert!(!honkai_impact::is_honkai_impact_display_name(
        "Zenless Zone Zero"
    ));
    assert!(!honkai_impact::is_honkai_impact_display_name(
        "Wuthering Waves"
    ));
}
