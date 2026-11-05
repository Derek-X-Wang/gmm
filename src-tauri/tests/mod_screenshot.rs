//! A Mod's screenshot for the detail panel (#437).
//!
//! Network paths run against loopback `mockito` servers, the way
//! `tests/gamebanana.rs` does: the client is injected into
//! `fetch_data_uri`, so no test touches gamebanana.com. Validation is a pure
//! function and is tested separately, because a loopback server is `http` on
//! `127.0.0.1` and would itself be rejected by it.

use std::sync::Arc;

use base64::Engine as _;
use gmm_lib::core::mod_screenshot::{
    fetch_data_uri, redirect_policy, validate_screenshot_url, FetchFailure, ModScreenshot,
    ScreenshotCache, CACHE_MAX_BYTES, CACHE_MAX_ENTRIES, MAX_IMAGE_BYTES,
};
use gmm_lib::core::network::ProxyConfig;
use gmm_lib::core::Core;
use sqlx::SqlitePool;
use tempfile::TempDir;

fn url(raw: &str) -> reqwest::Url {
    reqwest::Url::parse(raw).expect("test url")
}

// ---- URL validation ----------------------------------------------------

#[test]
fn gamebanana_https_addresses_are_accepted() {
    for ok in [
        "https://images.gamebanana.com/img/ss/mods/a.jpg",
        "https://gamebanana.com/a.png",
        "https://IMAGES.GameBanana.com/a.png",
        "https://a.b.gamebanana.com/a.png",
        "  https://images.gamebanana.com/a.png  ",
    ] {
        assert!(validate_screenshot_url(ok).is_ok(), "{ok} should pass");
    }
}

#[test]
fn every_other_scheme_is_rejected() {
    for bad in [
        "http://images.gamebanana.com/a.png",
        "file:///etc/passwd",
        "file://images.gamebanana.com/a.png",
        "data:image/png;base64,AAAA",
        "ftp://images.gamebanana.com/a.png",
        "javascript:alert(1)",
        "//images.gamebanana.com/a.png",
    ] {
        assert!(validate_screenshot_url(bad).is_err(), "{bad} should fail");
    }
}

#[test]
fn a_foreign_host_and_the_suffix_traps_are_rejected() {
    for bad in [
        "https://example.test/a.png",
        "https://gamebanana.com.example.test/a.png",
        "https://notgamebanana.com/a.png",
        "https://images.gamebanana.com.evil.test/a.png",
        "https://example.test/gamebanana.com/a.png",
        "https://example.test/?host=images.gamebanana.com",
        "https://127.0.0.1/a.png",
        "https://[::1]/a.png",
    ] {
        assert!(validate_screenshot_url(bad).is_err(), "{bad} should fail");
    }
}

#[test]
fn credentials_in_the_address_are_rejected_even_on_gamebanana() {
    for bad in [
        "https://user:password@images.gamebanana.com/a.png",
        "https://user@images.gamebanana.com/a.png",
        // The host here is evil.test; gamebanana.com is only the userinfo.
        "https://images.gamebanana.com@evil.test/a.png",
    ] {
        assert!(validate_screenshot_url(bad).is_err(), "{bad} should fail");
    }
}

#[test]
fn the_rejection_reason_never_contains_the_address() {
    let secret = "https://user:hunter2@evil.test/a.png";
    let reason = validate_screenshot_url(secret).expect_err("rejected");
    assert!(!reason.contains("hunter2") && !reason.contains("evil.test"));
}

#[test]
fn unparseable_text_is_rejected() {
    for bad in ["", "   ", "not a url", "https://"] {
        assert!(validate_screenshot_url(bad).is_err(), "{bad:?} should fail");
    }
}

// ---- Fetch: status, content type, size ---------------------------------

fn plain_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(redirect_policy())
        .build()
        .expect("client")
}

#[tokio::test]
async fn an_image_becomes_a_data_uri_of_the_exact_bytes() {
    let mut server = mockito::Server::new_async().await;
    let bytes: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
    let _m = server
        .mock("GET", "/a.png")
        .with_status(200)
        .with_header("content-type", "image/png")
        .with_body(bytes.clone())
        .create_async()
        .await;

    let uri = fetch_data_uri(
        &plain_client(),
        &url(&format!("{}/a.png", server.url())),
        false,
    )
    .await
    .expect("image");

    let payload = uri.strip_prefix("data:image/png;base64,").expect("prefix");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .expect("base64");
    assert_eq!(decoded, bytes);
}

#[tokio::test]
async fn content_type_parameters_are_dropped_and_case_is_ignored() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/a")
        .with_header("content-type", "Image/JPEG; charset=binary")
        .with_body("x")
        .create_async()
        .await;
    let uri = fetch_data_uri(&plain_client(), &url(&format!("{}/a", server.url())), false)
        .await
        .expect("image");
    assert!(uri.starts_with("data:image/jpeg;base64,"), "{uri}");
}

#[tokio::test]
async fn a_non_image_content_type_is_unavailable() {
    for content_type in [
        "text/html",
        "application/octet-stream",
        "image/",
        // Would escape the data URI it is spliced into.
        "image/png,evil",
        "",
    ] {
        let mut server = mockito::Server::new_async().await;
        let mut mock = server.mock("GET", "/a").with_body("<html>");
        if !content_type.is_empty() {
            mock = mock.with_header("content-type", content_type);
        }
        let _m = mock.create_async().await;
        let outcome =
            fetch_data_uri(&plain_client(), &url(&format!("{}/a", server.url())), false).await;
        assert!(
            matches!(outcome, Err(FetchFailure { .. })),
            "{content_type:?} must not produce an image: {outcome:?}"
        );
    }
}

#[tokio::test]
async fn an_error_status_is_unavailable_and_says_so() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/gone.png")
        .with_status(404)
        .with_header("content-type", "image/png")
        .with_body("nope")
        .create_async()
        .await;
    let failure = fetch_data_uri(
        &plain_client(),
        &url(&format!("{}/gone.png", server.url())),
        false,
    )
    .await
    .expect_err("404");
    assert!(failure.reason.contains("404"), "{}", failure.reason);
}

#[tokio::test]
async fn a_body_over_the_cap_is_unavailable_not_truncated() {
    let mut server = mockito::Server::new_async().await;
    // Declared length over the cap: refused before reading the body.
    let _declared = server
        .mock("GET", "/declared.png")
        .with_header("content-type", "image/png")
        .with_body(vec![0u8; MAX_IMAGE_BYTES + 1])
        .create_async()
        .await;
    // Chunked: no Content-Length, so only counting the stream can catch it.
    let _chunked = server
        .mock("GET", "/chunked.png")
        .with_header("content-type", "image/png")
        .with_chunked_body(|w| {
            let block = vec![0u8; 1024 * 1024];
            for _ in 0..=(MAX_IMAGE_BYTES / block.len()) {
                w.write_all(&block)?;
            }
            Ok(())
        })
        .create_async()
        .await;
    for path in ["/declared.png", "/chunked.png"] {
        let outcome = fetch_data_uri(
            &plain_client(),
            &url(&format!("{}{path}", server.url())),
            false,
        )
        .await;
        let failure = outcome.expect_err(path);
        assert!(
            failure.reason.contains("larger"),
            "{path}: {}",
            failure.reason
        );
    }
}

#[tokio::test]
async fn a_body_exactly_at_the_cap_is_still_an_image() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/edge.png")
        .with_header("content-type", "image/png")
        .with_body(vec![7u8; MAX_IMAGE_BYTES])
        .create_async()
        .await;
    let uri = fetch_data_uri(
        &plain_client(),
        &url(&format!("{}/edge.png", server.url())),
        false,
    )
    .await
    .expect("at the cap");
    assert!(uri.starts_with("data:image/png;base64,"));
}

#[tokio::test]
async fn an_empty_body_is_unavailable() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/empty.png")
        .with_header("content-type", "image/png")
        .create_async()
        .await;
    assert!(fetch_data_uri(
        &plain_client(),
        &url(&format!("{}/empty.png", server.url())),
        false
    )
    .await
    .is_err());
}

// ---- Redirects stay inside the validation ------------------------------

#[tokio::test]
async fn a_redirect_to_an_address_validation_refuses_is_not_followed() {
    let mut server = mockito::Server::new_async().await;
    let target = server
        .mock("GET", "/elsewhere.png")
        .with_header("content-type", "image/png")
        .with_body("image")
        .expect(0)
        .create_async()
        .await;
    // The loopback target is `http`, so the policy must refuse it.
    let _start = server
        .mock("GET", "/start.png")
        .with_status(302)
        .with_header("location", &format!("{}/elsewhere.png", server.url()))
        .create_async()
        .await;

    let outcome = fetch_data_uri(
        &plain_client(),
        &url(&format!("{}/start.png", server.url())),
        false,
    )
    .await;

    assert!(outcome.is_err(), "a refused redirect is not an image");
    target.assert_async().await;
}

// ---- The proxy is honoured --------------------------------------------

async fn core_in(tmp: &TempDir) -> (Core, SqlitePool) {
    let db = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &db).await.unwrap();
    let pool = SqlitePool::connect(&db).await.unwrap();
    (core, pool)
}

async fn insert_mod(pool: &SqlitePool, id: &str, screenshot_url: Option<&str>) {
    sqlx::query(
        "INSERT INTO mods (id, game_code, name, source, library_path, junction_dir_name,
                           enabled, created_at, screenshot_url)
         VALUES (?, 'gimi', 'Screenshot Mod', 'manual', ?, ?, 0,
                 '2026-11-01T00:00:00Z', ?)",
    )
    .bind(id)
    .bind(format!("/library/{id}"))
    .bind(format!("Screenshot Mod {id}"))
    .bind(screenshot_url)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn the_core_client_sends_the_image_request_through_the_users_proxy() {
    // Same idiom as the manifest refresh test: the host does not resolve, so
    // the only way to get bytes is through the configured proxy. This is the
    // property a WebView image load cannot have.
    let mut proxy = mockito::Server::new_async().await;
    let via_proxy = proxy
        .mock("GET", mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "image/png")
        .with_body("png-bytes")
        .expect(1)
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let (core, _pool) = core_in(&tmp).await;
    core.set_proxy_config(&ProxyConfig {
        url: Some(proxy.url()),
        username: None,
        password: None,
    })
    .await
    .unwrap();

    let client = core
        .http_client_builder()
        .await
        .unwrap()
        .redirect(redirect_policy())
        .build()
        .unwrap();
    let uri = fetch_data_uri(
        &client,
        &url("http://screenshots.gmm-test.invalid/a.png"),
        true,
    )
    .await
    .expect("reached through the proxy");

    via_proxy.assert_async().await;
    assert!(uri.starts_with("data:image/png;base64,"));
}

#[tokio::test]
async fn an_unreachable_proxy_reads_as_a_proxy_failure_not_a_broken_mod() {
    // Bind then drop, so the port is closed and the connection is refused.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_url = format!("http://{}", dead.local_addr().unwrap());
    drop(dead);

    let tmp = TempDir::new().unwrap();
    let (core, pool) = core_in(&tmp).await;
    core.set_proxy_config(&ProxyConfig {
        url: Some(proxy_url),
        username: None,
        password: None,
    })
    .await
    .unwrap();
    insert_mod(
        &pool,
        "mod-a",
        Some("https://images.gamebanana.com/img/ss/a.png"),
    )
    .await;

    // A valid address, so the request really is attempted, and the only
    // route out is the dead proxy.
    let outcome = core.mod_screenshot("mod-a").await.expect("never an Err");

    let ModScreenshot::Unavailable { reason } = outcome else {
        panic!("a dead proxy cannot yield an image: {outcome:?}");
    };
    assert!(reason.starts_with("Proxy unreachable"), "{reason}");
    let lowered = reason.to_lowercase();
    assert!(
        !lowered.contains("corrupt") && !lowered.contains("broken"),
        "{reason}"
    );
}

// ---- Core: stored addresses --------------------------------------------

#[tokio::test]
async fn a_rejected_stored_address_is_unavailable_and_makes_no_request() {
    let tmp = TempDir::new().unwrap();
    let (core, pool) = core_in(&tmp).await;
    // No proxy is configured, so a fetch of any of these would go to the real
    // network. They are rejected before a client is even built.
    for (index, bad) in [
        "http://images.gamebanana.com/a.png",
        "file:///etc/passwd",
        "data:image/png;base64,AAAA",
        "https://example.test/a.png",
        "https://gamebanana.com.example.test/a.png",
        "https://user:hunter2@images.gamebanana.com/a.png",
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("mod-{index}");
        insert_mod(&pool, &id, Some(bad)).await;
        let outcome = core.mod_screenshot(&id).await.expect("not an Err");
        let ModScreenshot::Unavailable { reason } = outcome else {
            panic!("{bad} must not produce an image");
        };
        assert!(!reason.contains("hunter2"), "{reason}");
        assert!(reason.contains("gamebanana.com"), "{reason}");
    }
}

#[tokio::test]
async fn a_mod_without_an_address_or_without_a_row_is_unavailable() {
    let tmp = TempDir::new().unwrap();
    let (core, pool) = core_in(&tmp).await;
    insert_mod(&pool, "none", None).await;
    insert_mod(&pool, "blank", Some("   ")).await;
    for id in ["none", "blank", "missing"] {
        assert!(
            matches!(
                core.mod_screenshot(id).await.unwrap(),
                ModScreenshot::Unavailable { .. }
            ),
            "{id}"
        );
    }
}

#[test]
fn the_wire_shape_is_a_tagged_status() {
    assert_eq!(
        serde_json::to_value(ModScreenshot::Image {
            data_uri: "data:image/png;base64,AA".into()
        })
        .unwrap(),
        serde_json::json!({"status": "image", "dataUri": "data:image/png;base64,AA"}),
    );
    assert_eq!(
        serde_json::to_value(ModScreenshot::Unavailable {
            reason: "why".into()
        })
        .unwrap(),
        serde_json::json!({"status": "unavailable", "reason": "why"}),
    );
}

// ---- LRU bound ---------------------------------------------------------

fn value(len: usize) -> Arc<str> {
    Arc::from("x".repeat(len).as_str())
}

#[test]
fn the_cache_never_holds_more_than_its_entry_cap() {
    let mut cache = ScreenshotCache::default();
    for i in 0..(CACHE_MAX_ENTRIES + 5) {
        cache.insert(format!("k{i}"), value(10));
    }
    assert_eq!(cache.len(), CACHE_MAX_ENTRIES);
    assert!(cache.get("k0").is_none(), "oldest evicted");
    assert!(cache.get("k4").is_none());
    assert!(cache.get("k5").is_some());
}

#[test]
fn the_cache_never_holds_more_than_its_byte_cap() {
    let mut cache = ScreenshotCache::default();
    let each = CACHE_MAX_BYTES / 3;
    for i in 0..5 {
        cache.insert(format!("k{i}"), value(each));
        assert!(cache.bytes() <= CACHE_MAX_BYTES);
    }
    assert_eq!(cache.len(), 3);
    assert!(cache.get("k0").is_none() && cache.get("k1").is_none());
    assert!(cache.get("k4").is_some());
}

#[test]
fn reading_an_entry_protects_it_from_eviction() {
    let mut cache = ScreenshotCache::default();
    for i in 0..CACHE_MAX_ENTRIES {
        cache.insert(format!("k{i}"), value(10));
    }
    assert!(cache.get("k0").is_some(), "touch the oldest");
    cache.insert("new".into(), value(10));
    assert!(cache.get("k0").is_some(), "recently used survives");
    assert!(cache.get("k1").is_none(), "least recently used goes");
}

#[test]
fn replacing_a_key_does_not_double_count_its_bytes() {
    let mut cache = ScreenshotCache::default();
    cache.insert("k".into(), value(100));
    cache.insert("k".into(), value(40));
    assert_eq!((cache.len(), cache.bytes()), (1, 40));
}

#[test]
fn an_entry_larger_than_the_byte_cap_is_refused_without_emptying_the_cache() {
    let mut cache = ScreenshotCache::default();
    cache.insert("keep".into(), value(10));
    cache.insert("huge".into(), value(CACHE_MAX_BYTES + 1));
    assert!(cache.get("keep").is_some());
    assert!(cache.get("huge").is_none());
}
