//! Slice 16-b (#24) — first-run onboarding wizard backend contract.
//!
//! The Core methods + Tauri commands light up the wizard's React
//! state machine. The wizard itself lives in `src/`; this test file
//! exercises the persistent state + the parallel detect dispatch.

use gmm_lib::core::Core;
use tempfile::TempDir;

async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init core")
}

#[tokio::test]
async fn onboarding_status_defaults_to_incomplete_not_skipped() {
    // First-run state. The wizard must auto-open until the user
    // either finishes or explicitly skips.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let status = core.onboarding_status().await.expect("status");
    assert!(!status.complete, "fresh core must not be marked complete");
    assert!(!status.skipped, "fresh core must not be marked skipped");
}

#[tokio::test]
async fn mark_onboarding_complete_finish_path_persists() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.mark_onboarding_complete(false)
        .await
        .expect("mark complete");
    let status = core.onboarding_status().await.expect("status");
    assert!(status.complete, "complete=true after finish");
    assert!(!status.skipped, "skipped=false on finish path");
}

#[tokio::test]
async fn mark_onboarding_complete_skip_path_persists() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.mark_onboarding_complete(true)
        .await
        .expect("mark complete via skip");
    let status = core.onboarding_status().await.expect("status");
    assert!(status.complete, "complete=true even when skipped");
    assert!(status.skipped, "skipped=true via skip path");
}

#[tokio::test]
async fn reset_onboarding_re_opens_the_wizard_on_next_launch() {
    // The Help → "Run setup again" entry point reopens the wizard.
    // After reset, the next `onboarding_status` call must look like
    // a fresh install.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.mark_onboarding_complete(true)
        .await
        .expect("mark via skip");
    core.reset_onboarding().await.expect("reset");
    let status = core.onboarding_status().await.expect("status");
    assert!(!status.complete);
    assert!(!status.skipped);
}
