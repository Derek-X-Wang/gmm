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
