//! Issue #78: the Loader update check could never report an update.
//!
//! Three independent defects conspired: the asset filter (`"Libs"`)
//! matched no asset in any `XXMI-Libs-Package` release, the resulting
//! error was swallowed by `.ok().flatten()`, and the "installed" side
//! of the comparison came from a settings row nothing ever wrote.
//!
//! The fixture in `tests/fixtures/github/xxmi-libs-package-latest.json`
//! is a **recorded copy of the real API response** —
//! `gh api repos/SpectrumQT/XXMI-Libs-Package/releases/latest`, taken
//! 2026-08-16. It is deliberately not hand-written: a hand-written
//! fixture would have been invented from the same wrong assumption as
//! the filter, and would have agreed with the bug.

use gmm_lib::core::importer;
use gmm_lib::core::updates::{self, LOADER_ASSET_FILTER};
use gmm_lib::core::Core;
use tempfile::TempDir;

/// The recorded upstream release payload, parsed as JSON.
fn recorded_release() -> serde_json::Value {
    let raw = include_str!("fixtures/github/xxmi-libs-package-latest.json");
    serde_json::from_str(raw).expect("recorded fixture is valid JSON")
}

#[test]
fn shipped_asset_filter_matches_the_real_release_asset() {
    let release = importer::parse_latest_release(&recorded_release(), LOADER_ASSET_FILTER)
        .expect("the filter GMM ships must match a real upstream asset");

    assert_eq!(release.tag_name, "v1.0.2");
    assert_eq!(release.asset_name, "XXMI-PACKAGE-v1.0.2.zip");
}

#[test]
fn the_old_libs_filter_matched_nothing_and_that_is_why_78_happened() {
    // Regression guard. `"Libs"` appears in the *repository* name, not
    // in any asset name — the assets are `XXMI-PACKAGE-v<version>.zip`
    // and `Manifest.json`.
    let err = importer::parse_latest_release(&recorded_release(), "Libs")
        .expect_err("`Libs` never matched an asset; that was the bug");

    assert!(
        err.to_string().contains("Libs"),
        "the error must name the filter that missed, got: {err}"
    );
}

#[test]
fn loader_repo_and_filter_are_the_ones_the_recording_came_from() {
    assert_eq!(updates::LOADER_REPO, "SpectrumQT/XXMI-Libs-Package");
}

#[test]
fn upstream_ahead_is_reported_when_the_tags_differ() {
    let status = updates::loader_status(Ok("v1.0.2".to_string()));

    assert_eq!(status.shipped_version, updates::SHIPPED_LOADER_VERSION);
    assert_eq!(status.latest_version.as_deref(), Some("v1.0.2"));
    assert!(status.upstream_ahead);
    assert!(status.check_error.is_none());
}

#[test]
fn up_to_date_when_upstream_matches_what_we_ship() {
    let status = updates::loader_status(Ok(updates::SHIPPED_LOADER_VERSION.to_string()));

    assert!(!status.upstream_ahead);
    assert!(status.check_error.is_none());
}

#[test]
fn a_failed_check_is_not_silently_up_to_date() {
    // The whole reason #78 survived: `.ok().flatten()` turned every
    // failure into `latest_version: None`, which rendered exactly like
    // "nothing to report".
    let failed = updates::loader_status(Err("GitHub returned 503".to_string()));
    let up_to_date = updates::loader_status(Ok(updates::SHIPPED_LOADER_VERSION.to_string()));

    assert_eq!(failed.check_error.as_deref(), Some("GitHub returned 503"));
    assert!(failed.latest_version.is_none());
    assert!(!failed.upstream_ahead);

    assert_ne!(
        failed, up_to_date,
        "a failed check must be distinguishable from a successful one"
    );
}

/// End of the pure decisions; below drives the real Core seam.
async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init")
}

#[tokio::test]
async fn core_surfaces_a_failing_fetch_instead_of_swallowing_it() {
    // Points at a repo that cannot resolve, so the fetch fails
    // whether or not the machine is online. Before #78 this path ran
    // `.ok().flatten()` and returned a status indistinguishable from
    // a healthy "no update".
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let status = core
        .check_loader_update_from("Derek-X-Wang/does-not-exist", ".zip")
        .await
        .expect("the check itself returns Ok; the failure rides inside the status");

    assert!(
        status.check_error.is_some(),
        "a failed fetch must be reported, got {status:?}"
    );
    assert!(status.latest_version.is_none());
    assert!(!status.upstream_ahead);
    assert_eq!(
        status.shipped_version,
        updates::SHIPPED_LOADER_VERSION,
        "we still know what we ship even when upstream is unreachable"
    );

    // This string is rendered verbatim in the UI. The Loader check
    // installs nothing, so the old `Error::Importer` prefix
    // ("importer install error: …") would have described the wrong
    // subsystem to the user.
    let message = status.check_error.expect("checked above");
    assert!(
        !message.contains("importer install"),
        "a Loader check failure must not be reported as an importer install \
         failure, got: {message}"
    );
}

/// The upstream `Manifest.json` that ships next to the vendored
/// `3dmloader.dll`, read at *run* time. `SHIPPED_LOADER_VERSION` is
/// baked in at *build* time from the same file, so comparing the two
/// proves the constant actually tracks the vendored bundle.
fn vendored_manifest_version() -> String {
    let manifest =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../vendor/3dmloader/Manifest.json");
    let raw = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest.display()));
    let json: serde_json::Value = serde_json::from_str(&raw).expect("Manifest.json is valid JSON");
    json["version"]
        .as_str()
        .expect("Manifest.json has a string `version`")
        .to_string()
}

#[test]
fn shipped_loader_version_is_baked_in_from_the_vendored_bundle() {
    assert_eq!(
        updates::SHIPPED_LOADER_VERSION,
        format!("v{}", vendored_manifest_version()),
        "the reported Loader version must come from the vendored bundle, \
         not from a settings row nothing writes (#78)"
    );
}

#[test]
fn shipped_loader_version_is_tag_shaped_so_it_compares_to_upstream() {
    // Upstream release tags are `v0.8.8`, `v1.0.2`, … while
    // `Manifest.json` records a bare `0.8.8`. The constant is
    // normalised to tag form so the comparison against `tag_name` is
    // like-for-like rather than always-unequal.
    let shipped = updates::SHIPPED_LOADER_VERSION;
    assert!(
        shipped.starts_with('v') && shipped.len() > 1,
        "expected a tag-shaped version, got {shipped:?}"
    );
}
