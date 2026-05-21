//! Tracer bullet: adopting a folder produces a Mod that round-trips through the Library and the DB.
//!
//! No junction yet (that comes in the next cycle). No Tauri runtime — exercises the pure-Rust
//! `core` module so the test runs identically on macOS and Windows.

use std::fs;

use gmm_lib::core::{Core, GameCode, Source};
use tempfile::TempDir;

#[tokio::test]
async fn adopt_folder_round_trip() {
    let tmp = TempDir::new().expect("tmp dir");
    let library_root = tmp.path().join("library");
    let fixture = tmp.path().join("fixture/HuTaoSkin");
    fs::create_dir_all(&fixture).expect("fixture dir");
    fs::write(
        fixture.join("merged.ini"),
        "[TextureOverride]\nhash=12345\n",
    )
    .expect("fixture ini");

    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root.clone(), &db_url)
        .await
        .expect("init core");

    let adopted = core
        .adopt_folder(GameCode::Gimi, &fixture, "Hu Tao Skin")
        .await
        .expect("adopt");

    assert_eq!(adopted.name, "Hu Tao Skin");
    assert_eq!(adopted.game, GameCode::Gimi);
    assert_eq!(adopted.source, Source::Manual);
    assert!(!adopted.enabled, "newly adopted mods start disabled");
    assert!(
        adopted.library_path.starts_with(&library_root),
        "mod files should live under the library root, got {:?}",
        adopted.library_path,
    );
    assert!(
        adopted.library_path.join("merged.ini").exists(),
        "the fixture's .ini should have been copied into the library",
    );

    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    assert_eq!(listed.len(), 1, "list should surface the adopted mod");
    assert_eq!(listed[0].id, adopted.id);
    assert_eq!(listed[0].name, "Hu Tao Skin");
}
