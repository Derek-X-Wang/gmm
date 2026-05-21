//! Slice 12: hash-conflict detection.
//!
//! Two layers of coverage:
//!
//! 1. The pure parser exercised with synthetic INI strings — section
//!    matching, comments, the `if 0`/`endif` skip rule, both override
//!    section flavours.
//! 2. The Core orchestration: two enabled mods that bind the same hash
//!    surface as a Conflict; disabling one drops the conflict.

use std::fs;

use gmm_lib::core::conflicts::{
    build_report, extract_hashes_from_dir, extract_hashes_from_str, HashBinding,
};
use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

#[test]
fn parser_extracts_hashes_from_texture_override_section() {
    let ini = r#"
; comment at top of file
[TextureOverridePlayer]
hash = 0xABCDEF12
match_priority = 0
"#;
    let bindings = extract_hashes_from_str(ini);
    assert_eq!(
        bindings,
        vec![HashBinding {
            hash: "abcdef12".into(),
            section: "TextureOverridePlayer".into(),
        }]
    );
}

#[test]
fn parser_supports_resource_override_sections() {
    let ini = r#"
[ResourceOverrideHair]
hash = 0x11112222
"#;
    let bindings = extract_hashes_from_str(ini);
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].section, "ResourceOverrideHair");
}

#[test]
fn parser_skips_hashes_inside_if_zero_block() {
    let ini = r#"
[TextureOverrideA]
if 0
hash = 0xDEADBEEF
endif
hash = 0xCAFEBABE
"#;
    let bindings = extract_hashes_from_str(ini);
    let hashes: Vec<_> = bindings.iter().map(|b| b.hash.as_str()).collect();
    assert_eq!(
        hashes,
        vec!["cafebabe"],
        "hash inside `if 0` block must be skipped",
    );
}

#[test]
fn parser_keeps_hashes_inside_truthy_if_block() {
    // Conservative: anything we cannot evaluate stays live.
    let ini = r#"
[TextureOverrideB]
if $active == 1
hash = 0x11111111
endif
"#;
    let bindings = extract_hashes_from_str(ini);
    let hashes: Vec<_> = bindings.iter().map(|b| b.hash.as_str()).collect();
    assert_eq!(hashes, vec!["11111111"]);
}

#[test]
fn parser_ignores_non_override_sections() {
    let ini = r#"
[Constants]
hash = 0xnope
[ShaderRegex]
hash = 0xalso_nope
"#;
    let bindings = extract_hashes_from_str(ini);
    assert!(bindings.is_empty());
}

#[test]
fn dir_scan_walks_recursively() {
    let tmp = TempDir::new().expect("tmp");
    fs::create_dir_all(tmp.path().join("a")).expect("a");
    fs::create_dir_all(tmp.path().join("b/c")).expect("b/c");
    fs::write(
        tmp.path().join("a/one.ini"),
        b"[TextureOverrideA]\nhash = 0x1\n",
    )
    .expect("a/one.ini");
    fs::write(
        tmp.path().join("b/c/two.ini"),
        b"[TextureOverrideB]\nhash = 0x2\n",
    )
    .expect("b/c/two.ini");
    let bindings = extract_hashes_from_dir(tmp.path()).expect("dir scan");
    let mut hashes: Vec<_> = bindings.iter().map(|b| b.hash.clone()).collect();
    hashes.sort();
    assert_eq!(hashes, vec!["1".to_string(), "2".to_string()]);
}

#[test]
fn build_report_flags_only_shared_hashes() {
    let bindings = vec![
        (
            "mod-alpha".to_string(),
            vec![
                HashBinding {
                    hash: "aaaa".into(),
                    section: "TextureOverrideA".into(),
                },
                HashBinding {
                    hash: "bbbb".into(),
                    section: "TextureOverrideA".into(),
                },
            ],
        ),
        (
            "mod-beta".to_string(),
            vec![HashBinding {
                hash: "aaaa".into(),
                section: "TextureOverrideB".into(),
            }],
        ),
    ];
    let report = build_report(&bindings);
    assert_eq!(report.conflicts.len(), 1, "only aaaa is shared");
    assert_eq!(report.conflicts[0].hash, "aaaa");
    let mut mods = report.conflicts[0].mod_ids.clone();
    mods.sort();
    assert_eq!(mods, vec!["mod-alpha", "mod-beta"]);
    assert_eq!(report.per_mod_count["mod-alpha"], 1);
    assert_eq!(report.per_mod_count["mod-beta"], 1);
}

#[tokio::test]
async fn enabled_mods_sharing_a_hash_surface_as_conflict() {
    let tmp = TempDir::new().expect("tmp");
    let library_root = tmp.path().join("library");
    let game_install = tmp.path().join("Genshin");
    let game_mods = game_install.join("Mods");
    fs::create_dir_all(&game_mods).expect("mods dir");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init");
    core.set_game_install_path(GameCode::Gimi, &game_install)
        .await
        .expect("install");

    // Adopt two mods that share hash 0xC0DE.
    let fixture_a = tmp.path().join("fixture_a");
    let fixture_b = tmp.path().join("fixture_b");
    for (dir, label) in [(&fixture_a, "A"), (&fixture_b, "B")] {
        fs::create_dir_all(dir).expect("fix dir");
        fs::write(
            dir.join("merged.ini"),
            format!("[TextureOverride{label}]\nhash = 0xC0DE\n").as_bytes(),
        )
        .expect("ini");
    }
    let mod_a = core
        .adopt_folder(GameCode::Gimi, &fixture_a, "Alpha Mod")
        .await
        .expect("adopt a");
    let mod_b = core
        .adopt_folder(GameCode::Gimi, &fixture_b, "Beta Mod")
        .await
        .expect("adopt b");
    core.set_enabled(&mod_a.id, true, &game_mods)
        .await
        .expect("enable a");
    core.set_enabled(&mod_b.id, true, &game_mods)
        .await
        .expect("enable b");

    let report = core.detect_conflicts(GameCode::Gimi).await.expect("detect");
    assert_eq!(
        report.conflicts.len(),
        1,
        "shared hash surfaced: {report:?}"
    );
    assert_eq!(report.conflicts[0].hash, "c0de");
    assert_eq!(report.per_mod_count[&mod_a.id], 1);
    assert_eq!(report.per_mod_count[&mod_b.id], 1);

    // Disable B → conflict goes away.
    core.set_enabled(&mod_b.id, false, &game_mods)
        .await
        .expect("disable b");
    let report = core
        .detect_conflicts(GameCode::Gimi)
        .await
        .expect("detect2");
    assert!(
        report.conflicts.is_empty(),
        "disabling B clears it: {report:?}"
    );
}
