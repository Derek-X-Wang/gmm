//! Log capture is isolated from concurrent snapshot tests that hit the same callsite.
use gmm_lib::core::Core;
use std::io::Read;

fn bundled_settings(bundle: &std::path::Path) -> serde_json::Value {
    let mut archive =
        zip::ZipArchive::new(std::fs::File::open(bundle).expect("open bundle")).expect("archive");
    let mut json = String::new();
    archive
        .by_name("settings.json")
        .expect("settings")
        .read_to_string(&mut json)
        .expect("read settings");
    serde_json::from_str(&json).expect("snapshot JSON")
}

#[tokio::test]
async fn unreadable_skip_is_logged_and_distinguished_from_absence_in_the_bundle() {
    use tauri::Manager;
    use tracing::instrument::WithSubscriber;

    let temp = tempfile::tempdir().expect("temp");
    let db = format!("sqlite://{}/gmm.db?mode=rwc", temp.path().display());
    let core = Core::new(temp.path().join("library"), &db)
        .await
        .expect("core");
    let absent = serde_json::to_value(core.settings_snapshot().await.expect("fresh snapshot"))
        .expect("snapshot JSON");
    assert_eq!(absent["updater_skip_status"], "absent");
    let stored = "DO_NOT_EXPORT_RAW_SKIP: not JSON";
    let pool = sqlx::SqlitePool::connect(&db).await.expect("open storage");
    gmm_lib::core::settings::put(
        &pool,
        gmm_lib::core::settings::keys::skipped_update(),
        Some(stored),
    )
    .await
    .expect("seed damaged skip");
    let log_dir = temp.path().join("logs");
    std::fs::create_dir(&log_dir).expect("logs directory");
    let log_path = log_dir.join("gmm.log");
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(
            std::fs::File::create(&log_path).expect("log"),
        ))
        .finish();
    let snapshot = core
        .settings_snapshot()
        .with_subscriber(subscriber)
        .await
        .expect("snapshot with damaged skip");
    let value = serde_json::to_value(&snapshot).expect("snapshot JSON");
    assert!(value["updater_skipped_build"].is_null());
    assert_eq!(value["updater_skip_status"], "unreadable");
    assert_eq!(
        core.skipped_update()
            .await
            .expect("raw skip after snapshot"),
        Some(stored.into()),
        "taking a snapshot must preserve the damaged bytes"
    );
    let app = tauri::test::mock_app();
    app.manage(core);
    let bundle = temp.path().join("diagnostics.zip");
    gmm_lib::commands::export_diagnostics_bundle(app.state(), log_dir, bundle.clone(), None)
        .await
        .expect("export with damaged skip");
    let bundled = bundled_settings(&bundle);
    assert!(bundled["updater_skipped_build"].is_null());
    assert_eq!(bundled["updater_skip_status"], "unreadable");
    assert_eq!(
        app.state::<Core>()
            .skipped_update()
            .await
            .expect("raw skip after export"),
        Some(stored.into()),
        "export must preserve the damaged bytes"
    );
    let mut archive =
        zip::ZipArchive::new(std::fs::File::open(bundle).expect("bundle")).expect("archive");
    let mut log = String::new();
    archive
        .by_name("logs/gmm.log")
        .expect("bundled log")
        .read_to_string(&mut log)
        .expect("log text");
    assert!(log.contains("stored updater skip is unreadable"), "{log}");
    assert!(
        !log.contains(stored),
        "unreadable raw storage must not be disclosed"
    );
    assert!(
        bundled["updater_error"].is_null(),
        "no UI updater error is introduced"
    );
}
