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
        r#"{"schemaVersion": 1}"#,
        r#"{"schemaVersion": 1, "game": {"gimi": {"status": "none"}}}"#,
        r#"{"schemaVersion": 1, "games": {"gimi": {"status": "recommended", "owner": "a",
             "repo": "b", "assetPattern": "v[.zip"}}}"#,
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

// ---------------------------------------------------------------
// #123 — ingestion is exactly as strict as the contract it advertises.
//
// This file is fetched at runtime by every shipped build, so an
// authoring mistake reaches every install within minutes and ingestion
// strictness is the only guard. Each test below covers a way a
// malformed document was previously accepted, misclassified, or
// silently turned into "there are no recommendations".
// ---------------------------------------------------------------

#[test]
fn a_manifest_with_no_games_key_is_rejected_rather_than_read_as_an_empty_one() {
    // `games` carried a serde default, so one mistyped key — `game`
    // instead of `games` — parsed as a perfectly valid manifest that
    // recommends nothing, replaced the cache, and silently emptied every
    // user's recommendations. No error anywhere, and the offline
    // validator checks different properties so it did not save this.
    //
    // An empty recommendation set is still expressible; it just has to
    // be written down as `"games": {}` rather than arrived at by
    // omission.
    for (label, raw) in [
        ("the key omitted entirely", r#"{"schemaVersion": 1}"#),
        (
            "the key mistyped",
            r#"{"schemaVersion": 1, "game": {"gimi": {"status": "none"}}}"#,
        ),
    ] {
        let error = manifest::parse(raw)
            .err()
            .unwrap_or_else(|| panic!("{label}: must be rejected, not read as an empty manifest"));
        assert!(
            error_message(&error).contains("games"),
            "{label}: the rejection must name the key that is missing: {error}",
        );
        assert!(
            manifest::validate(raw).is_err(),
            "{label}: the validator must reject it too",
        );
    }

    assert!(
        manifest::parse(r#"{"schemaVersion": 1, "games": {}}"#).is_ok(),
        "an explicitly empty recommendation set is still a valid manifest",
    );
}

#[test]
fn an_asset_pattern_that_cannot_compile_is_rejected_at_parse_time() {
    // The pattern was only checked for being a non-empty string, so an
    // invalid regex was cached and applied — failing later, at the point
    // of use, for one game, while every other entry stayed active. The
    // manifest is the one place this can be caught before it reaches a
    // user, and it costs one compile.
    let raw = r#"{"schemaVersion": 1, "games": {
        "gimi": {"status": "recommended", "owner": "SilentNightSound",
                 "repo": "GIMI-Package", "assetPattern": "GIMI-PACKAGE-v[.zip"}
    }}"#;

    let error = manifest::parse(raw).expect_err("an uncompilable pattern must be rejected");
    let message = error_message(&error);
    assert!(
        message.contains("gimi"),
        "the rejection must name the offending game key: {message}",
    );
    assert!(
        message.contains("GIMI-PACKAGE-v[.zip"),
        "the rejection must quote the pattern the author wrote: {message}",
    );
    assert!(
        manifest::validate(raw).is_err(),
        "the validator must reject it too, so a green check stays a guarantee",
    );
}

#[test]
fn an_unknown_game_key_is_skipped_before_its_contents_are_validated() {
    // The ignore branch exists so that shipping a seventh game does not
    // break already-shipped builds. It parsed the entry first "so a
    // malformed one is caught rather than hidden", which defeated the
    // whole point: the seventh game will arrive with whatever fields and
    // status values *its* schema needs, and validating them against this
    // build's vocabulary drops the entire layer on the day it lands.
    //
    // The status has to be an unrecognised one for this test to mean
    // anything — a well-formed entry under an unknown key passes either
    // way.
    let raw = r#"{"schemaVersion": 1, "games": {
        "gimi": {"status": "recommended", "owner": "SilentNightSound",
                 "repo": "GIMI-Package", "assetPattern": "GIMI-PACKAGE-v1[.]zip"},
        "hsri": {"status": "recommended-with-caveats", "someFutureField": 7}
    }}"#;

    let parsed = manifest::parse(raw).expect(
        "a future game's entry must be skipped whole, not validated against \
         this build's vocabulary",
    );
    assert!(
        parsed.recommendation_for(GameCode::Gimi).is_some(),
        "the games this build does know must still apply",
    );

    // The validator still rejects the key, because at review time an
    // unrecognised key is overwhelmingly a typo.
    assert!(
        manifest::validate(raw).is_err(),
        "the review gate must still catch an unrecognised game key",
    );
}
