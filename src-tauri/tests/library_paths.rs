//! Slice 15: Library path settings.
//!
//! Two flows:
//!   * Set the global Library root → every game's subtree relocates.
//!   * Set a per-game override → only that game relocates, the others
//!     stay put.
//!
//! Junctions are recreated on the new root for any mod that was
//! enabled before the move.

use std::fs;

use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

async fn make_mod(
    core: &Core,
    game: GameCode,
    name: &str,
    fixture_root: &std::path::Path,
) -> gmm_lib::core::Mod {
    fs::create_dir_all(fixture_root).expect("fixture dir");
    fs::write(
        fixture_root.join("merged.ini"),
        b"[TextureOverride]\nhash=1\n" as &[u8],
    )
    .expect("ini");
    core.adopt_folder(game, fixture_root, name)
        .await
        .expect("adopt")
}

#[tokio::test]
async fn changing_global_root_relocates_every_mod_and_rebuilds_junctions() {
    let tmp = TempDir::new().expect("tmp");
    let library_default = tmp.path().join("default_library");
    let library_new = tmp.path().join("relocated_library");
    let game_install = tmp.path().join("Genshin");
    let game_mods = game_install.join("Mods");
    fs::create_dir_all(&game_mods).expect("mods dir");

    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_default.clone(), &db_url)
        .await
        .expect("init core");
    core.set_game_install_path(GameCode::Gimi, &game_install)
        .await
        .expect("install path");

    let mod_a = make_mod(
        &core,
        GameCode::Gimi,
        "Mod A",
        &tmp.path().join("fixture_a"),
    )
    .await;
    let mod_b = make_mod(
        &core,
        GameCode::Gimi,
        "Mod B",
        &tmp.path().join("fixture_b"),
    )
    .await;
    core.set_enabled(&mod_a.id, true, &game_mods)
        .await
        .expect("enable A");
    core.set_enabled(&mod_b.id, true, &game_mods)
        .await
        .expect("enable B");

    let report = core
        .set_library_root(Some(&library_new))
        .await
        .expect("set root");

    assert_eq!(report.relocated.len(), 2, "both mods relocated: {report:?}");
    assert!(
        library_new.join("gimi").exists(),
        "new gimi subtree present"
    );
    assert!(
        !library_default.join("gimi").join(&mod_a.id).exists(),
        "old per-mod path is gone",
    );

    // Junctions point into the new Library.
    let link_a = game_mods.join("Mod A");
    let link_b = game_mods.join("Mod B");
    assert!(link_a.exists() && link_a.join("merged.ini").exists());
    assert!(link_b.exists() && link_b.join("merged.ini").exists());

    let resolved = core.resolved_library_root().await.expect("resolved");
    assert_eq!(resolved, library_new);
}

#[tokio::test]
async fn per_game_override_relocates_only_that_game() {
    let tmp = TempDir::new().expect("tmp");
    let library_default = tmp.path().join("default_library");
    let gimi_override = tmp.path().join("gimi_drive");
    let gimi_install = tmp.path().join("Genshin");
    let gimi_mods = gimi_install.join("Mods");
    let srmi_install = tmp.path().join("StarRail");
    let srmi_mods = srmi_install.join("Mods");
    fs::create_dir_all(&gimi_mods).expect("gimi mods dir");
    fs::create_dir_all(&srmi_mods).expect("srmi mods dir");

    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_default.clone(), &db_url)
        .await
        .expect("init core");
    core.set_game_install_path(GameCode::Gimi, &gimi_install)
        .await
        .expect("gimi install");
    core.set_game_install_path(GameCode::Srmi, &srmi_install)
        .await
        .expect("srmi install");

    let gimi_mod = make_mod(&core, GameCode::Gimi, "Genshin Mod", &tmp.path().join("g")).await;
    let srmi_mod = make_mod(
        &core,
        GameCode::Srmi,
        "Star Rail Mod",
        &tmp.path().join("s"),
    )
    .await;
    core.set_enabled(&gimi_mod.id, true, &gimi_mods)
        .await
        .expect("enable gimi");
    core.set_enabled(&srmi_mod.id, true, &srmi_mods)
        .await
        .expect("enable srmi");

    let srmi_old_path = library_default.join("srmi");
    assert!(
        srmi_old_path.exists(),
        "precondition: srmi subtree exists at default"
    );

    let report = core
        .set_library_path_for_game(GameCode::Gimi, Some(&gimi_override))
        .await
        .expect("set gimi override");

    assert_eq!(
        report.relocated.len(),
        1,
        "only the gimi mod relocates: {report:?}"
    );

    // Genshin junction now resolves into the override directory.
    let gimi_link = gimi_mods.join("Genshin Mod");
    assert!(gimi_link.exists() && gimi_link.join("merged.ini").exists());

    // Star Rail subtree untouched.
    assert!(
        srmi_old_path.exists(),
        "srmi must not be moved by a gimi-only override"
    );
    let srmi_link = srmi_mods.join("Star Rail Mod");
    assert!(srmi_link.exists() && srmi_link.join("merged.ini").exists());

    // Resolvers see the new gimi path but unchanged global default.
    assert_eq!(
        core.resolved_library_root_for(GameCode::Gimi)
            .await
            .unwrap(),
        gimi_override,
    );
    assert_eq!(
        core.resolved_library_root_for(GameCode::Srmi)
            .await
            .unwrap(),
        library_default.join("srmi"),
    );
}
