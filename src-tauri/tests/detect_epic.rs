//! Fixture files exercise the same Epic seams the six production chains use.

use std::fs;
use std::path::{Path, PathBuf};

use gmm_lib::core::detect::{
    self, endfield, genshin, honkai_impact, star_rail, wuthering, zenless,
};
use serde_json::{json, Value};
use tempfile::TempDir;

fn unity_install(path: &Path, exe: &str, data: &str) {
    fs::create_dir_all(path.join(data)).unwrap();
    fs::write(path.join(exe), b"fixture executable").unwrap();
}

fn manifest(store: &Path, name: &str, display: &str, install: &Path, launch: &str) {
    write_record(
        store,
        name,
        &json!({
            "DisplayName": display,
            "InstallLocation": install,
            "LaunchExecutable": launch,
            // Real Epic records contain other launcher-owned fields.
            "AppName": "fixture-app",
            "bIsIncompleteInstall": false,
        }),
    );
}

fn write_record(store: &Path, name: &str, value: &Value) {
    fs::create_dir_all(store).unwrap();
    fs::write(store.join(name), serde_json::to_vec(value).unwrap()).unwrap();
}

#[test]
fn recorded_paths_resolve_without_game_names_in_folder() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let install = tmp.path().join("arbitrary-epic-suffix-42");
    unity_install(&install, "GenshinImpact.exe", genshin::DATA_DIR_NAME);
    manifest(
        &store,
        "good.item",
        "Genshin Impact",
        &install,
        "GenshinImpact.exe",
    );
    assert_eq!(genshin::detect_from_epic(Some(&store)), vec![install]);
}

#[test]
fn executable_parent_resolves_a_deep_recorded_install() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let install = tmp.path().join("arbitrary-root");
    let playable = install.join("Client/Binaries/Win64");
    fs::create_dir_all(&playable).unwrap();
    fs::create_dir_all(install.join("Client/Content")).unwrap();
    fs::write(playable.join(wuthering::EXE_NAMES[0]), b"fixture").unwrap();
    manifest(
        &store,
        "good.item",
        "Wuthering Waves",
        &install,
        r"Client\Binaries\Win64\Client-Win64-Shipping.exe",
    );
    assert_eq!(wuthering::detect_from_epic(Some(&store)), vec![playable]);
}

#[test]
fn all_six_games_use_their_own_matcher_and_validator() {
    type EpicDetector = fn(Option<&Path>) -> Vec<PathBuf>;
    let games: [(&str, &str, Option<&str>, EpicDetector); 6] = [
        (
            "Genshin Impact",
            genshin::EXE_NAMES[0],
            Some(genshin::DATA_DIR_NAME),
            genshin::detect_from_epic,
        ),
        (
            "Honkai: Star Rail",
            star_rail::EXE_NAMES[0],
            Some(star_rail::DATA_DIR_NAME),
            star_rail::detect_from_epic,
        ),
        (
            "Zenless Zone Zero",
            zenless::EXE_NAMES[0],
            Some(zenless::DATA_DIR_NAME),
            zenless::detect_from_epic,
        ),
        (
            "Honkai Impact 3rd",
            honkai_impact::EXE_NAMES[0],
            Some(honkai_impact::DATA_DIR_NAME),
            honkai_impact::detect_from_epic,
        ),
        (
            "Wuthering Waves",
            wuthering::EXE_NAMES[0],
            None,
            wuthering::detect_from_epic,
        ),
        (
            "Arknights: Endfield",
            endfield::EXE_NAMES[0],
            None,
            endfield::detect_from_epic,
        ),
    ];
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let mut expected = Vec::new();
    for (index, (display, exe, data, _)) in games.iter().enumerate() {
        let install = tmp.path().join(format!("app-{index}-launcher-suffix"));
        let (playable, launch) = if let Some(data) = data {
            unity_install(&install, exe, data);
            (install.clone(), (*exe).to_owned())
        } else {
            let playable = install.join("Project/Binaries/Win64");
            fs::create_dir_all(&playable).unwrap();
            fs::create_dir_all(install.join("Project/Content")).unwrap();
            fs::write(playable.join(exe), b"fixture").unwrap();
            (playable, format!("Project/Binaries/Win64/{exe}"))
        };
        manifest(&store, &format!("{index}.item"), display, &install, &launch);
        expected.push(playable);
    }
    for ((_, _, _, detector), expected) in games.into_iter().zip(expected) {
        assert_eq!(detector(Some(&store)), vec![expected]);
    }
}

#[test]
fn recorded_path_without_matching_data_is_rejected() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("GenshinImpact.exe"), b"fixture").unwrap();
    let store = tmp.path().join("store");
    manifest(
        &store,
        "bad.item",
        "Genshin Impact",
        tmp.path(),
        "GenshinImpact.exe",
    );
    assert!(genshin::detect_from_epic(Some(&store)).is_empty());
}

#[test]
fn stale_recorded_location_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    manifest(
        &store,
        "stale.item",
        "Genshin Impact",
        &tmp.path().join("absent"),
        "GenshinImpact.exe",
    );
    assert!(genshin::detect_from_epic(Some(&store)).is_empty());
}

#[test]
fn bad_records_never_hide_a_good_record() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let install = tmp.path().join("install");
    unity_install(&install, "GenshinImpact.exe", genshin::DATA_DIR_NAME);
    manifest(
        &store,
        "z-good.item",
        "Genshin Impact",
        &install,
        "GenshinImpact.exe",
    );
    for (name, contents) in [
        ("a-malformed.item", "not json"),
        ("b-truncated.item", "{\"DisplayName\":"),
        ("c-schema.item", "[]"),
    ] {
        fs::write(store.join(name), contents).unwrap();
    }
    // A directory masquerading as an item cannot be read as a manifest.
    fs::create_dir(store.join("d-unreadable.item")).unwrap();
    fs::write(store.join("e-oversized.item"), vec![b' '; 1024 * 1024 + 1]).unwrap();
    assert_eq!(genshin::detect_from_epic(Some(&store)), vec![install]);
}

#[cfg(windows)]
#[test]
fn registry_source_uses_the_shared_one_level_expansion() {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};

    struct RegistryFixture(String);
    impl Drop for RegistryFixture {
        fn drop(&mut self) {
            RegKey::predef(HKEY_CURRENT_USER)
                .delete_subkey_all(&self.0)
                .expect("remove only the fixture uninstall key");
        }
    }

    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("install");
    let child = root.join("client-epic-suffix");
    unity_install(&child, "StarRail.exe", star_rail::DATA_DIR_NAME);
    let fixture = RegistryFixture(format!(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\gmm-detection-fixture-{}",
        ulid::Ulid::new()
    ));
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(&fixture.0)
        .unwrap();
    key.set_value("DisplayName", &"Honkai: Star Rail").unwrap();
    key.set_value("InstallLocation", &root.to_str().unwrap())
        .unwrap();
    assert!(star_rail::detect_from_registry().contains(&child));
    fs::remove_dir_all(&child).unwrap();
    let deep = root.join("one/two");
    unity_install(&deep, "StarRail.exe", star_rail::DATA_DIR_NAME);
    assert!(!star_rail::detect_from_registry().contains(&deep));
    drop(key);
    drop(fixture);
}

#[test]
fn missing_wrong_type_and_empty_fields_fail_closed() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    unity_install(tmp.path(), "GenshinImpact.exe", genshin::DATA_DIR_NAME);
    let good = json!({"DisplayName": "Genshin Impact", "InstallLocation": tmp.path(), "LaunchExecutable": "GenshinImpact.exe"});
    for field in ["DisplayName", "InstallLocation", "LaunchExecutable"] {
        for replacement in [
            None,
            Some(json!(null)),
            Some(json!(123)),
            Some(json!("")),
            Some(json!(" ")),
        ] {
            let mut record = good.clone();
            if let Some(value) = replacement {
                record[field] = value;
            } else {
                record.as_object_mut().unwrap().remove(field);
            }
            write_record(&store, "bad.item", &record);
            assert!(
                genshin::detect_from_epic(Some(&store)).is_empty(),
                "{record}"
            );
        }
    }
    for incomplete in [json!(true), json!("false"), json!(null)] {
        let mut record = good.clone();
        record["bIsIncompleteInstall"] = incomplete;
        write_record(&store, "bad.item", &record);
        assert!(genshin::detect_from_epic(Some(&store)).is_empty());
    }
}

#[test]
fn absent_store_and_store_that_is_a_file_are_empty() {
    let tmp = TempDir::new().unwrap();
    assert!(genshin::detect_from_epic(Some(&tmp.path().join("absent"))).is_empty());
    let file = tmp.path().join("file");
    fs::write(&file, b"not a directory").unwrap();
    assert!(genshin::detect_from_epic(Some(&file)).is_empty());
}

#[cfg(not(windows))]
#[test]
fn no_override_on_other_os_has_no_epic_source() {
    for detector in [
        genshin::detect_from_epic,
        star_rail::detect_from_epic,
        zenless::detect_from_epic,
        honkai_impact::detect_from_epic,
        wuthering::detect_from_epic,
        endfield::detect_from_epic,
    ] {
        assert!(detector(None).is_empty());
    }
}

#[test]
fn expansion_finds_an_arbitrarily_named_immediate_child() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let install = tmp.path().join("install");
    let child = install.join("client-with-epic-suffix");
    unity_install(&child, "StarRail.exe", star_rail::DATA_DIR_NAME);
    fs::write(install.join("launcher.exe"), b"fixture").unwrap();
    manifest(
        &store,
        "good.item",
        "Honkai: Star Rail",
        &install,
        "launcher.exe",
    );
    assert_eq!(
        star_rail::detect_from_epic(Some(&store)),
        vec![child.clone()]
    );
    assert_eq!(
        detect::validated_candidates([install], star_rail::validate),
        vec![child]
    );
}

#[test]
fn expansion_does_not_find_an_install_two_levels_down() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let install = tmp.path().join("install");
    unity_install(
        &install.join("one/two"),
        "StarRail.exe",
        star_rail::DATA_DIR_NAME,
    );
    fs::write(install.join("launcher.exe"), b"fixture").unwrap();
    manifest(
        &store,
        "deep.item",
        "Honkai: Star Rail",
        &install,
        "launcher.exe",
    );
    assert!(star_rail::detect_from_epic(Some(&store)).is_empty());
    assert!(detect::validated_candidates([install], star_rail::validate).is_empty());
}

#[test]
fn valid_parent_wins_and_candidates_are_deduplicated_in_order() {
    let tmp = TempDir::new().unwrap();
    let first = tmp.path().join("z-first");
    let second = tmp.path().join("a-second");
    unity_install(&first, "StarRail.exe", star_rail::DATA_DIR_NAME);
    unity_install(
        &first.join("nested"),
        "StarRail.exe",
        star_rail::DATA_DIR_NAME,
    );
    unity_install(&second, "StarRail.exe", star_rail::DATA_DIR_NAME);
    assert_eq!(
        detect::validated_candidates(
            [first.clone(), first.clone(), second.clone()],
            star_rail::validate
        ),
        vec![first.clone(), second]
    );
    let store = tmp.path().join("store");
    manifest(
        &store,
        "1.item",
        "Honkai: Star Rail",
        &first,
        "StarRail.exe",
    );
    manifest(
        &store,
        "2.item",
        "Honkai: Star Rail",
        &first,
        "StarRail.exe",
    );
    assert_eq!(star_rail::detect_from_epic(Some(&store)), vec![first]);
}

#[test]
fn immediate_children_are_sorted_and_not_emitted_twice() {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a");
    let z = tmp.path().join("z");
    unity_install(&z, "StarRail.exe", star_rail::DATA_DIR_NAME);
    unity_install(&a, "StarRail.exe", star_rail::DATA_DIR_NAME);
    assert_eq!(
        detect::validated_candidates([tmp.path().to_path_buf(), a.clone()], star_rail::validate),
        vec![a, z]
    );
}

#[test]
fn expansion_has_a_fixed_work_bound() {
    let tmp = TempDir::new().unwrap();
    for index in 0..(detect::MAX_CANDIDATE_CHILDREN + 10) {
        fs::create_dir(tmp.path().join(format!("child-{index}"))).unwrap();
    }
    fn any_child(path: &Path) -> bool {
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("child-")
    }
    assert_eq!(
        detect::validated_candidates([tmp.path().to_path_buf()], any_child).len(),
        detect::MAX_CANDIDATE_CHILDREN
    );
}

#[test]
fn an_explicit_later_candidate_can_expand_after_being_probed_as_a_child() {
    let tmp = TempDir::new().unwrap();
    let child = tmp.path().join("one");
    let playable = child.join("two");
    unity_install(&playable, "StarRail.exe", star_rail::DATA_DIR_NAME);
    assert_eq!(
        detect::validated_candidates([tmp.path().to_path_buf(), child], star_rail::validate),
        vec![playable]
    );
}

#[test]
fn unexpected_launch_paths_and_unrecorded_executables_are_rejected() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    unity_install(tmp.path(), "GenshinImpact.exe", genshin::DATA_DIR_NAME);
    for launch in [
        "../GenshinImpact.exe",
        "/GenshinImpact.exe",
        r"C:\GenshinImpact.exe",
        r"\GenshinImpact.exe",
        "./GenshinImpact.exe",
        "missing.exe",
        "nested//GenshinImpact.exe",
    ] {
        manifest(&store, "bad.item", "Genshin Impact", tmp.path(), launch);
        assert!(
            genshin::detect_from_epic(Some(&store)).is_empty(),
            "{launch}"
        );
    }
    manifest(
        &store,
        "bad.item",
        "Genshin Impact",
        Path::new("relative"),
        "GenshinImpact.exe",
    );
    assert!(genshin::detect_from_epic(Some(&store)).is_empty());
}

#[test]
fn only_item_records_are_consulted() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    unity_install(tmp.path(), "GenshinImpact.exe", genshin::DATA_DIR_NAME);
    manifest(
        &store,
        "ignored.json",
        "Genshin Impact",
        tmp.path(),
        "GenshinImpact.exe",
    );
    assert!(genshin::detect_from_epic(Some(&store)).is_empty());
    manifest(
        &store,
        "good.ITEM",
        "Genshin Impact",
        tmp.path(),
        "GenshinImpact.exe",
    );
    assert_eq!(
        genshin::detect_from_epic(Some(&store)),
        vec![tmp.path().to_path_buf()]
    );
}

#[cfg(unix)]
#[test]
fn unreadable_manifest_and_symlinked_child_do_not_block_valid_records() {
    use std::os::unix::fs::symlink;
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let install = tmp.path().join("install");
    unity_install(&install, "GenshinImpact.exe", genshin::DATA_DIR_NAME);
    manifest(
        &store,
        "good.item",
        "Genshin Impact",
        &install,
        "GenshinImpact.exe",
    );
    symlink("loop.item", store.join("loop.item")).unwrap();
    assert_eq!(
        genshin::detect_from_epic(Some(&store)),
        vec![install.clone()]
    );
    let parent = tmp.path().join("parent");
    fs::create_dir(&parent).unwrap();
    symlink(&install, parent.join("child")).unwrap();
    assert!(detect::validated_candidates([parent], genshin::validate).is_empty());
}
