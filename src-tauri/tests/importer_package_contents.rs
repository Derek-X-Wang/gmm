//! The live Model Importer package contract: GMM can install what
//! upstream actually publishes.
//!
//! Every other importer test in this suite runs against a zip that GMM's
//! own test helper wrote. `importer_archive_validation.rs` comes closest —
//! it rebuilds each package from a recorded entry listing — but the bytes
//! are synthetic, every file body is the literal
//! `"; recorded package entry"`, and the `sha256` recorded next to each
//! listing is not even a field on the struct that reads the fixture, so
//! serde drops it. Until this file, **no test had ever opened a real
//! Model Importer package.**
//!
//! That matters because the shape rule in
//! `importer::validate_importer_archive` was *derived* from those
//! recordings, and the recordings are frozen. The rule rejects any archive
//! that is not shaped like a Model Importer — which is correct until the
//! day upstream reshapes a package. Then GMM refuses the real thing, the
//! game becomes uninstallable, and nothing in CI notices, because every
//! test is still passing against a recording of how the package used to
//! look. That is the same failure as a game whose release asset was
//! renamed, which the sibling live contract in `importer_sources.rs`
//! already covers — one checks the asset can be *found*, this checks it
//! can be *installed*.
//!
//! Network-bound, so `#[ignore]`d and driven by the scheduled
//! `upstream importers` workflow rather than every pull request.
//! `ci_test_selection.rs` is what stops that invocation drifting into a
//! no-op.

use std::collections::BTreeSet;
use std::path::Path;

use gmm_lib::core::games::GAME_PROFILES;
use gmm_lib::core::importer::{
    self, sha256_of_file, validate_importer_archive, AssetPattern, IMPORTER_REQUIRED_DIRS,
};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde::Deserialize;
use tempfile::TempDir;

/// One live Model Importer package's recorded entry listing, from
/// `tests/fixtures/importer_package_layouts.json`.
///
/// `sha256` is read here, which is the point: the recording claims to be
/// of a specific asset, and nothing checked that claim before.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedLayout {
    game: String,
    #[allow(dead_code)]
    repo: String,
    asset: String,
    sha256: String,
    entries: Vec<String>,
}

fn recorded_layouts() -> Vec<RecordedLayout> {
    serde_json::from_str(include_str!("fixtures/importer_package_layouts.json"))
        .expect("recorded package layouts must be valid JSON")
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

/// The archive's raw entry names, in the same form the fixture records
/// them — directories keep their trailing slash.
fn raw_entry_names(zip_path: &Path) -> BTreeSet<String> {
    let file = std::fs::File::open(zip_path).expect("open downloaded package");
    let mut archive = zip::ZipArchive::new(file).expect("the downloaded asset must be a zip");
    (0..archive.len())
        .map(|i| {
            let entry = archive.by_index(i).expect("read zip entry");
            entry.name().to_string()
        })
        .collect()
}

/// The contract: for every supported game, the package upstream publishes
/// right now downloads, is a real zip, and is accepted by the same
/// validation an install runs.
///
/// A failure here means one of two things, and the message says which:
/// GMM would refuse to install the current package, or the checked-in
/// recording no longer describes the asset it names.
#[tokio::test]
#[ignore = "downloads the live Model Importer packages; run by the upstream-importers workflow"]
async fn every_live_importer_package_installs_as_a_model_importer() {
    let client = github_client();
    let tmp = TempDir::new().expect("tmp");
    let layouts = recorded_layouts();

    // Guard against the whole test quietly covering nothing if the
    // profile table or the fixture is emptied.
    assert_eq!(
        layouts.len(),
        GAME_PROFILES.len(),
        "every supported game must have a recorded package layout",
    );

    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut drifted: Vec<String> = Vec::new();

    for profile in GAME_PROFILES {
        let game = profile.code.as_str();
        let (repo, pattern) = profile
            .importer_repo
            .unwrap_or_else(|| panic!("{game} has no Importer Origin"));
        let pattern = AssetPattern::new(pattern)
            .unwrap_or_else(|e| panic!("{game} ships an uncompilable asset pattern: {e}"));

        let release = match importer::fetch_latest_release(
            &client,
            &importer::Endpoints::default(),
            repo,
            &pattern,
            None,
        )
        .await
        {
            Ok(Some(release)) => release,
            Ok(None) => {
                failures.push(format!(
                    "{game}: {repo} published no asset matching {:?} — the game is \
                     uninstallable right now",
                    pattern.as_str()
                ));
                continue;
            }
            Err(e) => {
                failures.push(format!("{game}: could not resolve {repo}: {e}"));
                continue;
            }
        };

        let dest = tmp.path().join(&release.asset_name);
        if let Err(e) = importer::download_to(&client, &release.asset_url, &dest).await {
            failures.push(format!(
                "{game}: could not download {}: {e}",
                release.asset_name
            ));
            continue;
        }
        checked += 1;

        // The load-bearing assertion. Anything upstream can ship that GMM
        // would refuse fails here, before a user meets it.
        if let Err(e) = validate_importer_archive(&dest) {
            failures.push(format!(
                "{game}: GMM would REFUSE to install {}, the package {repo} publishes \
                 today: {e}. Either the package reshaped upstream and \
                 IMPORTER_REQUIRED_DIRS ({IMPORTER_REQUIRED_DIRS:?}) needs revisiting, \
                 or upstream shipped something that genuinely is not an importer.",
                release.asset_name,
            ));
            continue;
        }

        let recorded = layouts
            .iter()
            .find(|l| l.game == game)
            .unwrap_or_else(|| panic!("no recorded layout for {game}"));
        let digest = sha256_of_file(&dest).expect("hash the downloaded package");

        if release.asset_name == recorded.asset {
            // Same asset the fixture claims to record: the recording must
            // be truthful, byte-for-byte and entry-for-entry. A mismatch
            // means the asset was re-uploaded under the same name, or the
            // fixture was hand-edited.
            if digest != recorded.sha256 {
                failures.push(format!(
                    "{game}: {} hashes {digest} but the recording claims {} — the \
                     release asset was replaced in place, or the fixture is fiction",
                    release.asset_name, recorded.sha256,
                ));
                continue;
            }
            let live: BTreeSet<String> = raw_entry_names(&dest);
            let expected: BTreeSet<String> = recorded.entries.iter().cloned().collect();
            if live != expected {
                let added: Vec<_> = live.difference(&expected).take(10).collect();
                let removed: Vec<_> = expected.difference(&live).take(10).collect();
                failures.push(format!(
                    "{game}: the recorded entry listing does not match the real \
                     asset it names. Only in the package: {added:?}. Only in the \
                     recording: {removed:?}",
                ));
            }
        } else {
            // Upstream moved on. Not a failure — the shape check above is
            // the contract — but the recording that the shape rule was
            // derived from is now describing an older package, and that is
            // worth saying out loud in the job log.
            drifted.push(format!(
                "{game}: upstream now publishes {} (sha256 {digest}); the fixture \
                 still records {} — re-record it when convenient",
                release.asset_name, recorded.asset,
            ));
        }
    }

    for note in &drifted {
        println!("DRIFT  {note}");
    }

    assert_eq!(
        checked,
        GAME_PROFILES.len(),
        "every supported game's package must have been downloaded and opened; \
         failures: {failures:?}",
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
