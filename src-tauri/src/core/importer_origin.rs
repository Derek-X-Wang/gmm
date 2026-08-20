//! Importer Origin — where a Game's Model Importer comes from.
//!
//! ADR 0005 makes this a first-class per-game value. GMM never
//! maintains, forks, hosts or mirrors Model Importer packages; what it
//! owns is the *answer to which package a game should use*, and the
//! user's ability to override that answer.
//!
//! An origin resolves through three layers, highest first:
//!
//! 1. the user's per-game override
//! 2. GMM's recommended manifest ([`Recommendation`]) — fetched and
//!    cached elsewhere; this module only consumes it
//! 3. the compiled-in default from [`crate::core::games::GAME_PROFILES`]
//!
//! See [`resolve`] for the precedence rules, including the one that is
//! easy to get wrong: a manifest entry of *no recommendation*
//! **retracts** the compiled-in default rather than falling through to
//! it.

use serde::{Deserialize, Serialize};

/// A GitHub release origin: an `owner`/`repo` pair plus the
/// release-asset match that picks one asset out of a release.
///
/// The asset-matching field's form is inherited from #79 — it is the
/// same anchored-regex source string that
/// [`crate::core::importer::AssetPattern`] compiles. There is
/// deliberately only one asset-matching rule in GMM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubOrigin {
    owner: String,
    repo: String,
    asset_pattern: String,
}

/// Where a Game's Model Importer comes from.
///
/// Only [`ImporterOrigin::GitHubRelease`] exists today. The enum is the
/// shape rather than a bare struct because ADR 0005's concept includes
/// a user-supplied local zip, whose update-check and Importer Pin
/// behaviour is not yet settled (#92). Callers must match, so adding
/// that variant is a compiler-guided change rather than a rework.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ImporterOrigin {
    GitHubRelease(GitHubOrigin),
}

impl ImporterOrigin {
    /// Build a GitHub release origin. Spelling is preserved verbatim —
    /// case folding happens in comparison only, never in the stored
    /// value, because GMM displays the origin and puts it in a URL.
    pub fn github(
        owner: impl Into<String>,
        repo: impl Into<String>,
        asset_pattern: impl Into<String>,
    ) -> Self {
        ImporterOrigin::GitHubRelease(GitHubOrigin {
            owner: owner.into(),
            repo: repo.into(),
            asset_pattern: asset_pattern.into(),
        })
    }

    pub fn owner(&self) -> &str {
        match self {
            ImporterOrigin::GitHubRelease(o) => &o.owner,
        }
    }

    pub fn repo(&self) -> &str {
        match self {
            ImporterOrigin::GitHubRelease(o) => &o.repo,
        }
    }

    /// The `owner/repo` form the GitHub API and the existing importer
    /// call sites take.
    pub fn repo_slug(&self) -> String {
        match self {
            ImporterOrigin::GitHubRelease(o) => format!("{}/{}", o.owner, o.repo),
        }
    }

    /// The asset-matching source string, for
    /// [`crate::core::importer::AssetPattern::new`].
    pub fn asset_pattern(&self) -> &str {
        match self {
            ImporterOrigin::GitHubRelease(o) => &o.asset_pattern,
        }
    }
}

/// Origin equality is **case-insensitive on `owner` and `repo`** and
/// exact on the asset pattern.
///
/// GitHub treats `silentnightsound/GIMI-Package` and
/// `SilentNightSound/GIMI-Package` as the same repository, and ADR 0005
/// makes origin equality load-bearing in three places — the decline
/// key, the pin-clearing trigger, and install bookkeeping. Comparing
/// those two spellings as different origins would let a capitalisation
/// fix in the manifest re-prompt every user who had already declined.
///
/// The asset pattern is deliberately *not* case-folded: it is a regex,
/// not a GitHub identifier, so `PACKAGE` and `package` select different
/// files. Two origins on the same repository that select different
/// assets install different bytes and are not interchangeable.
impl PartialEq for ImporterOrigin {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ImporterOrigin::GitHubRelease(a), ImporterOrigin::GitHubRelease(b)) => {
                a.owner.eq_ignore_ascii_case(&b.owner)
                    && a.repo.eq_ignore_ascii_case(&b.repo)
                    && a.asset_pattern == b.asset_pattern
            }
        }
    }
}

impl Eq for ImporterOrigin {}

/// What GMM's curated manifest says about **one** game.
///
/// This is layer 2's per-game input. Fetching, parsing and caching the
/// manifest happens elsewhere (#108); this module only consumes the
/// verdict, which keeps the precedence rules testable without a
/// network.
///
/// Note what is *not* here: "the game is absent from the manifest" and
/// "there is no manifest layer at all" are both expressed by passing
/// `None` to [`resolve`], never by a variant. Absence must fall through
/// to the compiled-in default, and collapsing it into
/// [`Recommendation::NoRecommendation`] would let one malformed commit
/// retract every default for every user (#93).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum Recommendation {
    /// GMM recommends this origin for the game.
    Recommended(ImporterOrigin),
    /// GMM has no recommendation. This **retracts** the compiled-in
    /// default: no origin is in effect until the user supplies one.
    #[serde(rename = "none")]
    NoRecommendation {
        /// Short human-readable explanation shown to the user. Optional
        /// by design — it earns its place when the state would
        /// otherwise be surprising.
        reason: Option<String>,
    },
}

/// Which precedence layer supplied the origin that is in effect.
///
/// Carried so the UI can explain *why* a game points where it does —
/// "your override" and "GMM's recommendation" are different things to
/// a user looking at the same repository name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OriginLayer {
    UserOverride,
    RecommendedManifest,
    CompiledInDefault,
}

/// The outcome of resolving a game's Importer Origin.
///
/// Deliberately **not** an `Option<ImporterOrigin>`. ADR 0005 requires
/// that "no origin is in effect" never share a representation with
/// "the user has set no override" (which is an input, an
/// `Option<&ImporterOrigin>`) or with "this install's origin is
/// unknown" (which is [`InstalledOrigin::Unknown`], a different type
/// entirely). Three distinct concepts, three distinct types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum OriginResolution {
    /// An origin is in effect; install and update-check use it.
    InEffect {
        origin: ImporterOrigin,
        layer: OriginLayer,
    },
    /// No origin is in effect — layer 2 retracted layer 3 and the user
    /// has supplied nothing (or the game was never wired at all).
    ///
    /// **Not an error and not "not installed".** GMM warns and never
    /// blocks (#97): the user is told they must supply an origin and
    /// can still proceed from a source of their own choosing.
    NoneInEffect { reason: Option<String> },
}

impl OriginResolution {
    /// The origin in effect, if there is one. Convenience for call
    /// sites that only need the happy path; the `NoneInEffect` arm
    /// still has to be handled to produce the warning.
    pub fn origin(&self) -> Option<&ImporterOrigin> {
        match self {
            OriginResolution::InEffect { origin, .. } => Some(origin),
            OriginResolution::NoneInEffect { .. } => None,
        }
    }
}

/// Resolve a game's effective Importer Origin through ADR 0005's three
/// layers, highest first.
///
/// - `user_override` — layer 1. `None` means the user has set none, so
///   the game follows layers 2 and 3.
/// - `recommendation` — layer 2, for this game. `None` means the game
///   is absent from the manifest *or* there is no manifest layer at all
///   (not fetched yet, fetch failed, user switched recommendations off,
///   or the file was unusable). All of those fall through; none of them
///   retracts. Only an explicit [`Recommendation::NoRecommendation`]
///   retracts.
/// - `compiled_default` — layer 3, from the game's
///   [`crate::core::games::GameProfile`]. `None` for a game that has
///   not been wired.
pub fn resolve(
    user_override: Option<&ImporterOrigin>,
    recommendation: Option<&Recommendation>,
    compiled_default: Option<&ImporterOrigin>,
) -> OriginResolution {
    // Layer 1. The user's own choice always wins — including over a
    // retraction, which is what keeps a retracted game usable.
    if let Some(origin) = user_override {
        return OriginResolution::InEffect {
            origin: origin.clone(),
            layer: OriginLayer::UserOverride,
        };
    }

    // Layer 2. A recommendation applies; a retraction stops here rather
    // than falling through to layer 3.
    match recommendation {
        Some(Recommendation::Recommended(origin)) => {
            return OriginResolution::InEffect {
                origin: origin.clone(),
                layer: OriginLayer::RecommendedManifest,
            };
        }
        Some(Recommendation::NoRecommendation { reason }) => {
            return OriginResolution::NoneInEffect {
                reason: reason.clone(),
            };
        }
        // Absent from the manifest, or no manifest layer at all.
        None => {}
    }

    // Layer 3.
    match compiled_default {
        Some(origin) => OriginResolution::InEffect {
            origin: origin.clone(),
            layer: OriginLayer::CompiledInDefault,
        },
        None => OriginResolution::NoneInEffect { reason: None },
    }
}

/// The Importer Origin an install was performed from.
///
/// A separate type from `Option<ImporterOrigin>` on purpose. ADR 0005
/// makes **unknown** a first-class value, and #99 spells out why it must
/// not be conflated with anything else:
///
/// - It is never backfilled to the compiled-in default. Before #77
///   three of the six defaults did not exist, so a user's GIMI install
///   provably did not come from them — writing that would be recording a
///   fiction that later decisions (pin clearing, install bookkeeping)
///   would then trust.
/// - It is never treated as "not installed". Those users hand-installed
///   their importers precisely because GMM could not help them; GMM's
///   bookkeeping does not get to invalidate a working setup.
/// - It becomes known only through an actual install.
/// - It is never surfaced proactively — it is noise the user cannot act
///   on, and it would fire for every hand-installed setup on every
///   launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum InstalledOrigin {
    Known(ImporterOrigin),
    Unknown,
}

/// Canonical settings keys for Importer Origin state.
///
/// Both live in the existing key/value `settings` table, so no
/// migration is involved — the same way `importer.<game>.installed_version`
/// and `importer.<game>.pinned_version` were added.
pub mod keys {
    use crate::core::games::GameCode;

    /// The user's per-game override (layer 1). Absent or NULL means the
    /// user has set none, so the game follows layers 2 and 3.
    pub fn origin_override(game: GameCode) -> String {
        format!("importer.{}.origin_override", game.as_str())
    }

    /// The origin an install was performed from. Absent means
    /// [`super::InstalledOrigin::Unknown`] — which is a real state, not
    /// a missing value to be filled in.
    pub fn installed_origin(game: GameCode) -> String {
        format!("importer.{}.installed_origin", game.as_str())
    }
}

/// The compiled-in default origin for a game (layer 3), read from its
/// [`crate::core::games::GameProfile`]. `None` for a game whose port has
/// not landed.
pub fn compiled_in_default(game: crate::core::games::GameCode) -> Option<ImporterOrigin> {
    let (repo_slug, asset_pattern) = game.profile().importer_repo?;
    let (owner, repo) = repo_slug.split_once('/')?;
    Some(ImporterOrigin::github(owner, repo, asset_pattern))
}
