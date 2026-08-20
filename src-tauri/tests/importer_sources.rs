//! Model Importer source contracts.
//!
//! Each game's recorded release JSON under `tests/fixtures/github/` is a
//! verbatim `gh api repos/<owner>/<repo>/releases/latest` response, so
//! profile drift is deterministic on every test run. The ignored live
//! test is run by the scheduled `upstream-importers` workflow so an
//! upstream repo move or release-asset rename is caught without putting
//! GitHub availability on every PR's critical path.
//!
//! The verdict each origin is expected to reach — selects an asset, or
//! refuses — is **derived from the recording** rather than listed here,
//! so the live test flags drift in either direction: an origin that
//! stops resolving, and an origin that starts resolving differently.

use gmm_lib::core::games::{GameProfile, GAME_PROFILES};
use gmm_lib::core::importer::{self, AssetPattern};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};

/// The recorded `releases/latest` payload for a game.
fn recorded(game: &str) -> serde_json::Value {
    let raw = match game {
        "gimi" => include_str!("fixtures/github/gimi-package-latest.json"),
        "srmi" => include_str!("fixtures/github/srmi-package-latest.json"),
        "zzmi" => include_str!("fixtures/github/zzmi-package-latest.json"),
        "wwmi" => include_str!("fixtures/github/wwmi-package-latest.json"),
        "himi" => include_str!("fixtures/github/himi-package-latest.json"),
        "efmi" => include_str!("fixtures/github/efmi-package-latest.json"),
        other => panic!("no recorded release fixture for {other}"),
    };
    serde_json::from_str(raw).expect("recorded release fixture must be valid JSON")
}

fn origin_of(profile: &GameProfile) -> (&'static str, AssetPattern) {
    let (repo, pattern) = profile
        .importer_repo
        .unwrap_or_else(|| panic!("{} has no Importer Origin", profile.code.as_str()));
    let pattern = AssetPattern::new(pattern).unwrap_or_else(|e| {
        panic!(
            "{} ships an uncompilable asset pattern: {e}",
            profile.code.as_str()
        )
    });
    (repo, pattern)
}

#[test]
fn every_importer_profile_points_at_the_repo_its_recording_came_from() {
    for profile in GAME_PROFILES {
        let game = profile.code.as_str();
        let (repo, _) = origin_of(profile);
        let json = recorded(game);
        let api_url = json["url"].as_str().expect("recording has a url");

        assert!(
            api_url.contains(&format!("/repos/{repo}/")),
            "{game}'s Importer Origin repo {repo:?} drifted from the recorded \
             live source {api_url:?}",
        );
    }
}

#[test]
fn selection_against_the_recordings_matches_the_recorded_verdict() {
    // All six origins select exactly one asset. SRMI was the lone
    // refusal under #79 — its only published asset is named
    // `SRMI-TEST-PACKAGE-…` — which left Star Rail uninstallable;
    // #116 widened that one origin's pattern to accept it. Every
    // supported game must now resolve, and this is the test that fails
    // if any of them stops.
    let mut selected = Vec::new();
    let mut refused = Vec::new();
    for profile in GAME_PROFILES {
        let game = profile.code.as_str();
        let (_, pattern) = origin_of(profile);
        match importer::parse_latest_release(&recorded(game), &pattern) {
            Ok(release) => selected.push((game, release.asset_name)),
            Err(e) => refused.push((game, e)),
        }
    }

    assert!(
        refused.is_empty(),
        "every supported game must be installable from its recorded \
         upstream release, but these refused: {refused:?}",
    );
    assert_eq!(selected.len(), GAME_PROFILES.len());
    assert!(
        selected.contains(&("srmi", "SRMI-TEST-PACKAGE-v2.4.2.zip".to_string())),
        "Star Rail must select the package upstream actually publishes, \
         got {selected:?}",
    );
}

fn github_client() -> reqwest::Client {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("gmm-upstream-check"));
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        let value = HeaderValue::from_str(&format!("Bearer {token}"))
            .expect("GITHUB_TOKEN must be a valid HTTP header value");
        headers.insert(AUTHORIZATION, value);
    }
    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .expect("build GitHub client")
}

#[tokio::test]
#[ignore = "live GitHub contract; run by the upstream-importers workflow"]
async fn every_importer_origin_reaches_the_verdict_its_recording_predicts() {
    let client = github_client();

    for profile in GAME_PROFILES {
        let game = profile.code.as_str();
        let (repo, pattern) = origin_of(profile);

        // What the recording says should happen. Deriving it means this
        // test fails when upstream changes shape in *either* direction,
        // including the day SRMI publishes a real package and the
        // deliberate refusal should be revisited.
        let expected = importer::parse_latest_release(&recorded(game), &pattern)
            .map(|release| release.asset_name);

        let live = importer::fetch_latest_release(&client, repo, &pattern, None).await;

        match (&expected, &live) {
            (Ok(recorded_asset), Ok(Some(release))) => {
                // The version moves; the shape must not.
                assert!(
                    pattern.matches(&release.asset_name),
                    "{game}: live asset {:?} does not match {:?}",
                    release.asset_name,
                    pattern.as_str(),
                );
                let _ = recorded_asset;
            }
            (Ok(recorded_asset), other) => panic!(
                "{game}: {repo} selected {recorded_asset:?} when recorded but \
                 now yields {other:?}"
            ),
            (Err(recorded_error), Err(live_error)) => {
                // Still refusing, as recorded. Confirm it is the same
                // kind of refusal rather than, say, a 404.
                assert_eq!(
                    std::mem::discriminant(recorded_error),
                    std::mem::discriminant(live_error),
                    "{game}: {repo} refused differently than recorded — \
                     recorded {recorded_error}, live {live_error}"
                );
            }
            (Err(recorded_error), Ok(live_release)) => panic!(
                "{game}: {repo} refused when recorded ({recorded_error}) but now \
                 resolves to {live_release:?}. Re-record the fixture and revisit \
                 this origin's pattern deliberately."
            ),
        }
    }
}
