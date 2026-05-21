//! The Genshin install path the user picks (or auto-detects in a later
//! slice) must survive a restart — it's stored in the games table.

use std::fs;

use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

#[tokio::test]
async fn game_install_path_round_trips_through_the_db() {
    let tmp = TempDir::new().expect("tmp");
    let library_root = tmp.path().join("library");
    let game_path = tmp.path().join("game/Genshin Impact Game");
    fs::create_dir_all(&game_path).expect("game dir");

    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init");

    // Fresh DB: no path persisted yet.
    let initial = core
        .game_install_path(GameCode::Gimi)
        .await
        .expect("read empty");
    assert!(initial.is_none(), "no install path set on a fresh DB");

    // Set, then read back.
    core.set_game_install_path(GameCode::Gimi, &game_path)
        .await
        .expect("write");

    let after = core.game_install_path(GameCode::Gimi).await.expect("read");
    assert_eq!(after, Some(game_path.clone()));

    // Reopen the same DB to prove persistence (not just in-memory state).
    drop(core);
    let db_url2 = db_url.clone();
    let core2 = Core::new(tmp.path().join("library"), &db_url2)
        .await
        .expect("reopen");

    let reopened = core2
        .game_install_path(GameCode::Gimi)
        .await
        .expect("read after reopen");
    assert_eq!(reopened, Some(game_path));
}
