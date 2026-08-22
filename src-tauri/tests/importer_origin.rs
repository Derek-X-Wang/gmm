//! ADR 0005 / #107 — Importer Origin as a first-class per-game value.
//!
//! Three layers of precedence (user override → recommended manifest →
//! compiled-in default), a per-install origin record where absent means
//! **unknown**, and a "no origin in effect" state that is none of the
//! others.

use gmm_lib::core::importer_origin::ImporterOrigin;

#[test]
fn two_origins_differing_only_in_case_are_the_same_origin() {
    // GitHub treats owner/repo case-insensitively. Origin equality is
    // load-bearing for the decline key, the pin-clearing trigger and
    // install bookkeeping (ADR 0005), so a capitalisation fix in the
    // manifest must not read as a different origin and re-prompt
    // everyone who already declined.
    let a = ImporterOrigin::github(
        "SilentNightSound",
        "GIMI-Package",
        r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip",
    );
    let b = ImporterOrigin::github(
        "silentnightsound",
        "gimi-package",
        r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip",
    );

    assert_eq!(a, b, "owner and repo compare case-insensitively");
}

#[test]
fn origins_pointing_at_different_repos_are_different_origins() {
    let a = ImporterOrigin::github("leotorrez", "ZZMI-Package", r"ZZMI-PACKAGE-v\d+\.zip");
    let b = ImporterOrigin::github("leotorrez", "HIMI-Package", r"HIMI-PACKAGE-v\d+\.zip");
    assert_ne!(a, b);
}

#[test]
fn the_asset_pattern_is_part_of_origin_identity_and_stays_case_sensitive() {
    // The pattern is a regex, not a GitHub identifier: `PACKAGE` and
    // `package` select different files, so the case-folding that makes
    // owner/repo equal must not reach it. Two origins on the same repo
    // that select different assets install different files and are
    // therefore not the same origin.
    let a = ImporterOrigin::github("SpectrumQT", "WWMI-Package", r"WWMI-PACKAGE-v\d+\.zip");
    let b = ImporterOrigin::github("SpectrumQT", "WWMI-Package", r"wwmi-package-v\d+\.zip");
    assert_ne!(a, b, "the asset pattern is compared exactly");
}

#[test]
fn an_origin_preserves_the_spelling_it_was_given_for_display_and_fetching() {
    // Equality folds case; the value itself must not. GMM shows the
    // origin to the user and puts it in a URL, so it keeps what the
    // manifest or the user actually wrote.
    let origin = ImporterOrigin::github("SilentNightSound", "GIMI-Package", r"GIMI-.*\.zip");
    assert_eq!(origin.owner(), "SilentNightSound");
    assert_eq!(origin.repo(), "GIMI-Package");
    assert_eq!(origin.repo_slug(), "SilentNightSound/GIMI-Package");
}

// ---------------------------------------------------------------
// Three-layer precedence (ADR 0005): user override → recommended
// manifest → compiled-in default.
// ---------------------------------------------------------------

use gmm_lib::core::importer_origin::{
    resolve, OriginLayer, OriginResolution, Recommendation, StoredOverride,
};

fn gimi_default() -> ImporterOrigin {
    ImporterOrigin::github(
        "SilentNightSound",
        "GIMI-Package",
        r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip",
    )
}

fn a_users_own_origin() -> ImporterOrigin {
    ImporterOrigin::github("someone", "GIMI-Fork", r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip")
}

fn a_recommended_origin() -> ImporterOrigin {
    ImporterOrigin::github(
        "curated",
        "GIMI-Package",
        r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip",
    )
}

#[test]
fn with_no_override_and_no_manifest_a_game_resolves_to_its_compiled_in_default() {
    // The layer-2 input is absent here, which is the state until #108
    // lands the fetch. Installs must behave exactly as they do today.
    let resolved = resolve(&StoredOverride::NotSet, None, Some(&gimi_default()));

    match resolved {
        OriginResolution::InEffect { origin, layer } => {
            assert_eq!(origin, gimi_default());
            assert_eq!(layer, OriginLayer::CompiledInDefault);
        }
        other => panic!("expected the compiled-in default to be in effect, got {other:?}"),
    }
}

#[test]
fn a_user_override_outranks_both_the_manifest_and_the_default() {
    let recommended = Recommendation::Recommended {
        origin: a_recommended_origin(),
        reason: None,
    };
    let resolved = resolve(
        &StoredOverride::Set(a_users_own_origin()),
        Some(&recommended),
        Some(&gimi_default()),
    );

    match resolved {
        OriginResolution::InEffect { origin, layer } => {
            assert_eq!(origin, a_users_own_origin());
            assert_eq!(layer, OriginLayer::UserOverride);
        }
        other => panic!("the user's own choice always wins, got {other:?}"),
    }
}

#[test]
fn a_recommendation_outranks_the_compiled_in_default() {
    let recommended = Recommendation::Recommended {
        origin: a_recommended_origin(),
        reason: None,
    };
    let resolved = resolve(
        &StoredOverride::NotSet,
        Some(&recommended),
        Some(&gimi_default()),
    );

    match resolved {
        OriginResolution::InEffect { origin, layer } => {
            assert_eq!(origin, a_recommended_origin());
            assert_eq!(layer, OriginLayer::RecommendedManifest);
        }
        other => panic!("expected the recommendation in effect, got {other:?}"),
    }
}

#[test]
fn a_none_recommendation_retracts_the_compiled_in_default() {
    // The rule most easily got wrong. GMM publishes no-recommendation
    // precisely *because* the compiled-in default went bad, so falling
    // through would leave GMM quietly recommending the exact thing it
    // just declined to recommend (#97). Retraction is the honest read.
    let retracted = Recommendation::NoRecommendation {
        reason: Some("No maintained package known right now.".to_string()),
    };
    let resolved = resolve(
        &StoredOverride::NotSet,
        Some(&retracted),
        Some(&gimi_default()),
    );

    match resolved {
        OriginResolution::NoneInEffect { reason } => {
            assert_eq!(
                reason.as_deref(),
                Some("No maintained package known right now."),
                "the reason must reach the user, it is why they are being asked to act",
            );
        }
        other => panic!("a `none` entry must retract, not fall through, got {other:?}"),
    }
}

#[test]
fn a_user_override_rescues_a_game_whose_default_was_retracted() {
    // The escape hatch that makes retraction acceptable: the user is
    // never stuck, because their own choice sits above the manifest.
    let retracted = Recommendation::NoRecommendation { reason: None };
    let resolved = resolve(
        &StoredOverride::Set(a_users_own_origin()),
        Some(&retracted),
        Some(&gimi_default()),
    );

    match resolved {
        OriginResolution::InEffect { origin, layer } => {
            assert_eq!(origin, a_users_own_origin());
            assert_eq!(layer, OriginLayer::UserOverride);
        }
        other => panic!("a user override must survive a retraction, got {other:?}"),
    }
}

#[test]
fn a_game_absent_from_the_manifest_falls_through_to_the_default() {
    // Absent is *not* retraction. This is the distinction #93 called
    // the dangerous one: if absence were read as retraction, one bad
    // commit would silently strip every user's default.
    let resolved = resolve(&StoredOverride::NotSet, None, Some(&gimi_default()));
    assert!(matches!(
        resolved,
        OriginResolution::InEffect {
            layer: OriginLayer::CompiledInDefault,
            ..
        }
    ));
}

#[test]
fn retraction_and_absence_are_different_outcomes_for_the_same_game() {
    let absent = resolve(&StoredOverride::NotSet, None, Some(&gimi_default()));
    let retracted = resolve(
        &StoredOverride::NotSet,
        Some(&Recommendation::NoRecommendation { reason: None }),
        Some(&gimi_default()),
    );

    assert!(matches!(absent, OriginResolution::InEffect { .. }));
    assert!(matches!(retracted, OriginResolution::NoneInEffect { .. }));
    assert_ne!(
        std::mem::discriminant(&absent),
        std::mem::discriminant(&retracted),
    );
}

#[test]
fn an_unported_game_with_no_default_has_no_origin_in_effect() {
    // Not an error and not a panic — the same warn-never-block state a
    // retraction produces.
    let resolved = resolve(&StoredOverride::NotSet, None, None);
    assert!(matches!(
        resolved,
        OriginResolution::NoneInEffect { reason: None }
    ));
}

// ---------------------------------------------------------------
// Persistence: the per-game override, and the per-install record
// where absent means unknown.
// ---------------------------------------------------------------

use gmm_lib::core::importer_origin::InstalledOrigin;
use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init")
}

/// Reopen the same database, as an app restart would.
async fn reopen(tmp: &TempDir) -> Core {
    fresh_core(tmp).await
}

#[tokio::test]
async fn a_per_game_override_survives_a_restart_and_changes_what_resolves() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    // Before: the compiled-in default.
    let before = core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve");
    assert_eq!(
        before.origin().expect("a default is in effect").repo_slug(),
        "SilentNightSound/GIMI-Package",
    );

    let mine = ImporterOrigin::github("someone", "GIMI-Fork", r"GIMI-PACKAGE-v\d+\.zip");
    core.set_importer_origin_override(GameCode::Gimi, Some(&mine))
        .await
        .expect("set override");

    drop(core);
    let core = reopen(&tmp).await;

    let after = core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve");
    match after {
        OriginResolution::InEffect { origin, layer } => {
            assert_eq!(origin, mine);
            assert_eq!(layer, OriginLayer::UserOverride);
        }
        other => panic!("the override must survive a restart, got {other:?}"),
    }
}

#[tokio::test]
async fn clearing_the_override_returns_the_game_to_the_compiled_in_default() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let mine = ImporterOrigin::github("someone", "GIMI-Fork", r"GIMI-PACKAGE-v\d+\.zip");
    core.set_importer_origin_override(GameCode::Gimi, Some(&mine))
        .await
        .expect("set");
    core.set_importer_origin_override(GameCode::Gimi, None)
        .await
        .expect("clear");

    let resolved = core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve");
    match resolved {
        OriginResolution::InEffect { origin, layer } => {
            assert_eq!(origin.repo_slug(), "SilentNightSound/GIMI-Package");
            assert_eq!(
                layer,
                OriginLayer::CompiledInDefault,
                "clearing returns the game to following layers 2 and 3",
            );
        }
        other => panic!("expected the default back, got {other:?}"),
    }
}

#[tokio::test]
async fn an_override_is_scoped_to_one_game() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let mine = ImporterOrigin::github("someone", "GIMI-Fork", r"GIMI-PACKAGE-v\d+\.zip");
    core.set_importer_origin_override(GameCode::Gimi, Some(&mine))
        .await
        .expect("set");

    let srmi = core
        .resolve_importer_origin(GameCode::Srmi)
        .await
        .expect("resolve");
    assert_eq!(
        srmi.origin().expect("srmi default").repo_slug(),
        "SpectrumQT/SRMI-Package",
        "overriding Genshin must not move Star Rail",
    );
}

#[tokio::test]
async fn an_install_predating_origin_tracking_reads_back_as_unknown() {
    // Every existing user is in this state: `importer.installed.<game>`
    // was written by an install that recorded no origin, or the
    // importer was hand-installed outside GMM entirely. #99: unknown is
    // a first-class value, never backfilled to the compiled-in default,
    // because for three of six games that would record a provable
    // fiction.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    // Seed the pre-existing shape: a version, no origin.
    core.set_importer_installed(GameCode::Gimi, "v8.8.0")
        .await
        .expect("seed version");

    let recorded = core
        .installed_importer_origin(GameCode::Gimi)
        .await
        .expect("read");
    assert_eq!(recorded, InstalledOrigin::Unknown);
}

#[tokio::test]
async fn an_unknown_origin_install_is_not_reported_as_not_installed() {
    // #99 rejected treating unknown as "not installed". These users
    // sorted themselves out when GMM could not help them; GMM's
    // bookkeeping does not get to invalidate a working setup.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    core.set_importer_installed(GameCode::Gimi, "v8.8.0")
        .await
        .expect("seed");

    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read"),
        InstalledOrigin::Unknown,
    );
    assert_eq!(
        core.installed_importer_version(GameCode::Gimi)
            .await
            .expect("version"),
        Some("v8.8.0".to_string()),
        "the recorded version is untouched — unknown origin is not absence of an install",
    );
}

#[tokio::test]
async fn recording_an_install_makes_the_origin_known() {
    // Unknown becomes known only through an actual install (#99).
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let origin = ImporterOrigin::github(
        "SilentNightSound",
        "GIMI-Package",
        r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip",
    );
    core.record_importer_install(GameCode::Gimi, "v8.8.9", &origin)
        .await
        .expect("record");

    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read"),
        InstalledOrigin::Known(origin),
    );
}

#[tokio::test]
async fn no_origin_in_effect_no_override_set_and_unknown_origin_are_three_distinct_things() {
    // The acceptance criterion that keeps ADR 0005 honest: these must
    // not be representable by the same value anywhere.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    // (1) no override set — a distinct state of the stored value, and
    // since #124 distinct from a stored value GMM cannot read
    let no_override = core
        .importer_origin_override(GameCode::Gimi)
        .await
        .expect("read override");
    assert_eq!(no_override, StoredOverride::NotSet);

    // (2) unknown origin on an install — its own type
    let unknown = core
        .installed_importer_origin(GameCode::Gimi)
        .await
        .expect("read install");
    assert_eq!(unknown, InstalledOrigin::Unknown);

    // (3) no origin in effect — a resolution outcome, not an Option
    let none_in_effect = resolve(
        &StoredOverride::NotSet,
        Some(&Recommendation::NoRecommendation { reason: None }),
        Some(&gimi_default()),
    );
    assert!(matches!(
        none_in_effect,
        OriginResolution::NoneInEffect { .. }
    ));

    // And (1) does not produce (3): with no override set the game still
    // resolves to its default.
    let resolved = core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve");
    assert!(
        matches!(resolved, OriginResolution::InEffect { .. }),
        "no override set must never be read as no origin in effect",
    );
}

// ---------------------------------------------------------------
// The update check and install run against the *resolved* origin,
// and "no origin in effect" warns without blocking or panicking.
// ---------------------------------------------------------------

#[tokio::test]
async fn the_update_check_runs_against_the_overridden_repo() {
    // Both origins below are unreachable, which is the point: the
    // check must report the failure naming the repo the *override*
    // selected, proving the override reached the network call rather
    // than the compiled-in default.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let mine = ImporterOrigin::github(
        "gmm-test-nonexistent-owner",
        "GIMI-Fork",
        r"GIMI-PACKAGE-v\d+\.zip",
    );
    core.set_importer_origin_override(GameCode::Gimi, Some(&mine))
        .await
        .expect("set override");

    let status = core
        .check_importer_update_for(GameCode::Gimi)
        .await
        .expect("the check must not fail hard");

    let error = status
        .check_error
        .expect("an unreachable origin must surface as a check error, never as up-to-date");
    assert!(
        error.contains("gmm-test-nonexistent-owner"),
        "the failure must name the origin actually used, got {error:?}",
    );
    assert!(
        !error.contains("SilentNightSound"),
        "the compiled-in default must not have been used, got {error:?}",
    );
    assert!(!status.available);
}

#[tokio::test]
async fn no_origin_in_effect_warns_without_blocking_or_panicking() {
    // #97: GMM warns and never blocks. The warning has to arrive as a
    // check error rather than as silence, because #78's whole lesson is
    // that "could not find out" must never be collapsed into "you are
    // up to date".
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let retracted = OriginResolution::NoneInEffect {
        reason: Some("No maintained package known right now.".to_string()),
    };

    let status = core
        .check_importer_update_with(GameCode::Himi, &retracted)
        .await
        .expect("must not hard-fail");

    let warning = status
        .check_error
        .expect("no origin in effect must be surfaced, not swallowed");
    assert!(
        warning.contains("No maintained package known right now."),
        "the manifest's reason must reach the user, got {warning:?}",
    );
    assert!(
        !status.available && !status.upstream_ahead,
        "nothing to offer, but nothing claimed either",
    );
    assert_eq!(
        status.latest_version, None,
        "no origin means no upstream version was learned",
    );
}

#[tokio::test]
async fn a_game_with_no_origin_in_effect_is_still_a_game_the_user_can_act_on() {
    // Not hidden, not "not installed", no panic. A user who already has
    // an importer keeps their recorded version — a retraction is not a
    // statement that their install stopped working (#97).
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    core.set_importer_installed(GameCode::Himi, "v1.0.2")
        .await
        .expect("seed an existing install");

    let status = core
        .check_importer_update_with(
            GameCode::Himi,
            &OriginResolution::NoneInEffect { reason: None },
        )
        .await
        .expect("must not hard-fail");

    assert_eq!(
        status.installed_version,
        Some("v1.0.2".to_string()),
        "the user's existing install is untouched by a retraction",
    );
}

#[tokio::test]
async fn the_resolved_origin_is_what_an_install_would_record() {
    // Ties the resolution to the bookkeeping: whatever resolves is what
    // gets recorded, so the pin-clearing comparison in #110 has a real
    // value to compare against.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let mine = ImporterOrigin::github("someone", "GIMI-Fork", r"GIMI-PACKAGE-v\d+\.zip");
    core.set_importer_origin_override(GameCode::Gimi, Some(&mine))
        .await
        .expect("set");

    let resolved = core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve");
    let origin = resolved.origin().expect("in effect").clone();
    core.record_importer_install(GameCode::Gimi, "v1.2.3", &origin)
        .await
        .expect("record");

    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read"),
        InstalledOrigin::Known(mine),
    );
    assert_eq!(
        core.installed_importer_version(GameCode::Gimi)
            .await
            .expect("read"),
        Some("v1.2.3".to_string()),
    );
}

#[test]
fn every_shipped_game_has_a_compiled_in_default_origin_matching_its_profile() {
    // Layer 3 must be exactly the profile table, not a second copy of
    // it that can drift.
    use gmm_lib::core::games::GAME_PROFILES;
    use gmm_lib::core::importer_origin::compiled_in_default;

    for profile in GAME_PROFILES {
        let (repo_slug, pattern) = profile.importer_repo.expect("every shipped game is ported");
        let origin = compiled_in_default(profile.code)
            .unwrap_or_else(|| panic!("{} has no compiled-in origin", profile.code.as_str()));
        assert_eq!(origin.repo_slug(), repo_slug);
        assert_eq!(origin.asset_pattern(), pattern);
    }
}

#[test]
fn nothing_in_the_ui_announces_that_an_install_has_an_unknown_origin() {
    // #99 is explicit that unknown origin is never surfaced
    // proactively: it is noise the user cannot act on, and it would
    // fire for every hand-installed setup on every launch. Asserted
    // against the real frontend sources so a later slice cannot add a
    // badge for it without deleting this test on purpose.
    let frontend = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .join("src");

    let mut offenders = Vec::new();
    let mut stack = vec![frontend];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read frontend dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let is_source = matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("ts") | Some("tsx")
            );
            if !is_source {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read source");
            for needle in ["installedOrigin", "unknownOrigin", "unknown origin"] {
                if text.contains(needle) {
                    offenders.push(format!("{} mentions {needle:?}", path.display()));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "unknown Importer Origin must not be surfaced proactively: {offenders:?}",
    );
}
