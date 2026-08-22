//! #109 — the Importer Origin surface, driven against the **committed**
//! `recommended-importers.json` rather than a fixture.
//!
//! The fixtures elsewhere prove the mechanism. This file proves the
//! mechanism against the file every shipped build actually fetches,
//! which is where the two can quietly disagree: the committed manifest
//! recommends origins byte-identical to the compiled-in defaults for
//! five games, and that is precisely the shape that made a layer-keyed
//! proposal guard fire on every launch (#125).
//!
//! **HIMI is the live retracted game.** Its entry deliberately retracts
//! the compiled-in default — `leotorrez/HIMI-Package` last released
//! 2025-07-24 and carries no licence, so there is no maintained package
//! GMM can recommend and none it could legally fork or mirror. It is not
//! a bug to be fixed here; it is the one real example of the no-origin
//! state, so it is what the no-origin surface gets checked against.

use gmm_lib::core::importer_origin::{InstallTargetView, OriginLayer, OriginResolution};
use gmm_lib::core::recommended_importers::MANIFEST_PATH;
use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

fn committed_manifest() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .join(MANIFEST_PATH);
    std::fs::read_to_string(&path).expect("the committed manifest")
}

/// A `Core` holding the committed manifest as its cache, the state every
/// real install reaches after its first successful launch.
async fn core_with_the_real_manifest(tmp: &TempDir) -> (Core, mockito::ServerGuard) {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(committed_manifest())
        .create_async()
        .await;
    let core = Core::new(
        tmp.path().join("library"),
        &format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display()),
    )
    .await
    .expect("init");
    core.refresh_recommended_importers_from(&format!(
        "{}/recommended-importers.json",
        server.url()
    ))
    .await
    .expect("refresh");
    (core, server)
}

#[tokio::test]
async fn himi_has_no_origin_in_effect_and_the_surface_says_what_to_do() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _server) = core_with_the_real_manifest(&tmp).await;

    let status = core
        .importer_origin_status(GameCode::Himi)
        .await
        .expect("status");

    match &status.resolved {
        OriginResolution::NoneInEffect { reason } => {
            let reason = reason.as_deref().expect("the retraction carries a reason");
            assert!(
                reason.contains("Honkai Impact"),
                "the reason has to be about this game: {reason}",
            );
        }
        other => panic!("HIMI's committed entry retracts its default; got {other:?}"),
    }
    assert!(
        matches!(
            status.install_target,
            InstallTargetView::NoneInEffect { .. }
        ),
        "there is nothing to install from, and that is not the same as \
         'not installed': got {:?}",
        status.install_target,
    );
    assert!(
        status.proposal.is_none(),
        "a retraction is not a proposal — there is no origin to move onto",
    );
}

#[tokio::test]
async fn himis_install_failure_explains_itself_and_points_at_the_control() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _server) = core_with_the_real_manifest(&tmp).await;
    core.set_game_install_path(GameCode::Himi, &tmp.path().join("HI3"))
        .await
        .expect("install path");

    let error = core
        .install_importer(GameCode::Himi)
        .await
        .expect_err("a retracted game has nothing to install from")
        .to_string();

    assert!(error.contains("Honkai Impact 3rd"), "{error}");
    assert!(
        error.contains(gmm_lib::core::error::SET_AN_ORIGIN_HINT),
        "the control exists now, so the message says where it is: {error}",
    );
}

#[tokio::test]
async fn a_user_supplied_origin_rescues_himi_end_to_end() {
    // The whole point of the conduit: a retracted game is a settings
    // change rather than an indefinite wait, and it works on an
    // already-shipped build because the manifest is fetched.
    use gmm_lib::core::importer_origin::ImporterOrigin;

    let tmp = TempDir::new().expect("tmp");
    let (core, _server) = core_with_the_real_manifest(&tmp).await;

    let mine = ImporterOrigin::github("someone", "HIMI-Rescue", r"HIMI-PACKAGE-v\d+\.zip");
    core.set_importer_origin_override(GameCode::Himi, Some(&mine))
        .await
        .expect("the user is never blocked from supplying an origin");

    let status = core
        .importer_origin_status(GameCode::Himi)
        .await
        .expect("status");
    match &status.resolved {
        OriginResolution::InEffect { origin, layer } => {
            assert_eq!(origin, &mine);
            assert_eq!(*layer, OriginLayer::UserOverride);
        }
        other => panic!("expected the user's own origin in effect, got {other:?}"),
    }
    match &status.install_target {
        InstallTargetView::Resolved { origin, .. } => assert_eq!(origin, &mine),
        other => panic!("a first install follows what resolves; got {other:?}"),
    }
}

#[tokio::test]
async fn the_committed_manifest_proposes_nothing_to_a_hand_installed_setup() {
    // #125's failure, checked against the real file. Five committed
    // entries are byte-identical to the compiled-in defaults, so once a
    // manifest is cached — i.e. after the first successful launch, i.e.
    // always — those games resolve at the manifest layer. A guard keyed
    // on the *layer* rather than on the origin's value would then fire
    // for every hand-installed setup on every launch, which is the
    // proactive surfacing of unknown origin that #99 rejects.
    let tmp = TempDir::new().expect("tmp");
    let (core, _server) = core_with_the_real_manifest(&tmp).await;

    for game in [
        GameCode::Gimi,
        GameCode::Srmi,
        GameCode::Zzmi,
        GameCode::Wwmi,
        GameCode::Efmi,
    ] {
        let status = core.importer_origin_status(game).await.expect("status");
        assert_eq!(
            status.resolved.origin(),
            gmm_lib::core::importer_origin::compiled_in_default(game).as_ref(),
            "{}: the committed entry mirrors the compiled-in default",
            game.as_str(),
        );
        assert!(
            status.proposal.is_none(),
            "{}: nothing has changed, so nothing is proposed",
            game.as_str(),
        );
    }
}

#[tokio::test]
async fn srmis_committed_reason_reaches_the_surface_when_it_is_proposed() {
    // `srmi` is the one committed entry carrying a `reason`, and until
    // #109 the parser read `reason` only for a `none` entry — so the
    // text sitting in the file since it was written went nowhere. The
    // proposal only exists when the install came from somewhere else, so
    // that is the state it gets checked in.
    use gmm_lib::core::importer_origin::ImporterOrigin;

    let tmp = TempDir::new().expect("tmp");
    let (core, _server) = core_with_the_real_manifest(&tmp).await;

    core.record_importer_install(
        GameCode::Srmi,
        "v2.4.0",
        &ImporterOrigin::github("someone-else", "SRMI-Old", r"SRMI-PACKAGE-v\d+\.zip"),
    )
    .await
    .expect("seed an install from elsewhere");

    let proposal = core
        .importer_origin_status(GameCode::Srmi)
        .await
        .expect("status")
        .proposal
        .expect("the committed manifest recommends a different origin");

    assert_eq!(proposal.origin.repo_slug(), "SpectrumQT/SRMI-Package");
    assert!(
        proposal
            .reason
            .as_deref()
            .is_some_and(|r| r.contains("TEST")),
        "the committed reason explains why the package is named TEST, and it \
         is the whole grounds a user has for trusting the prompt: {:?}",
        proposal.reason,
    );
}
