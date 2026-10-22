//! Batch coverage uses real reinstall swaps, SQLite failure injection and Library bytes.
mod support;

use gmm_lib::core::{
    gamebanana::Endpoints,
    mod_update_batch::{BatchStop, ModUpdateOutcome, ModUpdateProgress, STOP_REASON},
    Core, GameCode,
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
async fn stop_mid_swap_finishes_current_preserves_prior_and_skips_untouched_mods() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
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
    for (gid, name) in [(1, "First"), (2, "Current"), (3, "Later")] {
        let item = seed(&core, &pool, tmp.path(), name, gid, GameCode::Gimi).await;
        core.set_enabled(&item.id, true, &game.join("Mods"))
            .await
            .unwrap();
        seeded.push(item);
    }
    let order = core.list_mod_updates(GameCode::Gimi).await.unwrap();
    let stop = BatchStop::default();
    let swaps = Arc::new(AtomicUsize::new(0));
    let hook_stop = stop.clone();
    let hook_swaps = swaps.clone();
    let core = core.with_crash_hook(Arc::new(move |point| {
        if point == gmm_lib::core::crash_points::REINSTALL_AFTER_OLD_QUARANTINE_MOVE
            && hook_swaps.fetch_add(1, Ordering::SeqCst) == 1
        {
            hook_stop.request();
            hook_stop.request();
        }
    }));
    let mut server = mockito::Server::new_async().await;
    for gid in 1..=3 {
        upstream(&mut server, gid).await;
    }
    let later_gid = seeded
        .iter()
        .position(|item| item.id == order[2].mod_id)
        .unwrap()
        + 1;
    let no_later = server
        .mock(
            "GET",
            mockito::Matcher::Regex(format!("/apiv11/Mod/{later_gid}.*")),
        )
        .expect(0)
        .create_async()
        .await;
    let mut events = Vec::new();
    let report = core
        .apply_all_mod_updates_with_endpoints_and_stop(
            GameCode::Gimi,
            &Endpoints {
                api_base: server.url(),
            },
            &stop,
            |event| events.push(event),
        )
        .await
        .unwrap();
    let witnesses: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reinstall_swaps")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        witnesses, 0,
        "stop must not leave the in-flight Mod mid-operation with a swap witness"
    );
    assert_eq!(
        report.results[0].outcome,
        ModUpdateOutcome::Updated,
        "stop must preserve the outcome of an already-applied Mod"
    );
    assert_eq!(
        report.results[1].outcome,
        ModUpdateOutcome::Updated,
        "stop must finish the current Mod before stopping"
    );
    assert_eq!(
        report.results[2].outcome,
        ModUpdateOutcome::Skipped {
            reason: STOP_REASON.into()
        },
        "a stopped Mod must be Skipped with the explicit untouched-Library reason"
    );
    assert!(report.stopped);
    let mods = core.list_mods(GameCode::Gimi).await.unwrap();
    for (index, result) in report.results.iter().enumerate() {
        let item = mods
            .iter()
            .find(|item| item.id == result.item.mod_id)
            .unwrap();
        let bytes = std::fs::read(item.library_path.join("merged.ini")).unwrap();
        assert_eq!(
            item.version.as_deref(),
            Some(if index < 2 { "2" } else { "1" })
        );
        assert_eq!(
            bytes.starts_with(b"[TextureOverrideNew]"),
            index < 2,
            "only completed Mods may have replacement bytes"
        );
        assert_eq!(
            std::fs::read(
                game.join("Mods")
                    .join(
                        &seeded
                            .iter()
                            .find(|original| original.id == item.id)
                            .unwrap()
                            .name
                    )
                    .join("merged.ini")
            )
            .unwrap(),
            bytes,
            "the enabled Junction must still resolve to coherent Library bytes"
        );
    }
    assert_eq!(swaps.load(Ordering::SeqCst), 2);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ModUpdateProgress::CheckingConflicts))
            .count(),
        1
    );
    assert!(report.conflicts.is_some());
    assert!(report
        .conflict_error
        .unwrap()
        .contains("Mods not attempted"));
    no_later.assert_async().await;
}

#[tokio::test]
async fn stopping_after_last_mod_starts_is_harmless_and_keeps_complete_report() {
    let tmp = TempDir::new().unwrap();
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &db_url)
        .await
        .unwrap();
    let pool = sqlx::SqlitePool::connect(&db_url).await.unwrap();
    seed(&core, &pool, tmp.path(), "Only", 1, GameCode::Gimi).await;
    let stop = BatchStop::default();
    let mut server = mockito::Server::new_async().await;
    upstream(&mut server, 1).await;
    let report = core
        .apply_all_mod_updates_with_endpoints_and_stop(
            GameCode::Gimi,
            &Endpoints {
                api_base: server.url(),
            },
            &stop,
            |event| {
                if matches!(event, ModUpdateProgress::Applying { .. }) {
                    stop.request();
                    stop.request();
                }
            },
        )
        .await
        .unwrap();
    assert_eq!(report.results[0].outcome, ModUpdateOutcome::Updated);
    assert!(
        !report.stopped,
        "a late request must not invent an unattempted Mod"
    );
    assert!(report.conflict_error.is_none());
}

#[tokio::test]
async fn losing_progress_does_not_stop_remaining_mods() {
    let tmp = TempDir::new().unwrap();
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &db_url)
        .await
        .unwrap();
    let pool = sqlx::SqlitePool::connect(&db_url).await.unwrap();
    for gid in 1..=2 {
        seed(
            &core,
            &pool,
            tmp.path(),
            &format!("Mod{gid}"),
            gid,
            GameCode::Gimi,
        )
        .await;
    }
    let stop = BatchStop::default();
    let (tx, rx) = std::sync::mpsc::channel();
    drop(rx);
    let mut server = mockito::Server::new_async().await;
    for gid in 1..=2 {
        upstream(&mut server, gid).await;
    }
    let report = core
        .apply_all_mod_updates_with_endpoints_and_stop(
            GameCode::Gimi,
            &Endpoints {
                api_base: server.url(),
            },
            &stop,
            |event| {
                let _ = tx.send(event);
            },
        )
        .await
        .unwrap();
    assert_eq!(report.results.len(), 2);
    assert!(report
        .results
        .iter()
        .all(|result| result.outcome == ModUpdateOutcome::Updated));
    assert!(!report.stopped);
    assert!(report.conflict_error.is_none());
}
