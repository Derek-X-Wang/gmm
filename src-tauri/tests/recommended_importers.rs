//! ADR 0005 / #111 — the committed `recommended-importers.json` and its
//! offline shape validator.
//!
//! The manifest is fetched at runtime by every released build, so a bad
//! commit reconfigures every install within minutes. The review gate on
//! `main` plus this validator are the only things standing in the way,
//! which is why the validator has to fail loudly and name what is wrong.

use gmm_lib::core::recommended_importers::{
    self as manifest, ManifestError, MANIFEST_PATH, MANIFEST_URL, SUPPORTED_SCHEMA_VERSION,
};
use gmm_lib::core::GameCode;

/// The committed manifest, read from the path the app fetches.
fn committed() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .join(MANIFEST_PATH);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the manifest must exist at {}: {e}", path.display()))
}

#[test]
fn the_apps_fetch_url_points_at_the_committed_path() {
    // ADR 0005 makes this a permanent commitment: every build ever
    // shipped requests exactly this URL forever, so `main` can never be
    // renamed and the path can never move. If these two ever disagree,
    // the layer is silently dead for every user.
    assert_eq!(MANIFEST_PATH, "manifest/recommended-importers.json");
    assert_eq!(
        MANIFEST_URL,
        "https://raw.githubusercontent.com/Derek-X-Wang/gmm/main/manifest/recommended-importers.json",
    );
    assert!(
        MANIFEST_URL.ends_with(MANIFEST_PATH),
        "the fetch URL must end with the committed path",
    );
}

#[test]
fn the_committed_manifest_is_parseable_by_the_apps_own_type() {
    // The anti-drift check. The validator and the app must agree, and
    // the cheapest way to guarantee that is for both to use this type.
    let parsed = manifest::parse(&committed()).expect("the committed manifest must parse");
    assert_eq!(parsed.schema_version(), SUPPORTED_SCHEMA_VERSION);
}

#[test]
fn five_games_are_recommended_and_match_the_compiled_in_defaults() {
    use gmm_lib::core::importer_origin::{compiled_in_default, Recommendation};

    let parsed = manifest::parse(&committed()).expect("parse");

    for game in [
        GameCode::Gimi,
        GameCode::Srmi,
        GameCode::Zzmi,
        GameCode::Wwmi,
        GameCode::Efmi,
    ] {
        match parsed.recommendation_for(game) {
            Some(Recommendation::Recommended(origin)) => {
                let default = compiled_in_default(game)
                    .unwrap_or_else(|| panic!("{} has no compiled-in default", game.as_str()));
                assert_eq!(
                    origin,
                    default,
                    "{}'s manifest entry must match the compiled-in default it mirrors",
                    game.as_str(),
                );
            }
            other => panic!("{} must be recommended, got {other:?}", game.as_str()),
        }
    }
}

#[test]
fn himi_is_an_explicit_retraction_carrying_a_reason() {
    use gmm_lib::core::importer_origin::Recommendation;

    // `leotorrez/HIMI-Package` last released 2025-07-24 and carries no
    // licence at all, so there is no maintained package GMM can
    // recommend (ADR 0005). A `none` entry retracts the compiled-in
    // default rather than falling through to it.
    let parsed = manifest::parse(&committed()).expect("parse");

    match parsed.recommendation_for(GameCode::Himi) {
        Some(Recommendation::NoRecommendation { reason }) => {
            let reason = reason.expect("a retraction must tell the user what to do");
            assert!(
                !reason.trim().is_empty(),
                "an empty reason is worse than none — it fills the prompt with nothing",
            );
        }
        other => panic!("HIMI must be an explicit retraction, got {other:?}"),
    }
}

#[test]
fn the_committed_manifest_retracts_himi_rather_than_omitting_it() {
    use gmm_lib::core::importer_origin::compiled_in_default;
    use gmm_lib::core::importer_origin::{resolve, OriginResolution};

    // The distinction the whole schema exists to protect. If HIMI were
    // merely absent, the compiled-in default would still apply and the
    // retraction would do no work at all.
    let parsed = manifest::parse(&committed()).expect("parse");
    let recommendation = parsed.recommendation_for(GameCode::Himi);
    let default = compiled_in_default(GameCode::Himi);

    let resolved = resolve(None, recommendation.as_ref(), default.as_ref());
    assert!(
        matches!(resolved, OriginResolution::NoneInEffect { .. }),
        "HIMI's entry must retract its compiled-in default, got {resolved:?}",
    );
}

#[test]
fn parsing_needs_no_network_and_is_deterministic() {
    // Offline and repeatable: live resolution checks belong on a
    // schedule, never on the critical path of merging (#94).
    let raw = committed();
    let a = manifest::parse(&raw).expect("parse");
    let b = manifest::parse(&raw).expect("parse");
    for game in [GameCode::Gimi, GameCode::Himi, GameCode::Efmi] {
        assert_eq!(a.recommendation_for(game), b.recommendation_for(game));
    }
}

// ---------------------------------------------------------------
// The validator must reject each of these, naming what is wrong.
// ---------------------------------------------------------------

fn rejection(json: &str) -> ManifestError {
    manifest::parse(json).expect_err("this manifest must be rejected")
}

#[test]
fn invalid_json_is_rejected() {
    let error = rejection("{ not json");
    assert!(matches!(error, ManifestError::InvalidJson { .. }));
}

#[test]
fn an_unrecognised_schema_version_is_rejected_naming_the_version() {
    // The cheap early signal of "your build is too old". A higher
    // version means the whole layer drops out — never partial
    // application of a document the build has admitted it cannot read.
    let error = rejection(r#"{"schemaVersion": 99, "games": {}}"#);
    match error {
        ManifestError::UnsupportedSchemaVersion { found, supported } => {
            assert_eq!(found, 99);
            assert_eq!(supported, SUPPORTED_SCHEMA_VERSION);
        }
        other => panic!("expected an unsupported-version error, got {other:?}"),
    }
    assert!(error_message(&rejection(r#"{"schemaVersion": 99, "games": {}}"#)).contains("99"));
}

#[test]
fn an_unrecognised_status_is_rejected_naming_the_game() {
    let error = rejection(
        r#"{"schemaVersion": 1, "games": {"gimi": {"status": "deprecated", "reason": "x"}}}"#,
    );
    match error {
        ManifestError::UnknownStatus {
            ref game,
            ref status,
        } => {
            assert_eq!(game, "gimi");
            assert_eq!(status, "deprecated");
        }
        ref other => panic!("expected an unknown-status error, got {other:?}"),
    }
    let message = error_message(&error);
    assert!(message.contains("gimi"), "must name the game: {message}");
    assert!(
        message.contains("deprecated"),
        "must name the status: {message}"
    );
}

#[test]
fn a_recommended_entry_missing_a_required_field_is_rejected_naming_the_field() {
    // `repo` omitted.
    let error = rejection(
        r#"{"schemaVersion": 1, "games": {"gimi": {"status": "recommended",
            "owner": "SilentNightSound", "assetPattern": "GIMI-PACKAGE-v1.zip"}}}"#,
    );
    match error {
        ManifestError::MissingField {
            ref game,
            ref field,
        } => {
            assert_eq!(game, "gimi");
            assert_eq!(field, "repo");
        }
        ref other => panic!("expected a missing-field error, got {other:?}"),
    }
    let message = error_message(&error);
    assert!(
        message.contains("gimi") && message.contains("repo"),
        "{message}"
    );
}

#[test]
fn every_required_field_of_a_recommended_entry_is_actually_required() {
    for missing in ["owner", "repo", "assetPattern"] {
        let mut fields = vec![
            (r#""owner": "SilentNightSound""#, "owner"),
            (r#""repo": "GIMI-Package""#, "repo"),
            (r#""assetPattern": "GIMI-PACKAGE-v1\\.zip""#, "assetPattern"),
        ];
        fields.retain(|(_, name)| *name != missing);
        let body: Vec<&str> = fields.iter().map(|(text, _)| *text).collect();
        let json = format!(
            r#"{{"schemaVersion": 1, "games": {{"gimi": {{"status": "recommended", {}}}}}}}"#,
            body.join(", "),
        );
        match rejection(&json) {
            ManifestError::MissingField { game, field } => {
                assert_eq!(game, "gimi");
                assert_eq!(field, missing);
            }
            other => panic!("omitting {missing} must be rejected, got {other:?}"),
        }
    }
}

#[test]
fn an_unrecognised_game_key_is_rejected_by_the_validator_naming_the_key() {
    // A typo like this is exactly what the review gate is for: it would
    // otherwise sit in the file doing nothing, while the game it was
    // meant for silently kept its old default.
    let error = manifest::validate(
        r#"{"schemaVersion": 1, "games": {"grmi": {"status": "none", "reason": "typo"}}}"#,
    )
    .expect_err("the validator must reject an unknown game key");
    match error {
        ManifestError::UnknownGame { ref game } => assert_eq!(game, "grmi"),
        ref other => panic!("expected an unknown-game error, got {other:?}"),
    }
    assert!(error_message(&error).contains("grmi"));
}

#[test]
fn the_app_ignores_a_game_key_it_does_not_know_instead_of_dropping_the_layer() {
    // The additive-only rule exists so already-shipped builds keep
    // parsing this file. Adding a seventh game is a routine additive
    // change; if an old build rejected the whole manifest over a key
    // naming a game it does not have, every existing user would lose
    // the recommendation layer the day that game landed.
    //
    // This is the one place the app is deliberately more permissive
    // than the validator. It is not partial application of a document
    // the build cannot read — the structure is fully understood, the
    // key simply names a game this build does not have.
    let raw = r#"{"schemaVersion": 1, "games": {
        "gimi": {"status": "recommended", "owner": "SilentNightSound",
                 "repo": "GIMI-Package", "assetPattern": "GIMI-PACKAGE-v1[.]zip"},
        "xxmi": {"status": "recommended", "owner": "someone",
                 "repo": "XXMI-Package", "assetPattern": "XXMI-PACKAGE-v1[.]zip"}
    }}"#;

    let parsed = manifest::parse(raw).expect("an unknown game key must not drop the whole layer");
    assert!(
        parsed.recommendation_for(GameCode::Gimi).is_some(),
        "the games this build does know must still apply",
    );
}

#[test]
fn anything_the_validator_accepts_the_app_can_parse() {
    // The anti-drift property stated as a rule: the validator is
    // strictly stricter, so a green check is a guarantee about the app.
    let raw = committed();
    assert!(manifest::validate(&raw).is_ok());
    assert!(manifest::parse(&raw).is_ok());

    // And every rejection the app makes is also a rejection for the
    // validator — the validator never lets through what the app cannot
    // read.
    for bad in [
        "{ not json",
        r#"{"schemaVersion": 99, "games": {}}"#,
        r#"{"schemaVersion": 1, "games": {"gimi": {"status": "nope"}}}"#,
        r#"{"schemaVersion": 1, "games": {"gimi": {"status": "recommended", "owner": "a"}}}"#,
    ] {
        assert!(manifest::parse(bad).is_err(), "app must reject: {bad}");
        assert!(
            manifest::validate(bad).is_err(),
            "validator must reject: {bad}"
        );
    }
}

#[test]
fn a_none_entry_needs_no_origin_fields() {
    let parsed = manifest::parse(
        r#"{"schemaVersion": 1, "games": {"himi": {"status": "none", "reason": "none known"}}}"#,
    )
    .expect("a retraction carries no origin");
    assert!(parsed.recommendation_for(GameCode::Himi).is_some());
}

#[test]
fn a_game_absent_from_the_manifest_has_no_recommendation_at_all() {
    // Absent must be `None`, which falls through to the compiled-in
    // default — never a retraction (#93).
    let parsed = manifest::parse(r#"{"schemaVersion": 1, "games": {}}"#).expect("parse");
    assert_eq!(parsed.recommendation_for(GameCode::Gimi), None);
}

fn error_message(error: &ManifestError) -> String {
    error.to_string()
}

// ---------------------------------------------------------------
// The validator as a command: one invocation, non-zero exit on
// failure, a message naming the offending game key or field.
// ---------------------------------------------------------------

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

#[test]
fn every_recommended_pattern_compiles_and_selects_its_real_recorded_asset() {
    // The strongest offline check the review gate can make: not just
    // "is this well-formed JSON" but "would this entry actually install
    // anything". Asserted against the verbatim recorded GitHub releases
    // under tests/fixtures/github/, because hand-written fixtures are
    // what let the original substring bug survive (#79).
    //
    // This stays offline and deterministic. Checking that an origin
    // *still* resolves belongs on a schedule, not on a pull request.
    use gmm_lib::core::importer::{self, AssetPattern};
    use gmm_lib::core::importer_origin::Recommendation;

    let parsed = manifest::parse(&committed()).expect("parse");

    let recorded: [(GameCode, &str, &str); 5] = [
        (
            GameCode::Gimi,
            include_str!("fixtures/github/gimi-package-latest.json"),
            "GIMI-PACKAGE-v8.8.9.zip",
        ),
        (
            GameCode::Srmi,
            include_str!("fixtures/github/srmi-package-latest.json"),
            "SRMI-TEST-PACKAGE-v2.4.2.zip",
        ),
        (
            GameCode::Zzmi,
            include_str!("fixtures/github/zzmi-package-latest.json"),
            "ZZMI-PACKAGE-v1.4.5.zip",
        ),
        (
            GameCode::Wwmi,
            include_str!("fixtures/github/wwmi-package-latest.json"),
            "WWMI-PACKAGE-v1.0.0.zip",
        ),
        (
            GameCode::Efmi,
            include_str!("fixtures/github/efmi-package-latest.json"),
            "EFMI-PACKAGE-v1.3.0.zip",
        ),
    ];

    for (game, raw, expected_asset) in recorded {
        let origin = match parsed.recommendation_for(game) {
            Some(Recommendation::Recommended(origin)) => origin,
            other => panic!("{} must be recommended, got {other:?}", game.as_str()),
        };

        let pattern = AssetPattern::new(origin.asset_pattern()).unwrap_or_else(|e| {
            panic!(
                "{}'s manifest assetPattern does not compile: {e}",
                game.as_str()
            )
        });
        let release: serde_json::Value =
            serde_json::from_str(raw).expect("recorded fixture is valid JSON");
        let selected = importer::parse_latest_release(&release, &pattern).unwrap_or_else(|e| {
            panic!(
                "{}'s manifest entry selects nothing from its real release: {e}",
                game.as_str()
            )
        });

        assert_eq!(selected.asset_name, expected_asset, "{}", game.as_str());
    }
}
