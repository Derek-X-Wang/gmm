//! Issue #127, revisited by #109 — the "no Importer Origin in effect"
//! message tells the user the truth about where the control is.
//!
//! #127 removed "choose one in Settings" from these messages, and the
//! reason was not that pointing at a control is wrong: it was that **the
//! control did not exist**. No origin command was registered and nothing
//! in the frontend exposed one, so the copy sent every user of a
//! retracted game somewhere they could not go. The test written then
//! said so explicitly — "no user-facing no-origin message may mention
//! Settings until Settings actually has the control" — and noted that
//! #109 would have to change it, which is the point: the copy gets
//! revisited on purpose rather than inherited.
//!
//! #109 built the control. So the assertion inverts: these messages must
//! now name it, and must name the *same* one, which is why they all
//! render a single constant rather than each spelling it out.
//!
//! This remains a mechanism problem rather than a HIMI problem. HIMI is
//! the game that reaches the state today, because its manifest entry
//! deliberately retracts the compiled-in default (ADR 0005 — the
//! upstream package is 13 months stale and carries no licence). The
//! message has to be right for *any* retracted game, and for the next
//! one.

use gmm_lib::core::error::SET_AN_ORIGIN_HINT;
use gmm_lib::core::importer_origin::{
    resolve, ImporterOrigin, OriginResolution, Recommendation, StoredOverride,
};
use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

fn db_url(tmp: &TempDir) -> String {
    format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display())
}

async fn fresh_core(tmp: &TempDir) -> Core {
    Core::new(tmp.path().join("library"), &db_url(tmp))
        .await
        .expect("init")
}

/// A manifest that retracts Genshin's compiled-in default, the shape the
/// committed manifest uses for HIMI.
fn retracting_gimi() -> String {
    r#"{
      "schemaVersion": 1,
      "games": {
        "gimi": {"status": "none", "reason": "No maintained package is known right now."}
      }
    }"#
    .to_string()
}

async fn core_with_gimi_retracted(tmp: &TempDir) -> (Core, mockito::ServerGuard) {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(retracting_gimi())
        .create_async()
        .await;
    let core = fresh_core(tmp).await;
    core.refresh_recommended_importers_from(&format!(
        "{}/recommended-importers.json",
        server.url()
    ))
    .await
    .expect("refresh");
    (core, server)
}

/// Every no-origin message points the user at the control that exists,
/// by rendering the one constant that names it.
///
/// Asserting on the constant rather than on a phrase is what stops the
/// two drifting apart again: renaming the control changes the constant,
/// and every message that failed to render it fails here.
fn assert_points_at_the_real_control(context: &str, message: &str) {
    assert!(
        message.contains(SET_AN_ORIGIN_HINT),
        "{context}: a user told GMM cannot proceed has to be told what to do \
         about it, and the Importer Origin control now exists to do it with. \
         Message was: {message}",
    );
}

#[tokio::test]
async fn the_install_failure_for_a_retracted_game_names_the_origin_control() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _server) = core_with_gimi_retracted(&tmp).await;
    core.set_game_install_path(GameCode::Gimi, &tmp.path().join("Genshin"))
        .await
        .expect("set install path");

    let error = core
        .install_importer(GameCode::Gimi)
        .await
        .expect_err("a retracted game has nothing to install from");
    let message = error.to_string();

    assert_points_at_the_real_control("install", &message);
    assert!(
        message.contains("Genshin Impact"),
        "the message must name the game it is about: {message}",
    );
    assert!(
        message.contains("No maintained package is known right now."),
        "the manifest's reason is the only thing that explains *why*: {message}",
    );
}

#[tokio::test]
async fn the_update_check_for_a_retracted_game_names_the_origin_control() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _server) = core_with_gimi_retracted(&tmp).await;

    let status = core
        .check_importer_update_for(GameCode::Gimi)
        .await
        .expect("check");
    let message = status.check_error.expect(
        "a retracted game cannot be checked, and must say so rather than \
                 reporting itself up to date",
    );

    assert_points_at_the_real_control("update check", &message);
    assert!(!status.available, "nothing can be applied");
}

#[test]
fn an_unreadable_override_explains_its_own_cause() {
    // Same mechanism, different cause (#124): a stored override GMM
    // cannot read leaves no origin in effect, and the reason it carries
    // travels to exactly the same places.
    let resolved = resolve(
        &StoredOverride::Unreadable {
            raw: "{}".to_string(),
            error: "unknown variant".to_string(),
        },
        None,
        Some(&ImporterOrigin::github("owner", "repo", "x")),
    );
    match resolved {
        OriginResolution::NoneInEffect { reason } => {
            let reason = reason.expect("the user has to be told something");
            assert!(
                reason.contains("could not read"),
                "the reason has to name the cause; the control is named by the \
                 message that carries this reason. Was: {reason}",
            );
        }
        other => panic!("expected no origin in effect; got {other:?}"),
    }
}

#[test]
fn the_message_reads_correctly_for_a_game_that_is_simply_unwired() {
    // Not every no-origin state is a retraction. A game with no
    // compiled-in default and nothing above it reaches the same place
    // with no reason to offer, and the sentence still has to stand on
    // its own.
    let resolved = resolve(&StoredOverride::NotSet, None, None);
    assert!(matches!(
        resolved,
        OriginResolution::NoneInEffect { reason: None }
    ));

    // And a retraction with no reason written down, which the manifest
    // permits.
    let resolved = resolve(
        &StoredOverride::NotSet,
        Some(&Recommendation::NoRecommendation { reason: None }),
        Some(&ImporterOrigin::github("owner", "repo", "x")),
    );
    assert!(matches!(
        resolved,
        OriginResolution::NoneInEffect { reason: None }
    ));
}
