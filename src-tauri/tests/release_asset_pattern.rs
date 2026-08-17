//! Issue #79: release-asset selection is an anchored pattern per
//! Importer Origin, and exactly one asset must match.
//!
//! Every fixture under `tests/fixtures/github/` is a **verbatim
//! recording of the real GitHub API response** —
//! `gh api repos/<owner>/<repo>/releases/latest`, taken 2026-08-17.
//! They are deliberately not hand-written: the bare `str::contains`
//! filter this replaces drifted precisely because nothing ever compared
//! it to what upstream actually publishes (#78 matched nothing for the
//! whole life of the feature, #79 matched a TEST build for SRMI).

use gmm_lib::core::error::Error;
use gmm_lib::core::games::GAME_PROFILES;
use gmm_lib::core::importer::{self, AssetPattern};
use gmm_lib::core::updates::{LOADER_ASSET_PATTERN, LOADER_REPO};

fn recorded(name: &str) -> serde_json::Value {
    let raw = match name {
        "gimi" => include_str!("fixtures/github/gimi-package-latest.json"),
        "srmi" => include_str!("fixtures/github/srmi-package-latest.json"),
        "zzmi" => include_str!("fixtures/github/zzmi-package-latest.json"),
        "wwmi" => include_str!("fixtures/github/wwmi-package-latest.json"),
        "himi" => include_str!("fixtures/github/himi-package-latest.json"),
        "efmi" => include_str!("fixtures/github/efmi-package-latest.json"),
        "loader" => include_str!("fixtures/github/xxmi-libs-package-latest.json"),
        other => panic!("no recorded release fixture for {other}"),
    };
    serde_json::from_str(raw).expect("recorded fixture is valid JSON")
}

#[test]
fn an_anchored_pattern_selects_the_real_gimi_package_asset() {
    let pattern = AssetPattern::new(r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip").expect("valid pattern");

    let release = importer::parse_latest_release(&recorded("gimi"), &pattern)
        .expect("the pattern GMM ships must select GIMI's real release asset");

    assert_eq!(release.tag_name, "v8.8.9");
    assert_eq!(release.asset_name, "GIMI-PACKAGE-v8.8.9.zip");
}

/// The compiled-in pattern for a game, straight out of the registry —
/// so these tests exercise what GMM ships, not a pattern written for the
/// test's convenience.
fn shipped(game: &str) -> (&'static str, AssetPattern) {
    let profile = GAME_PROFILES
        .iter()
        .find(|p| p.code.as_str() == game)
        .unwrap_or_else(|| panic!("no profile for {game}"));
    let (repo, pattern) = profile
        .importer_repo
        .unwrap_or_else(|| panic!("{game} has no Importer Origin"));
    (
        repo,
        AssetPattern::new(pattern)
            .unwrap_or_else(|e| panic!("{game} pattern does not compile: {e}")),
    )
}

#[test]
fn every_shipped_asset_pattern_compiles() {
    // A compiled-in pattern that does not compile would be a build
    // defect that only shows up when a user clicks Install.
    for profile in GAME_PROFILES {
        let (_, pattern) = profile
            .importer_repo
            .unwrap_or_else(|| panic!("{} has no Importer Origin", profile.code.as_str()));
        AssetPattern::new(pattern).unwrap_or_else(|e| {
            panic!(
                "{} ships an uncompilable asset pattern: {e}",
                profile.code.as_str()
            )
        });
    }
    AssetPattern::new(LOADER_ASSET_PATTERN).expect("the Loader pattern must compile");
}

#[test]
fn every_conventionally_named_importer_origin_still_resolves() {
    // The install and update flows must keep working for every game
    // whose upstream naming is conventional. SRMI is the deliberate
    // exception and has its own test below.
    let expected = [
        ("gimi", "v8.8.9", "GIMI-PACKAGE-v8.8.9.zip"),
        ("zzmi", "v1.4.5", "ZZMI-PACKAGE-v1.4.5.zip"),
        ("wwmi", "v1.0.0", "WWMI-PACKAGE-v1.0.0.zip"),
        ("himi", "v1.0.2", "HIMI-PACKAGE-v1.0.2.zip"),
        ("efmi", "v1.3.0", "EFMI-PACKAGE-v1.3.0.zip"),
    ];

    for (game, tag, asset) in expected {
        let (_, pattern) = shipped(game);
        let release = importer::parse_latest_release(&recorded(game), &pattern)
            .unwrap_or_else(|e| panic!("{game} no longer selects an asset: {e}"));
        assert_eq!(release.tag_name, tag, "{game} tag");
        assert_eq!(release.asset_name, asset, "{game} asset");
        assert!(
            release.asset_url.contains(asset),
            "{game} download URL must point at the selected asset, got {:?}",
            release.asset_url
        );
    }
}

#[test]
fn srmi_refuses_the_test_package_rather_than_installing_it() {
    // `SpectrumQT/SRMI-Package`'s only asset today is
    // `SRMI-TEST-PACKAGE-v2.4.2.zip`. The bare substring `"SRMI"`
    // matched it, so GMM would have installed a build upstream
    // explicitly labelled TEST and presented it as the current importer
    // for Honkai: Star Rail. Refusing is the recorded decision on #79 —
    // this test exists so nobody "fixes" SRMI by widening the pattern
    // without making that call deliberately.
    let (repo, pattern) = shipped("srmi");
    assert_eq!(repo, "SpectrumQT/SRMI-Package");

    let error = importer::parse_latest_release(&recorded("srmi"), &pattern)
        .expect_err("a TEST package must not be selected");

    match error {
        Error::ReleaseAssetNoMatch {
            release,
            pattern,
            candidates,
        } => {
            assert_eq!(release, "v2.4.2");
            assert!(
                pattern.contains("SRMI-PACKAGE"),
                "the error must name the pattern that missed, got {pattern:?}"
            );
            assert!(
                candidates.contains("SRMI-TEST-PACKAGE-v2.4.2.zip"),
                "the error must name what upstream published so the user can \
                 tell a rename from a wrong pattern, got {candidates:?}"
            );
        }
        other => panic!("expected a no-match error naming the pattern, got {other:?}"),
    }
}

#[test]
fn the_old_substring_filter_would_have_installed_srmis_test_package() {
    // The bug, pinned. `"SRMI"` is a substring of
    // `SRMI-TEST-PACKAGE-v2.4.2.zip`; the anchored pattern is not.
    assert!(
        "SRMI-TEST-PACKAGE-v2.4.2.zip".contains("SRMI"),
        "this is what the shipped filter did"
    );
    let (_, pattern) = shipped("srmi");
    assert!(!pattern.matches("SRMI-TEST-PACKAGE-v2.4.2.zip"));
    assert!(
        pattern.matches("SRMI-PACKAGE-v2.4.1.zip"),
        "a conventionally named SRMI package must still be accepted the day \
         upstream publishes one"
    );
}

#[test]
fn the_loader_release_selects_its_package_and_not_its_manifest() {
    // The Loader repo publishes `Manifest.json` beside the zip, which is
    // the whole reason selection exists. `"Libs"` matched neither, and
    // `.ok().flatten()` reported that as "up to date" for the entire
    // life of the feature (#78).
    assert_eq!(LOADER_REPO, "SpectrumQT/XXMI-Libs-Package");
    let pattern = AssetPattern::new(LOADER_ASSET_PATTERN).expect("valid pattern");
    let json = recorded("loader");

    let names: Vec<&str> = json["assets"]
        .as_array()
        .expect("assets array")
        .iter()
        .map(|a| a["name"].as_str().expect("asset name"))
        .collect();
    assert!(
        names.contains(&"Manifest.json"),
        "the recording must still contain the sibling asset that makes this \
         a selection problem, got {names:?}"
    );

    let release = importer::parse_latest_release(&json, &pattern).expect("select the Loader zip");
    assert_eq!(release.asset_name, "XXMI-PACKAGE-v1.0.2.zip");
    assert_eq!(release.tag_name, "v1.0.2");
}

#[test]
fn a_pattern_matching_two_assets_is_ambiguous_rather_than_first_wins() {
    // Two matches means the pattern is wrong or upstream published
    // something unexpected. Picking by release order is how a TEST
    // package gets chosen over a real one when both exist.
    let loose = AssetPattern::new(r".*SRMI.*\.zip").expect("valid pattern");
    let json = serde_json::json!({
        "tag_name": "v2.4.2",
        "assets": [
            {"name": "SRMI-PACKAGE-v2.4.2.zip",
             "browser_download_url": "https://example.invalid/a.zip"},
            {"name": "SRMI-TEST-PACKAGE-v2.4.2.zip",
             "browser_download_url": "https://example.invalid/b.zip"},
        ]
    });

    match importer::parse_latest_release(&json, &loose).expect_err("two matches is an error") {
        Error::ReleaseAssetAmbiguous {
            release,
            count,
            matches,
            ..
        } => {
            assert_eq!(release, "v2.4.2");
            assert_eq!(count, 2);
            assert!(matches.contains("SRMI-PACKAGE-v2.4.2.zip"));
            assert!(matches.contains("SRMI-TEST-PACKAGE-v2.4.2.zip"));
        }
        other => panic!("expected an ambiguity error, got {other:?}"),
    }
}

#[test]
fn zero_and_two_matches_are_distinguishable_from_each_other_and_from_success() {
    // Per #96 these must stay distinct at every layer beneath the UI,
    // not merely in what is displayed.
    let strict = AssetPattern::new(r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip").expect("valid");
    let missing = AssetPattern::new(r"NOPE-v\d+\.zip").expect("valid");
    let loose = AssetPattern::new(r".*\.zip").expect("valid");
    let json = serde_json::json!({
        "tag_name": "v8.8.9",
        "assets": [
            {"name": "GIMI-PACKAGE-v8.8.9.zip",
             "browser_download_url": "https://example.invalid/a.zip"},
            {"name": "GIMI-EXTRA-v8.8.9.zip",
             "browser_download_url": "https://example.invalid/b.zip"},
        ]
    });

    assert!(importer::parse_latest_release(&json, &strict).is_ok());
    assert!(matches!(
        importer::parse_latest_release(&json, &missing),
        Err(Error::ReleaseAssetNoMatch { .. })
    ));
    assert!(matches!(
        importer::parse_latest_release(&json, &loose),
        Err(Error::ReleaseAssetAmbiguous { .. })
    ));
}

#[test]
fn a_pattern_is_anchored_even_when_written_unanchored() {
    // Patterns arrive from the recommended-importers manifest and from a
    // user's own origin (ADR 0005), not only from the compiled-in
    // defaults. If anchoring were the pattern author's job, a manifest
    // entry of `GIMI-PACKAGE` would silently behave like the substring
    // rule #79 removed.
    let pattern = AssetPattern::new(r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip").expect("valid");

    assert!(pattern.matches("GIMI-PACKAGE-v8.8.9.zip"));
    assert!(!pattern.matches("GIMI-TEST-PACKAGE-v8.8.9.zip"));
    assert!(!pattern.matches("prefix-GIMI-PACKAGE-v8.8.9.zip"));
    assert!(!pattern.matches("GIMI-PACKAGE-v8.8.9.zip.sig"));
}

#[test]
fn an_uncompilable_pattern_is_its_own_error_not_a_missing_asset() {
    // A broken pattern and a renamed upstream asset need different
    // fixes, so they must not arrive as the same error.
    match AssetPattern::new(r"GIMI-PACKAGE-v[").expect_err("unbalanced class") {
        Error::InvalidAssetPattern { pattern, .. } => {
            assert_eq!(pattern, r"GIMI-PACKAGE-v[");
        }
        other => panic!("expected an invalid-pattern error, got {other:?}"),
    }
}
