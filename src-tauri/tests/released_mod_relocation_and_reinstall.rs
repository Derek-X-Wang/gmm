//! Relocation and reinstall must not trust a released Mod's flag (#473).
//!
//! #462 added a release exit for an enable/disable GMM cannot finish: the user
//! confirms, GMM gives up its claim, and the Mod shows Unknown. That exit also
//! lifted the global Library lockout, which made two operations reachable
//! against a `mods.enabled` value GMM has just said it cannot trust: moving the
//! Library and the GameBanana reinstall swap. Each refuses for the unsettled
//! Mod and says how to settle it. A Mod settled since its release is unchanged,
//! and an unsettled Mod elsewhere does not block an unrelated Game.

use std::io::Write as _;
use std::path::PathBuf;

use gmm_lib::core::gamebanana::Endpoints;
use gmm_lib::core::{Core, Error, GameCode, Mod};
use sqlx::SqlitePool;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;

const GAMEBANANA_ID: u64 = 777_001;

struct TestEnv {
    tmp: TempDir,
    db_url: String,
    library: PathBuf,
    game_mods: PathBuf,
}

impl TestEnv {
    fn new() -> Self {
        let tmp = TempDir::new().expect("temporary directory");
        let data = tmp.path().join("data");
        let library = data.join("library");
        let game_mods = tmp.path().join("Genshin/Mods");
        std::fs::create_dir_all(&data).expect("data directory");
        std::fs::create_dir_all(&game_mods).expect("game Mods directory");
        Self {
            db_url: format!("sqlite://{}/gmm.db?mode=rwc", data.display()),
            tmp,
            library,
            game_mods,
        }
    }

    async fn core(&self) -> Core {
        Core::new(self.library.clone(), &self.db_url)
            .await
            .expect("open Core")
    }

    async fn pool(&self) -> SqlitePool {
        SqlitePool::connect(&self.db_url)
            .await
            .expect("open direct database connection")
    }

    fn new_root(&self) -> PathBuf {
        self.tmp.path().join("relocated-library")
    }
}

fn mod_zip(ini: &[u8]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buffer);
        zip.start_file("merged.ini", SimpleFileOptions::default())
            .expect("start ini");
        zip.write_all(ini).expect("write ini");
        zip.finish().expect("finish zip");
    }
    buffer.into_inner()
}

/// Serve one GameBanana submission at the given version and ZIP body.
async fn serve(version: &str, body: Vec<u8>) -> (mockito::ServerGuard, Endpoints) {
    let mut server = mockito::Server::new_async().await;
    let file_path = format!("/file/{GAMEBANANA_ID}/mod.zip");
    server
        .mock(
            "GET",
            mockito::Matcher::Regex(format!("/apiv11/Mod/{GAMEBANANA_ID}.*")),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "_idRow": {GAMEBANANA_ID}, "_sName": "Released Mod",
                "_sProfileUrl": "https://gamebanana.com/mods/{GAMEBANANA_ID}",
                "_sVersion": "{version}",
                "_aSubmitter": {{ "_sName": "Author" }},
                "_aPreviewMedia": {{ "_aImages": [] }},
                "_aFiles": [{{ "_sFile": "mod.zip", "_sDownloadUrl": "{base}{file_path}" }}]
            }}"#,
            base = server.url(),
        ))
        .create_async()
        .await;
    server
        .mock("GET", file_path.as_str())
        .with_status(200)
        .with_body(body)
        .create_async()
        .await;
    let endpoints = Endpoints {
        api_base: server.url(),
    };
    (server, endpoints)
}

async fn listed(core: &Core, mod_id: &str) -> Mod {
    core.list_mods(GameCode::Gimi)
        .await
        .expect("list Mods")
        .into_iter()
        .find(|candidate| candidate.id == mod_id)
        .expect("find Mod")
}

async fn scalar(pool: &SqlitePool, sql: &str, mod_id: &str) -> String {
    sqlx::query_scalar::<_, String>(sql)
        .bind(mod_id)
        .fetch_one(pool)
        .await
        .expect("read a Mod column")
}

async fn count(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(pool)
        .await
        .expect("count rows")
}

/// A GameBanana Mod whose interrupted enable GMM could not finish and the user
/// then released. The stored flag is set to enabled so that acting on it would
/// visibly rewire the deployment.
async fn released_mod(env: &TestEnv) -> (Core, Mod, Endpoints, mockito::ServerGuard) {
    let core = env.core().await;
    let (server, endpoints) = serve("1.0.0", mod_zip(b"[TextureOverride]\nhash=1\n")).await;
    let imported = core
        .import_gamebanana_with_endpoints(GameCode::Gimi, &GAMEBANANA_ID.to_string(), &endpoints)
        .await
        .expect("import the Mod");

    let pool = env.pool().await;
    sqlx::query(
        "CREATE TRIGGER obstruct_enabled_update
         BEFORE UPDATE OF enabled ON mods
         BEGIN SELECT RAISE(ABORT, 'enabled update obstruction'); END",
    )
    .execute(&pool)
    .await
    .expect("install obstruction");
    core.set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect_err("the obstruction leaves the transition unresolved");
    sqlx::query("DROP TRIGGER obstruct_enabled_update")
        .execute(&pool)
        .await
        .expect("remove obstruction");
    pool.close().await;
    drop(core);

    // Make the recorded Junction parent unmatchable so recovery cannot finish.
    let parked = env.game_mods.with_file_name("Mods-parked");
    std::fs::rename(&env.game_mods, &parked).expect("park the Mods directory");
    std::fs::create_dir(&env.game_mods).expect("recreate the Mods directory");

    let core = env.core().await;
    core.release_enabled_transition(&imported.id, true)
        .await
        .expect("release after the user confirms");
    assert!(
        listed(&core, &imported.id)
            .await
            .enabled_transition_recovery
            .expect("the released Mod keeps its record")
            .released
    );

    let pool = env.pool().await;
    sqlx::query("UPDATE mods SET enabled = 1 WHERE id = ?")
        .bind(&imported.id)
        .execute(&pool)
        .await
        .expect("store an untrustworthy enabled flag");
    pool.close().await;
    (core, imported, endpoints, server)
}

fn assert_names_the_mod_and_how_to_settle(error: &Error, mod_id: &str) {
    assert!(
        matches!(error, Error::EnabledStateUnknown { mod_id: named } if named == mod_id),
        "expected a refusal naming the unsettled Mod: {error:?}",
    );
    let message = error.to_string();
    assert!(message.contains(mod_id), "{message}");
    assert!(message.contains("Enable or disable the Mod"), "{message}");
}

/// Mutation oracle: removing the `enabled_state_is_unknown` check in
/// `move_root` lets the move run and leaves the first assertion red.
#[tokio::test]
async fn relocating_the_library_refuses_a_released_mod_and_moves_nothing() {
    let env = TestEnv::new();
    let (core, released, _endpoints, _server) = released_mod(&env).await;
    let before = listed(&core, &released.id).await;

    let error = core
        .set_library_root(Some(&env.new_root()))
        .await
        .expect_err("the Library must not move under an untrusted flag");

    assert_names_the_mod_and_how_to_settle(&error, &released.id);
    assert!(
        PathBuf::from(&before.library_path)
            .join("merged.ini")
            .is_file(),
        "the Mod's bytes must not move"
    );
    assert!(
        !env.new_root().exists(),
        "nothing was created at the target"
    );
    let pool = env.pool().await;
    assert_eq!(
        scalar(
            &pool,
            "SELECT library_path FROM mods WHERE id = ?",
            &released.id
        )
        .await,
        before.library_path
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM enabled_transitions WHERE released_at IS NOT NULL"
        )
        .await,
        1,
        "the Mod stays released, not silently settled",
    );
}

#[tokio::test]
async fn a_per_game_move_refuses_for_the_released_mods_game_but_not_another() {
    let env = TestEnv::new();
    let (core, released, _endpoints, _server) = released_mod(&env).await;
    let other_game_root = env.tmp.path().join("srmi-library");

    // The released Mod is a Genshin Impact (Gimi) Mod. Moving Star Rail's
    // Library touches none of its bytes, so it must go ahead: an uncertainty
    // about one Mod must not hold the Library hostage.
    core.set_library_path_for_game(GameCode::Srmi, Some(&other_game_root))
        .await
        .expect("a Game with no unsettled Mod can still be relocated");

    let error = core
        .set_library_path_for_game(GameCode::Gimi, Some(&env.new_root()))
        .await
        .expect_err("the released Mod's own Game must refuse");
    assert_names_the_mod_and_how_to_settle(&error, &released.id);
    assert!(!env.new_root().exists());
}

/// Mutation oracle: removing the `enabled_state_is_unknown` check from the
/// reinstall commit step lets the swap commit and leaves the error assertion
/// red.
#[tokio::test]
async fn reinstalling_refuses_a_released_mod_and_rolls_back_to_the_old_tree() {
    let env = TestEnv::new();
    let (core, released, _old_endpoints, _old_server) = released_mod(&env).await;
    let before = listed(&core, &released.id).await;
    let (_server, endpoints) = serve("2.0.0", mod_zip(b"[TextureOverride]\nhash=2\n")).await;

    let error = core
        .reinstall_gamebanana_mod_with_endpoints(&released.id, &endpoints)
        .await
        .expect_err("the swap must not act on an untrusted flag");

    assert_names_the_mod_and_how_to_settle(&error, &released.id);
    let installed = PathBuf::from(&before.library_path);
    assert_eq!(
        std::fs::read(installed.join("merged.ini")).expect("old bytes"),
        b"[TextureOverride]\nhash=1\n",
        "the installed tree is the old one"
    );
    let after = listed(&core, &released.id).await;
    assert_eq!(after.version.as_deref(), Some("1.0.0"));
    assert!(
        after
            .enabled_transition_recovery
            .expect("still released")
            .released
    );
    let pool = env.pool().await;
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM reinstall_swaps").await,
        0,
        "the rollback leaves no reinstall witness behind"
    );
    let leftovers: Vec<_> = std::fs::read_dir(installed.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".gmm-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "no staging or quarantine: {leftovers:?}"
    );
}

#[tokio::test]
async fn a_mod_settled_after_release_relocates_and_reinstalls_as_before() {
    let env = TestEnv::new();
    let (core, released, _old_endpoints, _old_server) = released_mod(&env).await;
    core.set_enabled(&released.id, true, &env.game_mods)
        .await
        .expect("an explicit enable settles the unknown state");
    assert!(listed(&core, &released.id)
        .await
        .enabled_transition_recovery
        .is_none());

    let report = core
        .set_library_root(Some(&env.new_root()))
        .await
        .expect("a settled Mod relocates normally");
    assert_eq!(report.relocated, vec![released.id.clone()]);
    let moved = PathBuf::from(&listed(&core, &released.id).await.library_path);
    assert!(moved.starts_with(env.new_root()), "{moved:?}");
    assert!(moved.join("merged.ini").is_file());

    let (_server, endpoints) = serve("2.0.0", mod_zip(b"[TextureOverride]\nhash=2\n")).await;
    core.reinstall_gamebanana_mod_with_endpoints(&released.id, &endpoints)
        .await
        .expect("a settled Mod reinstalls normally");
    let after = listed(&core, &released.id).await;
    assert_eq!(after.version.as_deref(), Some("2.0.0"));
    assert_eq!(
        std::fs::read(moved.join("merged.ini")).expect("new bytes"),
        b"[TextureOverride]\nhash=2\n"
    );
}
