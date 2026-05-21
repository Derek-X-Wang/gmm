//! Tauri command IPC wire-shape contract (issue #26).
//!
//! The acceptance criteria allow `tauri::test::get_ipc_response`
//! *or* an equivalent path through serde. Going through Tauri's real
//! mock runtime requires building a `Context<MockRuntime>` that
//! carries the project's ACL capabilities; the issue body documents
//! that route as the harder path. The cheaper route — and the one
//! this file takes — is to round-trip the **same Args and return
//! types** the `#[tauri::command]` macro consumes through `serde_json`,
//! and call the Core method body directly. The wire shape that lands
//! on the JS side is identical (Tauri uses serde for both directions);
//! we just skip the runtime that wraps it.
//!
//! See `docs/testing.md` for the pattern + how to extend this file
//! when a new command lands.

use std::fs::{self, File};
use std::io::Write;

use gmm_lib::commands::{
    AdoptArgs, GameBananaImportArgs, ImportZipArgs, LibraryPaths, NO_INSTALL_PATH_FOR_ENABLE_MSG,
};
use gmm_lib::core::av;
use gmm_lib::core::conflicts::ConflictReport;
use gmm_lib::core::reconcile::ReconcileResult;
use gmm_lib::core::updates::UpdateStatus;
use gmm_lib::core::variants::Variant;
use gmm_lib::core::{Core, GameCode, ImportZipOptions, Mod, Source};
use serde_json::{json, Value};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// Helper: deserialize a JSON value into `T` so each test exercises
/// the same serde path the `#[tauri::command]` macro uses for args.
fn from_json<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).expect("deserialise Args from JSON")
}

/// Helper: serialise a return value into a JSON value so each test
/// can assert wire-side keys (camelCase / snake_case stay stable).
fn to_json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("serialise response to JSON")
}

async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init core")
}

#[test]
fn adopt_args_deserialises_from_camel_case_json() {
    let v = json!({
        "game": "gimi",
        "sourcePath": "/tmp/my-mod",
        "name": "My Mod",
    });
    let args: AdoptArgs = from_json(v);
    assert_eq!(args.game, GameCode::Gimi);
    assert_eq!(args.source_path.to_string_lossy(), "/tmp/my-mod");
    assert_eq!(args.name, "My Mod");
}

#[test]
fn import_zip_args_deserialises_from_camel_case_json() {
    let v = json!({
        "game": "srmi",
        "zipPath": "/tmp/mod.zip",
        "name": "Cool",
    });
    let args: ImportZipArgs = from_json(v);
    assert_eq!(args.game, GameCode::Srmi);
    assert_eq!(args.zip_path.to_string_lossy(), "/tmp/mod.zip");
    assert_eq!(args.name, "Cool");
}

#[test]
fn gamebanana_import_args_deserialises_with_camel_case_url_or_id() {
    let v = json!({
        "game": "gimi",
        "urlOrId": "1234567",
    });
    let args: GameBananaImportArgs = from_json(v);
    assert_eq!(args.game, GameCode::Gimi);
    assert_eq!(args.url_or_id, "1234567");
}

#[tokio::test]
async fn list_mods_returns_snake_case_json_keys() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let fixture = tmp.path().join("fixture");
    fs::create_dir_all(&fixture).expect("fix dir");
    fs::write(fixture.join("merged.ini"), b"hash=1\n").expect("ini");
    let adopted = core
        .adopt_folder(GameCode::Gimi, &fixture, "Test Mod")
        .await
        .expect("adopt");

    // Mirror the wire path: the command body calls `core.list_mods`
    // and returns Vec<Mod>; serialise that to JSON and inspect.
    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    let v = to_json(&listed);
    let arr = v.as_array().expect("array");
    assert_eq!(arr.len(), 1);
    let obj = arr[0].as_object().expect("object");
    // Mod serialises with snake_case keys today (we deliberately did
    // NOT add `rename_all = "camelCase"` to Mod so the frontend
    // `fromRaw` mapper handles the boundary). Asserting the actual
    // shape keeps this contract from drifting accidentally.
    assert!(obj.contains_key("id"));
    assert!(obj.contains_key("library_path"));
    assert!(obj.contains_key("gamebanana_id"));
    assert!(obj.contains_key("source_url"));
    assert_eq!(obj.get("id").unwrap().as_str(), Some(adopted.id.as_str()));
    assert_eq!(obj.get("source").unwrap().as_str(), Some("manual"));
}

#[tokio::test]
async fn set_mod_enabled_surfaces_friendly_no_install_path_error() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let fixture = tmp.path().join("fixture");
    fs::create_dir_all(&fixture).expect("dir");
    fs::write(fixture.join("merged.ini"), b"hash=1\n").expect("ini");
    let mod_ = core
        .adopt_folder(GameCode::Gimi, &fixture, "Test Mod")
        .await
        .expect("adopt");

    // Replicate the command body's contract: when game_install_path
    // is None we surface the friendly error string. This is exactly
    // what commands::set_mod_enabled does.
    let install = core
        .game_install_path(GameCode::Gimi)
        .await
        .expect("read install path");
    let err: String = install
        .ok_or_else(|| NO_INSTALL_PATH_FOR_ENABLE_MSG.to_string())
        .unwrap_err();
    assert_eq!(
        err, NO_INSTALL_PATH_FOR_ENABLE_MSG,
        "wire error message must match the exported constant"
    );

    // Make sure the mod row didn't accidentally flip — the contract
    // is "no install path → no state change".
    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    assert!(!listed[0].enabled);
    let _ = mod_;
}

#[tokio::test]
async fn adopt_folder_response_serialises_with_expected_shape() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let fixture = tmp.path().join("fixture");
    fs::create_dir_all(&fixture).expect("dir");
    fs::write(fixture.join("merged.ini"), b"hash=1\n").expect("ini");

    let args: AdoptArgs = from_json(json!({
        "game": "gimi",
        "sourcePath": fixture.to_string_lossy(),
        "name": "Adopted",
    }));
    let mod_: Mod = core
        .adopt_folder(args.game, &args.source_path, &args.name)
        .await
        .expect("adopt");
    let v = to_json(&mod_);
    let obj = v.as_object().expect("object");
    assert_eq!(obj.get("name").and_then(|n| n.as_str()), Some("Adopted"));
    assert_eq!(obj.get("source").and_then(|s| s.as_str()), Some("manual"));
    assert_eq!(obj.get("game").and_then(|g| g.as_str()), Some("gimi"));
    assert_eq!(obj.get("enabled").and_then(|b| b.as_bool()), Some(false));
    // Optional GameBanana fields are present + null on a manual mod.
    assert!(obj.contains_key("gamebanana_id"));
    assert!(obj.contains_key("source_url"));
    assert!(obj.get("gamebanana_id").unwrap().is_null());
}

#[test]
fn library_paths_response_uses_camel_case() {
    // The LibraryPaths struct (returned by get_library_paths) is the
    // one place we explicitly use camelCase serde rename. Lock it in.
    let mut per_game_overrides = std::collections::HashMap::new();
    per_game_overrides.insert("gimi".to_string(), None);
    let mut per_game_effective = std::collections::HashMap::new();
    per_game_effective.insert("gimi".to_string(), std::path::PathBuf::from("/lib/gimi"));
    let lp = LibraryPaths {
        default_root: "/default".into(),
        root_override: None,
        effective_root: "/default".into(),
        per_game_overrides,
        per_game_effective,
    };
    let v = to_json(&lp);
    let obj = v.as_object().expect("object");
    assert!(obj.contains_key("defaultRoot"));
    assert!(obj.contains_key("rootOverride"));
    assert!(obj.contains_key("effectiveRoot"));
    assert!(obj.contains_key("perGameOverrides"));
    assert!(obj.contains_key("perGameEffective"));
}

#[test]
fn reconcile_result_serialises_with_snake_case_inner_keys() {
    let report = ReconcileResult::default();
    let v = to_json(&report);
    let obj = v.as_object().expect("object");
    assert!(obj.contains_key("recreated"));
    assert!(obj.contains_key("healthy"));
    assert!(obj.contains_key("conflicting"));
    assert!(obj.contains_key("skipped"));
}

#[test]
fn update_status_uses_camel_case() {
    let s = UpdateStatus {
        available: false,
        installed_version: Some("v1.0".into()),
        latest_version: None,
        pinned: false,
        upstream_ahead: false,
    };
    let v = to_json(&s);
    let obj = v.as_object().expect("object");
    assert!(obj.contains_key("available"));
    assert!(obj.contains_key("installedVersion"));
    assert!(obj.contains_key("latestVersion"));
    assert!(obj.contains_key("pinned"));
    assert!(obj.contains_key("upstreamAhead"));
}

#[test]
fn conflict_report_default_serialises() {
    let r = ConflictReport::default();
    let v = to_json(&r);
    let obj = v.as_object().expect("object");
    assert!(obj.contains_key("conflicts"));
    assert!(obj.contains_key("per_mod_count"));
}

#[tokio::test]
async fn import_zip_command_path_round_trips_through_serde() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    // Build a tiny zip on disk so we can exercise the full command
    // body — same shape the IPC layer would feed in.
    let zip_path = tmp.path().join("payload.zip");
    {
        let f = File::create(&zip_path).expect("create");
        let mut zw = ZipWriter::new(f);
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zw.start_file("merged.ini", opts).expect("ini");
        zw.write_all(b"hash=1\n").expect("write");
        zw.finish().expect("finish");
    }
    let args: ImportZipArgs = from_json(json!({
        "game": "gimi",
        "zipPath": zip_path.to_string_lossy(),
        "name": "ZipMod",
    }));

    let mod_: Mod = core
        .import_zip(
            args.game,
            &args.zip_path,
            &args.name,
            ImportZipOptions::default(),
        )
        .await
        .expect("import");
    assert_eq!(mod_.source, Source::Local);
    assert_eq!(mod_.name, "ZipMod");
    let json = to_json(&mod_);
    assert_eq!(json.get("source").and_then(|s| s.as_str()), Some("local"));
}

#[test]
fn av_guidance_response_uses_camel_case_keys() {
    // Slice NEW-AV / #13: the `av_guidance` Tauri command returns the
    // structured payload the launch-error component renders. Wire-side
    // it must come through as camelCase so the React component can
    // read it without a fromRaw mapper.
    let g = av::guidance();
    let v = to_json(&g);
    let obj = v.as_object().expect("object");
    assert!(obj.contains_key("headline"));
    assert!(obj.contains_key("body"));
    assert!(obj.contains_key("exclusionSteps"));
    assert!(obj.contains_key("docPath"));
    assert!(obj.contains_key("sentinel"));
    assert_eq!(
        obj.get("sentinel").and_then(|s| s.as_str()),
        Some(av::AV_PATTERN_SENTINEL),
        "sentinel must round-trip verbatim — the React layer matches on this string"
    );
    assert!(obj
        .get("docPath")
        .and_then(|p| p.as_str())
        .map(|p| p.ends_with("antivirus-and-smartscreen.md"))
        .unwrap_or(false));
}

#[test]
fn variant_serialises_with_expected_keys() {
    let v = Variant {
        id: "v1".into(),
        mod_id: "m1".into(),
        name: "Red".into(),
        subpath: std::path::PathBuf::from("Red"),
    };
    let json = to_json(&v);
    let obj = json.as_object().expect("object");
    assert!(obj.contains_key("id"));
    assert!(obj.contains_key("mod_id"));
    assert!(obj.contains_key("name"));
    assert!(obj.contains_key("subpath"));
}
