//! Enable creates a Junction pointing into the Library; disable removes it.
//! On macOS the Junction is realised as a symlink so the test runs on dev hosts;
//! on Windows the real `junction` crate creates an NTFS directory junction.

use std::fs;

use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

#[tokio::test]
async fn enable_creates_junction_disable_removes_it() {
    let tmp = TempDir::new().expect("tmp dir");
    let library_root = tmp.path().join("library");
    let game_mods = tmp.path().join("Genshin/Mods");
    fs::create_dir_all(&game_mods).expect("game mods dir");

    let fixture = tmp.path().join("fixture/Mod1");
    fs::create_dir_all(&fixture).expect("fixture dir");
    fs::write(fixture.join("merged.ini"), "[TextureOverride]\nhash=42\n").expect("fixture ini");

    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root.clone(), &db_url)
        .await
        .expect("init core");

    let adopted = core
        .adopt_folder(GameCode::Gimi, &fixture, "Mod 1")
        .await
        .expect("adopt");

    // Enable -> junction exists, resolves into the library
    core.set_enabled(&adopted.id, true, &game_mods)
        .await
        .expect("enable");

    let link = game_mods.join("Mod 1");
    assert!(link.exists(), "junction should exist after enable");
    assert!(
        link.join("merged.ini").exists(),
        "junction should resolve into the library copy",
    );

    let listed = core
        .list_mods(GameCode::Gimi)
        .await
        .expect("list after enable");
    assert!(listed[0].enabled, "list_mods should reflect enabled=true");

    // Disable -> junction gone, library copy untouched
    core.set_enabled(&adopted.id, false, &game_mods)
        .await
        .expect("disable");

    assert!(!link.exists(), "junction should be removed after disable");
    assert!(
        adopted.library_path.join("merged.ini").exists(),
        "library copy must survive disable (junctions never own the files)",
    );

    let listed = core
        .list_mods(GameCode::Gimi)
        .await
        .expect("list after disable");
    assert!(!listed[0].enabled, "list_mods should reflect enabled=false");
}
