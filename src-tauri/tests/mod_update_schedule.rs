//! The weekly Mod update check (#461, ADR 0004).
//!
//! Time is injected (`now` is an argument), so a week of cadence runs
//! instantly. Mockito stands in for GameBanana; its `expect(n)` counts are
//! what prove a network call did or did not happen. What this file cannot
//! show is the real thread in `mod_update_schedule::spawn` waking up in a
//! long-running GMM; that loop is a `sleep` around the function under test.

use chrono::{DateTime, Duration, TimeZone, Utc};
use gmm_lib::core::gamebanana::Endpoints;
use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 5, 12, 0, 0).unwrap()
}

struct Fixture {
    tmp: TempDir,
    core: Core,
    pool: sqlx::SqlitePool,
}

async fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &db_url)
        .await
        .unwrap();
    let pool = sqlx::SqlitePool::connect(&db_url).await.unwrap();
    Fixture { tmp, core, pool }
}

/// A GameBanana Mod installed at version "1" whose recorded upstream is "1".
async fn seed(fx: &Fixture, game: GameCode, name: &str, gid: i64) -> gmm_lib::core::Mod {
    let folder = fx.tmp.path().join(name);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("merged.ini"),
        b"[TextureOverrideOld]\nhash = old\n",
    )
    .unwrap();
    let item = fx.core.adopt_folder(game, &folder, name).await.unwrap();
    sqlx::query(
        "UPDATE mods SET source = 'gamebanana', gamebanana_id = ?, version = '1', upstream_version = '1' WHERE id = ?",
    )
    .bind(gid)
    .bind(&item.id)
    .execute(&fx.pool)
    .await
    .unwrap();
    item
}

/// Serve submission `gid` at `version`, expecting exactly `hits` API calls.
async fn upstream(
    server: &mut mockito::ServerGuard,
    gid: i64,
    version: &str,
    hits: usize,
) -> mockito::Mock {
    server
        .mock("GET", mockito::Matcher::Regex(format!("/apiv11/Mod/{gid}.*")))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{"_idRow":{gid},"_sName":"Mod {gid}","_sProfileUrl":"https://gamebanana.com/mods/{gid}","_sVersion":"{version}","_aSubmitter":{{"_sName":"Author"}},"_aPreviewMedia":{{"_aImages":[]}},"_aFiles":[{{"_sFile":"mod.zip","_sDownloadUrl":"{}/file/{gid}"}}]}}"#,
            server.url()
        ))
        .expect(hits)
        .create_async()
        .await
}

fn endpoints(server: &mockito::ServerGuard) -> Endpoints {
    Endpoints {
        api_base: server.url(),
    }
}

/// A base URL nothing listens on: bind, then drop, so connections are refused.
fn closed_port_endpoints() -> Endpoints {
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let api_base = format!("http://{}", dead.local_addr().unwrap());
    drop(dead);
    Endpoints { api_base }
}

#[tokio::test]
async fn an_overdue_game_is_checked_and_an_up_to_date_game_is_not() {
    let fx = fixture().await;
    seed(&fx, GameCode::Gimi, "Gimi Mod", 11).await;
    seed(&fx, GameCode::Zzmi, "Zzmi Mod", 22).await;
    let mut server = mockito::Server::new_async().await;
    // Gimi is polled at t0 and again a week later; Zzmi only once, at day 2.
    let gimi = upstream(&mut server, 11, "1", 2).await;
    let zzmi = upstream(&mut server, 22, "1", 1).await;
    let ep = endpoints(&server);

    fx.core
        .check_mod_updates_at(GameCode::Gimi, &ep, t0())
        .await
        .unwrap();

    let day2 = t0() + Duration::days(2);
    assert_eq!(
        fx.core.run_due_mod_update_checks(&ep, day2).await.unwrap(),
        vec![GameCode::Zzmi],
        "Gimi was checked two days ago; only never-checked Zzmi is due"
    );

    let day8 = t0() + Duration::days(8);
    assert_eq!(
        fx.core.run_due_mod_update_checks(&ep, day8).await.unwrap(),
        vec![GameCode::Gimi],
        "Gimi is now over a week old; Zzmi, checked on day 2, is not"
    );

    gimi.assert_async().await;
    zzmi.assert_async().await;
}

#[tokio::test]
async fn a_check_that_reached_nothing_does_not_satisfy_the_cadence() {
    let fx = fixture().await;
    seed(&fx, GameCode::Gimi, "Gimi Mod", 11).await;

    let attempted = fx
        .core
        .run_due_mod_update_checks(&closed_port_endpoints(), t0())
        .await
        .unwrap();
    assert_eq!(attempted, vec![GameCode::Gimi], "the check was tried");
    assert!(
        fx.core
            .mod_update_check_due(GameCode::Gimi, t0())
            .await
            .unwrap(),
        "a check that reached no Mod must leave the Game due"
    );

    // The very next opportunity tries again, and this time it lands.
    let mut server = mockito::Server::new_async().await;
    let api = upstream(&mut server, 11, "1", 1).await;
    let ep = endpoints(&server);
    let soon = t0() + Duration::minutes(1);
    assert_eq!(
        fx.core.run_due_mod_update_checks(&ep, soon).await.unwrap(),
        vec![GameCode::Gimi]
    );
    assert!(!fx
        .core
        .mod_update_check_due(GameCode::Gimi, soon)
        .await
        .unwrap());

    // And now it is satisfied: a later launch in the same week stays quiet.
    let later = t0() + Duration::days(3);
    assert!(fx
        .core
        .run_due_mod_update_checks(&ep, later)
        .await
        .unwrap()
        .is_empty());
    api.assert_async().await;
}

#[tokio::test]
async fn a_partial_check_satisfies_the_cadence_and_still_names_what_it_missed() {
    let fx = fixture().await;
    seed(&fx, GameCode::Gimi, "Reached", 11).await;
    seed(&fx, GameCode::Gimi, "Unreached", 12).await;
    let mut server = mockito::Server::new_async().await;
    upstream(&mut server, 11, "1", 1).await;
    // 12 has no mock, so mockito answers 501.
    let ep = endpoints(&server);

    let report = fx
        .core
        .check_mod_updates_at(GameCode::Gimi, &ep, t0())
        .await
        .unwrap();
    assert_eq!(report.unchecked.len(), 1);
    assert_eq!(report.unchecked[0].name, "Unreached");
    assert!(!fx
        .core
        .mod_update_check_due(GameCode::Gimi, t0())
        .await
        .unwrap());
}

#[tokio::test]
async fn global_toggle_off_means_no_network_call() {
    let fx = fixture().await;
    seed(&fx, GameCode::Gimi, "Gimi Mod", 11).await;
    fx.core
        .set_mod_updates_globally_enabled(false)
        .await
        .unwrap();
    let mut server = mockito::Server::new_async().await;
    let api = upstream(&mut server, 11, "2", 0).await;

    let attempted = fx
        .core
        .run_due_mod_update_checks(&endpoints(&server), t0())
        .await
        .unwrap();

    assert!(attempted.is_empty());
    assert!(
        fx.core
            .mod_update_check_due(GameCode::Gimi, t0())
            .await
            .unwrap(),
        "a skipped check records nothing; turning the toggle on checks at once"
    );
    api.assert_async().await;
}

#[tokio::test]
async fn a_mod_with_its_own_check_off_is_not_polled() {
    let fx = fixture().await;
    let item = seed(&fx, GameCode::Gimi, "Pinned", 11).await;
    fx.core
        .set_mod_update_check_enabled(&item.id, false)
        .await
        .unwrap();
    let mut server = mockito::Server::new_async().await;
    let api = upstream(&mut server, 11, "2", 0).await;

    let attempted = fx
        .core
        .run_due_mod_update_checks(&endpoints(&server), t0())
        .await
        .unwrap();

    assert!(attempted.is_empty());
    api.assert_async().await;
}

#[tokio::test]
async fn a_scheduled_check_flags_the_update_but_never_applies_it() {
    let fx = fixture().await;
    let item = seed(&fx, GameCode::Gimi, "Gimi Mod", 11).await;
    let installed_ini = item.library_path.join("merged.ini");
    let before = std::fs::read(&installed_ini).unwrap();
    let mut server = mockito::Server::new_async().await;
    let api = upstream(&mut server, 11, "2", 1).await;
    // Applying means downloading the archive; it must never be requested.
    let download = server
        .mock("GET", "/file/11")
        .with_status(200)
        .with_body("zip")
        .expect(0)
        .create_async()
        .await;

    fx.core
        .run_due_mod_update_checks(&endpoints(&server), t0())
        .await
        .unwrap();

    let rows = fx.core.list_mod_updates(GameCode::Gimi).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].upstream_ahead, "the badge is the visible outcome");
    assert_eq!(rows[0].installed_version.as_deref(), Some("1"));
    assert_eq!(rows[0].upstream_version.as_deref(), Some("2"));
    assert_eq!(std::fs::read(&installed_ini).unwrap(), before);
    api.assert_async().await;
    download.assert_async().await;
}

#[tokio::test]
async fn a_bad_recorded_time_makes_the_game_due_rather_than_silencing_it() {
    let fx = fixture().await;
    let key = "mod_updates.last_check_at.gimi";
    for value in ["not a time", &(t0() + Duration::days(400)).to_rfc3339()] {
        sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
            .bind(key)
            .bind(value)
            .execute(&fx.pool)
            .await
            .unwrap();
        assert!(
            fx.core
                .mod_update_check_due(GameCode::Gimi, t0())
                .await
                .unwrap(),
            "{value:?} must read as due"
        );
    }
}
