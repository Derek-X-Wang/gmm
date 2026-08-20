//! GMM's curated `recommended-importers.json` — layer 2 of ADR 0005's
//! Importer Origin precedence.
//!
//! The manifest lives on `main` in this repository and is fetched at
//! runtime as raw content, so a fix reaches users who are stranded on a
//! dead importer *without* requiring them to update GMM. That is the
//! entire value, and it is also the risk: a valid-but-wrong manifest
//! reaches every install within minutes. The review gate on `main`
//! (`enforce_admins: true`) plus the offline validator built on this
//! module are the only things standing between a bad commit and every
//! user.
//!
//! This module owns the *shape*. Fetching and caching are #108.
//!
//! # Authoring rules
//!
//! See `manifest/README.md`, which sits next to the file an editor will
//! actually open. The two that matter most:
//!
//! - **The schema is additive only.** Fields and status values may be
//!   added, never repurposed or redefined. This is what keeps older
//!   builds parsing successfully instead of hitting "your build is too
//!   old" on every routine change.
//! - **`none` retracts; absence falls through.** To hand a game back to
//!   its compiled-in default, *remove the key* — do not set it to
//!   `none`.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::games::{GameCode, GAME_PROFILES};
use super::importer_origin::{ImporterOrigin, Recommendation};

/// The path the manifest is committed at, relative to the repository
/// root.
///
/// **Permanent.** Every build ever shipped requests exactly this path
/// forever, so it can never move — and `main` can never be renamed.
/// ADR 0005 records this as a constraint on the repository, not a
/// preference.
pub const MANIFEST_PATH: &str = "manifest/recommended-importers.json";

/// The raw-content URL the app fetches. Must always agree with
/// [`MANIFEST_PATH`]; a test asserts it.
pub const MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/Derek-X-Wang/gmm/main/manifest/recommended-importers.json";

/// The only `schemaVersion` this build understands.
///
/// A higher version means the manifest was written for a newer GMM.
/// The whole layer then drops out — never partial application of a
/// document the build has already admitted it cannot read (#93).
pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// Why a manifest could not be used.
///
/// Every variant names the offending game key or field, because the
/// person reading this message is a maintainer staring at a failed
/// check on a one-line diff.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest is not valid JSON: {message}")]
    InvalidJson { message: String },

    #[error(
        "manifest declares schemaVersion {found}, but this build understands only \
         {supported} — the manifest was written for a newer GMM"
    )]
    UnsupportedSchemaVersion { found: u32, supported: u32 },

    #[error("game {game:?}: unrecognised status {status:?}")]
    UnknownStatus { game: String, status: String },

    #[error("game {game:?}: a \"recommended\" entry is missing the required field {field:?}")]
    MissingField { game: String, field: String },

    #[error(
        "unrecognised game key {game:?} — GMM does not know this game, so the entry \
         would silently do nothing"
    )]
    UnknownGame { game: String },
}

/// A parsed, validated manifest.
///
/// Construct with [`parse`]. A value of this type is one the app can
/// apply: validation happens once, up front, for the whole document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    schema_version: u32,
    games: BTreeMap<String, Recommendation>,
}

impl Manifest {
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// What the manifest says about `game`.
    ///
    /// `None` means the game is **absent** from the file, which falls
    /// through to the compiled-in default. It is deliberately not the
    /// same as `Some(Recommendation::NoRecommendation { .. })`, which
    /// retracts that default. Feeding this straight into
    /// [`super::importer_origin::resolve`] preserves the distinction.
    pub fn recommendation_for(&self, game: GameCode) -> Option<Recommendation> {
        self.games.get(game.as_str()).cloned()
    }
}

/// The on-the-wire shape, kept separate from the app's internal types.
///
/// The manifest is hand-edited by a human in a pull request, so its
/// JSON is spelled for that reader rather than derived from
/// [`ImporterOrigin`]'s internal representation. Both the app and the
/// validator go through this one type, which is what makes "the
/// validator and the app agree" structural rather than aspirational.
#[derive(Debug, Deserialize)]
struct WireManifest {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    #[serde(default)]
    games: BTreeMap<String, serde_json::Value>,
}

/// Parse and validate a manifest document.
///
/// Validation is whole-document: any problem rejects the entire
/// manifest rather than skipping the offending entry. Per-entry
/// degradation was considered and rejected (#93) — it is partial
/// application of a document the build does not understand, and for a
/// file that reconfigures every install that is the wrong failure mode.
///
/// Needs no network and is deterministic; live resolution checks belong
/// on a schedule, not on the critical path of merging (#94).
pub fn parse(raw: &str) -> Result<Manifest, ManifestError> {
    read(raw, UnknownGames::Ignore)
}

/// [`parse`], plus the checks that only make sense at the review gate.
///
/// This is what the `validate-manifest` command runs. It is **strictly
/// stricter** than [`parse`], so a manifest it accepts is by
/// construction one the app can read — that is the anti-drift property
/// #111 asks for, held structurally rather than by convention.
///
/// The one extra check is that every game key is one GMM knows. A typo
/// like `grmi` would otherwise sit in the file doing nothing while the
/// game it was meant for silently kept its old default — invisible at
/// runtime, obvious at review time. It is a check the *validator* can
/// make and the *app* deliberately cannot; see [`UnknownGames`].
pub fn validate(raw: &str) -> Result<Manifest, ManifestError> {
    read(raw, UnknownGames::Reject)
}

/// What to do with a game key this build does not recognise.
///
/// The app and the validator answer differently, on purpose.
///
/// The app **ignores** them. The additive-only authoring rule exists so
/// already-shipped builds keep parsing this file; adding a seventh game
/// is a routine additive change, and if an old build dropped the whole
/// layer over a key naming a game it does not have, every existing user
/// would lose their recommendations the day that game landed — the
/// precise outcome the rule exists to prevent. This is not partial
/// application of a document the build cannot read (#93): the structure
/// is fully understood, the key simply names a game this build has no
/// slot for.
///
/// The validator **rejects** them, because at review time an
/// unrecognised key is overwhelmingly a typo rather than a future game,
/// and catching it is the entire point of a gate.
#[derive(Debug, Clone, Copy)]
enum UnknownGames {
    Ignore,
    Reject,
}

fn read(raw: &str, unknown_games: UnknownGames) -> Result<Manifest, ManifestError> {
    let wire: WireManifest = serde_json::from_str(raw).map_err(|e| ManifestError::InvalidJson {
        message: e.to_string(),
    })?;

    if wire.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(ManifestError::UnsupportedSchemaVersion {
            found: wire.schema_version,
            supported: SUPPORTED_SCHEMA_VERSION,
        });
    }

    let mut games = BTreeMap::new();
    for (game, entry) in wire.games {
        if !is_known_game(&game) {
            match unknown_games {
                UnknownGames::Reject => return Err(ManifestError::UnknownGame { game }),
                // Still parse the entry, so a malformed one is caught
                // rather than hidden behind an unrecognised key.
                UnknownGames::Ignore => {
                    parse_entry(&game, &entry)?;
                    continue;
                }
            }
        }
        let recommendation = parse_entry(&game, &entry)?;
        games.insert(game, recommendation);
    }

    Ok(Manifest {
        schema_version: wire.schema_version,
        games,
    })
}

fn is_known_game(game: &str) -> bool {
    GAME_PROFILES.iter().any(|p| p.code.as_str() == game)
}

fn parse_entry(game: &str, entry: &serde_json::Value) -> Result<Recommendation, ManifestError> {
    let status = required_str(game, entry, "status")?;

    match status {
        "recommended" => {
            let owner = required_str(game, entry, "owner")?;
            let repo = required_str(game, entry, "repo")?;
            let asset_pattern = required_str(game, entry, "assetPattern")?;
            Ok(Recommendation::Recommended(ImporterOrigin::github(
                owner,
                repo,
                asset_pattern,
            )))
        }
        "none" => Ok(Recommendation::NoRecommendation {
            // Optional by design: it earns its place when the state
            // would otherwise be surprising.
            reason: entry
                .get("reason")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        }),
        other => Err(ManifestError::UnknownStatus {
            game: game.to_string(),
            status: other.to_string(),
        }),
    }
}

fn required_str<'a>(
    game: &str,
    entry: &'a serde_json::Value,
    field: &str,
) -> Result<&'a str, ManifestError> {
    entry
        .get(field)
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ManifestError::MissingField {
            game: game.to_string(),
            field: field.to_string(),
        })
}
