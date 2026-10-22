//! Batch coverage uses real reinstall swaps, SQLite failure injection and Library bytes.
mod support;

use gmm_lib::core::{
    gamebanana::Endpoints,
    mod_update_batch::{ModUpdateOutcome, ModUpdateProgress},
    Core, GameCode, SessionInfo,
};
use std::{io::Write, path::Path};
use tempfile::TempDir;
use zip::{write::SimpleFileOptions, ZipWriter};

fn zip(body: &[u8]) -> Vec<u8> {
    let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file("merged.ini", SimpleFileOptions::default())
        .unwrap();
    writer.write_all(body).unwrap();
    writer.finish().unwrap().into_inner()
}

async fn seed(
    core: &Core,
    pool: &sqlx::SqlitePool,
    root: &Path,
    name: &str,
    gid: i64,
    game: GameCode,
) -> gmm_lib::core::Mod {
    let folder = root.join(name);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("merged.ini"),
        format!("[TextureOverrideOld]\nhash = old{gid}\n"),
    )
    .unwrap();
    let item = core.adopt_folder(game, &folder, name).await.unwrap();
    sqlx::query("UPDATE mods SET source = 'gamebanana', gamebanana_id = ?, version = '1', upstream_version = '2' WHERE id = ?")
        .bind(gid).bind(&item.id).execute(pool).await.unwrap();
    item
}

async fn upstream(server: &mut mockito::ServerGuard, gid: i64) {
    server.mock("GET", mockito::Matcher::Regex(format!("/apiv11/Mod/{gid}.*")))
        .with_status(200).with_header("content-type", "application/json")
        .with_body(format!(r#"{{"_idRow":{gid},"_sName":"Updated {gid}","_sProfileUrl":"https://gamebanana.com/mods/{gid}","_sVersion":"2","_aSubmitter":{{"_sName":"Author"}},"_aPreviewMedia":{{"_aImages":[]}},"_aFiles":[{{"_sFile":"mod.zip","_sDownloadUrl":"{}/file/{gid}"}}]}}"#, server.url()))
        .create_async().await;
    server
        .mock("GET", format!("/file/{gid}").as_str())
        .with_status(200)
        .with_body(zip(b"[TextureOverrideNew]\nhash = shared\n"))
        .create_async()
        .await;
}

#[tokio::test]
async fn batch_continues_after_mid_batch_failure_reports_truth_and_restores_bytes_and_junction() {
    let tmp = TempDir::new().unwrap();
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &db_url)
        .await
        .unwrap();
    let pool = sqlx::SqlitePool::connect(&db_url).await.unwrap();
    let game = tmp.path().join("game");
    core.set_game_install_path(
        GameCode::Gimi,
        support::seed_game_install(GameCode::Gimi, &game),
    )
    .await
    .unwrap();
    std::fs::create_dir_all(game.join("Mods")).unwrap();
    let mut seeded = Vec::new();
    for (gid, name) in [(1, "First"), (2, "Broken"), (3, "Last")] {
        let item = seed(&core, &pool, tmp.path(), name, gid, GameCode::Gimi).await;
        core.set_enabled(&item.id, true, &game.join("Mods"))
            .await
            .unwrap();
        seeded.push(item);
    }
    // Read the exact production snapshot order rather than relying on SQLite's row order.
    let order = core.list_mod_updates(GameCode::Gimi).await.unwrap();
    let middle = seeded
        .iter()
        .find(|item| item.id == order[1].mod_id)
        .unwrap();
    let original = std::fs::read(middle.library_path.join("merged.ini")).unwrap();
    sqlx::query(&format!("CREATE TRIGGER reject_middle_update BEFORE UPDATE OF version ON mods WHEN OLD.id = '{}' AND NEW.version = '2' BEGIN SELECT RAISE(ABORT, 'injected metadata failure'); END", middle.id))
        .execute(&pool).await.unwrap();
    // Other Games and Mods without an available update must never be attempted.
    let other = seed(&core, &pool, tmp.path(), "Other game", 4, GameCode::Srmi).await;
    let current = seed(&core, &pool, tmp.path(), "Current", 5, GameCode::Gimi).await;
    sqlx::query("UPDATE mods SET upstream_version = version WHERE id = ?")
        .bind(&current.id)
        .execute(&pool)
        .await
        .unwrap();
    core.set_mod_updates_globally_enabled(false).await.unwrap();
    let mut server = mockito::Server::new_async().await;
    for gid in 1..=3 {
        upstream(&mut server, gid).await;
    }
    let no_other = server
        .mock("GET", mockito::Matcher::Regex("/apiv11/Mod/[45].*".into()))
        .expect(0)
        .create_async()
        .await;
    let mut events = Vec::new();
    let report = core
        .apply_all_mod_updates_with_endpoints(
            GameCode::Gimi,
            &Endpoints {
                api_base: server.url(),
            },
            |event| events.push(event),
        )
        .await
        .unwrap();
    assert_eq!(
        report.results.len(),
        3,
        "a failed Mod must not abandon remaining Mods"
    );
    assert_eq!(
        report.results[2].outcome,
        ModUpdateOutcome::Updated,
        "the Mod after a failure must still update"
    );
    assert!(
        matches!(&report.results[1].outcome, ModUpdateOutcome::Failed { reason } if reason.contains("injected metadata failure")),
        "a failed Mod must never be reported as updated: {:?}",
        report.results[1]
    );
    assert_eq!(
        std::fs::read(middle.library_path.join("merged.ini")).unwrap(),
        original,
        "batch must run per-Mod rollback and restore the original Library bytes"
    );
    assert_eq!(
        std::fs::read(game.join("Mods").join(&middle.name).join("merged.ini")).unwrap(),
        original,
        "rollback must restore the enabled Mod's Junction"
    );
    let mods = core.list_mods(GameCode::Gimi).await.unwrap();
    for result in &report.results {
        let item = mods
            .iter()
            .find(|item| item.id == result.item.mod_id)
            .unwrap();
        assert!(item.enabled, "enabled state must survive the batch");
        assert_eq!(
            item.version.as_deref(),
            Some(if item.id == middle.id { "1" } else { "2" })
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reinstall_swaps")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "all successful swaps and the completed rollback must retire their witnesses"
    );
    assert_eq!(
        core.list_mods(GameCode::Srmi).await.unwrap()[0].id,
        other.id
    );
    assert_eq!(
        report.conflicts.as_ref().unwrap().conflicts.len(),
        1,
        "new Conflicts must be surfaced after the batch"
    );
    assert!(report.conflict_error.is_none());
    assert!(matches!(events.first(), Some(ModUpdateProgress::Started { mods }) if mods.len() == 3));
    let completed = events
        .iter()
        .filter(|event| matches!(event, ModUpdateProgress::Completed { .. }))
        .count();
    assert_eq!(completed, 3);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ModUpdateProgress::CheckingConflicts))
            .count(),
        1
    );
    assert!(matches!(
        events.last(),
        Some(ModUpdateProgress::CheckingConflicts)
    ));
    for (index, item) in order.iter().enumerate() {
        assert!(
            matches!(&events[1 + index * 2], ModUpdateProgress::Applying { item: active } if active.mod_id == item.mod_id),
            "progress must name each Mod before its reinstall"
        );
    }
    no_other.assert_async().await;
}

#[tokio::test]
async fn batch_refuses_live_session_before_network_or_progress() {
    let tmp = TempDir::new().unwrap();
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &db_url)
        .await
        .unwrap();
    let pool = sqlx::SqlitePool::connect(&db_url).await.unwrap();
    seed(&core, &pool, tmp.path(), "Locked", 1, GameCode::Gimi).await;
    core.start_session(&SessionInfo {
        game: GameCode::Gimi,
        pid: std::process::id(),
        started_at: chrono::Utc::now(),
    })
    .await
    .unwrap();
    let mut events = Vec::new();
    let error = core
        .apply_all_mod_updates_with_endpoints(
            GameCode::Gimi,
            &Endpoints {
                api_base: "http://127.0.0.1:1".into(),
            },
            |event| events.push(event),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, gmm_lib::core::Error::SessionActive { .. }));
    assert!(
        events.is_empty(),
        "a live session must refuse the whole batch before it starts"
    );
}

#[tokio::test]
async fn batch_keeps_unresolved_rollback_unknown_and_conflicts_unverified() {
    let tmp = TempDir::new().unwrap();
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &db_url)
        .await
        .unwrap();
    let pool = sqlx::SqlitePool::connect(&db_url).await.unwrap();
    seed(&core, &pool, tmp.path(), "Uncertain", 1, GameCode::Gimi).await;
    sqlx::query("CREATE TRIGGER reject_retirement BEFORE DELETE ON reinstall_swaps BEGIN SELECT RAISE(ABORT, 'recovery still required'); END").execute(&pool).await.unwrap();
    let mut server = mockito::Server::new_async().await;
    upstream(&mut server, 1).await;
    let report = core
        .apply_all_mod_updates_with_endpoints(
            GameCode::Gimi,
            &Endpoints {
                api_base: server.url(),
            },
            |_| {},
        )
        .await
        .unwrap();
    assert!(
        matches!(&report.results[0].outcome, ModUpdateOutcome::Unknown { reason } if reason.contains("recovery still required")),
        "an unresolved rollback must stay unknown: {:?}",
        report.results
    );
    assert!(
        report.conflicts.is_some(),
        "keep any known Conflict findings"
    );
    assert!(report.conflict_error.is_some());
}
