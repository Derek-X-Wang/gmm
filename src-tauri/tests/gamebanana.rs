//! Slice 11: GameBanana URL ingest.
//!
//! All network paths are exercised against a `mockito` server so CI
//! never touches gamebanana.com. The full ingest test seeds both the
//! metadata endpoint and the file-download endpoint.

use std::fs::File;
use std::io::Write;

use gmm_lib::core::gamebanana::{
    download_to, fetch_submission, parse_url_or_id, Endpoints, GameBananaSubmission,
};
use gmm_lib::core::{Core, GameCode, Source};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

#[test]
fn parse_accepts_bare_id_and_url_shapes() {
    assert_eq!(parse_url_or_id("1234567"), Some(1234567));
    assert_eq!(parse_url_or_id("  1234567  "), Some(1234567));
    assert_eq!(
        parse_url_or_id("https://gamebanana.com/mods/1234567"),
        Some(1234567),
    );
    assert_eq!(
        parse_url_or_id("https://gamebanana.com/wips/42?utm_source=anything"),
        Some(42),
    );
    assert_eq!(parse_url_or_id("gamebanana.com/mods/77"), Some(77));
    assert!(parse_url_or_id("").is_none());
    assert!(parse_url_or_id("https://example.com/").is_none());
    assert!(parse_url_or_id("https://gamebanana.com/").is_none());
}

#[tokio::test]
async fn fetch_submission_parses_apiv11_payload() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock(
            "GET",
            mockito::Matcher::Regex("/apiv11/Mod/12345.*".to_string()),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
                "_idRow": 12345,
                "_sName": "Hu Tao Outfit",
                "_sProfileUrl": "https://gamebanana.com/mods/12345",
                "_sVersion": "1.2.3",
                "_aSubmitter": { "_sName": "ModderName" },
                "_aPreviewMedia": {
                    "_aImages": [{
                        "_sBaseUrl": "https://images.gamebanana.com/img/ss",
                        "_sFile": "hutao.png"
                    }]
                },
                "_aFiles": [{
                    "_sFile": "hutao_v1.zip",
                    "_sDownloadUrl": "https://files.gamebanana.com/12345/hutao_v1.zip"
                }]
            }"#,
        )
        .create_async()
        .await;

    let client = reqwest::Client::builder().build().expect("build client");
    let endpoints = Endpoints {
        api_base: server.url(),
    };
    let s: GameBananaSubmission = fetch_submission(&client, &endpoints, 12345)
        .await
        .expect("fetch");

    assert_eq!(s.id, 12345);
    assert_eq!(s.name, "Hu Tao Outfit");
    assert_eq!(s.author.as_deref(), Some("ModderName"));
    assert_eq!(s.version.as_deref(), Some("1.2.3"));
    assert_eq!(
        s.screenshot_url.as_deref(),
        Some("https://images.gamebanana.com/img/ss/hutao.png"),
    );
    assert_eq!(s.file_name, "hutao_v1.zip");
}

#[tokio::test]
async fn download_streams_bytes_to_disk() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/file/123/payload.zip")
        .with_status(200)
        .with_header("content-type", "application/zip")
        .with_body(b"PK\x03\x04fake-zip-bytes")
        .create_async()
        .await;
    let url = format!("{}/file/123/payload.zip", server.url());
    let client = reqwest::Client::builder().build().expect("build");
    let tmp = TempDir::new().expect("tmp");
    let dest = tmp.path().join("payload.zip");
    let n = download_to(&client, &url, &dest).await.expect("download");
    assert!(n > 0);
    assert!(dest.is_file());
    let bytes = std::fs::read(&dest).expect("read");
    assert_eq!(&bytes[..2], b"PK", "looks like a zip header");
}

#[tokio::test]
async fn full_import_records_source_gamebanana_and_metadata() {
    // Mock the API + the file download.
    let mut server = mockito::Server::new_async().await;
    let id = 999_001_u64;
    let api_path = format!("/apiv11/Mod/{id}");
    let file_path = format!("/file/{id}/mod.zip");

    let _api = server
        .mock("GET", mockito::Matcher::Regex(format!("{api_path}.*")))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "_idRow": {id},
                "_sName": "Mock Mod",
                "_sProfileUrl": "https://gamebanana.com/mods/{id}",
                "_sVersion": "0.1.0",
                "_aSubmitter": {{ "_sName": "Mock Author" }},
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

    // Build a real zip with one .ini inside so the slice-1b ingest path
    // accepts it.
    let tmp = TempDir::new().expect("tmp");
    let zip_path = tmp.path().join("source.zip");
    {
        let f = File::create(&zip_path).expect("create zip");
        let mut zw = ZipWriter::new(f);
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zw.start_file("merged.ini", opts).expect("start");
        zw.write_all(b"[TextureOverrideMock]\nhash = 0xABC\n")
            .expect("write");
        zw.finish().expect("finish");
    }
    let zip_bytes = std::fs::read(&zip_path).expect("read zip");
    let _file = server
        .mock("GET", file_path.as_str())
        .with_status(200)
        .with_header("content-type", "application/zip")
        .with_body(zip_bytes.clone())
        .create_async()
        .await;

    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init");

    let endpoints = Endpoints {
        api_base: server.url(),
    };
    let imported = core
        .import_gamebanana_with_endpoints(
            GameCode::Gimi,
            &format!("https://gamebanana.com/mods/{id}"),
            &endpoints,
        )
        .await
        .expect("import");

    assert_eq!(imported.source, Source::Gamebanana);
    assert_eq!(imported.gamebanana_id, Some(id));
    assert_eq!(
        imported.source_url.as_deref(),
        Some(format!("https://gamebanana.com/mods/{id}").as_str()),
    );
    assert_eq!(imported.author.as_deref(), Some("Mock Author"));
    assert_eq!(imported.version.as_deref(), Some("0.1.0"));

    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].source, Source::Gamebanana);
    assert_eq!(listed[0].gamebanana_id, Some(id));
}
