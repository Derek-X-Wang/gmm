//! `check-origins` — the live half of ADR 0005's two CI guarantees.
//!
//! ADR 0005 puts staleness detection in the maintainer's process rather
//! than on the user's screen: a scheduled job resolves every
//! *recommended* origin and tells the maintainer when one dies. This is
//! that job's executable half.
//!
//! Everything here runs against a `mockito` server, so the contract the
//! scheduled workflow depends on — exit status, and a machine-readable
//! report written **whether or not** the checks passed — is pinned
//! without touching GitHub.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// Run the binary against `api_base`, returning (exit ok, stdout+stderr,
/// parsed report).
fn run(manifest: &Path, api_base: &str, tmp: &TempDir) -> (bool, String, serde_json::Value) {
    let report = tmp.path().join("origin-report.json");
    let out = Command::new(env!("CARGO_BIN_EXE_check-origins"))
        .arg("--manifest")
        .arg(manifest)
        .arg("--report")
        .arg(&report)
        .arg("--api-base")
        .arg(api_base)
        .output()
        .expect("run check-origins");

    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // The report is the whole point of the job: the workflow reads it to
    // decide whether this is the second consecutive failure for an
    // origin. A run that fails without writing one is a run the
    // maintainer alerting cannot see.
    let raw = std::fs::read_to_string(&report).unwrap_or_else(|e| {
        panic!(
            "a report must be written at {}: {e}\n{logs}",
            report.display()
        )
    });
    let parsed = serde_json::from_str(&raw).expect("the report must be valid JSON");
    (out.status.success(), logs, parsed)
}

fn write_manifest(tmp: &TempDir, body: &str) -> std::path::PathBuf {
    let path = tmp.path().join("recommended-importers.json");
    std::fs::write(&path, body).expect("write manifest");
    path
}

fn release_json(tag: &str, assets: &[&str]) -> String {
    let assets: Vec<serde_json::Value> = assets
        .iter()
        .map(|name| {
            serde_json::json!({
                "name": name,
                "browser_download_url": format!("https://example.invalid/{name}"),
            })
        })
        .collect();
    serde_json::json!({ "tag_name": tag, "assets": assets }).to_string()
}

/// One recommended game, plus a `none` entry that must not be fetched.
const TWO_ENTRIES: &str = r#"{
  "schemaVersion": 1,
  "games": {
    "gimi": {
      "status": "recommended",
      "owner": "Someone",
      "repo": "GIMI-Package",
      "assetPattern": "GIMI-PACKAGE-v\\d+\\.\\d+\\.\\d+\\.zip"
    },
    "himi": {
      "status": "none",
      "reason": "no maintained package"
    }
  }
}"#;

#[tokio::test]
async fn a_resolving_origin_reports_ok_and_exits_zero() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("GET", "/repos/Someone/GIMI-Package/releases/latest")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(release_json("v8.8.9", &["GIMI-PACKAGE-v8.8.9.zip"]))
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let manifest = write_manifest(&tmp, TWO_ENTRIES);
    let (ok, logs, report) = run(&manifest, &server.url(), &tmp);
    m.assert_async().await;

    assert!(ok, "a resolving origin must exit 0:\n{logs}");
    let origins = report["origins"].as_array().expect("origins array");
    assert_eq!(
        origins.len(),
        1,
        "only `recommended` entries are checked — a `none` entry retracts a \
         default and names no origin to resolve: {report:#}"
    );
    assert_eq!(origins[0]["game"], "gimi");
    assert_eq!(origins[0]["origin"], "Someone/GIMI-Package");
    assert_eq!(origins[0]["ok"], true);
    assert_eq!(origins[0]["asset"], "GIMI-PACKAGE-v8.8.9.zip");
}

#[tokio::test]
async fn an_origin_whose_release_stopped_matching_fails_and_is_named() {
    // The HIMI shape: the repository is alive, the release is there, but
    // nothing in it is the package GMM expects any more.
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/repos/Someone/GIMI-Package/releases/latest")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(release_json("v9.0.0", &["source-code.zip"]))
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let manifest = write_manifest(&tmp, TWO_ENTRIES);
    let (ok, logs, report) = run(&manifest, &server.url(), &tmp);

    assert!(!ok, "a dead origin must exit non-zero:\n{logs}");
    let origins = report["origins"].as_array().expect("origins array");
    assert_eq!(origins[0]["ok"], false);
    let detail = origins[0]["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("source-code.zip"),
        "the failure must name what upstream actually published, so the \
         maintainer can fix the manifest without re-running anything: {detail:?}"
    );
    assert!(
        logs.contains("gimi"),
        "the run's own log must name the failing game: {logs}"
    );
}

#[tokio::test]
async fn an_unreachable_repository_fails_that_origin_only() {
    let mut server = mockito::Server::new_async().await;
    let _gone = server
        .mock("GET", "/repos/Someone/Deleted-Package/releases/latest")
        .with_status(404)
        .with_body("{}")
        .create_async()
        .await;
    let _alive = server
        .mock("GET", "/repos/Someone/GIMI-Package/releases/latest")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(release_json("v8.8.9", &["GIMI-PACKAGE-v8.8.9.zip"]))
        .create_async()
        .await;

    let tmp = TempDir::new().unwrap();
    let manifest = write_manifest(
        &tmp,
        r#"{
  "schemaVersion": 1,
  "games": {
    "gimi": {
      "status": "recommended",
      "owner": "Someone",
      "repo": "GIMI-Package",
      "assetPattern": "GIMI-PACKAGE-v\\d+\\.\\d+\\.\\d+\\.zip"
    },
    "zzmi": {
      "status": "recommended",
      "owner": "Someone",
      "repo": "Deleted-Package",
      "assetPattern": "ZZMI-PACKAGE-v\\d+\\.\\d+\\.\\d+\\.zip"
    }
  }
}"#,
    );
    let (ok, logs, report) = run(&manifest, &server.url(), &tmp);

    assert!(!ok, "exit non-zero when any origin fails:\n{logs}");
    let by_game: std::collections::BTreeMap<&str, &serde_json::Value> = report["origins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| (o["game"].as_str().unwrap(), o))
        .collect();
    assert_eq!(
        by_game["gimi"]["ok"], true,
        "one dead origin must not condemn the others — the alerting is \
         per-origin, so the report has to be too"
    );
    assert_eq!(by_game["zzmi"]["ok"], false);
    assert!(by_game["zzmi"]["detail"]
        .as_str()
        .unwrap_or_default()
        .contains("404"));
}

#[test]
fn a_malformed_manifest_is_refused_before_any_network_call() {
    // Same whole-document contract as the offline validator: this job
    // must not half-check a document GMM would refuse outright.
    let tmp = TempDir::new().unwrap();
    let manifest = write_manifest(&tmp, r#"{"schemaVersion": 1}"#);
    let report = tmp.path().join("origin-report.json");
    let out = Command::new(env!("CARGO_BIN_EXE_check-origins"))
        .arg("--manifest")
        .arg(&manifest)
        .arg("--report")
        .arg(&report)
        .arg("--api-base")
        .arg("http://127.0.0.1:1/unused")
        .output()
        .expect("run check-origins");

    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("games"),
        "the refusal must name the offending field: {err}"
    );
}
