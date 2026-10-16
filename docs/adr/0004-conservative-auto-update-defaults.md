# 0004 — Conservative auto-update defaults for game-touching artefacts

Date: 2026-05-20
Status: Accepted; GMM self-update decision amended 2026-10-16 (#325, #373, #380, #336)

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

- **GMM itself**: check for a signed update on launch, then wait for explicit consent. The status bar announces an available update and links to the GMM updates group in Settings. Only Settings offers `Install update` and `Skip this build`; checking never downloads or installs.
- **Importer + Loader**: check on launch, display update badge, never apply without explicit user click.
- **Mods**: weekly background check (cheap GameBanana API call per Mod with a source); badge in Mod row; never apply without explicit user click.
- **Per-game Importer pinning**: every game has a `pinned_version: Option<String>` setting; when pinned, GMM does not surface update prompts for that game's importer.

The original decision proposed per-tier auto-apply settings. That proposal does not apply to GMM self-updates: installing requires an explicit press of `Install update` (#325, #346).

### GMM self-update amendment — 2026-10-16

The original GMM decision called for background downloads and a prompt on the next launch. #325 superseded that decision with explicit consent before any download or install. #373 established the status-bar announcement and the single consent surface in Settings. This amendment records those changes and #380 for #336 item 3; #336 items 1 and 2 were already resolved by #373.

A launch check honours a remembered `Skip this build` choice. An explicit `Check for updates` deliberately re-offers that build so the user can review it again. The remembered choice identifies the signed build, not just its application version: another signed build with the same version remains eligible. If a build cannot be identified, skipping it applies only to the current offer and Settings explains that it will be offered again on launch.

After consent, Settings shows download progress and the status bar records that a download is in flight. Download, signature-verification and extraction failures remain visible in Settings and updater diagnostics. A failed check is distinct from finding the current release.

GMM ships Windows MSI only. `tauri-plugin-updater` 2.10.1 starts the installer and exits GMM itself (`src/updater.rs:865`); the MSI handles launching the updated application. There is no successful in-process return from that Windows install path. #380 therefore removes the unreachable UI outcome. A test pins the reviewed plugin version from `Cargo.lock` and requires another review of Windows `install_inner` when the version changes. macOS and Linux AppImage have successful return paths in 2.10.1, so another bundle target also requires reconsidering this assumption.

## Consequences

- Slightly worse "looks magical" first impression vs aggressive defaults.
- Significantly safer for the user's account during ban-wave windows.
- Pinning is a v1 requirement, not a v1.1 nice-to-have, because it is the only escape hatch when a brand-new importer breaks mods or trips detection.
- Mod update notifications require persisting last-known-version per Mod in the DB; cheap.
- We commit to publishing GMM core via signed GitHub Releases so update authenticity can be verified. Code-signing certificate cost is deferred (XXMI's same problem; users accept self-signed for community modding tools).
