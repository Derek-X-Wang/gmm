//! The weekly Mod update check (#461, ADR 0004).
//!
//! Time is injected (`now` is an argument), so a week of cadence runs
//! instantly. Mockito stands in for GameBanana; its `expect(n)` counts are
//! what prove a network call did or did not happen. What this file cannot
//! show is the real thread in `mod_update_schedule::spawn` waking up in a
//! long-running GMM; that loop is a `sleep` around the function under test.
//!
//! Cadence is per Mod (#471). [`FakeGameBanana`] exists because the tests
//! that pin that down need two Mods of one Game to meet different fates in
//! the same pass: one reached, one behind a dropped connection, one gone.
//! Mockito can only answer, never hang up on a single path.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, TimeZone, Utc};
use gmm_lib::core::gamebanana::Endpoints;
use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
async fn a_partial_check_names_what_it_missed_and_leaves_only_that_mod_due() {
    let fx = fixture().await;
    seed(&fx, GameCode::Gimi, "Reached", 11).await;
    seed(&fx, GameCode::Gimi, "Unreached", 12).await;
    let mut server = mockito::Server::new_async().await;
    // 11 is polled once; it was reached, so the retry leaves it alone.
    let reached = upstream(&mut server, 11, "1", 1).await;
    // 12 has no mock, so mockito answers 501: a server failure, which says
    // nothing about the Mod and so does not count as reaching it.
    let ep = endpoints(&server);

    let report = fx
        .core
        .check_mod_updates_at(GameCode::Gimi, &ep, t0())
        .await
        .unwrap();
    assert_eq!(report.unchecked.len(), 1);
    assert_eq!(report.unchecked[0].name, "Unreached");
    assert!(
        fx.core
            .mod_update_check_due(GameCode::Gimi, t0())
            .await
            .unwrap(),
        "the Mod the check missed keeps the Game due (#471)"
    );

    let retried = upstream(&mut server, 12, "1", 1).await;
    let soon = t0() + Duration::hours(6);
    assert_eq!(
        fx.core.run_due_mod_update_checks(&ep, soon).await.unwrap(),
        vec![GameCode::Gimi]
    );
    assert!(!fx
        .core
        .mod_update_check_due(GameCode::Gimi, soon)
        .await
        .unwrap());
    reached.assert_async().await;
    retried.assert_async().await;
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
    let item = seed(&fx, GameCode::Gimi, "Gimi Mod", 11).await;
    for value in ["not a time", &(t0() + Duration::days(400)).to_rfc3339()] {
        sqlx::query("UPDATE mods SET upstream_reached_at = ? WHERE id = ?")
            .bind(value)
            .bind(&item.id)
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

/// What [`FakeGameBanana`] does when asked about one submission.
#[derive(Clone, Copy)]
enum Upstream {
    /// Serve the submission at this version.
    Version(&'static str),
    /// Answer with this HTTP status and no submission.
    Status(u16),
    /// Accept the connection and close it without a byte: GameBanana
    /// could not be reached, as with a dropped link or a dying proxy.
    HangUp,
}

/// A GameBanana stand-in whose answer is chosen per submission and which
/// counts every request per submission. Any request that is not a
/// submission lookup, such as an archive download, is recorded in
/// `other_requests`.
struct FakeGameBanana {
    api_base: String,
    answers: Arc<Mutex<HashMap<i64, Upstream>>>,
    hits: Arc<Mutex<HashMap<i64, usize>>>,
    other_requests: Arc<Mutex<Vec<String>>>,
}

impl FakeGameBanana {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api_base = format!("http://{}", listener.local_addr().unwrap());
        let answers: Arc<Mutex<HashMap<i64, Upstream>>> = Arc::default();
        let hits: Arc<Mutex<HashMap<i64, usize>>> = Arc::default();
        let other_requests: Arc<Mutex<Vec<String>>> = Arc::default();
        let (a, h, o) = (answers.clone(), hits.clone(), other_requests.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut head = Vec::new();
                let mut buf = [0_u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                let gid = path
                    .strip_prefix("/apiv11/Mod/")
                    .and_then(|rest| rest.split('?').next())
                    .and_then(|id| id.parse::<i64>().ok());
                let Some(gid) = gid else {
                    o.lock().unwrap().push(path);
                    let _ = stream
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                    continue;
                };
                *h.lock().unwrap().entry(gid).or_default() += 1;
                let answer = a.lock().unwrap().get(&gid).copied();
                let response = match answer {
                    Some(Upstream::HangUp) => {
                        drop(stream);
                        continue;
                    }
                    Some(Upstream::Version(v)) => {
                        let body = format!(
                            r#"{{"_idRow":{gid},"_sName":"Mod {gid}","_sProfileUrl":"https://gamebanana.com/mods/{gid}","_sVersion":"{v}","_aSubmitter":{{"_sName":"Author"}},"_aPreviewMedia":{{"_aImages":[]}},"_aFiles":[{{"_sFile":"mod.zip","_sDownloadUrl":"http://127.0.0.1:9/file/{gid}"}}]}}"#
                        );
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    }
                    Some(Upstream::Status(code)) => format!(
                        "HTTP/1.1 {code} Status\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    ),
                    None => panic!("no answer set for submission {gid}"),
                };
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        Self {
            api_base,
            answers,
            hits,
            other_requests,
        }
    }

    fn answer(&self, gid: i64, upstream: Upstream) {
        self.answers.lock().unwrap().insert(gid, upstream);
    }

    fn hits(&self, gid: i64) -> usize {
        self.hits.lock().unwrap().get(&gid).copied().unwrap_or(0)
    }

    fn endpoints(&self) -> Endpoints {
        Endpoints {
            api_base: self.api_base.clone(),
        }
    }
}

async fn upstream_version_of(fx: &Fixture, mod_id: &str) -> Option<String> {
    sqlx::query_scalar("SELECT upstream_version FROM mods WHERE id = ?")
        .bind(mod_id)
        .fetch_one(&fx.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn an_unreached_mod_is_retried_on_the_next_pass_and_a_reached_sibling_is_not() {
    let fx = fixture().await;
    let reached = seed(&fx, GameCode::Gimi, "Reached", 11).await;
    let unreached = seed(&fx, GameCode::Gimi, "Unreached", 12).await;
    let gb = FakeGameBanana::start().await;
    gb.answer(11, Upstream::Version("1"));
    gb.answer(12, Upstream::HangUp);
    let ep = gb.endpoints();

    assert_eq!(
        fx.core.run_due_mod_update_checks(&ep, t0()).await.unwrap(),
        vec![GameCode::Gimi]
    );
    assert_eq!((gb.hits(11), gb.hits(12)), (1, 1));

    // The link comes back, and upstream has moved on for the Mod it missed.
    gb.answer(12, Upstream::Version("2"));
    let next_pass = t0() + Duration::hours(6);
    assert_eq!(
        fx.core
            .run_due_mod_update_checks(&ep, next_pass)
            .await
            .unwrap(),
        vec![GameCode::Gimi],
        "the Mod the first pass could not reach is retried hours later, not a week later"
    );
    assert_eq!(
        (gb.hits(11), gb.hits(12)),
        (1, 2),
        "only the unreached Mod is polled again"
    );
    assert_eq!(
        upstream_version_of(&fx, &unreached.id).await.as_deref(),
        Some("2"),
        "its badge now reflects upstream"
    );
    assert_eq!(
        upstream_version_of(&fx, &reached.id).await.as_deref(),
        Some("1")
    );

    // Every Mod has now been reached: the rest of the week stays quiet.
    let later = t0() + Duration::days(3);
    assert!(fx
        .core
        .run_due_mod_update_checks(&ep, later)
        .await
        .unwrap()
        .is_empty());
    assert_eq!((gb.hits(11), gb.hits(12)), (1, 2));
}

#[tokio::test]
async fn an_unreachable_upstream_and_a_gone_mod_are_treated_differently() {
    let fx = fixture().await;
    seed(&fx, GameCode::Gimi, "Unreachable", 11).await;
    seed(&fx, GameCode::Gimi, "Gone", 12).await;
    let gb = FakeGameBanana::start().await;
    gb.answer(11, Upstream::HangUp);
    gb.answer(12, Upstream::Status(404));
    let ep = gb.endpoints();

    let report = fx
        .core
        .check_mod_updates_at(GameCode::Gimi, &ep, t0())
        .await
        .unwrap();
    let mut missed: Vec<_> = report.unchecked.iter().map(|u| u.name.as_str()).collect();
    missed.sort_unstable();
    assert_eq!(
        missed,
        vec!["Gone", "Unreachable"],
        "neither Mod yielded a version, so the report names both"
    );

    let next_pass = t0() + Duration::hours(6);
    fx.core
        .run_due_mod_update_checks(&ep, next_pass)
        .await
        .unwrap();
    assert_eq!(
        (gb.hits(11), gb.hits(12)),
        (2, 1),
        "the unreachable Mod is retried; upstream already answered about the gone one"
    );
}

#[tokio::test]
async fn a_mod_gone_upstream_settles_and_is_not_repolled_every_pass() {
    let fx = fixture().await;
    let gone = seed(&fx, GameCode::Gimi, "Gone", 11).await;
    let gb = FakeGameBanana::start().await;
    gb.answer(11, Upstream::Status(404));
    let ep = gb.endpoints();

    assert_eq!(
        fx.core.run_due_mod_update_checks(&ep, t0()).await.unwrap(),
        vec![GameCode::Gimi]
    );
    for hours in [6, 12, 24 * 3, 24 * 7 - 1] {
        let at = t0() + Duration::hours(hours);
        assert!(
            fx.core
                .run_due_mod_update_checks(&ep, at)
                .await
                .unwrap()
                .is_empty(),
            "{hours}h later the gone Mod is still settled"
        );
    }
    assert_eq!(gb.hits(11), 1);
    assert_eq!(
        upstream_version_of(&fx, &gone.id).await.as_deref(),
        Some("1"),
        "an answer without a submission does not overwrite the cached version"
    );

    // It is still checked weekly, so a submission that comes back is seen.
    let week = t0() + Duration::days(7);
    assert_eq!(
        fx.core.run_due_mod_update_checks(&ep, week).await.unwrap(),
        vec![GameCode::Gimi]
    );
    assert_eq!(gb.hits(11), 2);
}

#[tokio::test]
async fn a_mod_gone_upstream_does_not_keep_its_siblings_due() {
    let fx = fixture().await;
    seed(&fx, GameCode::Gimi, "Sibling", 11).await;
    seed(&fx, GameCode::Gimi, "Gone", 12).await;
    let gb = FakeGameBanana::start().await;
    gb.answer(11, Upstream::Version("1"));
    gb.answer(12, Upstream::Status(404));
    let ep = gb.endpoints();

    fx.core.run_due_mod_update_checks(&ep, t0()).await.unwrap();
    let next_pass = t0() + Duration::hours(6);
    assert!(!fx
        .core
        .mod_update_check_due(GameCode::Gimi, next_pass)
        .await
        .unwrap());
    assert!(fx
        .core
        .run_due_mod_update_checks(&ep, next_pass)
        .await
        .unwrap()
        .is_empty());
    assert_eq!((gb.hits(11), gb.hits(12)), (1, 1));
}

#[tokio::test]
async fn a_game_whose_every_enabled_mod_was_reached_is_not_attempted_within_the_window() {
    let fx = fixture().await;
    seed(&fx, GameCode::Gimi, "First", 11).await;
    seed(&fx, GameCode::Gimi, "Second", 12).await;
    let off = seed(&fx, GameCode::Gimi, "Check off", 13).await;
    fx.core
        .set_mod_update_check_enabled(&off.id, false)
        .await
        .unwrap();
    let gb = FakeGameBanana::start().await;
    gb.answer(11, Upstream::Version("1"));
    gb.answer(12, Upstream::Version("1"));
    gb.answer(13, Upstream::HangUp);
    let ep = gb.endpoints();

    assert_eq!(
        fx.core.run_due_mod_update_checks(&ep, t0()).await.unwrap(),
        vec![GameCode::Gimi]
    );
    let just_inside = t0() + Duration::days(7) - Duration::minutes(1);
    assert!(
        fx.core
            .run_due_mod_update_checks(&ep, just_inside)
            .await
            .unwrap()
            .is_empty(),
        "a Mod with its own check off never makes the Game due"
    );
    let window = t0() + Duration::days(7);
    assert_eq!(
        fx.core
            .run_due_mod_update_checks(&ep, window)
            .await
            .unwrap(),
        vec![GameCode::Gimi]
    );
    assert_eq!((gb.hits(11), gb.hits(12), gb.hits(13)), (2, 2, 0));
}

#[tokio::test]
async fn a_server_failure_is_not_an_answer_about_the_mod() {
    let fx = fixture().await;
    seed(&fx, GameCode::Gimi, "Busy upstream", 11).await;
    let gb = FakeGameBanana::start().await;
    let ep = gb.endpoints();

    // 407 is a proxy refusing its credentials, not GameBanana answering.
    for (i, code) in [503_u16, 429, 408, 407].into_iter().enumerate() {
        gb.answer(11, Upstream::Status(code));
        let at = t0() + Duration::hours(6 * i as i64);
        fx.core.run_due_mod_update_checks(&ep, at).await.unwrap();
        assert!(
            fx.core
                .mod_update_check_due(GameCode::Gimi, at)
                .await
                .unwrap(),
            "a {code} leaves the Mod due"
        );
    }
    assert_eq!(gb.hits(11), 4);
}

#[tokio::test]
async fn a_gamebanana_mod_without_a_submission_id_does_not_keep_the_game_due() {
    let fx = fixture().await;
    seed(&fx, GameCode::Gimi, "Polled", 11).await;
    let orphan = seed(&fx, GameCode::Gimi, "No submission ID", 12).await;
    sqlx::query("UPDATE mods SET gamebanana_id = NULL WHERE id = ?")
        .bind(&orphan.id)
        .execute(&fx.pool)
        .await
        .unwrap();
    let gb = FakeGameBanana::start().await;
    gb.answer(11, Upstream::Version("1"));
    let ep = gb.endpoints();

    fx.core.run_due_mod_update_checks(&ep, t0()).await.unwrap();
    assert!(
        !fx.core
            .mod_update_check_due(GameCode::Gimi, t0())
            .await
            .unwrap(),
        "a Mod the check can never poll must not hold the Game due forever"
    );
}

#[tokio::test]
async fn a_retried_check_flags_the_update_but_never_applies_it() {
    let fx = fixture().await;
    let item = seed(&fx, GameCode::Gimi, "Gimi Mod", 11).await;
    let installed_ini = item.library_path.join("merged.ini");
    let before = std::fs::read(&installed_ini).unwrap();
    let gb = FakeGameBanana::start().await;
    gb.answer(11, Upstream::HangUp);
    let ep = gb.endpoints();

    fx.core.run_due_mod_update_checks(&ep, t0()).await.unwrap();
    gb.answer(11, Upstream::Version("2"));
    fx.core
        .run_due_mod_update_checks(&ep, t0() + Duration::hours(6))
        .await
        .unwrap();

    let rows = fx.core.list_mod_updates(GameCode::Gimi).await.unwrap();
    assert!(rows[0].upstream_ahead, "the badge is the visible outcome");
    assert_eq!(rows[0].installed_version.as_deref(), Some("1"));
    assert_eq!(std::fs::read(&installed_ini).unwrap(), before);
    assert!(
        gb.other_requests.lock().unwrap().is_empty(),
        "nothing but the submission lookup was requested: {:?}",
        gb.other_requests.lock().unwrap()
    );
}
