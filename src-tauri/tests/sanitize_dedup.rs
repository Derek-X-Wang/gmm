//! Two mods adopted with the same display name must produce distinct
//! junction directory names — otherwise the UNIQUE (game_code,
//! junction_dir_name) constraint would reject the second insert.

use std::fs;

use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

#[tokio::test]
async fn duplicate_display_names_get_dedup_suffix() {
    let tmp = TempDir::new().expect("tmp dir");
    let library_root = tmp.path().join("library");
    let game_mods = tmp.path().join("Genshin/Mods");
    fs::create_dir_all(&game_mods).expect("game mods dir");

    let make_fixture = |name: &str| {
        let p = tmp.path().join("fixtures").join(name);
        fs::create_dir_all(&p).expect("fixture dir");
        fs::write(p.join("merged.ini"), "").expect("fixture ini");
        p
    };

    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init core");

    let first = make_fixture("a");
    let second = make_fixture("b");

    let m1 = core
        .adopt_folder(GameCode::Gimi, &first, "Hu Tao Skin")
        .await
        .expect("first adopt");
    let m2 = core
        .adopt_folder(GameCode::Gimi, &second, "Hu Tao Skin")
        .await
        .expect("second adopt — must not collide with the first");

    assert_ne!(m1.id, m2.id);

    core.set_enabled(&m1.id, true, &game_mods)
        .await
        .expect("enable m1");
    core.set_enabled(&m2.id, true, &game_mods)
        .await
        .expect("enable m2");

    let mut names: Vec<String> = fs::read_dir(&game_mods)
        .expect("read mods dir")
        .filter_map(Result::ok)
        .map(|e| e.file_name().into_string().expect("utf8 name"))
        .collect();
    names.sort();

    assert_eq!(names.len(), 2, "two distinct junction dirs expected");
    assert_eq!(names[0], "Hu Tao Skin");
    assert_eq!(names[1], "Hu Tao Skin (2)");
}
