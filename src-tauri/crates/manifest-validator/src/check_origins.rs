//! Live resolution check for every *recommended* Importer Origin in
//! `manifest/recommended-importers.json`.
//!
//! ADR 0005 makes GMM a conduit rather than a maintainer, and pays for
//! that with one editorial responsibility: the recommendation has to
//! still be true. The ADR deliberately puts staleness detection in the
//! maintainer's process rather than on the user's screen — "GMM never
//! asks a user to judge whether an importer is abandoned, because age is
//! a bad proxy for health". This binary is that process's executable
//! half, run on a schedule by `.github/workflows/upstream-importers.yml`.
//!
//! It is the counterpart to `validate-manifest`, and the split is the
//! point (#94): shape validation is offline and runs on every pull
//! request, resolution is live and runs on a schedule. Putting a
//! third-party host on the critical path of merging would make the
//! manifest unmergeable whenever the GitHub API is rate-limited.
//!
//! ```text
//! check-origins [--manifest <path>] [--report <path>] [--api-base <url>]
//! ```
//!
//! Exit status is 0 when every recommended origin resolved and 1 when
//! any did not. The **report is written either way**: the workflow reads
//! it to decide whether this is the second consecutive failure for the
//! same origin before it opens an issue, and a failure that writes
//! nothing is one the alerting cannot see.

use std::path::PathBuf;
use std::process::ExitCode;

use gmm_lib::core::importer::{self, AssetPattern, Endpoints};
use gmm_lib::core::importer_origin::Recommendation;
use gmm_lib::core::recommended_importers::{self, MANIFEST_PATH};

/// The verdict for one recommended origin, as the workflow reads it.
///
/// `game` plus `origin` together are the alert key. Both are needed:
/// re-pointing a game at a different repository is a deliberate act that
/// must reset the consecutive-failure counter rather than inherit the
/// previous origin's death.
fn verdict(
    game: &str,
    origin: &str,
    pattern: &str,
    outcome: Result<String, String>,
) -> serde_json::Value {
    match outcome {
        Ok(asset) => serde_json::json!({
            "game": game,
            "origin": origin,
            "assetPattern": pattern,
            "ok": true,
            "asset": asset,
            "detail": format!("selected {asset}"),
        }),
        Err(detail) => serde_json::json!({
            "game": game,
            "origin": origin,
            "assetPattern": pattern,
            "ok": false,
            "detail": detail,
        }),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mut manifest_path: Option<PathBuf> = None;
    let mut report_path: Option<PathBuf> = None;
    let mut api_base: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut take = |name: &str| {
            args.next()
                .unwrap_or_else(|| panic!("{name} requires a value"))
        };
        match arg.as_str() {
            "--manifest" => manifest_path = Some(PathBuf::from(take("--manifest"))),
            "--report" => report_path = Some(PathBuf::from(take("--report"))),
            "--api-base" => api_base = Some(take("--api-base")),
            other => {
                eprintln!("unrecognised argument {other:?}");
                return ExitCode::FAILURE;
            }
        }
    }

    let manifest_path = manifest_path.unwrap_or_else(default_manifest_path);
    let raw = match std::fs::read_to_string(&manifest_path) {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!(
                "could not read manifest at {}: {e}",
                manifest_path.display()
            );
            return ExitCode::FAILURE;
        }
    };

    // Same whole-document contract as the offline validator. Resolving
    // half a document GMM would refuse outright would report health for
    // a manifest that never reaches a user.
    let manifest = match recommended_importers::validate(&raw) {
        Ok(manifest) => manifest,
        Err(e) => {
            eprintln!("{} is invalid: {e}", manifest_path.display());
            return ExitCode::FAILURE;
        }
    };

    let endpoints = api_base
        .map(|api_base| Endpoints { api_base })
        .unwrap_or_default();
    let client = match github_client() {
        Ok(client) => client,
        Err(e) => {
            eprintln!("could not build an HTTP client: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut origins = Vec::new();
    let mut failures = 0usize;

    for game in gmm_lib::core::games::GAME_PROFILES {
        let code = game.code;
        // Only `recommended` names an origin at all. A `none` entry
        // retracts the compiled-in default and points nowhere, so there
        // is nothing to resolve — and an absent key is layer 3's
        // business, not this file's.
        // The entry's `reason` is deliberately ignored here: this job
        // asks whether the origin still resolves, and prose about why it
        // was chosen has no bearing on that.
        let Some(Recommendation::Recommended { origin, .. }) = manifest.recommendation_for(code)
        else {
            continue;
        };
        let slug = origin.repo_slug();
        let pattern_src = origin.asset_pattern().to_string();
        // Already compiled once by `validate`; this cannot fail.
        let pattern = match AssetPattern::new(&pattern_src) {
            Ok(pattern) => pattern,
            Err(e) => {
                eprintln!(
                    "{}: uncompilable pattern {pattern_src:?}: {e}",
                    code.as_str()
                );
                return ExitCode::FAILURE;
            }
        };

        let outcome = match importer::fetch_latest_release(
            &client, &endpoints, &slug, &pattern, None,
        )
        .await
        {
            Ok(Some(release)) => Ok(release.asset_name),
            // Only reachable with a conditional request, which this
            // never makes. Treated as a failure rather than silently
            // passing: #78 is the precedent for a check that reported
            // health by collapsing an unexpected state into "fine".
            Ok(None) => {
                Err("upstream answered 304 Not Modified to an unconditional request".into())
            }
            Err(e) => Err(e.to_string()),
        };

        match &outcome {
            Ok(asset) => println!("ok    {}  {slug}  → {asset}", code.as_str()),
            Err(detail) => {
                failures += 1;
                println!("FAIL  {}  {slug}  → {detail}", code.as_str());
            }
        }
        origins.push(verdict(code.as_str(), &slug, &pattern_src, outcome));
    }

    let report = serde_json::json!({
        "manifest": manifest_path.display().to_string(),
        "checked": origins.len(),
        "failed": failures,
        "origins": origins,
    });

    let report_path = report_path.unwrap_or_else(|| PathBuf::from("origin-report.json"));
    let serialised = serde_json::to_string_pretty(&report).expect("report serialises");
    if let Err(e) = std::fs::write(&report_path, format!("{serialised}\n")) {
        eprintln!("could not write report to {}: {e}", report_path.display());
        return ExitCode::FAILURE;
    }
    println!("report written to {}", report_path.display());

    if failures == 0 {
        println!(
            "all {} recommended Importer Origins resolve",
            report["checked"]
        );
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "{failures} of {} recommended Importer Origins failed to resolve",
            report["checked"]
        );
        ExitCode::FAILURE
    }
}

/// A GitHub client that identifies itself and uses `GITHUB_TOKEN` when
/// one is present.
///
/// Unauthenticated GitHub allows 60 requests an hour per IP, shared
/// across every job on the runner. The scheduled workflow passes the
/// job's own `GITHUB_TOKEN`, which needs no scope beyond public reads.
fn github_client() -> Result<reqwest::Client, reqwest::Error> {
    use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};

    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("gmm-check-origins"));
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
            headers.insert(AUTHORIZATION, value);
        }
    }
    reqwest::Client::builder().default_headers(headers).build()
}

/// Locate the committed manifest by walking up from this crate, so the
/// command works from any directory and does not encode how deep the
/// crate sits.
// This CLI probe tries ancestors opportunistically and returns an actionable fallback path on any miss.
#[allow(clippy::disallowed_methods)]
fn default_manifest_path() -> PathBuf {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for ancestor in here.ancestors() {
        let candidate = ancestor.join(MANIFEST_PATH);
        if candidate.is_file() {
            return candidate;
        }
    }
    here.join(MANIFEST_PATH)
}
