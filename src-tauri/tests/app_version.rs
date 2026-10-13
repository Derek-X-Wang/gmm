use gmm_lib::{commands, core::Core};
use std::io::Read;
use tauri::Manager;

fn compiled_context() -> tauri::Context<tauri::test::MockRuntime> {
    tauri::generate_context!()
}

#[test]
fn version_command_returns_the_compiled_tauri_package_version() {
    // This is the same generated context used by the shipped application.
    let app = tauri::test::mock_builder()
        .build(compiled_context())
        .expect("app with compiled context");
    let expected = app.package_info().version.to_string();
    let build_input: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    assert_eq!(expected, build_input["version"].as_str().unwrap());
    assert_eq!(
        commands::get_app_version(app.handle().clone()).unwrap(),
        expected
    );

    // A different built context must not be replaced with a fixed version.
    let mut context = compiled_context();
    context.package_info_mut().version.patch += 1;
    let changed = tauri::test::mock_builder()
        .build(context)
        .expect("changed build");
    assert_eq!(
        commands::get_app_version(changed.handle().clone()).unwrap(),
        changed.package_info().version.to_string()
    );
    assert_ne!(changed.package_info().version.to_string(), expected);
}

#[tokio::test]
async fn diagnostics_export_records_the_running_package_version() {
    let temp = tempfile::tempdir().expect("temp");
    let core = Core::new(temp.path().join("library"), "sqlite::memory:")
        .await
        .expect("core");
    let app = tauri::test::mock_builder()
        .build(compiled_context())
        .expect("compiled context");
    app.manage(core);
    let expected = app.package_info().version.to_string();
    let bundle = temp.path().join("diagnostics.zip");
    commands::export_diagnostics_bundle(
        app.handle().clone(),
        app.state(),
        temp.path().join("logs"),
        bundle.clone(),
        None,
    )
    .await
    .expect("export");
    let mut archive = zip::ZipArchive::new(std::fs::File::open(bundle).unwrap()).unwrap();
    let mut settings = String::new();
    archive
        .by_name("settings.json")
        .unwrap()
        .read_to_string(&mut settings)
        .unwrap();
    let snapshot: serde_json::Value = serde_json::from_str(&settings).unwrap();
    assert_eq!(snapshot["app_version"], expected);
}
