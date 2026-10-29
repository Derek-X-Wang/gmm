//! Read-only preflight must avoid downloads; recovery must qualify Conflict readings.
mod support;

use gmm_lib::core::{
    gamebanana::Endpoints,
    mod_update_batch::{ModUpdateOutcome, ModUpdateProgress},
    Core, Error, GameCode,
};
use std::{io::Write, path::Path};
use tempfile::TempDir;
use zip::{write::SimpleFileOptions, ZipWriter};

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
        b"[TextureOverrideOld]\nhash = old\n",
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
        .expect(1).create_async().await;
    let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("merged.ini", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"[TextureOverrideNew]\nhash = shared\n")
        .unwrap();
    server
        .mock("GET", format!("/file/{gid}").as_str())
        .with_status(200)
        .with_body(zip.finish().unwrap().into_inner())
        .expect(1)
        .create_async()
        .await;
}

#[tokio::test]
async fn unrelated_games_pending_transition_refuses_batch_before_any_network_or_progress() {
    let tmp = TempDir::new().unwrap();
    let url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &url).await.unwrap();
    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    seed(&core, &pool, tmp.path(), "First", 1, GameCode::Gimi).await;
    seed(&core, &pool, tmp.path(), "Second", 2, GameCode::Gimi).await;
    let unrelated = seed(&core, &pool, tmp.path(), "Other", 3, GameCode::Srmi).await;
    let deployment = tmp.path().join("other-game/Mods");
    std::fs::create_dir_all(&deployment).unwrap();
    sqlx::query("CREATE TRIGGER reject_enabled_update BEFORE UPDATE OF enabled ON mods BEGIN SELECT RAISE(ABORT, 'pending transition'); END")
        .execute(&pool).await.unwrap();
    core.set_enabled(&unrelated.id, true, &deployment)
        .await
        .unwrap_err();
    let mut server = mockito::Server::new_async().await;
    let no_network = server
        .mock("GET", mockito::Matcher::Any)
        .expect(0)
        .create_async()
        .await;
    let mut events = Vec::new();
    let error = core
        .apply_all_mod_updates_with_endpoints(
            GameCode::Gimi,
            &Endpoints {
                api_base: server.url(),
            },
            |event| events.push(event),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&error, Error::EnabledTransitionPending { mod_id } if mod_id == &unrelated.id),
        "global transition must refuse the batch: {error}"
    );
    assert!(error
        .to_string()
        .contains("interrupted enable/disable transition"));
    assert!(
        events.is_empty(),
        "global refusal must happen before the started event"
    );
    no_network.assert_async().await;
}

async fn quarantine_case(outdated: bool, enabled: bool) {
    let tmp = TempDir::new().unwrap();
    let url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &url).await.unwrap();
    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    let game = tmp.path().join("game");
    core.set_game_install_path(
        GameCode::Gimi,
        support::seed_game_install(GameCode::Gimi, &game),
    )
    .await
    .unwrap();
    std::fs::create_dir_all(game.join("Mods")).unwrap();
    let quarantined = seed(&core, &pool, tmp.path(), "Recovery", 1, GameCode::Gimi).await;
    let healthy = seed(&core, &pool, tmp.path(), "Healthy", 2, GameCode::Gimi).await;
    if enabled {
        core.set_enabled(&quarantined.id, true, &game.join("Mods"))
            .await
            .unwrap();
    }
    sqlx::query("CREATE TRIGGER reject_retirement BEFORE DELETE ON reinstall_swaps BEGIN SELECT RAISE(ABORT, 'recovery required'); END")
        .execute(&pool).await.unwrap();
    let mut first_server = mockito::Server::new_async().await;
    upstream(&mut first_server, 1).await;
    let error = core
        .reinstall_gamebanana_mod_with_endpoints(
            &quarantined.id,
            &Endpoints {
                api_base: first_server.url(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::ReinstallRollbackFailed { .. }));
    sqlx::query("DROP TRIGGER reject_retirement")
        .execute(&pool)
        .await
        .unwrap();
    // A failed witness retirement is pending recovery, but it does not itself
    // quarantine the Mod. Move the old tree aside so the real recovery path
    // records an unresolved identity and produces the quarantine UI state.
    std::fs::rename(&quarantined.library_path, tmp.path().join("held-old-tree")).unwrap();
    assert!(matches!(
        core.retry_reinstall_recovery(&quarantined.id)
            .await
            .unwrap(),
        gmm_lib::core::ReinstallRecoveryOutcome::Quarantined { .. }
    ));
    let row = core
        .list_mods(GameCode::Gimi)
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.id == quarantined.id)
        .unwrap();
    assert!(
        row.enabled == enabled && row.reinstall_recovery.is_some(),
        "fixture must contain a pre-existing quarantined Mod with the requested enabled state"
    );
    if !outdated {
        // Conflict completeness covers all Sources, not only available updates.
        sqlx::query("UPDATE mods SET source = 'local' WHERE id = ?")
            .bind(&quarantined.id)
            .execute(&pool)
            .await
            .unwrap();
    }
    let mut server = mockito::Server::new_async().await;
    let no_recovery_download = server
        .mock(
            "GET",
            mockito::Matcher::Regex("/(apiv11/Mod/1.*|file/1)".into()),
        )
        .expect(0)
        .create_async()
        .await;
    upstream(&mut server, 2).await;
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
    assert_eq!(report.results.len(), if outdated { 2 } else { 1 });
    assert!(
        report
            .results
            .iter()
            .any(|result| result.item.mod_id == healthy.id
                && result.outcome == ModUpdateOutcome::Updated),
        "healthy Mods must still update"
    );
    if outdated {
        let skipped = report
            .results
            .iter()
            .find(|result| result.item.mod_id == quarantined.id)
            .unwrap();
        assert!(
            matches!(&skipped.outcome, ModUpdateOutcome::Skipped { reason } if reason.contains("recovery") && !reason.contains("io error") && !reason.contains("UNIQUE")),
            "recovery must be skipped with readable guidance: {skipped:?}"
        );
        assert_eq!(serde_json::to_value(skipped).unwrap()["status"], "skipped");
    }
    assert!(!events.iter().any(|event| matches!(event, ModUpdateProgress::Applying { item } if item.mod_id == quarantined.id)), "quarantined Mods must never enter the reinstall path");
    assert!(report.conflicts.is_some(), "retain known Conflict findings");
    assert_eq!(
        report.conflict_error.is_some(),
        enabled,
        "only enabled quarantined Mods make the full Conflict reading unverified"
    );
    assert_eq!(
        report.conflicts.as_ref().unwrap().omitted,
        if enabled {
            vec![quarantined.id.clone()]
        } else {
            vec![]
        },
        "disabled quarantined Mods are not omissions"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ModUpdateProgress::CheckingConflicts))
            .count(),
        1
    );
    no_recovery_download.assert_async().await;
}

#[tokio::test]
async fn preexisting_reinstall_recovery_is_skipped_without_download_and_qualifies_conflicts() {
    quarantine_case(true, true).await;
}

#[tokio::test]
async fn quarantined_local_mod_outside_update_set_still_makes_conflicts_unverified() {
    quarantine_case(false, true).await;
}

#[tokio::test]
async fn shared_reinstall_commit_failure_restores_bytes_while_database_keeps_old_version() {
    let tmp = TempDir::new().unwrap();
    let url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &url).await.unwrap();
    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    let item = seed(
        &core,
        &pool,
        tmp.path(),
        "Commit failure",
        1,
        GameCode::Gimi,
    )
    .await;
    let original = std::fs::read(item.library_path.join("merged.ini")).unwrap();
    sqlx::query("CREATE TABLE commit_parent (id INTEGER PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE commit_child (id INTEGER PRIMARY KEY, parent_id INTEGER NOT NULL, FOREIGN KEY (parent_id) REFERENCES commit_parent(id) DEFERRABLE INITIALLY DEFERRED)").execute(&pool).await.unwrap();
    sqlx::query("CREATE TRIGGER fail_metadata_commit AFTER UPDATE OF version ON mods WHEN NEW.version = '2' BEGIN INSERT INTO commit_child (id, parent_id) VALUES (1, 404); END").execute(&pool).await.unwrap();
    let mut server = mockito::Server::new_async().await;
    upstream(&mut server, 1).await;
    let error = core
        .reinstall_gamebanana_mod_with_endpoints(
            &item.id,
            &Endpoints {
                api_base: server.url(),
            },
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("FOREIGN KEY constraint failed"),
        "must exercise COMMIT failure: {error}"
    );
    let version: String = sqlx::query_scalar("SELECT version FROM mods WHERE id = ?")
        .bind(&item.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        (
            std::fs::read_to_string(item.library_path.join("merged.ini")).unwrap(),
            version
        ),
        (String::from_utf8(original).unwrap(), "1".to_string()),
        "commit failure must restore old Library bytes and keep the database at version 1"
    );
    let witnesses: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reinstall_swaps")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(witnesses, 0, "completed rollback must retire its witness");
}

#[tokio::test]
async fn quarantine_during_batch_is_omitted_and_makes_conflicts_unverified() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    };

    let tmp = TempDir::new().unwrap();
    let url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &url).await.unwrap();
    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    let game = tmp.path().join("game");
    core.set_game_install_path(
        GameCode::Gimi,
        support::seed_game_install(GameCode::Gimi, &game),
    )
    .await
    .unwrap();
    std::fs::create_dir_all(game.join("Mods")).unwrap();
    let unrelated = seed(&core, &pool, tmp.path(), "Recovery", 1, GameCode::Gimi).await;
    core.set_enabled(&unrelated.id, true, &game.join("Mods"))
        .await
        .unwrap();
    // Leave a real, non-quarantined witness after a failed retirement.
    sqlx::query("CREATE TRIGGER reject_retirement BEFORE DELETE ON reinstall_swaps BEGIN SELECT RAISE(ABORT, 'recovery required'); END")
        .execute(&pool).await.unwrap();
    let mut server = mockito::Server::new_async().await;
    upstream(&mut server, 1).await;
    assert!(matches!(
        core.reinstall_gamebanana_mod_with_endpoints(
            &unrelated.id,
            &Endpoints {
                api_base: server.url()
            },
        )
        .await
        .unwrap_err(),
        Error::ReinstallRollbackFailed { .. }
    ));
    sqlx::query("DROP TRIGGER reject_retirement")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE mods SET source = 'local' WHERE id = ?")
        .bind(&unrelated.id)
        .execute(&pool)
        .await
        .unwrap();
    let healthy = seed(&core, &pool, tmp.path(), "Healthy", 2, GameCode::Gimi).await;
    upstream(&mut server, 2).await;
    let before = core.list_mods(GameCode::Gimi).await.unwrap();
    let before = before.iter().find(|item| item.id == unrelated.id).unwrap();
    assert!(
        before.enabled && before.reinstall_recovery.is_none(),
        "the unrelated Mod must not be quarantined when the batch starts"
    );

    let (paused_tx, mut paused_rx) = tokio::sync::mpsc::unbounded_channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let resume_rx = Mutex::new(resume_rx);
    let parked = AtomicBool::new(false);
    let batch_core = core.clone().with_crash_hook(Arc::new(move |point| {
        if point == gmm_lib::core::crash_points::REINSTALL_AFTER_WITNESS_COMMIT
            && !parked.swap(true, Ordering::Relaxed)
        {
            paused_tx.send(()).unwrap();
            // Dropping resume_tx on a test panic also releases the blocking task.
            let _ = resume_rx.lock().unwrap().recv();
        }
    }));
    let endpoints = Endpoints {
        api_base: server.url(),
    };
    let runtime = tokio::runtime::Handle::current();
    // Handle::block_on in spawn_blocking drives IO/timers only while another thread is in Runtime::block_on; the main thread is always awaiting here.
    let mut batch = tokio::task::spawn_blocking(move || {
        runtime.block_on(batch_core.apply_all_mod_updates_with_endpoints(
            GameCode::Gimi,
            &endpoints,
            |event| {
                if let ModUpdateProgress::Applying { item } = event {
                    assert_eq!(item.mod_id, healthy.id);
                }
            },
        ))
    });
    // Use the established reinstall pause point: the batch snapshot is past,
    // preparation has committed, and no writer transaction is open. Keep the
    // synchronous hook off the test runtime's thread so recovery can run.
    tokio::select! {
        signal = paused_rx.recv() => assert_eq!(signal, Some(()), "batch finished before recovery interleaving"),
        result = &mut batch => panic!("batch finished before recovery interleaving: {result:?}"),
    }
    std::fs::rename(&unrelated.library_path, tmp.path().join("held-old-tree")).unwrap();
    assert!(matches!(
        core.retry_reinstall_recovery(&unrelated.id).await.unwrap(),
        gmm_lib::core::ReinstallRecoveryOutcome::Quarantined { .. }
    ));
    resume_tx.send(()).unwrap();
    let report = batch.await.unwrap().unwrap();
    assert!(!report.stopped);
    assert_eq!(report.results.len(), 1);
    assert_eq!(report.results[0].outcome, ModUpdateOutcome::Updated);
    assert!(report.conflict_error.is_some(),
        "a Mod quarantined during the batch must make the Conflict reading unverified with the omission cause: {:?}", report.conflict_error);
    let conflicts = report.conflicts.unwrap();
    assert_eq!(
        conflicts.omitted,
        vec![unrelated.id.clone()],
        "the scan must name the Mod quarantined after the batch snapshot"
    );
    assert!(!conflicts.per_mod_count.contains_key(&unrelated.id));
}

#[tokio::test]
async fn disabled_quarantined_mod_is_not_omitted_and_does_not_qualify_conflicts() {
    quarantine_case(false, false).await;
}
