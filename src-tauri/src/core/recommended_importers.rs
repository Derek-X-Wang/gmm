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

/// Process-local override used by the packaged startup smoke to hold a
/// refresh request open and prove startup does not wait for the network.
/// Ordinary launches leave it unset and always use [`MANIFEST_URL`]. The
/// value is accepted only after [`loopback_manifest_url_override`] proves it
/// names a numeric loopback host, so release builds cannot be redirected to
/// another internet origin by their environment.
pub const MANIFEST_URL_OVERRIDE_ENV: &str = "GMM_RECOMMENDED_IMPORTERS_URL";

/// Read the packaged-smoke URL seam and accept it only for HTTP(S) URLs whose
/// host is a numeric loopback address. Keeping the environment read inside
/// this accessor prevents callers from accidentally bypassing validation.
pub fn loopback_manifest_url_override() -> Option<String> {
    validate_loopback_manifest_url_override(std::env::var(MANIFEST_URL_OVERRIDE_ENV).ok())
}

/// Requiring an address rather than a hostname avoids trusting host-file or
/// DNS resolution for this release-build seam.
fn validate_loopback_manifest_url_override(candidate: Option<String>) -> Option<String> {
    let candidate = candidate?;
    let url = reqwest::Url::parse(&candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?;
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host)
        .parse::<std::net::IpAddr>()
        .ok()?;
    host.is_loopback().then_some(candidate)
}

#[cfg(test)]
mod loopback_override_tests {
    use super::{validate_loopback_manifest_url_override, MANIFEST_URL};

    #[test]
    fn packaged_smoke_override_cannot_redirect_releases_to_the_internet() {
        for loopback in [
            "http://127.0.0.1:48123/recommended-importers.json",
            "http://[::1]:48123/recommended-importers.json",
        ] {
            assert_eq!(
                validate_loopback_manifest_url_override(Some(loopback.to_string())).as_deref(),
                Some(loopback),
            );
        }

        for rejected in [
            MANIFEST_URL,
            "http://localhost:48123/recommended-importers.json",
            "file:///tmp/recommended-importers.json",
            "not a URL",
        ] {
            assert_eq!(
                validate_loopback_manifest_url_override(Some(rejected.to_string())),
                None,
                "the release-build seam must reject {rejected}",
            );
        }
        assert_eq!(validate_loopback_manifest_url_override(None), None);
    }
}

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

    #[error(
        "manifest has no \"games\" key. An empty recommendation set is written \
         \"games\": {{}}; omitting or mistyping the key would otherwise clear every \
         user's recommendations from a single keystroke"
    )]
    MissingGames,

    #[error(
        "game {game:?}: assetPattern {pattern:?} is not a valid regular expression: \
         {message}"
    )]
    InvalidAssetPattern {
        game: String,
        pattern: String,
        message: String,
    },
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
    /// Required, and deliberately an `Option` rather than a serde
    /// default (#123). With a default, `{"schemaVersion": 1}` — or one
    /// mistyped key, `game` for `games` — parsed as a perfectly valid
    /// manifest that recommends nothing, replaced the cache, and
    /// silently emptied every user's recommendations with no error
    /// anywhere. An empty set is still expressible; it just has to be
    /// written down as `"games": {}`.
    ///
    /// Unknown *other* top-level fields stay permitted, on purpose:
    /// `deny_unknown_fields` here would break the additive-only rule
    /// the whole schema rests on, since a field added by a later author
    /// would then drop the layer for every already-shipped build.
    games: Option<BTreeMap<String, serde_json::Value>>,
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

    let wire_games = wire.games.ok_or(ManifestError::MissingGames)?;

    let mut games = BTreeMap::new();
    for (game, entry) in wire_games {
        if !is_known_game(&game) {
            match unknown_games {
                UnknownGames::Reject => return Err(ManifestError::UnknownGame { game }),
                // Skipped **whole**, without looking inside (#123). The
                // entry used to be parsed first "so a malformed one is
                // caught rather than hidden", which defeated the branch's
                // only purpose: a seventh game arrives with whatever
                // fields and status values its own schema needs, and
                // validating those against this build's vocabulary drops
                // the entire layer for every shipped build on the day
                // that game lands. The validator still rejects the key,
                // which is where a typo is meant to be caught.
                UnknownGames::Ignore => continue,
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
            // Compile it here, once, rather than at the point of use.
            // An uncompilable pattern was previously cached and applied,
            // failing later for that one game while every other entry
            // stayed active — a per-entry failure smuggled past a
            // whole-document contract (#123). Compiled through
            // `AssetPattern` so what is checked is the anchored form GMM
            // will actually run (#79), not a looser reading of it.
            super::importer::AssetPattern::new(asset_pattern).map_err(|e| {
                ManifestError::InvalidAssetPattern {
                    game: game.to_string(),
                    pattern: asset_pattern.to_string(),
                    message: e.to_string(),
                }
            })?;
            Ok(Recommendation::Recommended {
                origin: ImporterOrigin::github(owner, repo, asset_pattern),
                // Optional, and documented as optional in
                // `manifest/README.md` since the file was written. It was
                // read only for a `none` entry until #109, so a
                // maintainer could write one on a recommendation and
                // nothing would ever show it.
                reason: optional_str(entry, "reason"),
            })
        }
        "none" => Ok(Recommendation::NoRecommendation {
            // Optional by design: it earns its place when the state
            // would otherwise be surprising.
            reason: optional_str(entry, "reason"),
        }),
        other => Err(ManifestError::UnknownStatus {
            game: game.to_string(),
            status: other.to_string(),
        }),
    }
}

/// An optional free-text field. Absent, non-string and blank all read
/// as "not written down" — a `reason` of `""` explains nothing, and
/// rendering an empty line in the prompt is worse than rendering none.
fn optional_str(entry: &serde_json::Value, field: &str) -> Option<String> {
    entry
        .get(field)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
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

// ---------------------------------------------------------------------
// Fetch + cache (#108) — layer 2's supply side.
//
// The rule everything below serves: **a fetch error, an unusable
// manifest, an explicit `none` and a game absent from the file are four
// distinct conditions with four different behaviours** (#96). The Loader
// update check collapsed a failure into a success value with
// `.ok().flatten()` and reported "up to date" for the entire life of the
// feature (#78); nothing on this path may do the same.
// ---------------------------------------------------------------------

use std::time::Duration;

/// How long one refresh may spend before GMM gives up on it.
///
/// Nothing waits on the refresh, so this is not about responsiveness —
/// it is so a host that accepts the connection and then says nothing
/// cannot leave a task and a connection alive for the rest of the
/// session.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

/// The packaged startup smoke holds its numeric-loopback response open while
/// waiting for IPC readiness. Keep this longer than the smoke's 90-second
/// startup deadline so a client timeout cannot release a network-blocked
/// startup and make the guard pass. This affects only the validated loopback
/// override; shipped manifest refreshes retain [`FETCH_TIMEOUT`].
pub const PACKAGED_SMOKE_FETCH_TIMEOUT: Duration = Duration::from_secs(120);

/// Settings keys holding the cached manifest.
///
/// The cache lives in the existing key/value `settings` table, so it
/// survives a restart with no migration. ADR 0005 leaves the physical
/// location to the implementer; what it fixes is that the last
/// successfully fetched manifest stays authoritative until replaced.
pub mod cache_keys {
    /// The last manifest GMM fetched **and could read**, stored raw.
    ///
    /// Raw rather than the parsed form on purpose: the document is the
    /// artefact GMM was given, and re-parsing on read keeps one parser
    /// rather than a parser plus a serialiser that can disagree.
    pub const DOCUMENT: &str = "importer.recommendations.cached_manifest";

    /// The `ETag` that came with [`DOCUMENT`], so a refresh usually
    /// costs a 304 rather than a download.
    pub const ETAG: &str = "importer.recommendations.etag";

    /// The global recommendations switch (#95 / #109). `"false"`
    /// removes the whole layer; absent means **on**, because this is an
    /// opt-out rather than an opt-in and the users the layer exists to
    /// rescue are exactly the ones who will never find a switch.
    ///
    /// It sits beside the cache rather than in a per-game key because
    /// it is a standing preference about GMM's curation as a whole, and
    /// deliberately distinct from a decline, which is a judgement about
    /// one proposal.
    pub const ENABLED: &str = "importer.recommendations.enabled";

    /// Why the **last** fetch produced a document this build cannot
    /// read, or absent when it did not.
    ///
    /// This key is what keeps "unusable" from going silent. The layer
    /// itself falls through on an unusable document (never retracts),
    /// which on its own would be indistinguishable from "no manifest
    /// yet" — so the reason is recorded separately for the surface that
    /// tells the user their build is too old.
    pub const UNUSABLE_REASON: &str = "importer.recommendations.unusable_reason";
}

/// What one HTTP attempt produced. Transport only — whether the bytes
/// are a manifest this build can read is a separate question, answered
/// by [`parse`], because collapsing the two is how "we could not ask"
/// becomes "the answer is nothing".
#[derive(Debug)]
pub enum Fetched {
    /// A document arrived. Nothing has been said about its contents yet.
    Document { raw: String, etag: Option<String> },
    /// 304 Not Modified: the cached document is still current.
    ///
    /// Only produced when an `ETag` was actually sent — [`fetch`]
    /// enforces that rather than trusting the caller, because a 304
    /// with nothing to revalidate confirms nothing (#123).
    NotModified,
    /// No document arrived — DNS, proxy, TLS, timeout, a 5xx. Says
    /// nothing whatsoever about what GMM recommends.
    Unreachable(String),
}

/// GET the manifest, conditionally when `etag` is known.
///
/// Never returns `Err`: an unreachable host is an expected, benign
/// outcome for a background refresh, and modelling it as a variant
/// rather than an error is what stops a caller from `?`-ing it into
/// something that looks like "no recommendations".
///
/// The caller must build `client` from
/// [`crate::core::Core::http_client_builder`] so the request honours the
/// user's proxy configuration, like every other network call in the app.
pub async fn fetch(client: &reqwest::Client, url: &str, etag: Option<&str>) -> Fetched {
    let mut req = client.get(url);
    if let Some(tag) = etag {
        req = req.header("If-None-Match", tag);
    }
    let res = match req.send().await {
        Ok(res) => res,
        Err(e) => return Fetched::Unreachable(format!("GET {url}: {e}")),
    };

    if res.status().as_u16() == 304 {
        return match etag {
            Some(_) => Fetched::NotModified,
            // Impossible by contract — GMM sends `If-None-Match` only
            // when it holds a document to revalidate — so in practice a
            // misbehaving proxy on a first launch. There is no cache
            // this can confirm, and calling it `NotModified` would say
            // the cache is current when there is none: "we could not
            // ask" rendered as "the answer is unchanged", which is the
            // #78 collapse (#123).
            None => Fetched::Unreachable(format!(
                "GET {url} returned 304 Not Modified, but GMM sent no ETag and \
                 holds no cached manifest to revalidate"
            )),
        };
    }
    if !res.status().is_success() {
        return Fetched::Unreachable(format!("GET {url} returned {}", res.status()));
    }

    let etag = res
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    match res.text().await {
        Ok(raw) => Fetched::Document { raw, etag },
        // A body that dies mid-stream is a transport failure, not a
        // malformed manifest: GMM never saw the whole document.
        Err(e) => Fetched::Unreachable(format!("read body of {url}: {e}")),
    }
}

/// What one refresh did to the cache. Five conditions, five values.
#[derive(Debug)]
pub enum Refreshed {
    /// The user has switched recommendations off, so no request was
    /// made (#95). Its own variant because "we did not ask" is not a
    /// network outcome: reporting it as [`Self::Unreachable`] would put
    /// a fabricated transport failure in the log for a decision the
    /// user made deliberately, and reporting it as
    /// [`Self::NotModified`] would claim a cache is current that GMM
    /// was told not to consult.
    Disabled,
    /// A readable manifest arrived and is now the cache.
    Replaced(Manifest),
    /// Upstream confirmed the cached document is still current.
    NotModified,
    /// A document arrived that this build cannot read. The cache is
    /// **left alone** — it is authoritative until *replaced*, and an
    /// unreadable document replaces nothing. With no cache the layer is
    /// simply absent, which falls through to the compiled-in defaults
    /// and never retracts them.
    Unusable(ManifestError),
    /// No document arrived. The cache is left alone and still correct.
    Unreachable(String),
}
