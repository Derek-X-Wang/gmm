//! ADR 0005 / #95 / #109 — proposing an Importer Origin change, and
//! what accepting and declining one mean.
//!
//! The manifest **proposes and never auto-applies**: when GMM would put
//! a game on a different Importer Origin than the one its install came
//! from, it says so and waits. Accepting installs from the proposed
//! origin through the ordinary backup-and-rollback path; declining
//! records a dismissal and nothing touches the game directory.
//!
//! The dismissal rules come from #95 and are all about *scope*:
//!
//! - A dismissal is remembered **by the origin it proposed** — not by
//!   the game, and not by origin plus version. Suppressing a whole game
//!   on one decline fails silently and severely: the user who dismisses
//!   once and whose importer dies months later is exactly the person
//!   this work exists for. Version scoping would re-prompt on every
//!   upstream release, and GIMI shipped two on one day.
//! - Origins compare **case-insensitively**, so a capitalisation fix in
//!   the manifest does not re-prompt everyone who declined.
//! - Dismissals are **visible and reversible on the affected game's own
//!   surface**, because dismissing is a one-click reflex and a silent
//!   permanent one has no trace.
//! - Turning recommendations off and back on **does not resurrect**
//!   dismissals as fresh prompts, or toggling the switch twice becomes a
//!   way to spam yourself.

use gmm_lib::core::importer_origin::{ImporterOrigin, InstalledOrigin, OverrideView};
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

fn the_fork() -> ImporterOrigin {
    ImporterOrigin::github("Curated", "GIMI-Fork", r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip")
}

fn a_third_origin() -> ImporterOrigin {
    ImporterOrigin::github(
        "someone",
        "GIMI-Rescue",
        r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip",
    )
}

fn manifest_recommending(origin: &ImporterOrigin, reason: Option<&str>) -> String {
    let reason = match reason {
        Some(r) => format!(",\n          \"reason\": {}", serde_json::json!(r)),
        None => String::new(),
    };
    format!(
        r#"{{
      "schemaVersion": 1,
      "games": {{
        "gimi": {{
          "status": "recommended",
          "owner": "{}",
          "repo": "{}",
          "assetPattern": "GIMI-PACKAGE-v\\d+\\.\\d+\\.\\d+\\.zip"{reason}
        }}
      }}
    }}"#,
        origin.owner(),
        origin.repo(),
    )
}

/// Cache `body` as the recommended-importers manifest.
///
/// The returned guard owns the stand-in host; the cache outlives it,
/// which is the point — resolution reads the cache and never the
/// network (#96).
async fn cache_manifest(core: &Core, body: &str) -> mockito::ServerGuard {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/recommended-importers.json")
        .with_status(200)
        .with_body(body)
        .create_async()
        .await;
    core.refresh_recommended_importers_from(&format!(
        "{}/recommended-importers.json",
        server.url()
    ))
    .await
    .expect("refresh");
    server
}

/// A game with an install GMM performed from one origin, and a cached
/// manifest recommending a different one.
async fn proposed_switch(core: &Core, tmp: &TempDir, reason: Option<&str>) -> mockito::ServerGuard {
    core.set_game_install_path(GameCode::Gimi, &tmp.path().join("Genshin"))
        .await
        .expect("install path");
    core.record_importer_install(GameCode::Gimi, "v8.8.0", &installed_from())
        .await
        .expect("seed install");
    cache_manifest(core, &manifest_recommending(&the_fork(), reason)).await
}

// ---------------------------------------------------------------------
// Proposing
// ---------------------------------------------------------------------

#[tokio::test]
async fn a_different_recommended_origin_is_offered_rather_than_applied() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let _m = proposed_switch(
        &core,
        &tmp,
        Some("The original package stopped receiving fixes."),
    )
    .await;

    let status = core
        .importer_origin_status(GameCode::Gimi)
        .await
        .expect("status");
    let proposal = status.proposal.expect("a switch must be offered");

    assert_eq!(proposal.origin, the_fork());
    assert_eq!(
        proposal.reason.as_deref(),
        Some("The original package stopped receiving fixes."),
        "the manifest's reason is what makes this a prompt a user can \
         evaluate rather than one they dismiss on reflex",
    );
    assert_eq!(
        proposal.replaces,
        InstalledOrigin::Known(installed_from()),
        "the prompt has to say plainly what it will replace",
    );
    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read origin"),
        InstalledOrigin::Known(installed_from()),
        "nothing is applied until the user accepts",
    );
}

#[tokio::test]
async fn nothing_is_proposed_when_the_install_already_matches() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.record_importer_install(GameCode::Gimi, "v8.8.0", &the_fork())
        .await
        .expect("seed install");
    let _m = cache_manifest(&core, &manifest_recommending(&the_fork(), None)).await;

    assert!(
        core.importer_origin_status(GameCode::Gimi)
            .await
            .expect("status")
            .proposal
            .is_none(),
        "re-proposing the origin the game is already on is pure noise",
    );
}

#[tokio::test]
async fn an_unknown_origin_is_never_announced_by_proposing_the_shipped_default() {
    // #99: unknown origin is a first-class state and is never surfaced
    // proactively. The compiled-in default is the status quo GMM has
    // always shipped, not something it has newly decided, so proposing
    // it would nag every hand-installed setup on every launch.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    assert!(core
        .importer_origin_status(GameCode::Gimi)
        .await
        .expect("status")
        .proposal
        .is_none(),);
}

// ---------------------------------------------------------------------
// Declining
// ---------------------------------------------------------------------

#[tokio::test]
async fn declining_stops_the_same_origin_being_proposed_again() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let _m = proposed_switch(&core, &tmp, None).await;

    core.dismiss_importer_origin(GameCode::Gimi, &the_fork())
        .await
        .expect("decline");

    let status = core
        .importer_origin_status(GameCode::Gimi)
        .await
        .expect("status");
    assert!(status.proposal.is_none(), "the same origin stays quiet");
    assert_eq!(
        status.dismissed,
        vec![the_fork()],
        "and the dismissal is visible on the game's own surface, because \
         dismissing is a one-click reflex",
    );
}

#[tokio::test]
async fn a_dismissal_is_scoped_to_the_origin_not_to_the_game() {
    // The rejected alternative — suppressing all recommendations for the
    // game on a single decline — fails silently and severely. A later
    // recommendation proposing a *different* origin must still reach the
    // user whose importer has since died.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let first = proposed_switch(&core, &tmp, None).await;
    core.dismiss_importer_origin(GameCode::Gimi, &the_fork())
        .await
        .expect("decline");
    drop(first);

    let _m = cache_manifest(&core, &manifest_recommending(&a_third_origin(), None)).await;

    let status = core
        .importer_origin_status(GameCode::Gimi)
        .await
        .expect("status");
    assert_eq!(
        status
            .proposal
            .expect("a different origin must prompt")
            .origin,
        a_third_origin(),
    );
}

#[tokio::test]
async fn a_dismissal_ignores_the_capitalisation_of_the_origin() {
    // GitHub treats owner/repo case-insensitively and ADR 0005 makes
    // origin equality case-insensitive with it. A capitalisation fix in
    // the manifest must not re-prompt everyone who declined.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let _m = proposed_switch(&core, &tmp, None).await;

    core.dismiss_importer_origin(
        GameCode::Gimi,
        &ImporterOrigin::github("CURATED", "GIMI-FORK", r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip"),
    )
    .await
    .expect("decline");

    assert!(
        core.importer_origin_status(GameCode::Gimi)
            .await
            .expect("status")
            .proposal
            .is_none(),
        "`Owner/Repo` and `owner/repo` are the same repository",
    );
}

#[tokio::test]
async fn a_dismissal_can_be_undone_from_the_games_own_surface() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let _m = proposed_switch(&core, &tmp, None).await;
    core.dismiss_importer_origin(GameCode::Gimi, &the_fork())
        .await
        .expect("decline");

    core.restore_importer_origin(GameCode::Gimi, &the_fork())
        .await
        .expect("undo the dismissal");

    let status = core
        .importer_origin_status(GameCode::Gimi)
        .await
        .expect("status");
    assert!(status.dismissed.is_empty());
    assert_eq!(
        status.proposal.expect("the proposal comes back").origin,
        the_fork(),
    );
}

#[tokio::test]
async fn declining_twice_records_one_dismissal() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let _m = proposed_switch(&core, &tmp, None).await;

    core.dismiss_importer_origin(GameCode::Gimi, &the_fork())
        .await
        .expect("decline");
    core.dismiss_importer_origin(GameCode::Gimi, &the_fork())
        .await
        .expect("decline again");

    assert_eq!(
        core.importer_origin_status(GameCode::Gimi)
            .await
            .expect("status")
            .dismissed,
        vec![the_fork()],
    );
}

// ---------------------------------------------------------------------
// The off switch is not a dismissal, and a dismissal is not an opt-out
// ---------------------------------------------------------------------

#[tokio::test]
async fn switching_recommendations_off_and_back_on_re_prompts_nothing_dismissed() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let _m = proposed_switch(&core, &tmp, None).await;
    core.dismiss_importer_origin(GameCode::Gimi, &the_fork())
        .await
        .expect("decline");

    core.set_importer_recommendations_enabled(false)
        .await
        .expect("off");
    core.set_importer_recommendations_enabled(true)
        .await
        .expect("on");

    assert!(
        core.importer_origin_status(GameCode::Gimi)
            .await
            .expect("status")
            .proposal
            .is_none(),
        "toggling the switch twice must not become a way to spam yourself",
    );
}

#[tokio::test]
async fn declining_never_escalates_into_opting_out() {
    // Declining is a judgement about one proposal; opting out is a
    // standing preference. They are deliberately different acts.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let _m = proposed_switch(&core, &tmp, None).await;

    core.dismiss_importer_origin(GameCode::Gimi, &the_fork())
        .await
        .expect("decline");

    assert!(
        core.importer_recommendations_enabled()
            .await
            .expect("read the switch"),
        "one decline must not switch the whole layer off",
    );
}

#[tokio::test]
async fn nothing_is_proposed_and_no_dismissals_are_offered_while_recommendations_are_off() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let _m = proposed_switch(&core, &tmp, None).await;
    core.dismiss_importer_origin(GameCode::Gimi, &a_third_origin())
        .await
        .expect("decline something else");

    core.set_importer_recommendations_enabled(false)
        .await
        .expect("off");

    let status = core
        .importer_origin_status(GameCode::Gimi)
        .await
        .expect("status");
    assert!(status.proposal.is_none(), "no prompts");
    assert!(
        status.dismissed.is_empty(),
        "and no dismissed-recommendation affordance either — the whole layer \
         is gone, not just its fetch",
    );
    assert!(!status.recommendations_enabled);
}

// ---------------------------------------------------------------------
// Accepting
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

async fn fake_upstream(origin: &ImporterOrigin, tag: &str, zip: &Path) -> mockito::ServerGuard {
    let mut server = mockito::Server::new_async().await;
    let asset = format!("GIMI-PACKAGE-{tag}.zip");
    let body = serde_json::json!({
        "tag_name": tag,
        "assets": [{
            "name": asset,
            "browser_download_url": format!("{}/download/{asset}", server.url()),
        }],
    })
    .to_string();
    server
        .mock(
            "GET",
            format!("/repos/{}/releases/latest", origin.repo_slug()).as_str(),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .create_async()
        .await;
    server
        .mock("GET", format!("/download/{asset}").as_str())
        .with_status(200)
        .with_body(std::fs::read(zip).expect("read zip"))
        .create_async()
        .await;
    server
}

#[tokio::test]
async fn accepting_installs_from_the_proposed_origin_and_records_it() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let _m = proposed_switch(&core, &tmp, None).await;
    core.set_importer_pinned(GameCode::Gimi, Some("v8.8.0"))
        .await
        .expect("pin");

    // Real files from the previous origin, so "accepting rewrites the
    // game directory" is something the assertions can actually observe.
    let game_dir = tmp.path().join("Genshin");
    std::fs::create_dir_all(game_dir.join("Core")).expect("game dir");
    std::fs::write(game_dir.join("d3dx.ini"), b"[Loader]\nloader = old.exe\n")
        .expect("previous importer");

    let zip = tmp.path().join("pkg.zip");
    build_importer_zip(&zip);
    let upstream = fake_upstream(&the_fork(), "v1.4.4", &zip).await;

    let report = core
        .accept_importer_origin_proposal_with_endpoints(
            GameCode::Gimi,
            &gmm_lib::core::importer::Endpoints {
                api_base: upstream.url(),
            },
        )
        .await
        .expect("accept");

    assert!(
        report.backup_dir.is_some(),
        "accepting rewrites the game directory, so it goes through the ordinary \
         backup-and-rollback path",
    );
    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read origin"),
        InstalledOrigin::Known(the_fork()),
    );
    assert_eq!(
        core.installed_importer_version(GameCode::Gimi)
            .await
            .expect("read version"),
        Some("v1.4.4".to_string()),
    );
    assert_eq!(
        core.importer_pinned(GameCode::Gimi).await.expect("pin"),
        None,
        "a version string taken against one origin means nothing against \
         another, so the Importer Pin goes with the switch (#110)",
    );
    assert!(
        core.importer_origin_status(GameCode::Gimi)
            .await
            .expect("status")
            .proposal
            .is_none(),
        "the proposal is answered",
    );
}

#[tokio::test]
async fn there_is_no_way_to_record_an_origin_without_installing() {
    // Explicitly rejected in #109: it records an origin and a version
    // for files GMM has never seen, and everything downstream then
    // trusts the fiction. A user who wants their existing files left
    // alone declines.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let _m = proposed_switch(&core, &tmp, None).await;

    // Nothing offers it, so accepting with no reachable upstream fails
    // rather than quietly booking the origin.
    let error = core
        .accept_importer_origin_proposal_with_endpoints(
            GameCode::Gimi,
            &gmm_lib::core::importer::Endpoints {
                api_base: "http://127.0.0.1:1/unreachable".to_string(),
            },
        )
        .await
        .expect_err("an install that cannot run is not an accepted proposal");

    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read origin"),
        InstalledOrigin::Known(installed_from()),
        "the recorded origin still describes the files on disk. Error was: {error}",
    );
}

#[tokio::test]
async fn accepting_when_nothing_is_proposed_is_an_error_not_a_silent_install() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.set_game_install_path(GameCode::Gimi, &tmp.path().join("Genshin"))
        .await
        .expect("install path");
    core.record_importer_install(GameCode::Gimi, "v8.8.0", &installed_from())
        .await
        .expect("seed install");

    let error = core
        .accept_importer_origin_proposal(GameCode::Gimi)
        .await
        .expect_err("there is nothing to accept");
    assert!(
        error.to_string().contains("Genshin Impact"),
        "the message must name the game: {error}",
    );
}

// ---------------------------------------------------------------------
// The override, and the state where no origin is in effect
// ---------------------------------------------------------------------

#[tokio::test]
async fn the_user_can_set_an_override_and_clear_it_back_to_the_recommendation() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let _m = proposed_switch(&core, &tmp, None).await;

    let mine = ImporterOrigin::github("me", "my-GIMI", r"GIMI-PACKAGE-v\d+\.zip");
    core.set_importer_origin_override(GameCode::Gimi, Some(&mine))
        .await
        .expect("set");

    let status = core
        .importer_origin_status(GameCode::Gimi)
        .await
        .expect("status");
    assert_eq!(status.user_override, OverrideView::Set(mine.clone()));
    assert_eq!(status.resolved.origin(), Some(&mine));

    core.set_importer_origin_override(GameCode::Gimi, None)
        .await
        .expect("clear");

    let status = core
        .importer_origin_status(GameCode::Gimi)
        .await
        .expect("status");
    assert_eq!(status.user_override, OverrideView::NotSet);
    assert_eq!(
        status.resolved.origin(),
        Some(&the_fork()),
        "clearing returns the game to following the recommendation",
    );
}

#[tokio::test]
async fn a_game_with_no_origin_in_effect_is_warned_about_and_not_blocked() {
    use gmm_lib::core::importer_origin::OriginResolution;

    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let _m = cache_manifest(
        &core,
        r#"{
          "schemaVersion": 1,
          "games": {
            "gimi": {
              "status": "none",
              "reason": "No maintained package is known right now."
            }
          }
        }"#,
    )
    .await;

    let status = core
        .importer_origin_status(GameCode::Gimi)
        .await
        .expect("status");

    match &status.resolved {
        OriginResolution::NoneInEffect { reason } => assert_eq!(
            reason.as_deref(),
            Some("No maintained package is known right now."),
        ),
        other => panic!("expected no origin in effect, got {other:?}"),
    }
    assert!(
        status.proposal.is_none(),
        "there is nothing to move onto; the no-origin warning is the surface \
         for this state, not a prompt",
    );
    // Still fully usable: the override control is right there, and
    // setting one rescues the game.
    let mine = ImporterOrigin::github("me", "my-HIMI", r"PACKAGE-v\d+\.zip");
    core.set_importer_origin_override(GameCode::Gimi, Some(&mine))
        .await
        .expect("the user is never blocked from supplying an origin");
    assert_eq!(
        core.importer_origin_status(GameCode::Gimi)
            .await
            .expect("status")
            .resolved
            .origin(),
        Some(&mine),
    );
}

// ---------------------------------------------------------------------
// The read that must not collapse into a benign value
// ---------------------------------------------------------------------

#[tokio::test]
async fn dismissal_state_that_cannot_be_read_is_surfaced_and_does_not_silence_the_prompt() {
    // This codebase's recurring defect is an error rendered as a benign
    // result — shipped three times (#78, #114, #122). A dismissal list
    // that will not parse has an especially innocent-looking wrong
    // answer: "nothing was declined", which reads as normal and would
    // instead have to be "everything was declined" to be safe. Neither
    // guess is reportable on its own.
    //
    // The proposal is shown, because a proposal applies nothing by
    // itself: the cost of showing one the user already answered is a
    // click, and the cost of hiding one they have not is a user stranded
    // on a dead importer with the fix silenced by a corrupt row. The
    // read failure travels with it either way.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let _m = proposed_switch(&core, &tmp, None).await;

    let pool = sqlx::SqlitePool::connect(&db_url(&tmp))
        .await
        .expect("open db");
    sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?)")
        .bind("importer.gimi.declined_origins")
        .bind("not json at all")
        .execute(&pool)
        .await
        .expect("write corrupt dismissals");
    pool.close().await;

    let status = core
        .importer_origin_status(GameCode::Gimi)
        .await
        .expect("status");

    assert!(
        status.dismissals_error.is_some(),
        "a read failure has to reach the user rather than being answered with \
         an empty list",
    );
    assert!(
        status.dismissed.is_empty(),
        "and GMM must not invent dismissals it cannot read",
    );
    assert!(
        status.proposal.is_some(),
        "the proposal is still offered: a corrupt row must not become a \
         permanent, invisible silence over the fix for a broken importer",
    );
}
