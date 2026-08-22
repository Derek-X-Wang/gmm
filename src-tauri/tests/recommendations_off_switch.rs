//! ADR 0005 / #95 / #109 — the global Importer Origin recommendations
//! switch, and what "off" has to mean.
//!
//! #95 rejected an off switch that only silences the UI, and named the
//! failure mode precisely: GMM would go on acting on a file the user
//! said not to consult, quietly clearing their compiled-in default with
//! no visible cause. Invisible behaviour change is worse than the
//! prompts they switched off.
//!
//! So "off" has to remove the **whole layer**, and that is two separate
//! preconditions rather than one:
//!
//! 1. no manifest fetch is attempted at all — a precondition on the
//!    fetch path, not a filter on its result;
//! 2. the **cache is not consulted either**, so a retraction that was
//!    fetched while recommendations were on does not survive being
//!    switched off.
//!
//! Gating only the fetch is the shape #95 explicitly rejected: it looks
//! correct on a machine that has never launched online, and does nothing
//! at all on every machine that has — which is every real one, since the
//! cache is written on the first successful launch.

use gmm_lib::core::importer_origin::{OriginLayer, OriginResolution};
use gmm_lib::core::recommended_importers::Refreshed;
use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init core")
}

/// The shape the committed manifest uses for HIMI: an explicit
/// retraction of a game's compiled-in default.
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

fn recommending_gimi_elsewhere() -> String {
    r#"{
      "schemaVersion": 1,
      "games": {
        "gimi": {
          "status": "recommended",
          "owner": "curated",
          "repo": "GIMI-Fork",
          "assetPattern": "GIMI-PACKAGE-v\\d+\\.\\d+\\.\\d+\\.zip"
        }
      }
    }"#
    .to_string()
}

#[tokio::test]
async fn recommendations_are_on_for_a_user_who_has_never_touched_the_setting() {
    // This is an opt-out, not an opt-in (#95). A fresh install gets the
    // layer, because the users it exists to rescue are exactly the ones
    // who will never find a switch.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    assert!(
        core.importer_recommendations_enabled()
            .await
            .expect("read the setting"),
        "recommendations must default to on",
    );
}

#[tokio::test]
async fn switching_recommendations_off_attempts_no_fetch_at_all() {
    let mut server = mockito::Server::new_async().await;
    // Expect **zero** hits. `mockito` asserts this on drop via
    // `assert_async`, which is the only way to tell "we did not ask"
    // apart from "we asked and ignored the answer".
    let mock = server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(recommending_gimi_elsewhere())
        .expect(0)
        .create_async()
        .await;

    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.set_importer_recommendations_enabled(false)
        .await
        .expect("switch off");

    let outcome = core
        .refresh_recommended_importers_from(&format!("{}/recommended-importers.json", server.url()))
        .await
        .expect("a refresh with the layer switched off is not an error");

    assert!(
        matches!(outcome, Refreshed::Disabled),
        "a refresh that never ran must say so rather than reporting a network \
         outcome it never obtained; got {outcome:?}",
    );
    mock.assert_async().await;
}

#[tokio::test]
async fn a_retraction_does_not_survive_switching_recommendations_off() {
    // The case that makes "off gates the fetch" insufficient. The
    // retraction is already cached — fetched on an earlier launch while
    // recommendations were on — so no further fetch is involved and a
    // fetch-only gate changes nothing at all.
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(retracting_gimi())
        .create_async()
        .await;

    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let url = format!("{}/recommended-importers.json", server.url());
    core.refresh_recommended_importers_from(&url)
        .await
        .expect("refresh");

    // With the layer on, the retraction is in force.
    assert!(
        matches!(
            core.resolve_importer_origin(GameCode::Gimi)
                .await
                .expect("resolve"),
            OriginResolution::NoneInEffect { .. }
        ),
        "precondition: the cached retraction clears the compiled-in default",
    );

    core.set_importer_recommendations_enabled(false)
        .await
        .expect("switch off");

    match core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve")
    {
        OriginResolution::InEffect { layer, .. } => assert_eq!(
            layer,
            OriginLayer::CompiledInDefault,
            "with the layer off, precedence collapses to user override → \
             compiled-in default",
        ),
        other => panic!(
            "a retraction GMM was told not to consult must not still be clearing \
             the default; got {other:?}"
        ),
    }
}

#[tokio::test]
async fn a_recommendation_does_not_apply_while_recommendations_are_off() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(recommending_gimi_elsewhere())
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

    core.set_importer_recommendations_enabled(false)
        .await
        .expect("switch off");

    match core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve")
    {
        OriginResolution::InEffect { origin, layer } => {
            assert_eq!(layer, OriginLayer::CompiledInDefault);
            assert_eq!(origin.repo_slug(), "SilentNightSound/GIMI-Package");
        }
        other => panic!("expected the compiled-in default, got {other:?}"),
    }
}

#[tokio::test]
async fn switching_recommendations_back_on_restores_the_cached_layer() {
    // The cache is *not consulted* while off, not deleted. Deleting it
    // would make the switch destructive in one direction, and a user who
    // toggles it back on would sit on the compiled-in defaults until the
    // next successful launch fetch — which, for the offline user this
    // whole mechanism exists to rescue, may be never.
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(recommending_gimi_elsewhere())
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

    core.set_importer_recommendations_enabled(false)
        .await
        .expect("off");
    core.set_importer_recommendations_enabled(true)
        .await
        .expect("on");

    match core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve")
    {
        OriginResolution::InEffect { origin, layer } => {
            assert_eq!(layer, OriginLayer::RecommendedManifest);
            assert_eq!(origin.repo_slug(), "curated/GIMI-Fork");
        }
        other => panic!("expected the cached recommendation back, got {other:?}"),
    }
}

#[tokio::test]
async fn a_user_override_still_applies_while_recommendations_are_off() {
    // ADR 0005's degradation promise: "even with GMM's curation switched
    // off, the user's own override still rescues them". Layer 1 is not
    // part of the layer this switch removes.
    use gmm_lib::core::importer_origin::ImporterOrigin;

    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.set_importer_recommendations_enabled(false)
        .await
        .expect("off");
    let mine = ImporterOrigin::github("me", "my-GIMI", "GIMI-PACKAGE-v.*\\.zip");
    core.set_importer_origin_override(GameCode::Gimi, Some(&mine))
        .await
        .expect("set override");

    match core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve")
    {
        OriginResolution::InEffect { origin, layer } => {
            assert_eq!(layer, OriginLayer::UserOverride);
            assert_eq!(origin, mine);
        }
        other => panic!("the user's own origin must survive the switch, got {other:?}"),
    }
}
