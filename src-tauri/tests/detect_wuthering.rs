//! Slice 8 (#18): Auto-detect WWMI (Wuthering Waves) install path.
//!
//! Same shape as the Hoyoverse detectors but with an Unreal Engine
//! discriminator: the playable directory must sit two levels below a
//! Wuthering Waves `Content/` tree.

use std::fs;
use std::path::Path;

use gmm_lib::core::detect::wuthering;
use tempfile::TempDir;

/// Build a fake Wuthering Waves install tree under `root`. Returns
/// the playable `Client/Binaries/Win64/` path, which is what the
/// detector should accept.
fn make_install(root: &Path, exe: &str, include_content: bool) -> std::path::PathBuf {
    let client_dir = root.join("Wuthering Waves Game").join("Client");
    let win64 = client_dir.join("Binaries").join("Win64");
    fs::create_dir_all(&win64).expect("win64 dir");
    fs::write(win64.join(exe), b"MZ\x00\x00fakeexe").expect("write exe");
    if include_content {
        fs::create_dir_all(client_dir.join("Content").join("Paks")).expect("content/paks");
        fs::write(
            client_dir
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
    let win64 = make_install(tmp.path(), "Client-Win64-Shipping.exe", true);
    assert!(wuthering::validate(&win64));
}

#[test]
fn validate_rejects_missing_exe() {
    let tmp = TempDir::new().expect("tmp");
    // Create the directory tree but no exe.
    let client_dir = tmp.path().join("Wuthering Waves Game").join("Client");
    let win64 = client_dir.join("Binaries").join("Win64");
    fs::create_dir_all(&win64).expect("dirs");
    fs::create_dir_all(client_dir.join("Content").join("Paks")).expect("content");
    assert!(!wuthering::validate(&win64));
}

#[test]
fn validate_rejects_missing_content_tree() {
    let tmp = TempDir::new().expect("tmp");
    // Exe is present but no Unreal `Content/` tree exists; this is
    // the case where someone dropped a renamed exe in a random folder.
    let win64 = make_install(tmp.path(), "Client-Win64-Shipping.exe", false);
    assert!(!wuthering::validate(&win64));
}

#[test]
fn validate_rejects_nonexistent_path() {
    let tmp = TempDir::new().expect("tmp");
    assert!(!wuthering::validate(&tmp.path().join("does-not-exist")));
}

#[test]
fn detect_from_paths_returns_first_valid_candidate() {
    let tmp = TempDir::new().expect("tmp");
    let bad_root = tmp.path().join("bad");
    let good_root = tmp.path().join("good");
    fs::create_dir_all(&bad_root).expect("bad dir");
    fs::create_dir_all(&good_root).expect("good dir");
    let good = make_install(&good_root, "Client-Win64-Shipping.exe", true);

    let found = wuthering::detect_from_paths([bad_root.clone(), good.clone()]);
    assert_eq!(found.as_deref(), Some(good.as_path()));
}

#[test]
fn common_candidates_lists_expected_program_files_paths() {
    let candidates = wuthering::common_install_candidates();
    let names: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();
    assert!(
        names.iter().any(|n| n.contains("Program Files")),
        "must include a Program Files candidate, got {names:?}",
    );
    assert!(
        names.iter().any(|n| n.contains("Wuthering Waves Game")),
        "every candidate descends through Wuthering Waves Game/Client/Binaries/Win64: {names:?}",
    );
    assert!(
        names.iter().any(|n| n.contains("Win64")),
        "every candidate ends in Win64: {names:?}",
    );
}

#[test]
fn display_name_matcher_handles_global_and_cn_strings() {
    assert!(wuthering::is_wuthering_display_name("Wuthering Waves"));
    // CN locale
    assert!(wuthering::is_wuthering_display_name("鸣潮"));
    // Negative: must NOT collide with any Hoyoverse title.
    assert!(!wuthering::is_wuthering_display_name("Genshin Impact"));
    assert!(!wuthering::is_wuthering_display_name("Honkai: Star Rail"));
    assert!(!wuthering::is_wuthering_display_name("Zenless Zone Zero"));
}
