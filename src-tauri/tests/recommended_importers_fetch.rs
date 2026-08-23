//! ADR 0005 / #108 — fetching and caching `recommended-importers.json`,
//! the middle layer of Importer Origin precedence.
//!
//! The property these tests exist to hold is the one #96 spells out and
//! that #78 got wrong: **a fetch error, an unusable manifest, an explicit
//! `none` and a game absent from the file are four distinct conditions
//! with four different behaviours**, and none of them may be represented
//! by the same value.

use gmm_lib::core::importer_origin::{ImporterOrigin, OriginLayer, OriginResolution};
use gmm_lib::core::network::ProxyConfig;
use gmm_lib::core::recommended_importers::Refreshed;
use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init core")
}

/// A manifest recommending one origin for Genshin and saying nothing
/// about any other game.
fn recommending_gimi(owner: &str, repo: &str) -> String {
    format!(
        r#"{{
          "schemaVersion": 1,
          "games": {{
            "gimi": {{
              "status": "recommended",
              "owner": "{owner}",
              "repo": "{repo}",
              "assetPattern": "GIMI-PACKAGE-v\\d+\\.\\d+\\.\\d+\\.zip"
            }}
          }}
        }}"#
    )
}

#[tokio::test]
async fn a_recommended_entry_is_the_resolved_origin_when_the_user_has_no_override() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(recommending_gimi("curated", "GIMI-Fork"))
        .create_async()
        .await;

    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.refresh_recommended_importers_from(&format!(
        "{}/recommended-importers.json",
        server.url()
    ))
    .await
    .expect("refresh");

    match core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve")
    {
        OriginResolution::InEffect { origin, layer } => {
            assert_eq!(origin.repo_slug(), "curated/GIMI-Fork");
            assert_eq!(
                layer,
                OriginLayer::RecommendedManifest,
                "the manifest, not the compiled-in default, supplied this origin",
            );
        }
        other => panic!("expected the recommended origin to be in effect, got {other:?}"),
    }
}

/// A manifest that retracts Genshin's compiled-in default and says
/// nothing about any other game.
fn retracting_gimi() -> String {
    r#"{
      "schemaVersion": 1,
      "games": {
        "gimi": {
          "status": "none",
          "reason": "No maintained package is known right now."
        }
      }
    }"#
    .to_string()
}

#[tokio::test]
async fn a_none_entry_retracts_the_compiled_in_default_rather_than_falling_through_to_it() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(retracting_gimi())
        .create_async()
        .await;

    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.refresh_recommended_importers_from(&format!(
        "{}/recommended-importers.json",
        server.url()
    ))
    .await
    .expect("refresh");

    match core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve")
    {
        OriginResolution::NoneInEffect { reason } => {
            assert_eq!(
                reason.as_deref(),
                Some("No maintained package is known right now."),
            );
        }
        other => panic!("`none` must retract layer 3, not fall through to it; got {other:?}"),
    }

    // ...while a game the manifest is silent about is untouched.
    match core
        .resolve_importer_origin(GameCode::Wwmi)
        .await
        .expect("resolve")
    {
        OriginResolution::InEffect { layer, .. } => {
            assert_eq!(layer, OriginLayer::CompiledInDefault);
        }
        other => panic!("an absent game key is not a retraction; got {other:?}"),
    }
}

#[tokio::test]
async fn a_failed_refresh_leaves_the_cached_manifest_in_force_including_its_retraction() {
    let mut server = mockito::Server::new_async().await;
    let good = server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(retracting_gimi())
        .expect(1)
        .create_async()
        .await;

    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let url = format!("{}/recommended-importers.json", server.url());
    core.refresh_recommended_importers_from(&url)
        .await
        .expect("first refresh");
    good.assert_async().await;

    // Now upstream goes down. Losing connectivity must not quietly
    // restore the package GMM withdrew — that is the wrong direction of
    // flap, and the whole reason the cache is authoritative (#96).
    let _down = server
        .mock("GET", "/recommended-importers.json")
        .with_status(503)
        .create_async()
        .await;
    let outcome = core
        .refresh_recommended_importers_from(&url)
        .await
        .expect("second refresh");
    assert!(
        matches!(outcome, Refreshed::Unreachable(_)),
        "a 503 is unreachable, not an empty recommendation; got {outcome:?}",
    );

    match core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve")
    {
        OriginResolution::NoneInEffect { .. } => {}
        other => panic!("the retraction must survive a failed refresh; got {other:?}"),
    }
}

#[tokio::test]
async fn a_first_launch_with_no_cache_and_a_failed_fetch_resolves_to_compiled_in_defaults() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/recommended-importers.json")
        .with_status(500)
        .create_async()
        .await;

    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let outcome = core
        .refresh_recommended_importers_from(&format!("{}/recommended-importers.json", server.url()))
        .await
        .expect("refresh");
    assert!(matches!(outcome, Refreshed::Unreachable(_)), "{outcome:?}");

    for game in [GameCode::Gimi, GameCode::Srmi, GameCode::Himi] {
        match core.resolve_importer_origin(game).await.expect("resolve") {
            OriginResolution::InEffect { layer, .. } => {
                assert_eq!(layer, OriginLayer::CompiledInDefault, "{game:?}");
            }
            other => panic!("no cache means the layer is absent, not retracting; got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_failed_fetch_and_an_empty_but_valid_manifest_are_not_the_same_value() {
    // The #78 property, stated as a test. `.ok().flatten()` on this path
    // would make these two identical, and the feature would then be dead
    // without anything ever saying so.
    let tmp = TempDir::new().expect("tmp");

    let mut broken = mockito::Server::new_async().await;
    let _b = broken
        .mock("GET", "/recommended-importers.json")
        .with_status(500)
        .create_async()
        .await;
    let failed = fresh_core(&tmp)
        .await
        .refresh_recommended_importers_from(&format!("{}/recommended-importers.json", broken.url()))
        .await
        .expect("refresh");

    let tmp2 = TempDir::new().expect("tmp");
    let mut empty = mockito::Server::new_async().await;
    let _e = empty
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(r#"{"schemaVersion": 1, "games": {}}"#)
        .create_async()
        .await;
    let recommends_nothing = fresh_core(&tmp2)
        .await
        .refresh_recommended_importers_from(&format!("{}/recommended-importers.json", empty.url()))
        .await
        .expect("refresh");

    assert!(
        matches!(failed, Refreshed::Unreachable(_)),
        "a failed fetch is `Unreachable`, got {failed:?}",
    );
    assert!(
        matches!(recommends_nothing, Refreshed::Replaced(_)),
        "a manifest that recommends nothing is still a manifest, got {recommends_nothing:?}",
    );
    assert_ne!(
        std::mem::discriminant(&failed),
        std::mem::discriminant(&recommends_nothing),
        "\"we could not ask\" and \"the answer is nothing\" must not share a value",
    );
}

#[tokio::test]
async fn the_loopback_override_does_not_follow_a_redirect_off_loopback() {
    // Bind the target on every local interface, then address it through this
    // machine's non-loopback interface. If reqwest follows the redirect, the
    // target observes a real second request that `.expect(0)` rejects.
    let route_probe = std::net::UdpSocket::bind("0.0.0.0:0").expect("bind route probe");
    route_probe
        .connect("192.0.2.1:80")
        .expect("select the host's outbound interface");
    let off_loopback_ip = route_probe.local_addr().expect("route probe address").ip();
    assert!(
        !off_loopback_ip.is_loopback() && !off_loopback_ip.is_unspecified(),
        "the redirect target must exercise a non-loopback interface: {off_loopback_ip}",
    );

    let mut escaped_target = mockito::Server::new_with_opts_async(mockito::ServerOpts {
        host: "0.0.0.0",
        ..Default::default()
    })
    .await;
    let escaped_url = format!(
        "http://{off_loopback_ip}:{}/escaped-manifest.json",
        escaped_target.socket_address().port(),
    );
    let no_escape = escaped_target
        .mock("GET", "/escaped-manifest.json")
        .with_status(200)
        .with_body(r#"{"schemaVersion": 1, "games": {}}"#)
        .expect(0)
        .create_async()
        .await;

    let mut loopback = mockito::Server::new_async().await;
    let redirect = loopback
        .mock("GET", "/recommended-importers.json")
        .with_status(302)
        .with_header("location", &escaped_url)
        .expect(1)
        .create_async()
        .await;

    let tmp = TempDir::new().expect("tmp");
    let outcome = fresh_core(&tmp)
        .await
        .refresh_recommended_importers_from_loopback_override(&format!(
            "{}/recommended-importers.json",
            loopback.url(),
        ))
        .await
        .expect("refresh");

    redirect.assert_async().await;
    no_escape.assert_async().await;
    match outcome {
        Refreshed::Unreachable(reason) => assert!(
            reason.contains("302"),
            "the failed fetch must identify the redirect response: {reason}",
        ),
        other => panic!(
            "a refused redirect is an unreachable fetch, not a successful empty manifest: {other:?}",
        ),
    }
}

/// The three shapes this build cannot make sense of. Each must drop the
/// whole layer — never apply the readable half, never land on retraction.
fn unusable_documents() -> Vec<(&'static str, &'static str)> {
    vec![
        ("malformed JSON", r#"{"schemaVersion": 1, "games": {"#),
        (
            "a schemaVersion from a newer GMM",
            r#"{"schemaVersion": 99, "games": {"gimi": {"status": "none"}}}"#,
        ),
        (
            "an unrecognised status",
            r#"{"schemaVersion": 1, "games": {
                "gimi": {"status": "deprecated"},
                "wwmi": {"status": "none"}
            }}"#,
        ),
        // #123: `games` carried a serde default, so this parsed as a
        // valid manifest that recommends nothing — and replaced the
        // cache with it.
        ("no games key at all", r#"{"schemaVersion": 1}"#),
        (
            "a mistyped games key",
            r#"{"schemaVersion": 1, "game": {"gimi": {"status": "none"}}}"#,
        ),
        (
            "an assetPattern that cannot compile",
            r#"{"schemaVersion": 1, "games": {
                "gimi": {"status": "recommended", "owner": "SilentNightSound",
                         "repo": "GIMI-Package", "assetPattern": "GIMI-PACKAGE-v[.zip"}
            }}"#,
        ),
    ]
}

#[tokio::test]
async fn an_unusable_manifest_drops_the_whole_layer_and_retracts_nothing() {
    for (label, body) in unusable_documents() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/recommended-importers.json")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let tmp = TempDir::new().expect("tmp");
        let core = fresh_core(&tmp).await;
        let outcome = core
            .refresh_recommended_importers_from(&format!(
                "{}/recommended-importers.json",
                server.url()
            ))
            .await
            .expect("refresh");

        assert!(
            matches!(outcome, Refreshed::Unusable(_)),
            "{label}: expected an unusable manifest, got {outcome:?}",
        );

        // Every game — including ones the document did manage to name —
        // falls through. Partial application of a document the build has
        // admitted it cannot read is what #93 forbids.
        for game in [GameCode::Gimi, GameCode::Wwmi, GameCode::Himi] {
            match core.resolve_importer_origin(game).await.expect("resolve") {
                OriginResolution::InEffect { layer, .. } => {
                    assert_eq!(layer, OriginLayer::CompiledInDefault, "{label} / {game:?}");
                }
                other => panic!(
                    "{label}: an unusable manifest must land on fall-through, \
                     never on retraction; got {other:?}",
                ),
            }
        }

        // Silent in the UI is a product choice; silent in the data model
        // is how a feature dies unnoticed (#78). The reason is recorded.
        assert!(
            core.recommended_importers_unusable_reason()
                .await
                .expect("reason")
                .is_some(),
            "{label}: the build-too-old reason must be recorded",
        );
    }
}

#[tokio::test]
async fn an_unusable_manifest_does_not_replace_a_cache_that_still_works() {
    let mut server = mockito::Server::new_async().await;
    let url = format!("{}/recommended-importers.json", server.url());

    let good = server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(recommending_gimi("curated", "GIMI-Fork"))
        .expect(1)
        .create_async()
        .await;

    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.refresh_recommended_importers_from(&url)
        .await
        .expect("first refresh");
    good.assert_async().await;

    let _bad = server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(r#"{"schemaVersion": 99, "games": {}}"#)
        .create_async()
        .await;
    let outcome = core
        .refresh_recommended_importers_from(&url)
        .await
        .expect("second refresh");
    assert!(matches!(outcome, Refreshed::Unusable(_)), "{outcome:?}");

    // Authoritative until *replaced* — and an unreadable document
    // replaces nothing.
    match core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve")
    {
        OriginResolution::InEffect { origin, layer } => {
            assert_eq!(origin.repo_slug(), "curated/GIMI-Fork");
            assert_eq!(layer, OriginLayer::RecommendedManifest);
        }
        other => panic!("the working cache must survive an unusable document; got {other:?}"),
    }
}

#[tokio::test]
async fn a_readable_manifest_clears_the_build_too_old_state() {
    let mut server = mockito::Server::new_async().await;
    let url = format!("{}/recommended-importers.json", server.url());

    let bad = server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(r#"{"schemaVersion": 99, "games": {}}"#)
        .expect(1)
        .create_async()
        .await;
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.refresh_recommended_importers_from(&url)
        .await
        .expect("refresh");
    bad.assert_async().await;
    assert!(core
        .recommended_importers_unusable_reason()
        .await
        .expect("reason")
        .is_some());

    // The user updated GMM, or the manifest was rolled back. Either way
    // the situation resolved and the notice must not linger.
    let _fixed = server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(recommending_gimi("curated", "GIMI-Fork"))
        .create_async()
        .await;
    core.refresh_recommended_importers_from(&url)
        .await
        .expect("refresh");

    assert_eq!(
        core.recommended_importers_unusable_reason()
            .await
            .expect("reason"),
        None,
    );
}

#[tokio::test]
async fn a_user_override_outranks_both_a_recommendation_and_a_retraction() {
    let mine = ImporterOrigin::github("me", "GIMI-Package", r"GIMI-PACKAGE-v\d+\.zip");

    for body in [recommending_gimi("curated", "GIMI-Fork"), retracting_gimi()] {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/recommended-importers.json")
            .with_status(200)
            .with_body(&body)
            .create_async()
            .await;

        let tmp = TempDir::new().expect("tmp");
        let core = fresh_core(&tmp).await;
        core.set_importer_origin_override(GameCode::Gimi, Some(&mine))
            .await
            .expect("set override");
        core.refresh_recommended_importers_from(&format!(
            "{}/recommended-importers.json",
            server.url()
        ))
        .await
        .expect("refresh");

        match core
            .resolve_importer_origin(GameCode::Gimi)
            .await
            .expect("resolve")
        {
            OriginResolution::InEffect { origin, layer } => {
                assert_eq!(origin, mine);
                assert_eq!(layer, OriginLayer::UserOverride);
            }
            other => panic!("the user's own choice always wins; got {other:?}"),
        }
    }
}

#[tokio::test]
async fn the_cached_manifest_survives_a_restart() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(retracting_gimi())
        .expect(1)
        .create_async()
        .await;

    let tmp = TempDir::new().expect("tmp");
    {
        let core = fresh_core(&tmp).await;
        core.refresh_recommended_importers_from(&format!(
            "{}/recommended-importers.json",
            server.url()
        ))
        .await
        .expect("refresh");
    }
    m.assert_async().await;

    // A new Core over the same data directory — the next launch, with no
    // fetch of its own yet.
    let next_launch = fresh_core(&tmp).await;
    match next_launch
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve")
    {
        OriginResolution::NoneInEffect { .. } => {}
        other => panic!("the cache must outlive the process; got {other:?}"),
    }
}

#[tokio::test]
async fn a_refresh_sends_the_previous_etag_and_acts_on_a_304() {
    let mut server = mockito::Server::new_async().await;
    let url = format!("{}/recommended-importers.json", server.url());

    let first = server
        .mock("GET", "/recommended-importers.json")
        .match_header("if-none-match", mockito::Matcher::Missing)
        .with_status(200)
        .with_header("etag", "\"v1\"")
        .with_body(retracting_gimi())
        .expect(1)
        .create_async()
        .await;

    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.refresh_recommended_importers_from(&url)
        .await
        .expect("first refresh");
    first.assert_async().await;

    // Most refreshes should cost a 304 rather than a download.
    let conditional = server
        .mock("GET", "/recommended-importers.json")
        .match_header("if-none-match", "\"v1\"")
        .with_status(304)
        .expect(1)
        .create_async()
        .await;
    let outcome = core
        .refresh_recommended_importers_from(&url)
        .await
        .expect("second refresh");
    conditional.assert_async().await;

    assert!(
        matches!(outcome, Refreshed::NotModified),
        "a 304 means the cached document is confirmed current, not that \
         nothing is recommended; got {outcome:?}",
    );
    match core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve")
    {
        OriginResolution::NoneInEffect { .. } => {}
        other => panic!("the cache stays in force across a 304; got {other:?}"),
    }
}

#[tokio::test]
async fn resolution_does_not_wait_on_a_host_that_never_answers() {
    // "GitHub is slow today" must never become "GMM won't start". The
    // refresh is background and nothing waits on it, so a host that
    // accepts the connection and then says nothing must not hold up a
    // question GMM can already answer from its cache.
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(recommending_gimi("curated", "GIMI-Fork"))
        .expect(1)
        .create_async()
        .await;
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.refresh_recommended_importers_from(&format!(
        "{}/recommended-importers.json",
        server.url()
    ))
    .await
    .expect("seed the cache");
    m.assert_async().await;

    // A socket that accepts and then never replies.
    let black_hole = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = black_hole.local_addr().expect("addr");

    let refreshing = tokio::spawn({
        let core = core.clone();
        let url = format!("http://{addr}/recommended-importers.json");
        async move { core.refresh_recommended_importers_from(&url).await }
    });

    let resolved = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        core.resolve_importer_origin(GameCode::Gimi),
    )
    .await
    .expect("resolution must not wait on the refresh")
    .expect("resolve");

    assert!(
        !refreshing.is_finished(),
        "the refresh should still be in flight — otherwise this test \
         proves nothing about not waiting on it",
    );
    match resolved {
        OriginResolution::InEffect { origin, layer } => {
            assert_eq!(origin.repo_slug(), "curated/GIMI-Fork");
            assert_eq!(layer, OriginLayer::RecommendedManifest);
        }
        other => panic!("the cache answers immediately; got {other:?}"),
    }

    refreshing.abort();
    drop(black_hole);
}

#[tokio::test]
async fn the_refresh_goes_through_the_users_proxy_like_every_other_network_call() {
    // ADR 0005 and #96 both make this explicit: the manifest fetch is a
    // network call like any other and must honour `Core::http_client`'s
    // proxy configuration. A user behind a corporate proxy who cannot
    // reach raw.githubusercontent.com directly would otherwise silently
    // never receive a recommendation.
    let mut proxy = mockito::Server::new_async().await;
    let via_proxy = proxy
        .mock("GET", mockito::Matcher::Any)
        .with_status(200)
        .with_body(recommending_gimi("curated", "GIMI-Fork"))
        .expect(1)
        .create_async()
        .await;

    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.set_proxy_config(&ProxyConfig {
        url: Some(proxy.url()),
        username: None,
        password: None,
    })
    .await
    .expect("set proxy");

    // A host that does not resolve: the only way this can succeed is
    // through the proxy.
    let outcome = core
        .refresh_recommended_importers_from(
            "http://gmm-manifest-host.invalid/recommended-importers.json",
        )
        .await
        .expect("refresh");

    via_proxy.assert_async().await;
    assert!(
        matches!(outcome, Refreshed::Replaced(_)),
        "the manifest should have arrived through the proxy; got {outcome:?}",
    );
}

#[tokio::test]
async fn a_304_with_nothing_cached_is_not_a_success() {
    // A 304 is only meaningful as an answer to a conditional request.
    // GMM sends `If-None-Match` only when it actually holds a cached
    // document, so a 304 arriving with no ETag sent is impossible by
    // contract — which in practice means a misbehaving proxy on a first
    // launch. Treating it as `NotModified` claims the cache is confirmed
    // current when there is no cache at all: "we could not ask"
    // rendered as "the answer is unchanged", which is #78 exactly.
    let mut server = mockito::Server::new_async().await;
    let url = format!("{}/recommended-importers.json", server.url());
    let m = server
        .mock("GET", "/recommended-importers.json")
        .match_header("if-none-match", mockito::Matcher::Missing)
        .with_status(304)
        .expect(1)
        .create_async()
        .await;

    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let outcome = core
        .refresh_recommended_importers_from(&url)
        .await
        .expect("refresh");
    m.assert_async().await;

    match outcome {
        Refreshed::Unreachable(reason) => assert!(
            reason.contains("304"),
            "the reason must say what upstream actually did: {reason}",
        ),
        other => panic!(
            "a 304 with no cache to revalidate says nothing was learned, \
             not that the cache is current; got {other:?}",
        ),
    }

    // And the layer is absent, so every game falls through — it must
    // never look like a manifest that recommends nothing.
    match core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve")
    {
        OriginResolution::InEffect { layer, .. } => {
            assert_eq!(layer, OriginLayer::CompiledInDefault)
        }
        other => panic!("nothing cached means fall-through; got {other:?}"),
    }
}
