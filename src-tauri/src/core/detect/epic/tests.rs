use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use tempfile::TempDir;

use crate::core::detect::{self, endfield, genshin, honkai_impact, star_rail, wuthering, zenless};

type Detector = fn(Option<&Path>) -> Vec<PathBuf>;

struct Fixture {
    _temp: TempDir,
    store: PathBuf,
    root: PathBuf,
    playable: PathBuf,
}

fn observed_fixture(display: &str, suffix: &str, exe: &str, data: Option<&str>) -> Fixture {
    let temp = TempDir::new().unwrap();
    let store = temp.path().join("store");
    let root = temp.path().join("epic-install-variable-suffix");
    let playable = root.join(suffix);
    fs::create_dir_all(&playable).unwrap();
    fs::write(playable.join(exe), b"fixture game executable").unwrap();
    let data = match data {
        Some(data) => playable.join(data),
        None => playable.parent().unwrap().parent().unwrap().join("Content"),
    };
    fs::create_dir_all(data).unwrap();
    fs::write(root.join("launcher_epic.exe"), b"fixture launcher shim").unwrap();
    fs::create_dir(&store).unwrap();
    fs::write(
        store.join("00000000000000000000000000000000.item"),
        serde_json::to_vec(&json!({
            "DisplayName": display,
            "InstallLocation": root,
            "LaunchExecutable": "launcher_epic.exe",
            "bIsIncompleteInstall": false,
        }))
        .unwrap(),
    )
    .unwrap();
    Fixture {
        _temp: temp,
        store,
        root,
        playable,
    }
}

fn assert_playable(fixture: Fixture, detector: Detector, validate: fn(&Path) -> bool) {
    assert!(!validate(&fixture.root));
    assert!(validate(&fixture.playable));
    assert_eq!(detector(Some(&fixture.store)), vec![fixture.playable]);
}

fn set_launch_executable(fixture: &Fixture, launch: &str) {
    let record = fixture.store.join("00000000000000000000000000000000.item");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    value["LaunchExecutable"] = json!(launch);
    fs::write(record, serde_json::to_vec(&value).unwrap()).unwrap();
}

#[cfg(any(unix, windows))]
fn link_directory(fixture: &Fixture, directory: &Path) {
    let target = fixture._temp.path().join("relocated");
    fs::rename(directory, &target).unwrap();
    // Real NTFS junction on Windows; directory symlink on Unix.
    crate::core::junction::create(directory, &target).unwrap();
}

#[cfg(any(unix, windows))]
#[test]
fn epic_linked_install_root_preserves_genshin_immediate_child() {
    let fixture = observed_fixture(
        "Genshin Impact",
        "Game",
        genshin::EXE_NAMES[0],
        Some(genshin::DATA_DIR_NAME),
    );
    link_directory(&fixture, &fixture.root);
    assert_playable(fixture, genshin::detect_from_epic, genshin::validate);
}

#[cfg(any(unix, windows))]
#[test]
fn epic_linked_install_root_preserves_valid_genshin_root() {
    let fixture = observed_fixture(
        "Genshin Impact",
        "",
        genshin::EXE_NAMES[0],
        Some(genshin::DATA_DIR_NAME),
    );
    link_directory(&fixture, &fixture.root);
    assert!(genshin::validate(&fixture.root));
    assert_eq!(
        genshin::detect_from_epic(Some(&fixture.store)),
        vec![fixture.root]
    );
}

#[cfg(any(unix, windows))]
#[test]
fn epic_linked_launch_parent_preserves_star_rail_directory() {
    let fixture = observed_fixture(
        "Honkai: Star Rail",
        "Game",
        "StarRail.exe",
        Some(star_rail::DATA_DIR_NAME),
    );
    set_launch_executable(&fixture, "Game/StarRail.exe");
    link_directory(&fixture, &fixture.playable);
    assert_playable(fixture, star_rail::detect_from_epic, star_rail::validate);
}

#[test]
fn epic_valid_immediate_child_precedes_valid_suffix() {
    let fixture = observed_fixture(
        "Honkai: Star Rail",
        "games/Star Rail Games",
        star_rail::EXE_NAMES[0],
        Some(star_rail::DATA_DIR_NAME),
    );
    let child = fixture.root.join("Game");
    fs::create_dir_all(child.join(star_rail::DATA_DIR_NAME)).unwrap();
    fs::write(child.join(star_rail::EXE_NAMES[0]), b"child executable").unwrap();
    assert!(!star_rail::validate(&fixture.root));
    assert!(star_rail::validate(&child));
    assert!(star_rail::validate(&fixture.playable));
    assert_eq!(
        star_rail::detect_from_epic(Some(&fixture.store)),
        vec![child, fixture.playable]
    );
}

#[test]
fn epic_valid_root_excludes_valid_suffix() {
    let fixture = observed_fixture(
        "Honkai: Star Rail",
        "games/Star Rail Games",
        star_rail::EXE_NAMES[0],
        Some(star_rail::DATA_DIR_NAME),
    );
    fs::create_dir(fixture.root.join(star_rail::DATA_DIR_NAME)).unwrap();
    fs::write(
        fixture.root.join(star_rail::EXE_NAMES[0]),
        b"root executable",
    )
    .unwrap();
    assert!(star_rail::validate(&fixture.root));
    assert!(star_rail::validate(&fixture.playable));
    assert_eq!(
        star_rail::detect_from_epic(Some(&fixture.store)),
        vec![fixture.root]
    );
}

#[test]
fn epic_wuthering_launch_parent_and_install_suffix_emit_once() {
    let fixture = observed_fixture(
        "Wuthering Waves",
        "Wuthering Waves Game/Client/Binaries/Win64",
        "Client-Win64-Shipping.exe",
        None,
    );
    set_launch_executable(
        &fixture,
        "Wuthering Waves Game/Client/Binaries/Win64/Client-Win64-Shipping.exe",
    );
    assert_playable(fixture, wuthering::detect_from_epic, wuthering::validate);
}

#[test]
fn epic_observed_star_rail_returns_exact_playable_directory() {
    let display = "《崩坏：星穹铁道》";
    assert!(star_rail::is_star_rail_display_name(display));
    assert_playable(
        observed_fixture(
            display,
            "games/Star Rail Games",
            star_rail::EXE_NAMES[0],
            Some(star_rail::DATA_DIR_NAME),
        ),
        star_rail::detect_from_epic,
        star_rail::validate,
    );
}

#[test]
fn epic_observed_zenless_returns_exact_playable_directory() {
    let display = "绝区零";
    assert!(zenless::is_zenless_display_name(display));
    assert_playable(
        observed_fixture(
            display,
            "games/ZenlessZoneZero Game",
            zenless::EXE_NAMES[0],
            Some(zenless::DATA_DIR_NAME),
        ),
        zenless::detect_from_epic,
        zenless::validate,
    );
}

#[test]
fn epic_observed_wuthering_returns_exact_playable_directory() {
    let display = "鸣潮";
    assert!(wuthering::is_wuthering_display_name(display));
    assert_playable(
        observed_fixture(
            display,
            "Wuthering Waves Game/Client/Binaries/Win64",
            wuthering::EXE_NAMES[0],
            None,
        ),
        wuthering::detect_from_epic,
        wuthering::validate,
    );
}

#[test]
fn unknown_epic_layout_is_rejected_and_other_games_still_use_non_epic_paths() {
    let fixture = observed_fixture(
        "《崩坏：星穹铁道》",
        "unknown/deep/layout",
        star_rail::EXE_NAMES[0],
        Some(star_rail::DATA_DIR_NAME),
    );
    assert!(star_rail::detect_from_epic(Some(&fixture.store)).is_empty());
    type PathDetector = fn(Vec<PathBuf>) -> Option<PathBuf>;
    let games: [(Detector, PathDetector, &str, Option<&str>); 5] = [
        (
            genshin::detect_from_epic,
            genshin::detect_from_paths,
            genshin::EXE_NAMES[0],
            Some(genshin::DATA_DIR_NAME),
        ),
        (
            zenless::detect_from_epic,
            zenless::detect_from_paths,
            zenless::EXE_NAMES[0],
            Some(zenless::DATA_DIR_NAME),
        ),
        (
            honkai_impact::detect_from_epic,
            honkai_impact::detect_from_paths,
            honkai_impact::EXE_NAMES[0],
            Some(honkai_impact::DATA_DIR_NAME),
        ),
        (
            wuthering::detect_from_epic,
            wuthering::detect_from_paths,
            wuthering::EXE_NAMES[0],
            None,
        ),
        (
            endfield::detect_from_epic,
            endfield::detect_from_paths,
            endfield::EXE_NAMES[0],
            None,
        ),
    ];
    for (index, (epic, from_paths, exe, data)) in games.into_iter().enumerate() {
        let root = fixture._temp.path().join(format!("non-epic-{index}"));
        let playable = if let Some(data) = data {
            fs::create_dir_all(root.join(data)).unwrap();
            root
        } else {
            fs::create_dir_all(root.join("Client/Content")).unwrap();
            root.join("Client/Binaries/Win64")
        };
        fs::create_dir_all(&playable).unwrap();
        fs::write(playable.join(exe), b"non-Epic executable").unwrap();
        let mut chain = epic(Some(&fixture.store));
        assert!(chain.is_empty());
        chain.push(playable.clone());
        assert_eq!(from_paths(chain), Some(playable));
    }
}

#[test]
fn known_suffix_does_not_accept_an_incomplete_install_or_missing_data() {
    let fixture = observed_fixture(
        "绝区零",
        "games/ZenlessZoneZero Game",
        zenless::EXE_NAMES[0],
        Some(zenless::DATA_DIR_NAME),
    );
    let record = fixture.store.join("00000000000000000000000000000000.item");
    let original = fs::read(&record).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    value["bIsIncompleteInstall"] = json!(true);
    fs::write(&record, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(zenless::detect_from_epic(Some(&fixture.store)).is_empty());
    fs::write(&record, original).unwrap();
    fs::remove_dir(fixture.playable.join(zenless::DATA_DIR_NAME)).unwrap();
    assert!(zenless::detect_from_epic(Some(&fixture.store)).is_empty());
}

#[test]
fn suffix_expansion_has_a_linear_bound_and_deduplicates_roots() {
    use std::cell::Cell;
    thread_local! { static CALLS: Cell<usize> = const { Cell::new(0) }; }
    fn reject(_: &Path) -> bool {
        CALLS.with(|calls| calls.set(calls.get() + 1));
        false
    }
    let fixture = observed_fixture(
        "鸣潮",
        "Wuthering Waves Game/Client/Binaries/Win64",
        wuthering::EXE_NAMES[0],
        None,
    );
    for index in 0..(detect::MAX_CANDIDATE_CHILDREN + 10) {
        fs::create_dir(fixture.root.join(format!("unrelated-{index}"))).unwrap();
    }
    CALLS.with(|calls| calls.set(0));
    assert!(super::expanded_candidates(
        vec![fixture.root.clone(), fixture.root.clone()],
        reject,
        Some("Wuthering Waves Game/Client/Binaries/Win64")
    )
    .is_empty());
    let calls = CALLS.with(Cell::get);
    // The shim file may consume one of the 256 inspected entries.
    assert!(
        (detect::MAX_CANDIDATE_CHILDREN + 1..=detect::MAX_CANDIDATE_CHILDREN + 2).contains(&calls)
    );
    // The known suffix is probed directly even when the root has more entries
    // than the enumeration limit; no deeper read_dir is needed.
    assert_eq!(
        wuthering::detect_from_epic(Some(&fixture.store)),
        vec![fixture.playable]
    );
}

#[test]
fn suffix_expansion_rejects_traversal_and_excess_depth() {
    let fixture = observed_fixture(
        "绝区零",
        "one/two/three/four/five",
        zenless::EXE_NAMES[0],
        Some(zenless::DATA_DIR_NAME),
    );
    assert!(super::expanded_candidates(
        vec![fixture.root.clone()],
        zenless::validate,
        Some("one/two/three/four/five")
    )
    .is_empty());
    let outside = fixture._temp.path().join("outside");
    fs::create_dir_all(outside.join(zenless::DATA_DIR_NAME)).unwrap();
    fs::write(outside.join(zenless::EXE_NAMES[0]), b"outside executable").unwrap();
    assert!(super::expanded_candidates(
        vec![fixture.root.clone()],
        zenless::validate,
        Some("../outside")
    )
    .is_empty());
}

#[cfg(any(unix, windows))]
#[test]
fn suffix_expansion_never_follows_directory_symlinks() {
    for linked_depth in 0..=4 {
        let fixture = observed_fixture(
            "鸣潮",
            "Wuthering Waves Game/Client/Binaries/Win64",
            wuthering::EXE_NAMES[0],
            None,
        );
        let mut linked = fixture.root.clone();
        for part in ["Wuthering Waves Game", "Client", "Binaries", "Win64"]
            .into_iter()
            .take(linked_depth)
        {
            linked.push(part);
        }
        let target = fixture._temp.path().join("relocated");
        fs::rename(&linked, &target).unwrap();
        // Real NTFS junction on Windows; directory symlink on Unix.
        crate::core::junction::create(&linked, &target).unwrap();
        assert!(
            wuthering::detect_from_epic(Some(&fixture.store)).is_empty(),
            "linked depth {linked_depth}"
        );
    }
}
