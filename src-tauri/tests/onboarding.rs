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

async fn seed_onboarding_and_reject_skipped_write(tmp: &TempDir, complete: bool, skipped: bool) {
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let pool = sqlx::SqlitePool::connect(&db_url).await.expect("open db");
    for (key, value) in [
        ("onboarding.complete", complete),
        ("onboarding.skipped", skipped),
    ] {
        gmm_lib::core::settings::put(&pool, key, Some(if value { "true" } else { "false" }))
            .await
            .expect("seed onboarding");
    }
    // ABORT rejects only the second statement. The first flag must be
    // restored by the caller's transaction rather than by the trigger.
    sqlx::query(
        "CREATE TRIGGER reject_skipped_write BEFORE INSERT ON settings
         WHEN NEW.key = 'onboarding.skipped'
         BEGIN SELECT RAISE(ABORT, 'injected onboarding write failure'); END",
    )
    .execute(&pool)
    .await
    .expect("arm failure");
    pool.close().await;
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
async fn failed_mark_onboarding_complete_preserves_both_previous_flags() {
    for skipped in [false, true] {
        let tmp = TempDir::new().expect("tmp");
        let core = fresh_core(&tmp).await;
        seed_onboarding_and_reject_skipped_write(&tmp, false, !skipped).await;

        let error = core
            .mark_onboarding_complete(skipped)
            .await
            .expect_err("second onboarding write must fail");
        assert!(error
            .to_string()
            .contains("injected onboarding write failure"));
        let status = core.onboarding_status().await.expect("read after failure");
        assert_eq!(
            (status.complete, status.skipped),
            (false, !skipped),
            "failed completion with skipped={skipped} must preserve both onboarding keys"
        );
    }
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

#[tokio::test]
async fn failed_reset_onboarding_preserves_both_previous_flags() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    seed_onboarding_and_reject_skipped_write(&tmp, true, true).await;

    let error = core
        .reset_onboarding()
        .await
        .expect_err("second onboarding write must fail");
    assert!(error
        .to_string()
        .contains("injected onboarding write failure"));
    let status = core.onboarding_status().await.expect("read after failure");
    assert_eq!(
        (status.complete, status.skipped),
        (true, true),
        "failed reset must preserve both onboarding keys"
    );
}
