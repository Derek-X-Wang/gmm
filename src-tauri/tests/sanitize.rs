//! When a Mod's display name contains NTFS-reserved characters, enabling it
//! must still produce a valid junction directory under `<Game>/Mods/`.

use std::fs;

use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

#[tokio::test]
async fn reserved_chars_in_display_name_are_stripped_from_junction_dir() {
    let tmp = TempDir::new().expect("tmp dir");
    let library_root = tmp.path().join("library");
    let game_mods = tmp.path().join("Genshin/Mods");
    fs::create_dir_all(&game_mods).expect("game mods dir");

    let fixture = tmp.path().join("fixture/Skin");
    fs::create_dir_all(&fixture).expect("fixture dir");
    fs::write(fixture.join("merged.ini"), "").expect("fixture ini");

    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url)
        .await
        .expect("init core");

    // Every NTFS-reserved character in one go.
    let nasty = r#"Hu<Tao>:"|?*\/"#;
    let adopted = core
        .adopt_folder(GameCode::Gimi, &fixture, nasty)
        .await
        .expect("adopt");

    // Display name is preserved as-is for the UI.
    assert_eq!(adopted.name, nasty);

    core.set_enabled(&adopted.id, true, &game_mods)
        .await
        .expect("enable");

    // Junction dir name on disk must not contain any of the NTFS-reserved chars.
    let entries: Vec<_> = fs::read_dir(&game_mods)
        .expect("read game mods")
        .filter_map(Result::ok)
        .collect();
    assert_eq!(entries.len(), 1, "exactly one junction in Mods/");

    let on_disk = entries[0].file_name().into_string().expect("utf8 name");
    for bad in ['<', '>', ':', '"', '|', '?', '*', '\\', '/'] {
        assert!(
            !on_disk.contains(bad),
            "junction dir name {on_disk:?} must not contain {bad:?}",
        );
    }
    assert!(!on_disk.is_empty(), "junction dir name must not be empty");
}
