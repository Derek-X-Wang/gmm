//! Model Importer source contracts.
//!
//! The checked-in release inventory makes profile drift deterministic on every
//! test run. The ignored live test is run by the scheduled
//! `upstream-importers` workflow so an upstream repo move or release-asset
//! rename is caught without putting GitHub availability on every PR's critical
//! path.

use std::collections::HashMap;

use gmm_lib::core::games::GAME_PROFILES;
use gmm_lib::core::importer;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct RecordedRelease {
    game: String,
    repo: String,
    tag: String,
    assets: Vec<String>,
}

fn recorded_releases() -> Vec<RecordedRelease> {
    serde_json::from_str(include_str!("fixtures/importer_releases.json"))
        .expect("recorded importer releases must be valid JSON")
}

#[test]
fn every_importer_profile_matches_a_recorded_live_release_and_asset() {
    let releases: HashMap<String, RecordedRelease> = recorded_releases()
        .into_iter()
        .map(|release| (release.game.clone(), release))
        .collect();

    assert_eq!(releases.len(), GAME_PROFILES.len());
    for profile in GAME_PROFILES {
        let release = releases
            .get(profile.code.as_str())
            .unwrap_or_else(|| panic!("missing recorded release for {}", profile.code.as_str()));
        let (repo, asset_filter) = profile
            .importer_repo
            .unwrap_or_else(|| panic!("{} has no Model Importer repo", profile.code.as_str()));

        assert_eq!(
            repo,
            release.repo,
            "{} Model Importer repo drifted from the recorded live source at tag {}",
            profile.code.as_str(),
            release.tag,
        );
        assert!(
            release
                .assets
                .iter()
                .any(|asset| asset.contains(asset_filter)),
            "{} has no recorded release asset matching {asset_filter:?}: {:?}",
            profile.code.as_str(),
            release.assets,
        );
    }
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
async fn every_importer_profile_resolves_to_a_live_release_with_a_matching_asset() {
    let client = github_client();

    for profile in GAME_PROFILES {
        let (repo, asset_filter) = profile
            .importer_repo
            .unwrap_or_else(|| panic!("{} has no Model Importer repo", profile.code.as_str()));
        let release = importer::fetch_latest_release(&client, repo, asset_filter, None)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "{} Model Importer source {repo} did not resolve: {error}",
                    profile.code.as_str()
                )
            })
            .unwrap_or_else(|| {
                panic!(
                    "{} Model Importer source {repo} returned no release",
                    profile.code.as_str()
                )
            });

        assert!(
            release.asset_name.contains(asset_filter),
            "{} latest asset {:?} does not match {asset_filter:?}",
            profile.code.as_str(),
            release.asset_name,
        );
    }
}
