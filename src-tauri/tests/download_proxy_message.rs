//! A dead proxy must say so on both download paths (#468).
//!
//! Installing a Mod and installing a Model Importer each download one file.
//! Their transport failure used to reach the user as a bare reqwest error,
//! which reads as a problem with the Mod or the release rather than with the
//! user's proxy. Each helper now classifies its own transport failure, the
//! same way `fetch_submission` does (#460), and the caller resolves
//! `proxy_configured`.
//!
//! The closed-port idiom matches `mod_updates.rs`: bind, then drop, so the
//! connection is refused. The destination host never resolves, so with a
//! proxy configured the dead proxy is the only way out.

use gmm_lib::core::network::ProxyConfig;
use gmm_lib::core::{gamebanana, importer, Core};
use tempfile::TempDir;

/// A base URL nothing listens on.
fn closed_port_url() -> String {
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", dead.local_addr().unwrap());
    drop(dead);
    url
}

/// A client built the way production builds it, behind a dead proxy.
async fn client_behind_dead_proxy(tmp: &TempDir) -> reqwest::Client {
    let db = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &db).await.unwrap();
    core.set_proxy_config(&ProxyConfig {
        url: Some(closed_port_url()),
        username: None,
        password: None,
    })
    .await
    .unwrap();
    core.http_client().await.unwrap()
}

const UNRESOLVABLE: &str = "http://files.gmm-test.invalid/payload.zip";

#[tokio::test]
async fn a_mod_download_through_a_dead_proxy_names_the_proxy() {
    let tmp = TempDir::new().unwrap();
    let client = client_behind_dead_proxy(&tmp).await;
    let dest = tmp.path().join("downloads/mod.zip");

    let error = gamebanana::download_to(&client, UNRESOLVABLE, &dest, true)
        .await
        .expect_err("a dead proxy cannot serve the file")
        .to_string();

    assert!(error.contains("Proxy unreachable"), "{error}");
    assert!(!dest.exists(), "nothing was written");
}

#[tokio::test]
async fn a_mod_download_with_no_proxy_names_the_transport_and_not_a_proxy() {
    let tmp = TempDir::new().unwrap();
    let client = reqwest::Client::new();
    let url = format!("{}/payload.zip", closed_port_url());

    let error = gamebanana::download_to(&client, &url, &tmp.path().join("mod.zip"), false)
        .await
        .expect_err("nothing listens there")
        .to_string();

    assert!(
        error.contains(&url),
        "names what could not be reached: {error}"
    );
    assert!(
        !error.to_lowercase().contains("proxy"),
        "no proxy is configured: {error}"
    );
}

#[tokio::test]
async fn an_importer_download_through_a_dead_proxy_names_the_proxy() {
    let tmp = TempDir::new().unwrap();
    let client = client_behind_dead_proxy(&tmp).await;
    let dest = tmp.path().join("downloads/importer.zip");

    let error = importer::download_to(&client, UNRESOLVABLE, &dest, true)
        .await
        .expect_err("a dead proxy cannot serve the file")
        .to_string();

    assert!(error.contains("Proxy unreachable"), "{error}");
    assert!(!dest.exists(), "nothing was written");
}

#[tokio::test]
async fn an_importer_download_with_no_proxy_names_the_transport_and_not_a_proxy() {
    let tmp = TempDir::new().unwrap();
    let client = reqwest::Client::new();
    let url = format!("{}/payload.zip", closed_port_url());

    let error = importer::download_to(&client, &url, &tmp.path().join("importer.zip"), false)
        .await
        .expect_err("nothing listens there")
        .to_string();

    assert!(
        error.contains(&url),
        "names what could not be reached: {error}"
    );
    assert!(
        !error.to_lowercase().contains("proxy"),
        "no proxy is configured: {error}"
    );
}

#[tokio::test]
async fn an_http_status_keeps_its_own_message_on_both_paths() {
    let mut server = mockito::Server::new_async().await;
    let _gone = server
        .mock("GET", "/gone.zip")
        .with_status(404)
        .expect(2)
        .create_async()
        .await;
    let url = format!("{}/gone.zip", server.url());
    let tmp = TempDir::new().unwrap();
    let client = reqwest::Client::new();

    // `true` must not turn a status into a proxy complaint: a status is not a
    // transport failure.
    let mod_error = gamebanana::download_to(&client, &url, &tmp.path().join("a.zip"), true)
        .await
        .expect_err("404")
        .to_string();
    let importer_error = importer::download_to(&client, &url, &tmp.path().join("b.zip"), true)
        .await
        .expect_err("404")
        .to_string();

    for error in [mod_error, importer_error] {
        assert!(error.contains("404"), "{error}");
        assert!(!error.contains("Proxy unreachable"), "{error}");
    }
}
