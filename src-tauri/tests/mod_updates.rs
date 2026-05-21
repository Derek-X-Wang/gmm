//! Slice 13c: per-mod update badges.
//!
//! Mockito drives every fetch so we never hit gamebanana.com in CI.
//! The reinstall flow is exercised end-to-end with a real bytes-on-
//! disk swap.

use std::fs::File;
use std::io::Write;

use gmm_lib::core::gamebanana::Endpoints;
use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

fn write_zip(zip_path: &std::path::Path, ini_body: &[u8]) {
    let f = File::create(zip_path).expect("create zip");
    let mut zw = ZipWriter::new(f);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zw.start_file("merged.ini", opts).expect("start");
    zw.write_all(ini_body).expect("write");
    zw.finish().expect("finish");
}

async fn ingest(
    server: &mut mockito::ServerGuard,
    core: &Core,
    id: u64,
    name: &str,
    version: &str,
    ini_body: &[u8],
    zip_path: &std::path::Path,
) -> gmm_lib::core::Mod {
    write_zip(zip_path, ini_body);
    let zip_bytes = std::fs::read(zip_path).expect("read zip");

    let api_path = format!("/apiv11/Mod/{id}");
    let file_path = format!("/file/{id}/mod.zip");
    let _api = server
        .mock("GET", mockito::Matcher::Regex(format!("{api_path}.*")))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "_idRow": {id},
                "_sName": "{name}",
                "_sProfileUrl": "https://gamebanana.com/mods/{id}",
                "_sVersion": "{version}",
                "_aSubmitter": {{ "_sName": "Author" }},
                "_aPreviewMedia": {{ "_aImages": [] }},
                "_aFiles": [{{
                    "_sFile": "mod.zip",
                    "_sDownloadUrl": "{base}{file_path}"
                }}]
            }}"#,
            base = server.url(),
            file_path = file_path,
        ))
        .create_async()
        .await;
    let _file = server
        .mock("GET", file_path.as_str())
        .with_status(200)
        .with_body(zip_bytes)
        .create_async()
        .await;

    let endpoints = Endpoints {
        api_base: server.url(),
    };
    core.import_gamebanana_with_endpoints(
        GameCode::Gimi,
        &format!("https://gamebanana.com/mods/{id}"),
        &endpoints,
    )
    .await
    .expect("ingest")
}

#[tokio::test]
async fn list_mod_updates_only_returns_gamebanana_mods() {
    let tmp = TempDir::new().expect("tmp");
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init");

    // Adopt a non-gamebanana mod — must NOT appear in list.
    let fixture = tmp.path().join("manual_fix");
    std::fs::create_dir_all(&fixture).unwrap();
    std::fs::write(fixture.join("merged.ini"), b"hash=1\n").unwrap();
    core.adopt_folder(GameCode::Gimi, &fixture, "Manual")
        .await
        .unwrap();

    // Ingest a gamebanana mod.
    let mut server = mockito::Server::new_async().await;
    let _imported = ingest(
        &mut server,
        &core,
        4242,
        "GB Mod",
        "0.1.0",
        b"hash=1\n",
        &tmp.path().join("gb.zip"),
    )
    .await;

    let rows = core.list_mod_updates(GameCode::Gimi).await.expect("list");
    assert_eq!(rows.len(), 1, "manual mod must not appear: {rows:?}");
    assert_eq!(rows[0].name, "GB Mod");
    assert_eq!(rows[0].installed_version.as_deref(), Some("0.1.0"));
    assert!(rows[0].update_check_enabled, "default-on");
}

#[tokio::test]
async fn check_now_writes_upstream_version_and_flags_when_ahead() {
    let tmp = TempDir::new().expect("tmp");
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init");

    // Initial ingest at v0.1.0.
    let mut ingest_server = mockito::Server::new_async().await;
    let imported = ingest(
        &mut ingest_server,
        &core,
        4242,
        "GB Mod",
        "0.1.0",
        b"hash=v1\n",
        &tmp.path().join("v1.zip"),
    )
    .await;

    // A second mockito server stands in for the *next* poll. It
    // returns v0.2.0 — upstream_version should bump and the row should
    // show upstream_ahead=true.
    let mut poll_server = mockito::Server::new_async().await;
    let id = 4242_u64;
    let api_path = format!("/apiv11/Mod/{id}");
    let _api = poll_server
        .mock("GET", mockito::Matcher::Regex(format!("{api_path}.*")))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "_idRow": {id},
                "_sName": "GB Mod",
                "_sProfileUrl": "https://gamebanana.com/mods/{id}",
                "_sVersion": "0.2.0",
                "_aSubmitter": {{ "_sName": "Author" }},
                "_aPreviewMedia": {{ "_aImages": [] }},
                "_aFiles": [{{ "_sFile": "mod.zip", "_sDownloadUrl": "{base}/file/{id}/mod.zip" }}]
            }}"#,
            base = poll_server.url(),
        ))
        .create_async()
        .await;

    let endpoints = Endpoints {
        api_base: poll_server.url(),
    };
    let rows = core
        .check_mod_updates_now_with_endpoints(GameCode::Gimi, &endpoints)
        .await
        .expect("check");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].mod_id, imported.id);
    assert_eq!(rows[0].upstream_version.as_deref(), Some("0.2.0"));
    assert!(rows[0].upstream_ahead, "v0.1.0 != v0.2.0 → ahead");
}

#[tokio::test]
async fn global_toggle_off_skips_network_but_still_lists_rows() {
    let tmp = TempDir::new().expect("tmp");
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init");

    let mut server = mockito::Server::new_async().await;
    let _imported = ingest(
        &mut server,
        &core,
        7777,
        "GB Mod",
        "0.1.0",
        b"hash=1\n",
        &tmp.path().join("a.zip"),
    )
    .await;

    core.set_mod_updates_globally_enabled(false)
        .await
        .expect("toggle");
    // No mock registered for this server URL → if the check were to
    // hit the network, the test would fail. The toggle must short-
    // circuit.
    let dummy = Endpoints {
        api_base: "http://localhost:1".to_string(),
    };
    let rows = core
        .check_mod_updates_now_with_endpoints(GameCode::Gimi, &dummy)
        .await
        .expect("check");
    assert_eq!(rows.len(), 1, "rows still listed");
    assert_eq!(
        rows[0].upstream_version, None,
        "no network = no upstream_version write"
    );
}

#[tokio::test]
async fn reinstall_replaces_library_bytes_and_bumps_version() {
    let tmp = TempDir::new().expect("tmp");
    let library_root = tmp.path().join("library");
    let game_install = tmp.path().join("Genshin");
    let game_mods = game_install.join("Mods");
    std::fs::create_dir_all(&game_mods).expect("mods dir");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init");
    core.set_game_install_path(GameCode::Gimi, &game_install)
        .await
        .expect("install");

    // Ingest v1 with hash=ALPHA, enable.
    let mut ingest_server = mockito::Server::new_async().await;
    let id = 9999_u64;
    let imported = ingest(
        &mut ingest_server,
        &core,
        id,
        "Bumpable",
        "1.0.0",
        b"[TextureOverrideX]\nhash = 0xALPHA\n",
        &tmp.path().join("v1.zip"),
    )
    .await;
    core.set_enabled(&imported.id, true, &game_mods)
        .await
        .expect("enable");

    let link = game_mods.join("Bumpable");
    let before = std::fs::read_to_string(link.join("merged.ini")).expect("read before");
    assert!(before.contains("0xALPHA"));

    // Mock the reinstall fetch on a fresh server returning v2 with new bytes.
    let mut reinstall_server = mockito::Server::new_async().await;
    let api_path = format!("/apiv11/Mod/{id}");
    let file_path = format!("/file/{id}/mod.zip");
    // Build the new zip bytes.
    let new_zip = tmp.path().join("v2.zip");
    write_zip(&new_zip, b"[TextureOverrideY]\nhash = 0xBETA\n");
    let new_bytes = std::fs::read(&new_zip).expect("read v2 zip");
    let _api = reinstall_server
        .mock("GET", mockito::Matcher::Regex(format!("{api_path}.*")))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "_idRow": {id},
                "_sName": "Bumpable v2",
                "_sProfileUrl": "https://gamebanana.com/mods/{id}",
                "_sVersion": "2.0.0",
                "_aSubmitter": {{ "_sName": "Author" }},
                "_aPreviewMedia": {{ "_aImages": [] }},
                "_aFiles": [{{ "_sFile": "mod.zip", "_sDownloadUrl": "{base}{file_path}" }}]
            }}"#,
            base = reinstall_server.url(),
        ))
        .create_async()
        .await;
    let _file = reinstall_server
        .mock("GET", file_path.as_str())
        .with_status(200)
        .with_body(new_bytes)
        .create_async()
        .await;

    let endpoints = Endpoints {
        api_base: reinstall_server.url(),
    };
    core.reinstall_gamebanana_mod_with_endpoints(&imported.id, &endpoints)
        .await
        .expect("reinstall");

    // Junction still resolves; bytes are now v2.
    let after = std::fs::read_to_string(link.join("merged.ini")).expect("read after");
    assert!(after.contains("0xBETA"), "library bytes updated: {after}");
    assert!(!after.contains("0xALPHA"));

    // Row metadata bumped.
    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    assert_eq!(listed[0].id, imported.id);
    assert_eq!(listed[0].name, "Bumpable v2", "name updated");
    assert_eq!(listed[0].version.as_deref(), Some("2.0.0"));
    assert!(listed[0].enabled, "previously enabled mod stays enabled");
}
