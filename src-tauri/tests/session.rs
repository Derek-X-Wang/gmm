//! Tracer bullet for slice 4b: GameSession state persistence + mutation
//! lock. Exercises the Core directly so it runs on macOS dev hosts; the
//! real game-spawning + loader-hooking belongs in the Tauri command
//! layer and is covered by `cargo xtask test-loader` on Windows CI.

use std::fs;

use chrono::Utc;
use gmm_lib::core::{Core, GameCode, SessionInfo};
use sqlx::{sqlite::SqliteConnectOptions, Row, SqlitePool};
use tempfile::TempDir;

async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init core")
}

async fn raw_pool(tmp: &TempDir) -> SqlitePool {
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let options = db_url
        .parse::<SqliteConnectOptions>()
        .expect("parse test database URL");
    SqlitePool::connect_with(options)
        .await
        .expect("open raw test database")
}

async fn insert_launch_claim(
    tmp: &TempDir,
    token: &str,
    owner_pid: u32,
    owner_started_at: Option<u64>,
    child_pid: Option<u32>,
    child_started_at: Option<u64>,
) {
    let pool = raw_pool(tmp).await;
    sqlx::query(
        "INSERT INTO session_launch_claims (
            token, game_code, owner_pid, owner_started_at,
            child_pid, child_started_at, started_at
         ) VALUES (?, 'gimi', ?, ?, ?, ?, '2026-08-24T00:00:00Z')",
    )
    .bind(token)
    .bind(owner_pid as i64)
    .bind(owner_started_at.map(|value| value as i64))
    .bind(child_pid.map(|value| value as i64))
    .bind(child_started_at.map(|value| value as i64))
    .execute(&pool)
    .await
    .expect("insert launch claim");
    pool.close().await;
}

#[tokio::test]
async fn session_starts_persists_and_ends() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    // Fresh DB → no session
    assert!(
        core.session_info().await.expect("read empty").is_none(),
        "no session on a fresh DB",
    );

    // Start a session
    let info = SessionInfo {
        game: GameCode::Gimi,
        pid: 12345,
        started_at: Utc::now(),
    };
    core.start_session(&info).await.expect("start session");

    // session_info reflects it
    let read = core
        .session_info()
        .await
        .expect("read active")
        .expect("Some");
    assert_eq!(read.game, GameCode::Gimi);
    assert_eq!(read.pid, 12345);

    // End it
    core.end_session().await.expect("end session");
    assert!(
        core.session_info().await.expect("read after end").is_none(),
        "session cleared after end_session",
    );
}

async fn start_a_session(core: &Core) {
    core.start_session(&SessionInfo {
        game: GameCode::Gimi,
        pid: 99999,
        started_at: Utc::now(),
    })
    .await
    .expect("start session");
}

fn assert_session_error(err: &gmm_lib::core::Error) {
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("session") || msg.contains("game running"),
        "expected a session-active error, got: {err}",
    );
}

#[tokio::test]
async fn adopt_folder_is_rejected_while_a_session_is_active() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let fixture = tmp.path().join("fixture/Mod");
    fs::create_dir_all(&fixture).expect("fixture");
    fs::write(fixture.join("merged.ini"), "").expect("fixture ini");

    start_a_session(&core).await;

    let err = core
        .adopt_folder(GameCode::Gimi, &fixture, "Mod")
        .await
        .expect_err("adopt_folder must error while a session is active");
    assert_session_error(&err);
}

#[tokio::test]
async fn set_active_variant_is_rejected_while_a_session_is_active() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let game_mods = tmp.path().join("Genshin/Mods");
    fs::create_dir_all(&game_mods).expect("game mods dir");

    // Adopt a multi-variant mod before starting the session.
    let fixture = tmp.path().join("fixture/Mod");
    fs::create_dir_all(fixture.join("Variant A")).expect("variant A");
    fs::create_dir_all(fixture.join("Variant B")).expect("variant B");
    fs::write(fixture.join("Variant A/merged.ini"), "").expect("ini A");
    fs::write(fixture.join("Variant B/merged.ini"), "").expect("ini B");

    let m = core
        .adopt_folder(GameCode::Gimi, &fixture, "Variant Mod")
        .await
        .expect("adopt");
    let variants = core.list_variants(&m.id).await.expect("list variants");
    assert!(variants.len() >= 2, "fixture should produce >= 2 variants");
    let variant_id = &variants[0].id;

    start_a_session(&core).await;

    let err = core
        .set_active_variant(&m.id, variant_id, &game_mods)
        .await
        .expect_err("set_active_variant must error while a session is active");
    assert_session_error(&err);
}

#[tokio::test]
async fn start_session_twice_without_end_errors_on_the_second() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let info = SessionInfo {
        game: GameCode::Gimi,
        pid: 12345,
        started_at: Utc::now(),
    };
    core.start_session(&info).await.expect("first start");

    let err = core
        .start_session(&SessionInfo {
            game: GameCode::Srmi,
            pid: 67890,
            started_at: Utc::now(),
        })
        .await
        .expect_err(
            "the singleton CHECK + plain INSERT must reject a second concurrent start_session",
        );
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("constraint") || msg.contains("unique") || msg.contains("primary key"),
        "expected a uniqueness-related db error, got: {err}",
    );

    // The first session row must survive.
    let still = core
        .session_info()
        .await
        .expect("info")
        .expect("first session still active");
    assert_eq!(still.pid, 12345);
}

#[tokio::test]
async fn set_library_root_is_rejected_while_a_session_is_active() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    start_a_session(&core).await;

    let new_root = tmp.path().join("new-library");
    fs::create_dir_all(&new_root).expect("new root");

    let err = core
        .set_library_root(Some(&new_root))
        .await
        .expect_err("set_library_root must error during a session");
    assert_session_error(&err);
}

#[tokio::test]
async fn clean_stale_session_evicts_a_dead_pid() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    core.start_session(&SessionInfo {
        game: GameCode::Gimi,
        // 4_294_967_294 = u32::MAX - 1, will never be a real PID
        pid: u32::MAX - 1,
        started_at: Utc::now(),
    })
    .await
    .expect("start");

    let evicted = core.clean_stale_session().await.expect("clean");
    let evicted = evicted.expect("a stale row should be evicted");
    assert_eq!(evicted.game, GameCode::Gimi);
    assert!(
        core.session_info().await.expect("info").is_none(),
        "session cleared after eviction",
    );

    // Calling again is a no-op.
    assert!(
        core.clean_stale_session()
            .await
            .expect("idempotent clean")
            .is_none(),
        "no stale row → returns None",
    );
}

#[tokio::test]
async fn clean_stale_session_keeps_a_live_session() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let my_pid = std::process::id();
    core.start_session(&SessionInfo {
        game: GameCode::Gimi,
        pid: my_pid,
        started_at: Utc::now(),
    })
    .await
    .expect("start");

    let result = core.clean_stale_session().await.expect("clean");
    assert!(result.is_none(), "a live PID must NOT be evicted");
    assert!(
        core.session_info().await.expect("info").is_some(),
        "session row still there",
    );
}

#[tokio::test]
async fn dead_owner_without_recorded_child_survives_startup_and_is_retirable() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    drop(core);
    let token = "01JINTERRUPTEDLAUNCH000001";
    insert_launch_claim(&tmp, token, u32::MAX - 1, Some(1), None, None).await;

    let core = fresh_core(&tmp).await;
    let launches = core
        .interrupted_session_launches()
        .await
        .expect("surface interrupted launch");
    assert_eq!(
        launches.len(),
        1,
        "a dead owner without a recorded child must remain surfaced because GMM cannot know whether spawn already happened",
    );
    assert_eq!(launches[0].id, token);
    assert_eq!(launches[0].game, GameCode::Gimi);
    assert_eq!(launches[0].child_pid, None);

    core.retire_interrupted_session_launch(token)
        .await
        .expect("retire after the user confirms the game is closed");
    assert!(
        core.interrupted_session_launches()
            .await
            .expect("list after retire")
            .is_empty(),
        "the in-app retire action must release the otherwise permanent Library blocker",
    );
}

#[tokio::test]
async fn reused_pids_do_not_impersonate_launch_processes_or_make_the_message_claim_a_game_runs() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    drop(core);
    let current_pid = std::process::id();

    // Both rows use a live PID but pair it with an impossible historical
    // identity. One covers a reused GMM owner slot; the other covers a reused
    // game-child slot. Neither unrelated process may keep the Library locked.
    insert_launch_claim(
        &tmp,
        "01JREUSEDOWNERPID000000001",
        current_pid,
        Some(1),
        Some(u32::MAX - 1),
        Some(1),
    )
    .await;
    insert_launch_claim(
        &tmp,
        "01JREUSEDCHILDPID000000001",
        u32::MAX - 1,
        Some(1),
        Some(current_pid),
        Some(1),
    )
    .await;

    // Identity can occasionally be unavailable even while a numeric PID is
    // in use. Keep that third row conservatively so its refusal copy is also
    // pinned: it must describe uncertainty, never claim a game is running.
    let uncertain = "01JUNKNOWNCHILDIDENTITY0001";
    insert_launch_claim(
        &tmp,
        uncertain,
        u32::MAX - 1,
        Some(1),
        Some(current_pid),
        None,
    )
    .await;

    let core = fresh_core(&tmp).await;
    let launches = core
        .interrupted_session_launches()
        .await
        .expect("list interrupted launches");
    assert_eq!(
        launches
            .iter()
            .map(|launch| launch.id.as_str())
            .collect::<Vec<_>>(),
        [uncertain],
        "a live PID with the wrong start identity must not impersonate either original process",
    );

    let fixture = tmp.path().join("fixture/reused-pid-message");
    fs::create_dir_all(&fixture).expect("fixture");
    fs::write(fixture.join("merged.ini"), "").expect("fixture ini");
    let error = core
        .adopt_folder(GameCode::Gimi, &fixture, "Blocked by uncertainty")
        .await
        .expect_err("the unknown identity must remain a conservative blocker")
        .to_string();
    assert!(
        error.contains("cannot determine whether a game"),
        "the blocker must state what GMM does and does not know: {error}",
    );
    assert!(
        !error.contains("is running"),
        "a PID in use without a matching process identity is not proof a game is running: {error}",
    );
}

#[tokio::test]
async fn retire_refuses_a_launch_still_owned_by_its_original_process() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.begin_session_launch(GameCode::Gimi)
        .await
        .expect("begin owned launch");
    let pool = raw_pool(&tmp).await;
    let id: String = sqlx::query("SELECT token FROM session_launch_claims")
        .fetch_one(&pool)
        .await
        .expect("read claim")
        .try_get("token")
        .expect("claim token");
    pool.close().await;

    let error = core
        .retire_interrupted_session_launch(&id)
        .await
        .expect_err("the recovery action must not cancel an active launch");
    assert!(
        error.to_string().contains("still owned"),
        "the refusal must explain why an active launch cannot be retired: {error}",
    );
}

#[tokio::test]
async fn set_enabled_is_rejected_while_a_session_is_active() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let game_mods = tmp.path().join("Genshin/Mods");
    fs::create_dir_all(&game_mods).expect("game mods dir");

    // Adopt a mod (allowed before session)
    let fixture = tmp.path().join("fixture/Mod1");
    fs::create_dir_all(&fixture).expect("fixture");
    fs::write(fixture.join("merged.ini"), "").expect("fixture ini");
    let m = core
        .adopt_folder(GameCode::Gimi, &fixture, "Mod 1")
        .await
        .expect("adopt");

    // Start a session
    core.start_session(&SessionInfo {
        game: GameCode::Gimi,
        pid: 99999,
        started_at: Utc::now(),
    })
    .await
    .expect("start");

    // set_enabled must now error out
    let err = core
        .set_enabled(&m.id, true, &game_mods)
        .await
        .expect_err("set_enabled must error while a session is active");
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("session") || msg.to_lowercase().contains("game running"),
        "error should mention the active session, got: {msg}",
    );

    // After end_session, set_enabled works again
    core.end_session().await.expect("end");
    core.set_enabled(&m.id, true, &game_mods)
        .await
        .expect("set_enabled after end_session");
}
