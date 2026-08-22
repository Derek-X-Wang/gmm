//! #109 — a recommendation decides a **new** install; it never switches
//! an existing one.
//!
//! ADR 0005 read both ways: it stated a three-layer precedence *and*
//! that the manifest "proposes and never auto-applies". As built, a
//! recommendation took effect everywhere, including for the ordinary
//! Update action — `install_importer` and the update badge both asked
//! `resolve_importer_origin` which origin applied and acted on the
//! answer.
//!
//! The settled rule: **an existing install's Importer Origin changes
//! only when the user accepts a proposal.** A fresh install has no game
//! directory to damage and the user has just clicked Install, so
//! honouring the recommendation there is both safe and the entire point
//! of the mechanism. An existing install is where silent substitution
//! would rewrite a game directory with a different maintainer's package,
//! and ADR 0004's posture is that nothing reaches a game directory
//! without a click.
//!
//! It also removes an incoherence rather than adding a rule. #110
//! established that changing origin **invalidates the install and
//! requires a fresh one**, so an "update" across an origin change was
//! already contradictory: the thing being updated is not the thing
//! installed. A secondary consequence is that comparing a version taken
//! against origin Y with the latest release of origin X — a meaningless
//! `upstream_ahead` — can no longer arise.
//!
//! **Retraction is unaffected** (#97). It only *removes* GMM's own
//! default; it never installs anything, and substituting a different
//! origin is a different act.

use gmm_lib::core::importer_origin::{
    origin_for_install, ImporterOrigin, InstallOrigin, InstalledOrigin, OriginLayer,
    OriginResolution,
};
use gmm_lib::core::{Core, GameCode};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

fn db_url(tmp: &TempDir) -> String {
    format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display())
}

async fn fresh_core(tmp: &TempDir) -> Core {
    Core::new(tmp.path().join("library"), &db_url(tmp))
        .await
        .expect("init")
}

fn installed_from() -> ImporterOrigin {
    ImporterOrigin::github(
        "SilentNightSound",
        "GIMI-Package",
        r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip",
    )
}

fn recommended_instead() -> ImporterOrigin {
    ImporterOrigin::github("curated", "GIMI-Fork", r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip")
}

fn in_effect(origin: &ImporterOrigin, layer: OriginLayer) -> OriginResolution {
    OriginResolution::InEffect {
        origin: origin.clone(),
        layer,
    }
}

// ---------------------------------------------------------------------
// The rule itself, stated against the decision function.
// ---------------------------------------------------------------------

#[test]
fn an_existing_install_stays_on_the_origin_it_came_from() {
    let target = origin_for_install(
        &InstalledOrigin::Known(installed_from()),
        &in_effect(&recommended_instead(), OriginLayer::RecommendedManifest),
    );

    match target {
        InstallOrigin::Installed(origin) => assert_eq!(
            origin,
            installed_from(),
            "an ordinary Update acts on the package that is installed, not on the \
             one GMM would like it to be",
        ),
        other => panic!("expected the installed origin, got {other:?}"),
    }
}

#[test]
fn an_install_that_does_not_exist_yet_is_decided_by_the_recommendation() {
    // The case the mechanism exists for. Nothing is installed, so there
    // is no game directory to damage and nothing to preserve.
    let target = origin_for_install(
        &InstalledOrigin::Unknown,
        &in_effect(&recommended_instead(), OriginLayer::RecommendedManifest),
    );

    match target {
        InstallOrigin::Resolved { origin, layer } => {
            assert_eq!(origin, recommended_instead());
            assert_eq!(layer, OriginLayer::RecommendedManifest);
        }
        other => panic!("expected the resolved origin to decide, got {other:?}"),
    }
}

#[test]
fn re_applying_the_same_origin_in_a_different_spelling_is_not_a_switch() {
    // Origin equality is case-insensitive on owner and repo (ADR 0005),
    // so a capitalisation fix upstream must not read as a different
    // package.
    let target = origin_for_install(
        &InstalledOrigin::Known(ImporterOrigin::github(
            "silentnightsound",
            "gimi-package",
            r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip",
        )),
        &in_effect(&installed_from(), OriginLayer::RecommendedManifest),
    );

    assert!(
        matches!(target, InstallOrigin::Installed(_)),
        "got {target:?}",
    );
}

#[test]
fn a_retraction_still_leaves_nothing_to_install_from() {
    // Retraction is unaffected by this decision (#97). It removes GMM's
    // own default and never installs anything, so a recorded origin does
    // not become a licence to keep pulling releases from a package GMM
    // has withdrawn its recommendation from.
    let target = origin_for_install(
        &InstalledOrigin::Known(installed_from()),
        &OriginResolution::NoneInEffect {
            reason: Some("No maintained package is known right now.".to_string()),
        },
    );

    match target {
        InstallOrigin::NoneInEffect { reason } => assert_eq!(
            reason.as_deref(),
            Some("No maintained package is known right now."),
        ),
        other => panic!("a retracted game has nothing in effect, got {other:?}"),
    }
}

#[test]
fn an_unreadable_recorded_origin_is_not_answered_by_substituting_another() {
    // GMM recorded an install and can no longer say from where (#124).
    // "We could not tell" must not be rendered as "then use this one":
    // that is the switch this whole decision forbids, performed on the
    // one install GMM understands least.
    let target = origin_for_install(
        &InstalledOrigin::Unreadable {
            raw: "{\"kind\":\"localZip\"}".to_string(),
            error: "unknown variant".to_string(),
        },
        &in_effect(&recommended_instead(), OriginLayer::RecommendedManifest),
    );

    assert!(
        matches!(target, InstallOrigin::InstalledUnreadable { .. }),
        "got {target:?}",
    );
}

// ---------------------------------------------------------------------
// The same rule, driven through the paths a user actually clicks.
// ---------------------------------------------------------------------

fn opts() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)
}

fn build_importer_zip(zip_path: &Path) {
    let mut zw = ZipWriter::new(File::create(zip_path).expect("create zip"));
    zw.add_directory("Core", opts()).expect("core dir");
    zw.start_file("Core/library.ini", opts()).expect("core ini");
    zw.write_all(b"; core\n").expect("write core");
    zw.add_directory("ShaderFixes", opts())
        .expect("shaders dir");
    zw.start_file("d3dx.ini", opts()).expect("d3dx");
    zw.write_all(b"[Loader]\nloader = XXMI Launcher.exe\n")
        .expect("write d3dx");
    zw.finish().expect("finish zip");
}

/// A stand-in GitHub that serves `releases/latest` for **two** origins
/// and counts which one was actually asked.
struct TwoOrigins {
    server: mockito::ServerGuard,
    installed: mockito::Mock,
    recommended: mockito::Mock,
    _downloads: Vec<mockito::Mock>,
}

impl TwoOrigins {
    async fn start(zip_bytes: Vec<u8>) -> Self {
        let mut server = mockito::Server::new_async().await;
        let base = server.url();

        let mut mocks = Vec::new();
        let make = |origin: &ImporterOrigin, tag: &str, asset: &str| {
            let body = serde_json::json!({
                "tag_name": tag,
                "assets": [{
                    "name": asset,
                    "browser_download_url": format!("{base}/download/{asset}"),
                }],
            })
            .to_string();
            (origin.repo_slug(), body, asset.to_string())
        };
        let installed_spec = make(&installed_from(), "v8.8.9", "GIMI-PACKAGE-v8.8.9.zip");
        let recommended_spec = make(&recommended_instead(), "v1.4.4", "GIMI-PACKAGE-v1.4.4.zip");

        for spec in [&installed_spec, &recommended_spec] {
            mocks.push(
                server
                    .mock("GET", format!("/download/{}", spec.2).as_str())
                    .with_status(200)
                    .with_body(zip_bytes.clone())
                    .create_async()
                    .await,
            );
        }

        let installed = server
            .mock(
                "GET",
                format!("/repos/{}/releases/latest", installed_spec.0).as_str(),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(installed_spec.1)
            .create_async()
            .await;
        let recommended = server
            .mock(
                "GET",
                format!("/repos/{}/releases/latest", recommended_spec.0).as_str(),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(recommended_spec.1)
            .expect(0)
            .create_async()
            .await;

        Self {
            server,
            installed,
            recommended,
            _downloads: mocks,
        }
    }

    fn endpoints(&self) -> gmm_lib::core::importer::Endpoints {
        gmm_lib::core::importer::Endpoints {
            api_base: self.server.url(),
        }
    }
}

/// Cache a manifest recommending `curated/GIMI-Fork` for Genshin, which
/// is *not* the compiled-in default and not what the seeded install came
/// from.
async fn recommend_the_fork(core: &Core, tmp: &TempDir) {
    core.set_game_install_path(GameCode::Gimi, &tmp.path().join("Genshin"))
        .await
        .expect("set install path");

    let mut manifest_host = mockito::Server::new_async().await;
    let _m = manifest_host
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(
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
            }"#,
        )
        .create_async()
        .await;
    core.refresh_recommended_importers_from(&format!(
        "{}/recommended-importers.json",
        manifest_host.url()
    ))
    .await
    .expect("cache the recommendation");

    // Precondition: the recommendation *is* in force for resolution.
    assert!(
        matches!(
            core.resolve_importer_origin(GameCode::Gimi)
                .await
                .expect("resolve"),
            OriginResolution::InEffect {
                layer: OriginLayer::RecommendedManifest,
                ..
            },
        ),
        "the test is meaningless unless the recommendation resolves",
    );
}

/// [`recommend_the_fork`], plus an install GMM performed from a
/// different origin — the state the decision is about.
async fn installed_here_recommended_there(core: &Core, tmp: &TempDir) {
    recommend_the_fork(core, tmp).await;
    core.record_importer_install(GameCode::Gimi, "v8.8.0", &installed_from())
        .await
        .expect("seed the existing install");
}

#[tokio::test]
async fn the_update_check_asks_the_origin_the_install_actually_came_from() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    installed_here_recommended_there(&core, &tmp).await;

    let zip = tmp.path().join("pkg.zip");
    build_importer_zip(&zip);
    let upstream = TwoOrigins::start(std::fs::read(&zip).expect("read zip")).await;

    let status = core
        .check_importer_update_with_endpoints(GameCode::Gimi, &upstream.endpoints())
        .await
        .expect("check");

    upstream.installed.assert_async().await;
    upstream.recommended.assert_async().await;
    assert_eq!(
        status.latest_version.as_deref(),
        Some("v8.8.9"),
        "the badge compares like with like: a version taken against the \
         installed origin, against that origin's latest release",
    );
}

#[tokio::test]
async fn the_ordinary_install_action_does_not_move_the_game_to_the_recommendation() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    installed_here_recommended_there(&core, &tmp).await;

    let zip = tmp.path().join("pkg.zip");
    build_importer_zip(&zip);
    let upstream = TwoOrigins::start(std::fs::read(&zip).expect("read zip")).await;

    core.install_importer_with_endpoints(GameCode::Gimi, &upstream.endpoints())
        .await
        .expect("install");

    upstream.recommended.assert_async().await;
    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read origin"),
        InstalledOrigin::Known(installed_from()),
        "clicking Update must not rewrite the game directory with a different \
         maintainer's package",
    );
    assert_eq!(
        core.installed_importer_version(GameCode::Gimi)
            .await
            .expect("read version"),
        Some("v8.8.9".to_string()),
    );
}

#[tokio::test]
async fn a_first_install_does_follow_the_recommendation() {
    // The other half of the rule, and the reason "propose only,
    // including for fresh installs" was rejected: it would make the
    // manifest useless in the case where it is safest.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    // No `record_importer_install`: nothing GMM performed, which is the
    // never-installed and the hand-installed state alike (#99).
    recommend_the_fork(&core, &tmp).await;
    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read origin"),
        InstalledOrigin::Unknown,
    );

    let zip = tmp.path().join("pkg.zip");
    build_importer_zip(&zip);
    let mut upstream = TwoOrigins::start(std::fs::read(&zip).expect("read zip")).await;
    // This time the recommended origin is the one that must be asked.
    upstream.recommended = upstream
        .server
        .mock(
            "GET",
            format!(
                "/repos/{}/releases/latest",
                recommended_instead().repo_slug()
            )
            .as_str(),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "tag_name": "v1.4.4",
                "assets": [{
                    "name": "GIMI-PACKAGE-v1.4.4.zip",
                    "browser_download_url":
                        format!("{}/download/GIMI-PACKAGE-v1.4.4.zip", upstream.server.url()),
                }],
            })
            .to_string(),
        )
        .create_async()
        .await;

    core.install_importer_with_endpoints(GameCode::Gimi, &upstream.endpoints())
        .await
        .expect("install");

    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read origin"),
        InstalledOrigin::Known(recommended_instead()),
        "an install that does not exist yet is exactly what a recommendation is \
         for",
    );
}
