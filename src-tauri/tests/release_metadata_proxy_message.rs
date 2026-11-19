//! A dead proxy must say so when GMM reads upstream release metadata (#481).
//!
//! #468 taught both download paths to name an unreachable proxy. Reading the
//! release metadata that comes before a download was left behind: behind a
//! dead proxy, Reinstall, the Model Importer update check and the Loader
//! check reported a bare transport error, which reads as a problem with the
//! release or the user's internet rather than with the proxy.
//!
//! The shape follows `download_proxy_message.rs`: a client built the way
//! production builds it, behind a proxy whose port was bound and dropped, so
//! the connection is refused. Every Core surface below asks the real GitHub
//! host, but with a proxy configured the dead proxy is the only way out, so
//! no test reaches the internet.

mod support;

use gmm_lib::core::importer::{self, AssetPattern, Endpoints};
use gmm_lib::core::network::ProxyConfig;
use gmm_lib::core::{updates, Core, GameCode};
use tempfile::TempDir;

/// A base URL nothing listens on.
fn closed_port_url() -> String {
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", dead.local_addr().unwrap());
    drop(dead);
    url
}

async fn fresh_core(tmp: &TempDir) -> Core {
    let db = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(tmp.path().join("library"), &db).await.unwrap()
}

/// A Core whose saved proxy is dead.
async fn core_behind_dead_proxy(tmp: &TempDir) -> Core {
    let core = fresh_core(tmp).await;
    core.set_proxy_config(&ProxyConfig {
        url: Some(closed_port_url()),
        username: None,
        password: None,
    })
    .await
    .unwrap();
    core
}

fn any_zip() -> AssetPattern {
    AssetPattern::new(r".*\.zip").unwrap()
}

const PROXY_MESSAGE: &str = "Proxy unreachable";
const PROXY_HINT: &str = "Verify the URL, credentials, and that the proxy is running.";

fn assert_names_the_proxy(error: &str) {
    assert!(error.contains(PROXY_MESSAGE), "{error}");
    assert!(error.contains(PROXY_HINT), "{error}");
}

#[tokio::test]
async fn a_metadata_read_through_a_dead_proxy_names_the_proxy() {
    let tmp = TempDir::new().unwrap();
    let client = core_behind_dead_proxy(&tmp)
        .await
        .http_client()
        .await
        .unwrap();

    let error = importer::fetch_latest_release(
        &client,
        &Endpoints::default(),
        "SpectrumQT/SRMI-Package",
        &any_zip(),
        None,
        true,
    )
    .await
    .expect_err("a dead proxy cannot serve the release")
    .to_string();

    assert_names_the_proxy(&error);
    assert!(
        error.contains("could not read the upstream release"),
        "still names the subsystem that failed: {error}"
    );
}

#[tokio::test]
async fn a_metadata_read_with_no_proxy_keeps_the_plain_transport_error() {
    let endpoints = Endpoints {
        api_base: closed_port_url(),
    };
    let url = format!("{}/repos/o/r/releases/latest", endpoints.api_base);

    let error = importer::fetch_latest_release(
        &reqwest::Client::new(),
        &endpoints,
        "o/r",
        &any_zip(),
        None,
        false,
    )
    .await
    .expect_err("nothing listens there")
    .to_string();

    assert!(
        error.starts_with(&format!(
            "could not read the upstream release: GET {url}: error sending request"
        )),
        "the wording is unchanged: {error}"
    );
    assert!(
        !error.to_lowercase().contains("proxy"),
        "no proxy is configured: {error}"
    );
}

#[tokio::test]
async fn an_http_status_or_bad_json_is_not_described_as_a_proxy_failure() {
    let mut server = mockito::Server::new_async().await;
    let _gone = server
        .mock("GET", "/repos/o/gone/releases/latest")
        .with_status(404)
        .create_async()
        .await;
    let _garbled = server
        .mock("GET", "/repos/o/garbled/releases/latest")
        .with_status(200)
        .with_body("not json")
        .create_async()
        .await;
    let endpoints = Endpoints {
        api_base: server.url(),
    };
    let client = reqwest::Client::new();

    // `true` must not turn either into a proxy complaint: neither is a
    // transport failure, so the proxy evidently answered.
    let status =
        importer::fetch_latest_release(&client, &endpoints, "o/gone", &any_zip(), None, true)
            .await
            .expect_err("404")
            .to_string();
    let json =
        importer::fetch_latest_release(&client, &endpoints, "o/garbled", &any_zip(), None, true)
            .await
            .expect_err("not json")
            .to_string();

    assert!(status.contains("GitHub returned 404"), "{status}");
    assert!(json.contains("parse JSON from"), "{json}");
    for error in [status, json] {
        assert!(!error.to_lowercase().contains("proxy"), "{error}");
    }
}

// Below: every product surface that reads release metadata reaches the
// classification with the proxy fact it actually has.

#[tokio::test]
async fn the_model_importer_reinstall_names_the_dead_proxy() {
    let tmp = TempDir::new().unwrap();
    let core = core_behind_dead_proxy(&tmp).await;
    let game_dir = tmp.path().join("Genshin");
    support::seed_game_install(GameCode::Gimi, &game_dir);
    core.set_game_install_path(GameCode::Gimi, &game_dir)
        .await
        .unwrap();

    let error = core
        .install_importer(GameCode::Gimi)
        .await
        .expect_err("a dead proxy cannot serve the release")
        .to_string();

    assert_names_the_proxy(&error);
}

#[tokio::test]
async fn the_latest_model_importer_release_lookup_names_the_dead_proxy() {
    let tmp = TempDir::new().unwrap();
    let core = core_behind_dead_proxy(&tmp).await;

    let error = core
        .latest_importer_release(GameCode::Gimi)
        .await
        .expect_err("a dead proxy cannot serve the release")
        .to_string();

    assert_names_the_proxy(&error);
}

#[tokio::test]
async fn the_model_importer_update_check_names_the_dead_proxy() {
    let tmp = TempDir::new().unwrap();
    let core = core_behind_dead_proxy(&tmp).await;

    let status = core
        .check_importer_update_for(GameCode::Gimi)
        .await
        .expect("the failure rides inside the status");

    assert_names_the_proxy(&status.check_error.expect("the check failed"));
}

#[tokio::test]
async fn the_model_importer_update_check_by_repo_names_the_dead_proxy() {
    let tmp = TempDir::new().unwrap();
    let core = core_behind_dead_proxy(&tmp).await;

    let status = core
        .check_importer_update(GameCode::Gimi, "SilentNightSound/GIMI-Package", r".*\.zip")
        .await
        .expect("the failure rides inside the status");

    assert_names_the_proxy(&status.check_error.expect("the check failed"));
}

#[tokio::test]
async fn the_loader_check_names_the_dead_proxy() {
    let tmp = TempDir::new().unwrap();
    let core = core_behind_dead_proxy(&tmp).await;

    let status = core
        .check_loader_update_from(updates::LOADER_REPO, updates::LOADER_ASSET_PATTERN)
        .await
        .expect("the failure rides inside the status");

    assert_names_the_proxy(&status.check_error.expect("the check failed"));
}
