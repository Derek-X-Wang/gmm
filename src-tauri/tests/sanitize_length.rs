//! NTFS allows filenames up to 255 UTF-16 code units, but the full path
//! under `<Game>/Mods/<dir>/` plus the mod's files needs to stay well
//! within MAX_PATH-friendly bounds for legacy tooling. Cap the on-disk
//! junction dir name regardless of how long the display name is.

use std::fs;

use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

#[tokio::test]
async fn very_long_display_name_truncates_to_safe_length() {
    let tmp = TempDir::new().expect("tmp");
    let library_root = tmp.path().join("library");
    let game_mods = tmp.path().join("Genshin/Mods");
    fs::create_dir_all(&game_mods).expect("game mods dir");

    let fixture = tmp.path().join("fixture/X");
    fs::create_dir_all(&fixture).expect("fixture dir");
    fs::write(fixture.join("merged.ini"), "").expect("fixture ini");

    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init core");

    let nasty: String = "A".repeat(400);
    let adopted = core
        .adopt_folder(GameCode::Gimi, &fixture, &nasty)
        .await
        .expect("adopt");

    assert_eq!(adopted.name, nasty, "display name preserved verbatim");

    core.set_enabled(&adopted.id, true, &game_mods)
        .await
        .expect("enable");

    let entries: Vec<_> = fs::read_dir(&game_mods)
        .expect("read mods")
        .filter_map(Result::ok)
        .collect();
    assert_eq!(entries.len(), 1);

    let on_disk = entries[0].file_name().into_string().expect("utf8 name");
    assert!(
        on_disk.chars().count() <= 200,
        "junction dir name should be capped at 200 chars, got {}",
        on_disk.chars().count(),
    );
    assert!(!on_disk.is_empty());
}
