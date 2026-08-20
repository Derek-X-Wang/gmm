//! The validator as a command: one invocation, non-zero exit on
//! failure, and a message naming the offending game key or field.
//!
//! These live here rather than in the Tauri package's test suite
//! because `CARGO_BIN_EXE_*` is only set for binaries in the same
//! package as the test — and this binary deliberately sits outside that
//! package so it cannot disturb what the bundler ships.

use std::process::Command;

fn run_validator(args: &[&str]) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_validate-manifest"))
        .args(args)
        .output()
        .expect("the validator binary must be runnable");
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), text)
}

fn write_temp(json: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().expect("tmp");
    let path = dir.path().join("recommended-importers.json");
    std::fs::write(&path, json).expect("write");
    (dir, path)
}

#[test]
fn the_validator_passes_on_the_committed_manifest() {
    // Run with no arguments it validates the committed file, which is
    // what CI invokes.
    let (ok, output) = run_validator(&[]);
    assert!(ok, "the committed manifest must validate: {output}");
}

#[test]
fn the_validator_exits_non_zero_and_names_the_problem_for_each_rejection() {
    let cases: [(&str, &[&str]); 5] = [
        ("{ not json", &["JSON"]),
        (r#"{"schemaVersion": 42, "games": {}}"#, &["42"]),
        (
            r#"{"schemaVersion": 1, "games": {"gimi": {"status": "retired"}}}"#,
            &["gimi", "retired"],
        ),
        (
            r#"{"schemaVersion": 1, "games": {"wwmi": {"status": "recommended", "owner": "SpectrumQT"}}}"#,
            &["wwmi", "repo"],
        ),
        (
            r#"{"schemaVersion": 1, "games": {"nope": {"status": "none"}}}"#,
            &["nope"],
        ),
    ];

    for (json, needles) in cases {
        let (dir, path) = write_temp(json);
        let (ok, output) = run_validator(&[path.to_str().expect("utf8 path")]);
        assert!(!ok, "this manifest must be rejected: {json}");
        for needle in needles {
            assert!(
                output.contains(needle),
                "the failure must name {needle:?} so the maintainer can find it; got: {output}",
            );
        }
        drop(dir);
    }
}

#[test]
fn the_validator_reports_a_missing_file_rather_than_passing_vacuously() {
    // A validator that silently succeeds when it cannot find the file
    // is worse than no validator, because CI goes green.
    let (ok, output) = run_validator(&["/nonexistent/recommended-importers.json"]);
    assert!(!ok, "a missing manifest must fail: {output}");
    assert!(output.contains("/nonexistent/recommended-importers.json"));
}
