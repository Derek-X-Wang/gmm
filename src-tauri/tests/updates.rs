//! Slice 13b: importer + loader update badges + per-game pin.
//!
//! Drives the pure decision (`compute_status`) and the persistence
//! layer via the public Core methods. The network-fetch shape is
//! already covered in tests/gamebanana.rs + tests/importer.rs; we
//! don't repeat that here.

use gmm_lib::core::updates::{compute_status, UpdateStatus};
use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init")
}

#[test]
fn compute_status_no_install_no_badge() {
    let s = compute_status(None, Ok("v0.7.1".into()), false);
    assert!(
        !s.available,
        "no installed_version → nothing to upgrade from"
    );
    assert!(!s.upstream_ahead);
}

#[test]
fn compute_status_installed_equal_latest_no_badge() {
    let s = compute_status(Some("v0.7.1".into()), Ok("v0.7.1".into()), false);
    assert!(!s.available);
    assert!(!s.upstream_ahead);
}

#[test]
fn compute_status_upstream_ahead_no_pin_available() {
    let s = compute_status(Some("v0.7.0".into()), Ok("v0.7.1".into()), false);
    assert!(s.upstream_ahead);
    assert!(s.available);
    assert!(!s.pinned);
}

#[test]
fn compute_status_pin_suppresses_available_but_keeps_upstream_ahead() {
    let s: UpdateStatus = compute_status(Some("v0.7.0".into()), Ok("v0.7.1".into()), true);
    assert!(!s.available, "pin must suppress the badge per ADR 0004",);
    assert!(
        s.upstream_ahead,
        "dialog still needs to know upstream moved",
    );
    assert!(s.pinned);
}

#[test]
fn a_failed_asset_selection_is_not_reported_as_up_to_date() {
    // #79: zero matches and two-or-more matches are both errors, and
    // neither may be collapsed into "no update available". The importer
    // path swallowed them with `.ok().flatten()` — the same defect #78
    // fixed on the Loader side, left behind here.
    let failed = compute_status(
        Some("v8.8.8".into()),
        Err("no asset in release v8.8.9 matched the pattern \"GIMI-PACKAGE-…\"".into()),
        false,
    );
    let current = compute_status(Some("v8.8.9".into()), Ok("v8.8.9".into()), false);

    assert!(
        failed.check_error.is_some(),
        "a failed selection must say so, got {failed:?}"
    );
    assert!(failed.latest_version.is_none());
    assert!(!failed.available);
    assert!(!failed.upstream_ahead);
    assert_ne!(
        failed, current,
        "a failed check must be distinguishable from a successful one"
    );
}

#[tokio::test]
async fn core_surfaces_a_failed_importer_check_instead_of_swallowing_it() {
    // Points at a repo that cannot resolve, so the fetch fails whether
    // or not the machine is online.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.set_importer_installed(GameCode::Gimi, "v8.8.8")
        .await
        .expect("seed install");

    let status = core
        .check_importer_update(GameCode::Gimi, "Derek-X-Wang/does-not-exist", r"x\.zip")
        .await
        .expect("the check itself returns Ok; the failure rides inside the status");

    assert!(
        status.check_error.is_some(),
        "a failed fetch must be reported, got {status:?}"
    );
    assert!(!status.available, "we do not know that an update exists");
    assert_eq!(status.installed_version.as_deref(), Some("v8.8.8"));
}

#[tokio::test]
async fn installed_version_round_trips_through_settings() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    core.set_importer_installed(GameCode::Gimi, "v0.7.0")
        .await
        .expect("seed install");

    // Read back via the pin/install settings layer through Core's
    // check function — using a deliberately-bogus repo so the latest
    // fetch fails and `latest_version` is None. The installed value
    // should still surface.
    let status = core
        .check_importer_update(GameCode::Gimi, "Derek-X-Wang/does-not-exist", ".zip")
        .await
        .expect("check");
    assert_eq!(status.installed_version.as_deref(), Some("v0.7.0"));
}

#[tokio::test]
async fn pinning_persists_and_clears() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    core.set_importer_installed(GameCode::Gimi, "v0.7.0")
        .await
        .expect("seed install");
    core.set_importer_pinned(GameCode::Gimi, Some("v0.7.0"))
        .await
        .expect("pin");

    let status = core
        .check_importer_update(GameCode::Gimi, "Derek-X-Wang/does-not-exist", ".zip")
        .await
        .expect("check");
    assert!(status.pinned);
    assert!(!status.available);

    core.set_importer_pinned(GameCode::Gimi, None)
        .await
        .expect("unpin");
    let status = core
        .check_importer_update(GameCode::Gimi, "Derek-X-Wang/does-not-exist", ".zip")
        .await
        .expect("check");
    assert!(!status.pinned);
}
