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

    /// Build a GitHub release origin from what a user typed into the
    /// override control (#109).
    ///
    /// The one place an [`ImporterOrigin`] is constructed from
    /// unvalidated input — everywhere else it comes from the compiled-in
    /// defaults or from a manifest that has already been through
    /// [`crate::core::recommended_importers::parse`]. Every rejection
    /// names the offending field, because the user is looking at three
    /// boxes and one of them is the problem.
    ///
    /// Surrounding whitespace is trimmed rather than rejected: it is
    /// what a paste leaves behind, and the value goes into a URL where
    /// it would be a 404 with no explanation. The *asset pattern* is
    /// trimmed too but otherwise untouched — it is a regex, and its
    /// interior is the user's business.
    pub fn from_user_input(
        owner: &str,
        repo: &str,
        asset_pattern: &str,
    ) -> std::result::Result<Self, String> {
        let owner = owner.trim();
        let repo = repo.trim();
        let asset_pattern = asset_pattern.trim();

        if owner.is_empty() {
            return Err("Enter the GitHub owner the Model Importer is published under.".into());
        }
        if repo.is_empty() {
            return Err("Enter the GitHub repository the Model Importer is published in.".into());
        }
        if asset_pattern.is_empty() {
            return Err(
                r"Enter the asset pattern that picks the package out of a release, for example GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip."
                    .into(),
            );
        }
        // `owner/repo` pasted whole into one box would otherwise build a
        // slug of `owner/repo/repo`: a URL that 404s during an install
        // the user has already committed to, with nothing pointing back
        // at the typo.
        for (field, value) in [("owner", owner), ("repository", repo)] {
            if value.contains('/') {
                return Err(format!(
                    "The {field} must not contain a \"/\" — put the owner and \
                     the repository in their own boxes."
                ));
            }
        }
        // Compiled here, once, for the reason #123 gives for compiling
        // the manifest's patterns at parse time: a pattern that cannot
        // compile must fail while the user is looking at the box, not
        // later during an install.
        crate::core::importer::AssetPattern::new(asset_pattern)
            .map_err(|e| format!("The asset pattern is not a valid regular expression: {e}"))?;

        Ok(ImporterOrigin::github(owner, repo, asset_pattern))
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
    ///
    /// A struct variant rather than a newtype so it can carry the
    /// manifest entry's optional `reason` (#109). `manifest/README.md`
    /// has documented that field since the file was written and the
    /// committed manifest uses it, but the parser dropped it: the only
    /// `reason` that reached the app was the one on a retraction. The
    /// accept/decline prompt is where it earns its place — a trust
    /// prompt with no grounds to evaluate is one people dismiss on
    /// reflex.
    Recommended {
        origin: ImporterOrigin,
        reason: Option<String>,
    },
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

/// What the settings row for a game's user override actually held.
///
/// Three states, because a stored value that cannot be read back is not
/// the same thing as no stored value (#124). Collapsing them with
/// `.ok()` discarded the user's highest-precedence choice and dropped
/// the game to whatever layer 2 or layer 3 says — which, for anyone who
/// set an override *because* the default went bad, is GMM quietly
/// reinstating the package they moved away from.
///
/// Unreachable with today's serialisation, and deliberately modelled
/// anyway: ADR 0005 already specifies a local-zip variant of
/// [`ImporterOrigin`], and a user who downgrades past the build that
/// adds it lands here on their next launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredOverride {
    /// The user has set no override; the game follows layers 2 and 3.
    NotSet,
    /// The user's own choice, which outranks everything.
    Set(ImporterOrigin),
    /// Something is stored and this build cannot read it.
    Unreadable {
        /// The stored text, verbatim, so a maintainer can see what was
        /// written rather than only that something was.
        raw: String,
        /// The parser's complaint.
        error: String,
    },
}

impl StoredOverride {
    /// Decode a stored settings value. `None` is [`Self::NotSet`]; text
    /// that does not parse is [`Self::Unreadable`], never absence.
    pub fn decode(raw: Option<String>) -> Self {
        let Some(raw) = raw else {
            return StoredOverride::NotSet;
        };
        match serde_json::from_str(&raw) {
            Ok(origin) => StoredOverride::Set(origin),
            Err(e) => StoredOverride::Unreadable {
                raw,
                error: e.to_string(),
            },
        }
    }
}

/// Resolve a game's effective Importer Origin through ADR 0005's three
/// layers, highest first.
///
/// - `user_override` — layer 1. [`StoredOverride::NotSet`] means the
///   user has set none, so the game follows layers 2 and 3. An
///   [`StoredOverride::Unreadable`] stops here rather than falling
///   through (#124): the user made a choice, and answering a read
///   failure by silently applying GMM's own opinion instead is the
///   opposite of what layer 1 is for.
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
    user_override: &StoredOverride,
    recommendation: Option<&Recommendation>,
    compiled_default: Option<&ImporterOrigin>,
) -> OriginResolution {
    // Layer 1. The user's own choice always wins — including over a
    // retraction, which is what keeps a retracted game usable.
    match user_override {
        StoredOverride::Set(origin) => {
            return OriginResolution::InEffect {
                origin: origin.clone(),
                layer: OriginLayer::UserOverride,
            };
        }
        // Warn, never block (#97), and never demote. The game is
        // installable again the moment the user sets an origin — the
        // same recovery a retraction offers — whereas falling through
        // would install a package they had explicitly replaced and say
        // nothing about it.
        StoredOverride::Unreadable { .. } => {
            return OriginResolution::NoneInEffect {
                reason: Some(
                    "GMM could not read the Importer Origin saved for this game, \
                     so none is in effect."
                        .to_string(),
                ),
            };
        }
        StoredOverride::NotSet => {}
    }

    // Layer 2. A recommendation applies; a retraction stops here rather
    // than falling through to layer 3.
    match recommendation {
        Some(Recommendation::Recommended { origin, .. }) => {
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
    /// An origin was recorded and this build cannot read it back
    /// (#124).
    ///
    /// Its own variant rather than folding into [`Self::Unknown`],
    /// because the two make opposite claims about the user's machine.
    /// `Unknown` says *GMM never performed this install* — which is why
    /// #99 forbids backfilling it and forbids treating it as "not
    /// installed". This says GMM did perform it and can no longer say
    /// from where. Reporting the second as the first is GMM asserting
    /// something false about a machine it cannot see.
    Unreadable {
        /// The stored text, verbatim.
        raw: String,
        /// The parser's complaint.
        error: String,
    },
}

impl InstalledOrigin {
    /// Decode a stored settings value. Absent is [`Self::Unknown`] — a
    /// real state, not a missing value — and unparseable text is
    /// [`Self::Unreadable`], never either of the other two.
    pub fn decode(raw: Option<String>) -> Self {
        let Some(raw) = raw else {
            return InstalledOrigin::Unknown;
        };
        match serde_json::from_str(&raw) {
            Ok(origin) => InstalledOrigin::Known(origin),
            Err(e) => InstalledOrigin::Unreadable {
                raw,
                error: e.to_string(),
            },
        }
    }
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

    /// The Importer Origins the user has declined for this game, as a
    /// JSON array (#95 / #109).
    ///
    /// Per-game and keyed by origin, which is the scope #95 settled on:
    /// a game-wide suppression would silently strand the user a later
    /// recommendation is meant to rescue, and a version-scoped one would
    /// re-prompt several times a week.
    pub fn declined_origins(game: GameCode) -> String {
        format!("importer.{}.declined_origins", game.as_str())
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

/// What a move onto a new Importer Origin does to the state the
/// *previous* origin left behind (ADR 0005 / #110).
///
/// Two effects rather than one boolean, because the unknown case pulls
/// them apart — see [`change_effects`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangeEffects {
    /// Delete the game's Importer Pin.
    pub clears_pin: bool,
    /// Delete the game's recorded install — version *and* origin — so
    /// it reports as not installed for the new origin.
    pub invalidates_install: bool,
}

impl ChangeEffects {
    /// Nothing to undo: the game is already on this origin.
    pub const NOTHING: Self = ChangeEffects {
        clears_pin: false,
        invalidates_install: false,
    };

    /// `true` when this move touches anything at all.
    pub fn is_change(&self) -> bool {
        self.clears_pin || self.invalidates_install
    }
}

/// Decide what moving a game onto `next` invalidates, given the origin
/// its current install came from.
///
/// - **Same origin** (case-insensitively, per [`ImporterOrigin`]'s
///   `PartialEq`) — nothing. Re-applying an origin, including a
///   capitalisation fix, must not throw away a working install and the
///   user's pin.
/// - **A different known origin** — both. The pin holds a version
///   string taken against a package that is not this one, and version
///   schemes do not survive an origin change (one package is at v8.8.9,
///   another at v1.4.4). Carrying it across is the defect this exists
///   to prevent: [`crate::core::updates::compute_status`] gates on
///   *pinned as a boolean*, so a stale pin would suppress **every**
///   update for the new origin, indefinitely, while the user believed
///   they were current — #78's class of failure. The install goes too,
///   because the game directory still physically holds the previous
///   origin's package and a record that says otherwise lets the
///   database and the disk disagree.
/// - **[`InstalledOrigin::Unknown`]** — the pin only. Asymmetric on
///   purpose. GMM cannot compare a pin against an origin it does not
///   know, so keeping it is keeping a gate it cannot reason about; but
///   #99 rejected treating unknown as "not installed", and those users
///   hand-installed their importers precisely because GMM could not
///   help them. In practice this is invisible: an unknown origin means
///   no install GMM performed, so there is usually neither a pin nor a
///   recorded version to clear. It is visible for one real cohort —
///   installs made by GMM builds that predate origin tracking (#107),
///   which recorded a version but no origin.
pub fn change_effects(installed: &InstalledOrigin, next: &ImporterOrigin) -> ChangeEffects {
    match installed {
        InstalledOrigin::Known(current) if current == next => ChangeEffects::NOTHING,
        InstalledOrigin::Known(_) => ChangeEffects {
            clears_pin: true,
            invalidates_install: true,
        },
        // Same effects as `Unknown`, for the same reason and no other:
        // GMM cannot compare a pin against an origin it cannot read, so
        // the pin is a gate it can no longer reason about; but the
        // recorded *version* is still readable and still describes real
        // files in the game directory, so invalidating the install would
        // tell the user they have nothing installed when they do. The
        // variants stay distinct because what they claim about the
        // machine is opposite — see [`InstalledOrigin::Unreadable`].
        InstalledOrigin::Unknown | InstalledOrigin::Unreadable { .. } => ChangeEffects {
            clears_pin: true,
            invalidates_install: false,
        },
    }
}

/// Whether moving a game onto `next` is a change GMM can honestly
/// **propose**, as opposed to one that merely invalidates state.
///
/// Separate from [`change_effects`] on purpose (#125). The proposal
/// logic used to read *would this clear the pin?* as its signal for *is
/// there a change worth proposing?*, and those are two different
/// questions. [`InstalledOrigin::Unknown`] is exactly where they
/// diverge: an unknown origin always clears the pin — GMM cannot reason
/// about a pin taken against an origin it does not know — so under that
/// rule it always looked like a change.
///
/// `compiled_default` is the game's layer-3 origin, and it is compared
/// **by value rather than by which layer resolution came from**. The
/// layer was the old signal and it made correctness depend on manifest
/// contents: the committed manifest recommends origins byte-identical to
/// the compiled-in defaults, so once a manifest is cached — after the
/// first successful launch, i.e. always — those games resolve at the
/// manifest layer and a layer-keyed guard never fires.
pub fn is_worth_proposing(
    installed: &InstalledOrigin,
    next: &ImporterOrigin,
    compiled_default: Option<&ImporterOrigin>,
) -> bool {
    match installed {
        // A known origin that differs is a real move, whatever layer
        // proposed it — including a corrected compiled-in default.
        InstalledOrigin::Known(current) => current != next,
        // GMM does not know where this install came from, so the only
        // thing it can honestly propose is an origin it has actually
        // *decided* on. The default it has always shipped is the status
        // quo, not a decision, and proposing it would nag every
        // hand-installed setup on every launch — the proactive surfacing
        // of unknown origin that #99 rejects. A genuinely different
        // recommendation still reaches them; that is the one route by
        // which unknown becomes known.
        InstalledOrigin::Unknown => !matches!(compiled_default, Some(d) if d == next),
        // The comparison cannot be made at all (#124). "We could not
        // tell" must not be rendered as "yes, switch".
        InstalledOrigin::Unreadable { .. } => false,
    }
}

/// Which Importer Origin an ordinary Install / Update action acts on.
///
/// Four outcomes rather than an `Option<ImporterOrigin>`, because the
/// two that carry no origin make different claims and need different
/// messages, and because *which* of the two origins was chosen is the
/// thing this decision is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOrigin {
    /// The origin the recorded install came from. Update stays here.
    Installed(ImporterOrigin),
    /// GMM has no install of its own to preserve, so the resolved
    /// origin decides — which is exactly what a recommendation is for.
    Resolved {
        origin: ImporterOrigin,
        layer: OriginLayer,
    },
    /// No origin is in effect, so there is nothing to install from.
    /// Warn, never block (#97).
    NoneInEffect { reason: Option<String> },
    /// An install is recorded and GMM cannot read the origin it came
    /// from (#124). Not an origin, and deliberately not silently
    /// answered with a different one.
    InstalledUnreadable { raw: String, error: String },
}

/// Decide which Importer Origin an ordinary Install / Update acts on.
///
/// **A recommendation decides a *new* install; it never switches an
/// existing one** (#109). ADR 0005 read both ways — a three-layer
/// precedence *and* "the manifest proposes and never auto-applies" — and
/// as built, resolution drove every path including the ordinary Update
/// action. An existing install's origin now changes only when the user
/// accepts a proposal.
///
/// The risk is asymmetric, which is why the two halves differ:
///
/// - A **fresh** install has no game directory to damage and the user
///   has just clicked Install, so honouring the recommendation is both
///   safe and the entire point of the mechanism. Proposing even here was
///   rejected: it would make the manifest useless in the case where it
///   is safest, leaving the compiled-in defaults as the real source of
///   truth while the manifest pretended otherwise.
/// - An **existing** install is where silent substitution would rewrite
///   a game directory with a different maintainer's package, and ADR
///   0004's posture is that nothing reaches a game directory without a
///   click.
///
/// It removes an incoherence rather than adding a rule: #110 already
/// established that changing origin **invalidates the install and
/// requires a fresh one**, so an "update" across an origin change was
/// contradictory — the thing being updated is not the thing installed.
/// A secondary consequence is that comparing a version taken against
/// origin Y with the latest release of origin X, which produces a
/// meaningless `upstream_ahead`, can no longer arise.
///
/// **Retraction is unaffected** (#97) and is checked first. It only
/// *removes* GMM's own default and never installs anything, so a
/// recorded origin is not a licence to keep pulling releases from a
/// package GMM has withdrawn. Substituting a different origin is a
/// different act, and that is the one this governs.
pub fn origin_for_install(
    installed: &InstalledOrigin,
    resolution: &OriginResolution,
) -> InstallOrigin {
    let (origin, layer) = match resolution {
        OriginResolution::InEffect { origin, layer } => (origin, *layer),
        OriginResolution::NoneInEffect { reason } => {
            return InstallOrigin::NoneInEffect {
                reason: reason.clone(),
            }
        }
    };

    match installed {
        // Including when it equals the resolved origin: "stay where you
        // are" and "go where you already are" are the same instruction,
        // and case-insensitive origin equality means a capitalisation
        // fix upstream is not a different package either way.
        InstalledOrigin::Known(current) => InstallOrigin::Installed(current.clone()),
        InstalledOrigin::Unknown => InstallOrigin::Resolved {
            origin: origin.clone(),
            layer,
        },
        // "We could not tell" must not be rendered as "then use this
        // one" — that is precisely the switch this rule forbids,
        // performed on the one install GMM understands least. The
        // caller surfaces it; it never quietly becomes an origin.
        InstalledOrigin::Unreadable { raw, error } => InstallOrigin::InstalledUnreadable {
            raw: raw.clone(),
            error: error.clone(),
        },
    }
}

/// Whether GMM has an Importer Origin change to *propose* for a game,
/// and which origin it would propose.
///
/// The decision takes exactly two inputs — what resolves, and what is
/// installed. **The Importer Pin is deliberately not one of them.** A
/// pin means "don't move me to a newer build of *this* package"; it
/// says nothing about whether the package's source is still alive, so a
/// pinned game still gets told its origin is dead and can decline and
/// stay pinned. Withholding that does not protect the pinned user, it
/// only means they find out later with less room to react (#98).
///
/// The compiled-in default is not a proposal when the install's origin
/// is [`InstalledOrigin::Unknown`]: it is the status quo GMM has always
/// shipped, not something it has newly decided, and proposing it would
/// nag every hand-installed setup on every launch — exactly the
/// proactive surfacing of unknown origin that #99 rejects. A game whose
/// origin is *known* and differs from a corrected default is a real
/// proposal, which is why the exclusion is scoped to unknown rather than
/// to the origin alone. See [`is_worth_proposing`].
///
/// This answers "is there something to propose?". Whether the user has
/// already declined this particular origin, and how the proposal is
/// rendered, belong to the recommendation surface (#109).
pub fn pending_change<'a>(
    resolution: &'a OriginResolution,
    installed: &InstalledOrigin,
    compiled_default: Option<&ImporterOrigin>,
) -> Option<&'a ImporterOrigin> {
    let origin = match resolution {
        OriginResolution::InEffect { origin, .. } => origin,
        // Nothing is in force, so there is nothing to move onto. That
        // state has its own surface: a warning that the user must
        // supply an origin (#97).
        OriginResolution::NoneInEffect { .. } => return None,
    };

    is_worth_proposing(installed, origin, compiled_default).then_some(origin)
}

// ---------------------------------------------------------------------
// The recommendation surface (#109) — what the user sees and answers.
// ---------------------------------------------------------------------

/// [`StoredOverride`] as the UI sees it.
///
/// A separate type from the domain value so the IPC shape can be chosen
/// for a TypeScript reader without pinning the internal representation,
/// and so the three states survive the crossing. In particular
/// [`Self::Unreadable`] must not be flattened into [`Self::NotSet`] on
/// the way out: "the user set nothing" and "the user set something GMM
/// cannot read" ask for opposite things from the surface — an empty
/// editor versus an explanation and a way to replace the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum OverrideView {
    NotSet,
    Set(ImporterOrigin),
    Unreadable { raw: String, error: String },
}

impl From<&StoredOverride> for OverrideView {
    fn from(stored: &StoredOverride) -> Self {
        match stored {
            StoredOverride::NotSet => OverrideView::NotSet,
            StoredOverride::Set(origin) => OverrideView::Set(origin.clone()),
            StoredOverride::Unreadable { raw, error } => OverrideView::Unreadable {
                raw: raw.clone(),
                error: error.clone(),
            },
        }
    }
}

/// The Importer Origins a user has declined for one game.
///
/// Scoped **to the origin**, per #95: not to the game, which would
/// silently strand the one user who most needs a later fix, and not to
/// origin plus version, which would re-prompt on every upstream release
/// — GIMI shipped two on one day. Entry-scoped was rejected too: a
/// copy-edit to a `reason` string would re-nag everyone who declined.
///
/// Three states for the same reason [`StoredOverride`] has three (#124):
/// a stored value that cannot be read back is not the same thing as no
/// stored value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredDeclines {
    /// Nothing has been declined for this game.
    NotSet,
    /// The origins the user has declined, in the order they declined
    /// them.
    Set(Vec<ImporterOrigin>),
    /// Something is stored and this build cannot read it.
    Unreadable { raw: String, error: String },
}

impl StoredDeclines {
    /// Decode a stored settings value. `None` is [`Self::NotSet`]; text
    /// that does not parse is [`Self::Unreadable`], never absence and
    /// never an empty list.
    pub fn decode(raw: Option<String>) -> Self {
        let Some(raw) = raw else {
            return StoredDeclines::NotSet;
        };
        match serde_json::from_str::<Vec<ImporterOrigin>>(&raw) {
            Ok(origins) => StoredDeclines::Set(origins),
            Err(e) => StoredDeclines::Unreadable {
                raw,
                error: e.to_string(),
            },
        }
    }

    /// The declined origins, for the affordance that makes them visible
    /// and reversible. Empty for both of the non-`Set` states — a
    /// dismissal GMM cannot read is not a dismissal it can offer to
    /// undo, so it is reported through
    /// [`OriginStatus::dismissals_error`] instead of appearing here as a
    /// row that does nothing.
    pub fn origins(&self) -> &[ImporterOrigin] {
        match self {
            StoredDeclines::Set(origins) => origins,
            StoredDeclines::NotSet | StoredDeclines::Unreadable { .. } => &[],
        }
    }

    /// Whether a proposal of `origin` should stay quiet.
    ///
    /// [`Self::Unreadable`] answers **no**, and that is the deliberate
    /// direction. GMM cannot tell whether this proposal was declined, so
    /// it either shows a prompt the user may have already answered, or
    /// hides one they have not. A proposal applies nothing on its own —
    /// the cost of the first is a click, the cost of the second is a
    /// user stranded on a dead importer with the fix silenced by a
    /// corrupt row. Declining again also rewrites the row, so the state
    /// heals rather than persisting.
    pub fn suppresses(&self, origin: &ImporterOrigin) -> bool {
        self.origins().iter().any(|o| o == origin)
    }

    /// The list to store after declining `origin`. Idempotent: declining
    /// twice records one dismissal.
    ///
    /// From [`Self::Unreadable`] this starts a fresh list rather than
    /// merging, because there is nothing readable to merge with. That is
    /// the healing path named in [`Self::suppresses`].
    pub fn with(&self, origin: &ImporterOrigin) -> Vec<ImporterOrigin> {
        let mut next = self.origins().to_vec();
        if !next.iter().any(|o| o == origin) {
            next.push(origin.clone());
        }
        next
    }

    /// The list to store after undoing the dismissal of `origin`.
    pub fn without(&self, origin: &ImporterOrigin) -> Vec<ImporterOrigin> {
        self.origins()
            .iter()
            .filter(|o| *o != origin)
            .cloned()
            .collect()
    }

    /// The read failure to surface, or `None` when there was none.
    pub fn error(&self) -> Option<String> {
        match self {
            StoredDeclines::Unreadable { error, .. } => Some(error.clone()),
            _ => None,
        }
    }
}

/// An Importer Origin change GMM is offering, and the grounds for it.
///
/// Answering it is the *only* way an existing install's origin changes
/// (#109), and accepting it is the only way an unknown origin becomes
/// known (#99).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginProposal {
    /// The origin the user is being offered.
    pub origin: ImporterOrigin,
    /// The manifest entry's optional explanation. Present only when the
    /// proposal comes from a recommendation that wrote one down — it is
    /// the difference between a trust prompt someone can evaluate and
    /// one they dismiss on reflex.
    pub reason: Option<String>,
    /// What accepting replaces. The prompt has to say plainly what it
    /// will do: a user with an unknown-origin install who accepts gets
    /// their game directory rewritten, and that is the accepted cost.
    pub replaces: InstalledOrigin,
}

/// Everything one game's Importer Origin surface needs, in one read.
///
/// One aggregate rather than six commands because these values are only
/// meaningful together: a resolved origin without its layer cannot be
/// explained, a proposal without the dismissal state cannot be rendered,
/// and a dismissal list without the global switch would offer to undo
/// something that is switched off entirely.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginStatus {
    pub game: crate::core::games::GameCode,
    pub display_name: String,
    /// Which origin is in effect and which layer supplied it — or that
    /// none is, with the reason to show the user. Never a bare
    /// `Option`: see [`OriginResolution`].
    pub resolved: OriginResolution,
    /// Which origin an ordinary Install / Update would act on. Differs
    /// from `resolved` exactly when a recommendation is proposing a
    /// change that has not been accepted (#109).
    pub install_target: InstallTargetView,
    /// The origin the recorded install came from, `unknown` included.
    pub installed: InstalledOrigin,
    pub user_override: OverrideView,
    /// The compiled-in default, so the surface can say what clearing an
    /// override falls back to.
    pub compiled_default: Option<ImporterOrigin>,
    /// The change GMM is offering, if any and if not dismissed.
    pub proposal: Option<OriginProposal>,
    /// Origins the user has declined for this game, so the dismissal is
    /// visible and reversible where the user is looking at the affected
    /// game. Empty while recommendations are switched off — the whole
    /// layer is gone, not just its fetch.
    pub dismissed: Vec<ImporterOrigin>,
    /// Set when GMM holds dismissal state it cannot read. Surfaced
    /// rather than swallowed: silently reading it as "nothing was
    /// declined" is the benign-looking value this codebase has shipped
    /// three times.
    pub dismissals_error: Option<String>,
    pub recommendations_enabled: bool,
    /// Why the last manifest GMM fetched could not be used, when that
    /// happened. `None` while recommendations are off.
    pub recommendations_unusable_reason: Option<String>,
}

/// [`InstallOrigin`] as the UI sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum InstallTargetView {
    /// Update stays on the origin the install came from.
    Installed(ImporterOrigin),
    /// Nothing is installed, so the resolved origin decides.
    Resolved {
        origin: ImporterOrigin,
        layer: OriginLayer,
    },
    NoneInEffect {
        reason: Option<String>,
    },
    InstalledUnreadable {
        raw: String,
        error: String,
    },
}

impl From<&InstallOrigin> for InstallTargetView {
    fn from(target: &InstallOrigin) -> Self {
        match target {
            InstallOrigin::Installed(origin) => InstallTargetView::Installed(origin.clone()),
            InstallOrigin::Resolved { origin, layer } => InstallTargetView::Resolved {
                origin: origin.clone(),
                layer: *layer,
            },
            InstallOrigin::NoneInEffect { reason } => InstallTargetView::NoneInEffect {
                reason: reason.clone(),
            },
            InstallOrigin::InstalledUnreadable { raw, error } => {
                InstallTargetView::InstalledUnreadable {
                    raw: raw.clone(),
                    error: error.clone(),
                }
            }
        }
    }
}
