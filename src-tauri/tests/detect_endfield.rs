//! Slice 10 (#20): Auto-detect EFMI (Arknights: Endfield) install path.
//!
//! UE5 game; same general shape as Wuthering Waves (#18). The
//! validator uses Unreal's `Content/` directory two levels above the
//! shipping exe as the discriminator.

use std::fs;
use std::path::Path;

use gmm_lib::core::detect::endfield;
use tempfile::TempDir;

/// Build a fake Endfield install tree under `root`. Returns the
/// playable `Endfield/Binaries/Win64/` path the detector should
/// accept.
fn make_install(root: &Path, exe: &str, include_content: bool) -> std::path::PathBuf {
    let project_dir = root.join("Endfield Game").join("Endfield");
    let win64 = project_dir.join("Binaries").join("Win64");
    fs::create_dir_all(&win64).expect("win64 dir");
    fs::write(win64.join(exe), b"MZ\x00\x00fakeexe").expect("write exe");
    if include_content {
        fs::create_dir_all(project_dir.join("Content").join("Paks")).expect("content/paks");
        fs::write(
            project_dir
                .join("Content")
                .join("Paks")
                .join("pakchunk0.pak"),
            b"unreal gunk",
        )
        .expect("write pak marker");
    }
    win64
}

#[test]
fn validate_accepts_a_real_looking_install() {
    let tmp = TempDir::new().expect("tmp");
    let win64 = make_install(tmp.path(), "Endfield-Win64-Shipping.exe", true);
    assert!(endfield::validate(&win64));
}

#[test]
fn validate_also_accepts_beta_endfield_exe() {
    let tmp = TempDir::new().expect("tmp");
    let win64 = make_install(tmp.path(), "Endfield.exe", true);
    assert!(endfield::validate(&win64));
}

#[test]
fn validate_rejects_missing_exe() {
    let tmp = TempDir::new().expect("tmp");
    let project_dir = tmp.path().join("Endfield Game").join("Endfield");
    let win64 = project_dir.join("Binaries").join("Win64");
    fs::create_dir_all(&win64).expect("dirs");
    fs::create_dir_all(project_dir.join("Content").join("Paks")).expect("content");
    assert!(!endfield::validate(&win64));
}

#[test]
fn validate_rejects_missing_content_tree() {
    let tmp = TempDir::new().expect("tmp");
    let win64 = make_install(tmp.path(), "Endfield-Win64-Shipping.exe", false);
    assert!(!endfield::validate(&win64));
}

#[test]
fn detect_from_paths_returns_first_valid_candidate() {
    let tmp = TempDir::new().expect("tmp");
    let bad_root = tmp.path().join("bad");
    let good_root = tmp.path().join("good");
    fs::create_dir_all(&bad_root).expect("bad dir");
    fs::create_dir_all(&good_root).expect("good dir");
    let good = make_install(&good_root, "Endfield-Win64-Shipping.exe", true);

    let found = endfield::detect_from_paths([bad_root.clone(), good.clone()]);
    assert_eq!(found.as_deref(), Some(good.as_path()));
}

#[test]
fn common_candidates_lists_expected_program_files_paths() {
    let candidates = endfield::common_install_candidates();
    let names: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();
    assert!(
        names.iter().any(|n| n.contains("Program Files")),
        "must include a Program Files candidate, got {names:?}",
    );
    assert!(
        names.iter().any(|n| n.contains("Endfield Game")),
        "every candidate descends through Endfield Game/Endfield/Binaries/Win64: {names:?}",
    );
    assert!(
        names.iter().any(|n| n.contains("Win64")),
        "every candidate ends in Win64: {names:?}",
    );
}

#[test]
fn display_name_matcher_handles_global_and_cn_strings() {
    assert!(endfield::is_endfield_display_name("Arknights: Endfield"));
    assert!(endfield::is_endfield_display_name("Endfield"));
    assert!(endfield::is_endfield_display_name("明日方舟：终末地"));
    assert!(endfield::is_endfield_display_name(
        "アークナイツ：エンドフィールド"
    ));
    // Negative: must NOT collide with any other supported title.
    assert!(!endfield::is_endfield_display_name("Genshin Impact"));
    assert!(!endfield::is_endfield_display_name("Honkai: Star Rail"));
    assert!(!endfield::is_endfield_display_name("Honkai Impact 3rd"));
    assert!(!endfield::is_endfield_display_name("Zenless Zone Zero"));
    assert!(!endfield::is_endfield_display_name("Wuthering Waves"));
}
