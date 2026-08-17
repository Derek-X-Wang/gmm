# 0005 — Importer Origin: GMM is a conduit for Model Importers, not a maintainer

Date: 2026-08-17
Status: Accepted

## Context

Every supported game depends on a Model Importer package published by a third party. GMM hardcodes one repository per game and installs from its GitHub releases. When that package goes stale or breaks, the user has no move inside GMM: their game does not work and the only path forward is waiting on an author who may never return.

HIMI is the live example. `leotorrez/HIMI-Package` last released 2025-07-24 — 13 months before this ADR — while the other five packages saw releases between three days and four and a half months ago. A user modding Honkai Impact 3rd today is dependent on a repository that shows no sign of moving, and GMM offers them nothing.

The question this started as was "should GMM maintain Model Importers in its own repo?" A Model Importer is not a build artifact — `GIMI-PACKAGE-v8.8.9.zip` is 67 files of `d3dx.ini`, `Core/`, `ShaderFixes/`, HLSL and markdown, with the DLLs shipping separately in the Loader package (ADR 0001). Fixing a broken importer is a text edit, not a compile. So maintaining them is technically within reach, which is exactly why the question needed a real answer rather than a shrug.

The answer is **no**, for four reasons:

- **Ops burden.** Six packages across six live-service games, each rebroken by every game patch. That is a permanent second job, and GMM's own surface — Library, junctions, GameBanana ingest — is the reason the project exists.
- **Ban-wave liability.** ADR 0004 exists because publishers fingerprint importer signatures and have run ban-waves against them. A GMM-authored importer would make GMM the author of the artefact whose signature gets a user's account banned. GMM's posture is to notify and let the user apply; owning the payload inverts that.
- **It puts GMM in the ecosystem's way.** The importer authors are the ecosystem. A mod manager that forks its own competing packages fragments the thing it depends on.
- **For HIMI specifically it is moot.** `leotorrez/HIMI-Package` carries **no license at all** — all rights reserved. The other five are GPL-3.0. GMM cannot legally redistribute or fork the one package that is actually stale.

So the problem has to be solved without GMM taking ownership of any package. What GMM can own is the *answer to which package a game should use* — and the ability for a user to override that answer themselves.

The seven charting tickets (#93–#99) settled the mechanism. This ADR records it.

## Considered alternatives

- **Fork all six packages into GMM.** Solves staleness completely and permanently. Rejected for the four reasons above, decisively by the HIMI licence: the one package that needs it cannot be forked.
- **Mirror the six packages for availability.** No maintenance, only redistribution, so a deleted or renamed upstream repo stops breaking GMM. Rejected: availability is not the failure mode. Every one of the six repos is reachable; the problem is that one of them has stopped changing. A mirror preserves a stale package perfectly. It also runs into the same HIMI licence wall and creates a supply-chain surface (users trusting GMM-hosted zips) for a problem it does not solve.
- **Conditional adoption — GMM takes over a package only once it goes stale.** Appealing because it scopes the burden to the actual failures. Rejected: "stale" has no honest definition. A finished importer and an abandoned one look identical from the outside; adoption would trigger on a quiet month and GMM would have committed to a package it cannot hand back. And it is the full maintenance burden, merely deferred and made unpredictable — the worst shape for a solo-maintained project.
- **Conduit plus override.** GMM maintains no packages. It gains the ability to point a game at a *different* published package, plus a curated recommendation of which one that should be. **Chosen.** It solves the actual failure — the user is stuck on GMM's hardcoded choice — at the cost of one small manifest and a settings surface, carries no licence exposure, keeps the packages and their signatures in their authors' hands, and degrades gracefully: even with GMM's curation switched off, the user's own override still rescues them.

An **arbitrary download URL** as the override was also considered and rejected while charting. It costs the version string, which silently disables both Importer Pin and the update badge — GMM would be handing out a download it can say nothing about.

## Decision

GMM never maintains, forks, hosts, or mirrors Model Importer packages. Instead:

**Importer Origin becomes a first-class per-game concept** — where a game's Model Importer comes from. It is either a GitHub release origin (an `owner`/`repo` pair plus a release-asset match) or a user-supplied local zip. It is not an arbitrary URL.

**Three layers of precedence: user override → recommended manifest → compiled-in defaults.** The user's own choice always wins. The compiled-in defaults are the offline and bootstrap fallback — effectively the last shipped snapshot of the manifest — not a separate authority.

**A `recommended-importers.json` manifest lives on `main` in this repository and is fetched at runtime as raw content.** Not a release asset: self-healing is the entire value, and a release asset only reaches users who update GMM, excluding the very people stranded on a dead importer. `main` was chosen over a dedicated branch, a separate repo, or an editable release asset because the review gate this file most needs already exists there — a PR with a green `check`, with `enforce_admins: true`, so no one including the maintainer can change it unreviewed.

**The manifest proposes and never auto-applies.** ADR 0004 requires it: GMM notifies, the user applies, nothing reaches a game directory without an explicit click.

**Entries are tagged, keyed by game, and a `none` status retracts the compiled-in default.** Three states must never collapse into each other: an origin is recommended; there is explicitly no recommendation, which *clears* layer 3 so no origin is in effect until the user supplies one; and the game is simply absent from the file, which falls through to layer 3. A tagged discriminator rather than a nullable value, because `null` and key-absent are one keystroke apart and many JSON tools drop null keys — turning an editing accident into "every user quietly gets the dead default back". Retraction is the honest reading of the state: GMM publishes no-recommendation precisely *because* the compiled-in default went bad, so falling through would make the state do no work at all. A manifest the build cannot interpret at all behaves as absent, never as retraction, and drops the whole layer rather than being partially applied.

**The last successfully fetched manifest is authoritative until replaced.** Refresh is a background best-effort once per app start; nothing waits on it and a failure is invisible to the user, because the cache is still in force and still correct. Without a cache, a user's configuration would flap with their network — and in the wrong direction, quietly restoring a package GMM had withdrawn. Silence in the UI is a product choice; a failed fetch must never be collapsed into "recommends nothing" anywhere in the data model, which is the defect that left the Loader update check reporting "up to date" for its entire life (#78).

**Declining a recommendation is scoped to the origin it proposed** — not to the game, and not to origin plus version. Suppressing a whole game would silently strand the one user who most needs a later fix. Version scoping would re-prompt on every upstream release; GIMI shipped two versions on one day. Declines are visible and reversible on the affected game's surface, because dismissing is a one-click reflex.

**Recommendations can be switched off entirely, and off removes the whole layer** — no fetch, no prompts, no consulting the cache, and no retraction of the compiled-in default. Turning off only the UI would leave GMM silently acting on a file the user said not to consult. Recommendations are on by default; this is an opt-out, and it is deliberately distinct from a decline, which is a judgement about one proposal rather than a standing preference.

**An Importer Pin suppresses version updates only, never origin recommendations.** A pin means "don't move me to a newer build of this package"; it is not a request to stop being informed that the package's source is dead. **Changing origin clears the pin**, because a version string taken against one origin is meaningless against another, and `compute_status` gates on pinned-as-boolean — a carried-over pin would suppress every update for the new origin indefinitely.

**Unknown origin is a first-class state.** Installs predating origin tracking are never backfilled to the compiled-in default (for GIMI, ZZMI and HIMI that would record a provable fiction, since those defaults never existed before #77) and never treated as "not installed" — those users hand-installed their importers precisely because GMM could not help them. Unknown becomes known only through an actual install, i.e. by the user accepting a recommendation. Nothing about it is surfaced proactively.

Two supporting commitments follow: CI validates the manifest's shape offline on every PR touching it, and a **scheduled** job checks that each recommended origin still resolves and opens an issue when one stops. That scheduled job is where staleness detection lives — the maintainer's process, not the user's screen. GMM never asks a user to judge whether an importer is abandoned, because age is a bad proxy for health.

## Consequences

- **The user is never stuck.** A dead upstream package becomes a settings change instead of an indefinite wait, and it works on already-shipped builds because the manifest is fetched rather than shipped.
- **A fresh user on a retracted game gets no default and must supply an origin.** Real friction, accepted deliberately: better than silently installing a package GMM has withdrawn its recommendation from.
- **`main` can never be renamed and the manifest path can never move.** Every build ever shipped requests exactly one URL, forever. This is a permanent constraint on the repository, not a preference.
- **A valid-but-wrong manifest reaches every user immediately.** Raw content is served with a cache measured in minutes, and there is no staged rollout. The review gate on `main` plus offline shape validation are the only things standing between a bad commit and every install; that is why the gate, not the branch, drove the location decision.
- **A user who opts out and later sits on a dead compiled-in default gets no warning.** They have explicitly taken on managing this themselves, and their own override sits above everything regardless.
- **Switching Importer Origin invalidates the install.** The game directory still physically holds the old package, so the install is cleared and must be re-run; `backup_existing` and `rollback_to` bound the downside. A user with a working hand-install who accepts a recommendation gets their game directory rewritten — acceptable, because accepting is an explicit act on a prompt that says so.
- **Origin equality is load-bearing in three places** — the decline key, the pin-clearing trigger, and install bookkeeping — so it is compared case-insensitively, since GitHub treats owner/repo case-insensitively and a capitalisation fix must not re-prompt everyone who declined.
- **GMM now curates an opinion about third-party software.** That is a small, ongoing editorial responsibility with no code attached, and the scheduled resolution check is what keeps it from silently rotting.
- **The manifest's asset-matching field inherits whatever rule #79 settles.** Asset matching is currently a bare substring; this ADR does not introduce a second matching rule.
- **Signature verification remains out of scope.** The XXMI launcher verifies ECDSA signatures from release bodies against per-package pinned keys; GMM verifies nothing, and pointing a game at a different origin does not change that. Real, tracked separately, and not the worry driving this decision.
