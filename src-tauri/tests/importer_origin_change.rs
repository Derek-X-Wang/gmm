//! ADR 0005 / #110 — an Importer Origin change clears the Importer Pin
//! and invalidates the recorded install.
//!
//! An Importer Pin is a per-game version string, and `compute_status`
//! gates on it as a **boolean**: any pin at all suppresses the badge,
//! whatever string it holds. A pin carried across an origin change
//! would therefore suppress every update for the new origin
//! indefinitely while the user believed they were being kept current —
//! the same silent-and-alive-looking defect as #78.
//!
//! Seams under test: the `Core` API the Tauri commands call, plus the
//! pure decision in `core::importer_origin`. Nothing here reaches the
//! network or a game directory.

use gmm_lib::core::importer_origin::{ImporterOrigin, InstalledOrigin};
use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init")
}

fn gimi_default() -> ImporterOrigin {
    ImporterOrigin::github(
        "SilentNightSound",
        "GIMI-Package",
        r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip",
    )
}

fn a_different_origin() -> ImporterOrigin {
    ImporterOrigin::github("someone", "GIMI-Fork", r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip")
}

/// A game installed from its compiled-in default, pinned to the
/// version that install recorded — the ban-wave shape from ADR 0004.
async fn installed_and_pinned(core: &Core, origin: &ImporterOrigin, version: &str) {
    core.record_importer_install(GameCode::Gimi, version, origin)
        .await
        .expect("record install");
    core.set_importer_pinned(GameCode::Gimi, Some(version))
        .await
        .expect("pin");
}

#[tokio::test]
async fn overriding_a_game_onto_a_different_origin_clears_its_pin() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    installed_and_pinned(&core, &gimi_default(), "v8.8.9").await;

    core.set_importer_origin_override(GameCode::Gimi, Some(&a_different_origin()))
        .await
        .expect("set override");

    assert_eq!(
        core.importer_pinned(GameCode::Gimi)
            .await
            .expect("read pin"),
        None,
        "a pin taken against the old origin is meaningless against the new one",
    );
}

#[tokio::test]
async fn overriding_a_game_onto_a_different_origin_invalidates_its_install() {
    // The game directory still physically holds the previous origin's
    // package. Leaving the install recorded as valid would let the
    // database and the disk disagree; backup and rollback already exist
    // and make a re-install safe.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    installed_and_pinned(&core, &gimi_default(), "v8.8.9").await;

    core.set_importer_origin_override(GameCode::Gimi, Some(&a_different_origin()))
        .await
        .expect("set override");

    assert_eq!(
        core.installed_importer_version(GameCode::Gimi)
            .await
            .expect("read version"),
        None,
        "the game reports as not installed for the new origin",
    );
    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read origin"),
        InstalledOrigin::Unknown,
        "the recorded origin goes with the recorded version",
    );
}

#[tokio::test]
async fn re_applying_the_same_origin_in_different_letter_case_clears_nothing() {
    // GitHub treats owner/repo case-insensitively, and origin equality
    // already folds case. A capitalisation fix must not read as a
    // change and throw away a working install plus the user's pin.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    installed_and_pinned(&core, &gimi_default(), "v8.8.9").await;

    let same_origin_other_spelling = ImporterOrigin::github(
        "silentnightsound",
        "gimi-package",
        r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip",
    );
    core.set_importer_origin_override(GameCode::Gimi, Some(&same_origin_other_spelling))
        .await
        .expect("set override");

    assert_eq!(
        core.importer_pinned(GameCode::Gimi)
            .await
            .expect("read pin"),
        Some("v8.8.9".to_string()),
        "the same origin spelled differently is not an origin change",
    );
    assert_eq!(
        core.installed_importer_version(GameCode::Gimi)
            .await
            .expect("read version"),
        Some("v8.8.9".to_string()),
    );
}

// ---------------------------------------------------------------
// Accepting a recommended origin. The user-facing act is
// `install_importer`, which installs from the resolved origin and then
// records it — the only way an unknown origin becomes known (#99), and
// therefore the only place an installed origin can change.
// ---------------------------------------------------------------

#[tokio::test]
async fn installing_from_a_different_origin_clears_the_pin() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    installed_and_pinned(&core, &gimi_default(), "v8.8.9").await;

    let recommended = a_different_origin();
    core.record_importer_install(GameCode::Gimi, "v1.4.4", &recommended)
        .await
        .expect("accept the recommendation");

    assert_eq!(
        core.importer_pinned(GameCode::Gimi)
            .await
            .expect("read pin"),
        None,
        "the user re-pins against the new origin if they still want to hold still",
    );
    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read origin"),
        InstalledOrigin::Known(recommended),
    );
    assert_eq!(
        core.installed_importer_version(GameCode::Gimi)
            .await
            .expect("read version"),
        Some("v1.4.4".to_string()),
        "the install that just happened is what is recorded",
    );
}

#[tokio::test]
async fn installing_a_new_version_from_the_same_origin_keeps_the_pin() {
    // A pin suppresses version updates; applying one anyway is the
    // user's own explicit act and says nothing about the origin. Only
    // an origin change clears the pin.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    installed_and_pinned(&core, &gimi_default(), "v8.8.8").await;

    core.record_importer_install(GameCode::Gimi, "v8.8.9", &gimi_default())
        .await
        .expect("apply a version update");

    assert_eq!(
        core.importer_pinned(GameCode::Gimi)
            .await
            .expect("read pin"),
        Some("v8.8.8".to_string()),
        "same origin, new version: the pin is untouched",
    );
}

#[tokio::test]
async fn accepting_a_recommendation_for_an_unknown_origin_install_clears_any_pin() {
    // #99: accepting from unknown *is* an origin change — an install
    // happens and the version recorded afterwards comes from the new
    // origin. These users normally have no pin, because nothing
    // recorded a version to pin against; a GMM build predating origin
    // tracking (#107) is the cohort that can have both.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.set_importer_installed(GameCode::Gimi, "v8.8.0")
        .await
        .expect("seed a pre-origin-tracking install");
    core.set_importer_pinned(GameCode::Gimi, Some("v8.8.0"))
        .await
        .expect("pin");

    core.record_importer_install(GameCode::Gimi, "v1.4.4", &a_different_origin())
        .await
        .expect("accept");

    assert_eq!(
        core.importer_pinned(GameCode::Gimi)
            .await
            .expect("read pin"),
        None,
    );
}

#[tokio::test]
async fn accepting_a_recommendation_with_no_pin_to_clear_completes_without_error() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    core.record_importer_install(GameCode::Gimi, "v1.4.4", &a_different_origin())
        .await
        .expect("accepting must not require a pin to exist");

    assert_eq!(
        core.importer_pinned(GameCode::Gimi)
            .await
            .expect("read pin"),
        None,
    );
    assert_eq!(
        core.installed_importer_version(GameCode::Gimi)
            .await
            .expect("read version"),
        Some("v1.4.4".to_string()),
    );
}

// ---------------------------------------------------------------
// A pin suppresses *version* updates only, never origin
// recommendations. The two propositions are unrelated: a pin says
// "don't move me to a newer build of this package", not "stop telling
// me my package's source is dead".
//
// #110 owns the rule; #109 owns the prompt that renders it. What is
// asserted here is the decision itself — that it never consults the
// pin.
// ---------------------------------------------------------------

/// A manifest recommending one origin for Genshin, served from a local
/// mock so the layer is real rather than hand-seeded.
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

async fn core_with_a_gimi_recommendation(tmp: &TempDir) -> (Core, mockito::ServerGuard) {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(recommending_gimi("curated", "GIMI-Fork"))
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

#[tokio::test]
async fn a_pinned_game_still_has_an_origin_recommendation_to_surface() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _server) = core_with_a_gimi_recommendation(&tmp).await;
    installed_and_pinned(&core, &gimi_default(), "v8.8.9").await;

    let proposed = core
        .pending_importer_origin_change(GameCode::Gimi)
        .await
        .expect("decide");

    assert_eq!(
        proposed.as_ref().map(|o| o.repo_slug()),
        Some("curated/GIMI-Fork".to_string()),
        "a pin is not a request to stop being told the package's source is dead",
    );
}

#[tokio::test]
async fn the_pin_makes_no_difference_to_what_is_proposed() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _server) = core_with_a_gimi_recommendation(&tmp).await;
    core.record_importer_install(GameCode::Gimi, "v8.8.9", &gimi_default())
        .await
        .expect("record");

    let unpinned = core
        .pending_importer_origin_change(GameCode::Gimi)
        .await
        .expect("decide");
    core.set_importer_pinned(GameCode::Gimi, Some("v8.8.9"))
        .await
        .expect("pin");
    let pinned = core
        .pending_importer_origin_change(GameCode::Gimi)
        .await
        .expect("decide");

    assert_eq!(unpinned, pinned, "the pin is not an input to this decision");
    assert!(pinned.is_some());
}

#[tokio::test]
async fn a_pinned_game_still_has_its_version_update_badge_suppressed() {
    // The other half of the same rule, and the reason clearing matters:
    // `compute_status` gates on the pin as a boolean, so a pin that
    // survived an origin change would suppress the badge forever.
    use gmm_lib::core::updates::compute_status;

    let pinned = compute_status(Some("v8.8.8".into()), Ok("v8.8.9".into()), true);
    assert!(
        !pinned.available,
        "a pinned game surfaces no version update",
    );
    assert!(
        pinned.upstream_ahead,
        "but GMM still knows one exists, so the UI can say so",
    );

    let unpinned = compute_status(Some("v8.8.8".into()), Ok("v8.8.9".into()), false);
    assert!(unpinned.available);
}

#[tokio::test]
async fn a_game_already_on_the_recommended_origin_has_nothing_to_propose() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _server) = core_with_a_gimi_recommendation(&tmp).await;
    let recommended =
        ImporterOrigin::github("curated", "GIMI-Fork", r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip");
    core.record_importer_install(GameCode::Gimi, "v1.0.0", &recommended)
        .await
        .expect("record");

    assert_eq!(
        core.pending_importer_origin_change(GameCode::Gimi)
            .await
            .expect("decide"),
        None,
    );
}

#[tokio::test]
async fn an_unknown_origin_install_is_never_nagged_towards_the_compiled_in_default() {
    // #99: unknown origin is never surfaced proactively. Without this,
    // every hand-installed setup would be prompted on every launch to
    // adopt a default GMM merely ships — which is not a recommendation
    // at all.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.set_importer_installed(GameCode::Gimi, "v8.8.0")
        .await
        .expect("seed a hand-installed shape");

    assert_eq!(
        core.pending_importer_origin_change(GameCode::Gimi)
            .await
            .expect("decide"),
        None,
        "the compiled-in default is the status quo, not a proposal",
    );
}

#[tokio::test]
async fn an_unknown_origin_install_is_offered_a_real_recommendation() {
    // The flip side, and the only route by which unknown becomes known
    // (#99): when GMM actually recommends something, the user is asked.
    let tmp = TempDir::new().expect("tmp");
    let (core, _server) = core_with_a_gimi_recommendation(&tmp).await;
    core.set_importer_installed(GameCode::Gimi, "v8.8.0")
        .await
        .expect("seed a hand-installed shape");

    assert_eq!(
        core.pending_importer_origin_change(GameCode::Gimi)
            .await
            .expect("decide")
            .map(|o| o.repo_slug()),
        Some("curated/GIMI-Fork".to_string()),
    );
}

// ---------------------------------------------------------------
// After an origin change the game needs a fresh install, and the
// chokepoint that guarantees no call path can skip the reconciliation.
// ---------------------------------------------------------------

#[tokio::test]
async fn after_an_origin_change_the_update_check_reports_a_fresh_install() {
    // `compute_status` reads a missing installed version as "fresh
    // install, nothing to upgrade" — which is the honest answer once
    // the record is gone, and it is what puts the user in front of an
    // Install rather than an Update.
    use gmm_lib::core::importer_origin::OriginResolution;

    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    installed_and_pinned(&core, &gimi_default(), "v8.8.9").await;

    core.set_importer_origin_override(GameCode::Gimi, Some(&a_different_origin()))
        .await
        .expect("set override");

    let status = core
        .check_importer_update_with(
            GameCode::Gimi,
            &OriginResolution::NoneInEffect { reason: None },
        )
        .await
        .expect("must not hard-fail");

    assert_eq!(
        status.installed_version, None,
        "nothing GMM installed is in that game directory for the new origin",
    );
    assert!(
        !status.pinned,
        "and no pin is left to suppress the new origin's updates",
    );
}

#[tokio::test]
async fn clearing_an_override_that_moves_the_game_back_onto_a_different_origin_reconciles_too() {
    // Clearing an override is as much an origin change as setting one:
    // it can drop the game onto the recommendation or the compiled-in
    // default. Reconciling only the obvious half would leave a stale
    // pin behind through the back door.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let mine = a_different_origin();
    core.set_importer_origin_override(GameCode::Gimi, Some(&mine))
        .await
        .expect("set override");
    installed_and_pinned(&core, &mine, "v1.0.0").await;

    core.set_importer_origin_override(GameCode::Gimi, None)
        .await
        .expect("clear override");

    assert_eq!(
        core.importer_pinned(GameCode::Gimi)
            .await
            .expect("read pin"),
        None,
        "the game now follows the compiled-in default, which is a different origin",
    );
    assert_eq!(
        core.installed_importer_version(GameCode::Gimi)
            .await
            .expect("read version"),
        None,
    );
}

#[test]
fn no_tauri_command_records_an_install_without_going_through_the_reconciliation() {
    // `record_importer_install` is the only writer of the installed
    // origin, and therefore the only place that can notice an origin
    // change on the accept path. A command that reached
    // `set_importer_installed` directly would record a version against
    // a new origin and quietly leave the old pin in place — which is
    // the whole defect. Asserted against the real source so it cannot
    // be reintroduced by accident.
    let commands = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("commands.rs");
    let text = std::fs::read_to_string(&commands).expect("read commands.rs");

    assert!(
        !text.contains("set_importer_installed"),
        "commands.rs must record installs through record_importer_install, \
         which reconciles the pin and the install record against the origin (#110)",
    );
    assert!(
        text.contains("record_importer_install"),
        "the install command must still record what it installed",
    );
}
