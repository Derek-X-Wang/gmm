//! Exercise the stop-before-first-poll race through the real command.
use gmm_lib::{
    commands::{apply_all_mod_updates, stop_mod_update_batch},
    core::{mod_update_batch::ModUpdateOutcome, Core, GameCode},
};
use tauri::Manager;

#[tokio::test]
async fn stop_before_command_first_poll_is_preserved() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &db_url)
        .await
        .unwrap();
    let folder = tmp.path().join("mod");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("merged.ini"),
        b"[TextureOverrideOld]\nhash = old\n",
    )
    .unwrap();
    let item = core
        .adopt_folder(GameCode::Gimi, &folder, "Waiting")
        .await
        .unwrap();
    let pool = sqlx::SqlitePool::connect(&db_url).await.unwrap();
    // Leave the submission ID absent so a regressed stop fails locally, without network work.
    sqlx::query(
        "UPDATE mods SET source = 'gamebanana', version = '1', upstream_version = '2' WHERE id = ?",
    )
    .bind(&item.id)
    .execute(&pool)
    .await
    .unwrap();
    let app = tauri::test::mock_app();
    app.manage(core);
    let batch_id = ulid::Ulid::new().to_string();
    let future = apply_all_mod_updates(
        app.state::<Core>(),
        GameCode::Gimi,
        batch_id.clone(),
        tauri::ipc::Channel::new(|_| Ok(())),
    );
    stop_mod_update_batch(batch_id).unwrap();
    let report = future.await.unwrap();
    assert!(
        report.stopped,
        "a stop before the command's first poll must not be lost"
    );
    assert!(matches!(
        report.results[0].outcome,
        ModUpdateOutcome::Skipped { .. }
    ));
    assert_eq!(
        std::fs::read(item.library_path.join("merged.ini")).unwrap(),
        b"[TextureOverrideOld]\nhash = old\n"
    );
}
