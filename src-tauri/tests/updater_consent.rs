//! Updater decisions use the same database that survives installer upgrades.
use gmm_lib::core::{
    diagnostics::{build_bundle, DEFAULT_BUNDLE_LOG_DAYS},
    Core,
};
use std::io::Read;

#[tokio::test]
async fn skipped_build_survives_database_reopen_and_is_in_diagnostics() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("library");
    let db = format!("sqlite://{}/gmm.db?mode=rwc", temp.path().display());
    let build = r#"{"version":"0.4.0","identity":"signed-alpha.2"}"#;
    {
        let core = Core::new(root.clone(), &db).await.expect("first launch");
        assert_eq!(core.skipped_update().await.expect("initial skip"), None);
        core.set_skipped_update(build).await.expect("remember skip");
    }
    let restarted = Core::new(root, &db).await.expect("restart");
    assert_eq!(
        restarted.skipped_update().await.expect("read skip"),
        Some(build.into())
    );
    let mut snapshot = restarted.settings_snapshot().await.expect("snapshot");
    let skipped = snapshot
        .updater_skipped_build
        .as_ref()
        .expect("skipped build");
    assert_eq!(skipped.version, "0.4.0");
    assert_eq!(skipped.identity, "signed-alpha.2");
    snapshot.updater_error = Some("Update check failed: offline".into());
    let bundle = temp.path().join("diagnostics.zip");
    build_bundle(
        &temp.path().join("logs"),
        &snapshot,
        &bundle,
        DEFAULT_BUNDLE_LOG_DAYS,
    )
    .expect("bundle");
    let mut archive =
        zip::ZipArchive::new(std::fs::File::open(bundle).expect("open bundle")).expect("archive");
    let mut json = String::new();
    archive
        .by_name("settings.json")
        .expect("settings")
        .read_to_string(&mut json)
        .expect("read settings");
    let value: serde_json::Value = serde_json::from_str(&json).expect("snapshot JSON");
    assert_eq!(value["updater_skipped_build"]["version"], "0.4.0");
    assert_eq!(value["updater_skipped_build"]["identity"], "signed-alpha.2");
    assert_eq!(value["updater_error"], "Update check failed: offline");
}

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
async fn diagnostics_command_exports_with_unparseable_stored_skips() {
    use tauri::Manager;

    // A downgrade can leave valid JSON of an older shape; damaged storage can
    // also contain invalid JSON. Neither may disable the diagnostics escape hatch.
    for stored in [
        r#"{"version":"0.4.0","identity":123}"#,
        r#"{"version":"0.4.0"}"#,
        "not JSON",
    ] {
        let temp = tempfile::tempdir().expect("temp");
        let db = format!("sqlite://{}/gmm.db?mode=rwc", temp.path().display());
        let core = Core::new(temp.path().join("library"), &db)
            .await
            .expect("core");
        let pool = sqlx::SqlitePool::connect(&db).await.expect("open storage");
        gmm_lib::core::settings::put(
            &pool,
            gmm_lib::core::settings::keys::skipped_update(),
            Some(stored),
        )
        .await
        .expect("seed incompatible skip without the current write validation");
        let app = tauri::test::mock_app();
        app.manage(core);
        let bundle = temp.path().join("diagnostics.zip");
        gmm_lib::commands::export_diagnostics_bundle(
            app.state(),
            temp.path().join("logs"),
            bundle.clone(),
            None,
        )
        .await
        .expect("an unparseable skip must not block diagnostics export");
        assert!(bundled_settings(&bundle)["updater_skipped_build"].is_null());
        assert_eq!(
            app.state::<Core>()
                .skipped_update()
                .await
                .expect("raw skip"),
            Some(stored.into()),
            "diagnostics reads must not delete or rewrite the incompatible value",
        );
    }
}

#[tokio::test]
async fn diagnostics_command_writes_the_supplied_updater_error() {
    use tauri::Manager;

    let temp = tempfile::tempdir().expect("temp");
    let core = Core::new(temp.path().join("library"), "sqlite::memory:")
        .await
        .expect("core");
    let app = tauri::test::mock_app();
    app.manage(core);
    let bundle = temp.path().join("diagnostics.zip");
    gmm_lib::commands::export_diagnostics_bundle(
        app.state(),
        temp.path().join("logs"),
        bundle.clone(),
        Some("Update check failed: offline".into()),
    )
    .await
    .expect("export command");
    assert_eq!(
        bundled_settings(&bundle)["updater_error"],
        "Update check failed: offline"
    );
}

#[tokio::test]
async fn invalid_skip_writes_are_rejected_without_replacing_a_valid_skip() {
    let temp = tempfile::tempdir().expect("temp");
    let core = Core::new(temp.path().join("library"), "sqlite::memory:")
        .await
        .expect("core");
    let valid = r#"{"version":"0.4.0","identity":"signed-alpha.2"}"#;
    core.set_skipped_update(valid).await.expect("valid skip");
    for invalid in [
        r#"{"version":"0.4.0","identity":123}"#,
        r#"{"version":"0.4.0"}"#,
        "not JSON",
    ] {
        let error = core
            .set_skipped_update(invalid)
            .await
            .expect_err("invalid skip write");
        assert!(error.to_string().contains("invalid skipped update"));
        assert_eq!(
            core.skipped_update().await.expect("valid skip retained"),
            Some(valid.into())
        );
    }
}
