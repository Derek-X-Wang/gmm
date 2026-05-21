# Onboarding wizard — spec for slice 16-b

Specifies the GMM first-run experience. Slice 16-b (#24) is the implementation; this file is the contract it implements against. UX shape decided in a design Q&A on 2026-05-21.

## Top-level shape

Linear wizard, four steps. A persistent **Skip setup** control sits in the lower-left of every step and routes the user to Settings with a "Finish setup" banner that surfaces every step they skipped. The wizard only runs on the very first launch; subsequent launches go straight to the main app unless the user re-opens the wizard via **Help → Run setup again** or the Settings banner.

| # | Step                | Skippable | Time budget |
|---|---------------------|-----------|-------------|
| 1 | Welcome             | Always    | <1 s        |
| 2 | Game detection      | Always    | <1 s (parallel scan) |
| 3 | Library path        | Always    | <1 s (default accepted) |
| 4 | Importer install    | Always    | 30 s – 2 min depending on selection |

There is intentionally no "Continue" button on the welcome screen until the user has acknowledged the AV/SmartScreen banner. Subsequent steps allow Continue unconditionally — they can be skipped, not gated.

## Step 1 — Welcome

**Purpose:** explain the Library model in two sentences and disclose the AV / SmartScreen risk before the user touches anything.

**Copy (Library model, two sentences):**

> GMM keeps every mod in a central Library, separate from your game installs.
> Enabling a mod links it into the game's `Mods/` folder; disabling removes the link, leaving your library copy untouched.

**AV / SmartScreen banner (top of step, dismissible-with-acknowledgement):**

> ⚠ Windows may flag GMM as unknown on first launch. We don't ship code signing yet (cert cost). [Read the safe-exclusion steps →]
>
> ☐ I've read the AV note

The link target is the `Antivirus and SmartScreen` section of the repo README, written in slice NEW-AV (#13). Until the checkbox is ticked, **Continue** is disabled.

**ASCII wireframe:**

```
┌──────────────────────────────────────────────────────────────┐
│ GMM — first-run setup                              [_][o][x] │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│   ┌────────────────────────────────────────────────────┐    │
│   │ ⚠  Windows may flag GMM as unknown on first       │    │
│   │    launch. We don't ship code signing yet         │    │
│   │    (cert cost). Read the safe-exclusion steps →   │    │
│   │                                                    │    │
│   │    ☐ I've read the AV note                        │    │
│   └────────────────────────────────────────────────────┘    │
│                                                              │
│   Welcome to GMM                                            │
│                                                              │
│   GMM keeps every mod in a central Library, separate from   │
│   your game installs. Enabling a mod links it into the      │
│   game's Mods/ folder; disabling removes the link, leaving  │
│   your library copy untouched.                               │
│                                                              │
│                                                              │
│   [Skip setup]                              [Continue →]    │
│                                                Step 1 of 4   │
└──────────────────────────────────────────────────────────────┘
```

**States:**

- *initial*: AV checkbox unchecked, Continue disabled
- *acknowledged*: AV checkbox checked, Continue enabled
- *skipped*: user clicks Skip setup → route to Settings with "Finish setup" banner

**Errors:** none — pure copy step.

## Step 2 — Game detection

**Purpose:** run the per-game install-path detection in parallel for all six supported games and let the user confirm, override, or skip each.

**Behaviour:**

- On entry, the wizard fires `detect_all_games()` once. The command runs the six per-game detections in parallel (each is ~20–50 ms IO-bound).
- Each game has one row showing the detection result.
- The user can:
  - **Accept** an auto-detected path (default — no click needed)
  - **Browse manually** if the path is wrong or detection failed
  - **Skip this game** if they don't play it

**Row states:**

| State                       | Visual | Right-side control            |
|-----------------------------|--------|-------------------------------|
| Detecting                   | spinner | (none)                        |
| Detected                    | ✓     | `<path>` + [Change]           |
| Not found                   | ?     | [Browse…] [Skip]              |
| Manually browsed            | ✓     | `<path>` + [Change]           |
| Skipped                     | dimmed| [Add later]                   |

Continue is always enabled, even if every game is skipped (we land them in the main app with empty per-game tabs).

**ASCII wireframe:**

```
┌──────────────────────────────────────────────────────────────┐
│ GMM — first-run setup                              [_][o][x] │
├──────────────────────────────────────────────────────────────┤
│ Find your games                                              │
│ GMM scans common install locations + the Windows registry.   │
│                                                              │
│ ✓ Genshin Impact      C:\…\Genshin Impact Game    [Change]  │
│ ✓ Star Rail           D:\…\Star Rail              [Change]  │
│ … Zenless Zone Zero   detecting…                            │
│ ? Wuthering Waves     not found       [Browse…]  [Skip]     │
│ ? Honkai Impact 3rd   not found       [Browse…]  [Skip]     │
│ — Endfield            (skipped)                  [Add later]│
│                                                              │
│  [Re-scan]                                                   │
│                                                              │
│   [Skip setup]            [← Back]          [Continue →]    │
│                                                Step 2 of 4   │
└──────────────────────────────────────────────────────────────┘
```

**Errors:**

- *Folder access denied* (user picks a folder GMM can't read): inline error on that row — "Can't read this folder. Pick a folder GMM has access to or skip for now." Row returns to *Not found*.
- *Volume is not NTFS* (user picks a folder on exFAT/FAT32 — see ADR 0003): inline error on that row — "GMM needs an NTFS volume for the game install (so we can use junctions). Pick an NTFS folder or skip." Row returns to *Not found*.
- *Detection itself crashed* (unlikely; registry permissions etc): aggregate banner — "We couldn't run automatic detection. Browse manually for each game you want to set up." All rows show Browse + Skip.

## Step 3 — Library path

**Purpose:** confirm the global Library root before any importer install. Defaults to `%AppData%\GMM\library`; the user can change it.

**Behaviour:**

- Show the resolved default path. Beneath it, an estimate: "Importers + your future mods. Plan on ~5–20 GB."
- **Change** opens a folder picker. On selection, validate (NTFS check, write check, not the same path as any game's install root).
- Per-game library overrides (slice 15) are NOT exposed in the wizard — they're a power-user setting in Settings. Wizard sets the global root only.

**ASCII wireframe:**

```
┌──────────────────────────────────────────────────────────────┐
│ GMM — first-run setup                              [_][o][x] │
├──────────────────────────────────────────────────────────────┤
│ Where should GMM keep your Library?                          │
│                                                              │
│ Your mods live here, separate from your game installs.       │
│ Plan on ~5–20 GB depending on how many mods you collect.     │
│                                                              │
│   ┌────────────────────────────────────────────┐  ┌────────┐│
│   │ C:\Users\you\AppData\Roaming\GMM\library  │  │ Change ││
│   └────────────────────────────────────────────┘  └────────┘│
│                                                              │
│   ℹ Per-game overrides are available in Settings later.     │
│                                                              │
│   [Skip setup]            [← Back]          [Continue →]    │
│                                                Step 3 of 4   │
└──────────────────────────────────────────────────────────────┘
```

**Errors:**

- *Picked path is on a non-NTFS volume*: blocking error in-place — "GMM needs an NTFS volume for the Library so it can create junctions. Pick an NTFS folder."
- *Picked path is inside a game's install folder*: blocking — "This is inside your <Genshin Impact> install. Pick a folder outside any game install."
- *Picked path is not writable*: blocking — "GMM can't write to this folder. Pick a folder you can write to, or run GMM as the file owner."

## Step 4 — Importer install

**Purpose:** install the per-game Model Importer (`*MI-Package` GitHub release) for each detected game the user wants to play. Single screen, batched.

**Behaviour:**

- Show one row per detected game. Skipped-in-step-2 games are not listed here.
- Each row defaults to checked. Uncheck to skip.
- Right side shows the importer's release version and the approx download size.
- **Install selected** runs the per-game install flow from slice 3 (#9) sequentially for each checked row. A progress bar shows current step ("Downloading GIMI 1.2.3…", "Verifying checksum…", "Installing into game folder…").
- On any error, the row turns red with a one-line message + Retry button. Other rows continue.

**ASCII wireframe (mid-install):**

```
┌──────────────────────────────────────────────────────────────┐
│ GMM — first-run setup                              [_][o][x] │
├──────────────────────────────────────────────────────────────┤
│ Install Model Importers                                      │
│ One Model Importer (3dmigoto-derived DLL) per game.          │
│                                                              │
│ ☑ Genshin Impact         GIMI v1.2.3     ~15 MB    ✓ Done   │
│ ☑ Star Rail              SRMI v1.4.0     ~12 MB    Installing 60% ████░░ │
│ ☑ Zenless Zone Zero      ZZMI v0.9.1     ~11 MB    Queued   │
│ ☐ Wuthering Waves        WWMI v1.0.2     ~14 MB    Skipped  │
│ ☐ Honkai Impact 3rd      HIMI v0.8.0     ~10 MB    Skipped  │
│                                                              │
│  [Install selected]   [Pause]                                │
│                                                              │
│   [Skip setup]            [← Back]          [Finish →]      │
│                                                Step 4 of 4   │
└──────────────────────────────────────────────────────────────┘
```

**Errors:**

- *Network unreachable / GitHub 5xx*: row turns red — "Couldn't reach GitHub. [Retry]" — Retry re-tries the single row.
- *Checksum mismatch*: row turns red — "Importer file failed integrity check. [Retry]" — slice 3's rollback already restored prior state. Retry re-downloads.
- *Write to game folder denied*: row turns red — "GMM can't write into <C:\…\Genshin Impact Game\>. Close the game, or restart GMM as administrator." Retry available.
- *User cancels mid-install*: rolls back the in-flight row (slice 3 contract), leaves prior rows installed, returns to step 4 with mixed-state UI.

After the last row finishes (success or skipped), **Finish** routes the user to the main app's Genshin tab (the closest analogue to today's UI) or, if Genshin was skipped, the first detected game's tab; or, if everything was skipped, the empty main app with the "Finish setup" banner Settings shows.

## Skip-all destination

User clicks **Skip setup** at any step → wizard closes → user lands on the Settings panel. A banner sits at the top of Settings:

```
┌──────────────────────────────────────────────────────────────┐
│ ℹ  Setup isn't finished yet. You can resume any step below. │
│    [Resume setup]                                            │
└──────────────────────────────────────────────────────────────┘
```

The banner persists across launches until the user clicks **Resume setup** (re-opens the wizard) or completes the steps individually in Settings (game detection, library path, per-game importer install all already have Settings entry points from slices 1a / 2 / 3).

## Backend contract (for slice 16-b)

The wizard needs the following Tauri commands. Items marked **(new)** must be added by 16-b; items marked **(existing)** are already in the codebase.

| Command                          | Status   | Shape |
|----------------------------------|----------|-------|
| `is_onboarding_complete`         | new      | `() -> bool` — reads a row in the `settings` table |
| `mark_onboarding_complete`       | new      | `(skipped: bool) -> ()` — writes the row; `skipped=true` means user used Skip-all |
| `detect_all_games`               | new      | `() -> Vec<GameDetection>` — runs slice 2's per-game probe in parallel; returns `{game, detected_path?, status}` |
| `set_game_install_path`          | existing | from slice 1a |
| `get_library_root`               | existing | from slice 15 (#8) |
| `set_library_root`               | existing | from slice 15 |
| `install_importer`               | existing | from slice 3 (#9) — exposed for the wizard to drive |
| `list_supported_games`           | existing | returns the six game codes |

The new `settings` table (single-row key/value, or one column per setting — implementation choice) needs at minimum:

```
onboarding_complete:    BOOL (default false)
onboarding_skipped_steps: TEXT (JSON array of step names the user skipped)
```

## Frontend routes (React Router or equivalent)

| Route             | Component        | Notes |
|-------------------|------------------|-------|
| `/`               | App router       | Reads `is_onboarding_complete`; redirects to `/onboarding` if false |
| `/onboarding`     | OnboardingWizard | Internal state machine; no per-step subroutes — `?step=1..4` query param for deep-linkable resume |
| `/settings`       | Settings         | Shows the "Finish setup" banner when `onboarding_complete = false` |
| `/game/<code>`    | GameTab          | Existing |

## State machine

```
                  ┌─────────┐
                  │ welcome │
                  └────┬────┘
                       │ Continue (after AV ack)
                       ▼
                  ┌─────────┐
                  │  detect │ ── parallel probe ──┐
                  └────┬────┘                     │
                       │ Continue                 │
                       ▼                          │
                  ┌─────────┐                     │
                  │ library │                     │
                  └────┬────┘                     │
                       │ Continue                 │
                       ▼                          │
                  ┌─────────┐                     │
                  │ install │ ◀───────────────────┘
                  └────┬────┘
                       │ Finish
                       ▼
                  ┌─────────┐
                  │  main   │
                  └─────────┘

Any step ──Skip setup──► Settings (with "Finish setup" banner)
```

## Out of scope for 16-b

- Multi-language copy (English only v1, per the v0.5 cut).
- Animated transitions between steps. Default React Router transitions are fine.
- Telemetry on wizard completion / drop-off. Slice NEW-LOG (#5) writes local diagnostics; no remote analytics in v1.
- A re-onboarding flow for breaking schema changes. If a future migration invalidates the user's setup state, that's a v1.1 problem.

## Open questions

None at this point. If 16-b implementation surfaces a UX call we didn't make here, the implementer should comment on this file in their PR with the question; we'll resolve it in the PR before merging.
