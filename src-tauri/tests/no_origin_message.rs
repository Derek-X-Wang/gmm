//! Issue #127 — the "no Importer Origin in effect" message tells the
//! user the truth and does not point at a control that does not exist.
//!
//! When a game has no Importer Origin in effect, install and the update
//! check both told the user to *choose one in Settings*. There is no
//! such control: no origin command is registered with Tauri and nothing
//! in the frontend exposes one. The surface that will provide it is
//! #109, still open.
//!
//! This is a mechanism problem rather than a HIMI problem. HIMI is the
//! game that reaches the state today, because its manifest entry
//! deliberately retracts the compiled-in default (ADR 0005 — the
//! upstream package is 13 months stale and carries no licence). The
//! message is wrong for *any* retracted game, and will be wrong for the
//! next one.
//!
//! The assertion is deliberately blunt: no user-facing no-origin message
//! may mention Settings until Settings actually has the control. When
//! #109 lands it will have to change this test, which is the point —
//! the copy gets revisited on purpose rather than inherited.

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

fn assert_points_at_no_imaginary_control(context: &str, message: &str) {
    assert!(
        !message.to_lowercase().contains("settings"),
        "{context}: the message sends the user to Settings, which has no Importer \\
         Origin control — #109 is the ticket that adds one. Message was: {message}",
    );
    assert!(
        !message.to_lowercase().contains("choose one"),
        "{context}: \"choose one\" is an instruction to use a control that does not \\
         exist. Message was: {message}",
    );
}

#[tokio::test]
async fn the_install_failure_for_a_retracted_game_does_not_send_the_user_to_settings() {
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

    assert_points_at_no_imaginary_control("install", &message);
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
async fn the_update_check_for_a_retracted_game_does_not_send_the_user_to_settings() {
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

    assert_points_at_no_imaginary_control("update check", &message);
    assert!(!status.available, "nothing can be applied");
}

#[test]
fn the_unreadable_override_message_does_not_send_the_user_to_settings_either() {
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
            assert_points_at_no_imaginary_control(
                "unreadable override",
                &reason.expect("the user has to be told something"),
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
