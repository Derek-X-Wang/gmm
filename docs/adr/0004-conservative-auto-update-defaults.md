# 0004 — Conservative auto-update defaults for game-touching artefacts

Date: 2026-05-20
Status: Accepted

## Context

GMM has four independently-versioned things that can update:

1. GMM itself (Tauri-shipped, GitHub Releases)
2. Model Importer per game (`*MI-Package` GitHub releases)
3. The Loader (`3dmloader.dll` from `XXMI-Libs-Package` releases)
4. Individual Mods (GameBanana submissions when present)

The user's stated #1 pain point includes "auto updater". XXMI today defaults to aggressive auto-update behaviour: importers update silently on launch.

The risk asymmetry across the four tiers is severe. GMM core update touches only GMM's own install dir. Importer/Loader/Mod updates touch the game install directory. Gacha publishers (Mihoyo/Kuro) actively detect mod usage and have run ban-waves keyed to importer signatures; an importer update during such a window can put accounts at risk. A botched silent Mod update during a competitive event mid-banner can lose users their account or their mod state with no warning.

## Considered alternatives

- **Aggressive default ("just like XXMI").** Auto-update all four tiers on launch. Mirrors prior art. Fastest happy-path UX. Worst tail risk.
- **Fully manual default.** No background checks; user clicks "Check for updates". Lowest surprise, highest friction; defeats the user's auto-updater goal.
- **Tiered with grace window.** Importers auto-apply after N days unless user dismisses. Adds complexity (timer state, persistent dismiss); marginal benefit over plain notify+manual-approve.

## Decision

- **GMM itself**: auto-download in background, prompt on next launch (Tauri's `tauri-plugin-updater` happy path).
- **Importer + Loader**: check on launch, display update badge, never apply without explicit user click.
- **Mods**: weekly background check (cheap GameBanana API call per Mod with a source); badge in Mod row; never apply without explicit user click.
- **Per-game Importer pinning**: every game has a `pinned_version: Option<String>` setting; when pinned, GMM does not surface update prompts for that game's importer.

The defaults can be flipped to "auto-apply" by power users in settings, per tier. We never auto-apply by default.

## Consequences

- Slightly worse "looks magical" first impression vs aggressive defaults.
- Significantly safer for the user's account during ban-wave windows.
- Pinning is a v1 requirement, not a v1.1 nice-to-have, because it is the only escape hatch when a brand-new importer breaks mods or trips detection.
- Mod update notifications require persisting last-known-version per Mod in the DB; cheap.
- We commit to publishing GMM core via signed GitHub Releases so the silent updater is trustworthy. Code-signing certificate cost is deferred (XXMI's same problem; users accept self-signed for community modding tools).
