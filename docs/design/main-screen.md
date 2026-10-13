# Main screen and Settings implementation specification

## Status and authority

This specification covers the specification half of #285. It records the
accepted information architecture and both human decisions on the attention
indicator and status strip. It also records the application-shell decisions of
#326: a native menu, Settings as a modal dialog, and a bottom status bar.
Implementation ticket granularity and slicing need a separate human decision
and are outside this specification.

The binding sources are:

- [#282 — wayfinder map](https://github.com/Derek-X-Wang/gmm/issues/282): purpose,
  stays/moves split, recovery rules, first-run landing, and non-goals.
- [#283 — candidate A resolution](https://github.com/Derek-X-Wang/gmm/issues/283#issuecomment-6103858665):
  **Status at a glance**, including the read-only strip, global attention
  placement, and prominent Launch action.
- [#284 — round-1 review](https://github.com/Derek-X-Wang/gmm/issues/284#issuecomment-6104009504)
  and [round-2 candidate B resolution](https://github.com/Derek-X-Wang/gmm/issues/284#issuecomment-6104674157):
  **Independent list**, full-screen Settings, separate GMM page, all six Games,
  and independent selection. Round 2 supersedes the earlier candidates. The
  map's two sections become separate pages; its sidebar prohibition applies
  only to the main screen. These are explicit refinements, not contradictions.
  #326 later replaced the full-screen presentation with a modal dialog; the
  rest of round 2 stands.
- [#285 — final brief](https://github.com/Derek-X-Wang/gmm/issues/285#issuecomment-6104678223):
  the attention and status-strip requirements, Add game destination, Settings
  row state, and Library split. Human review on #285 accepted both presentation
  recommendations; those decisions are recorded under their surfaces below.
  The earlier strip brief also requires a read-only report.
- [#326 — application shell](https://github.com/Derek-X-Wang/gmm/issues/326):
  the [modal-dialog decision](https://github.com/Derek-X-Wang/gmm/issues/326#issuecomment-6133172167)
  and the [Close-only decision with the settled list](https://github.com/Derek-X-Wang/gmm/issues/326#issuecomment-6133386264).
  These revise the shell only. Settings becomes a modal dialog instead of a
  full-screen destination, global actions move to a native menu, the window
  bar's wordmark goes, and the status strip becomes a bottom status bar. #326
  states that the honesty rules, the per-Game status reporting, and the
  Settings information architecture from #282 through #285 stand. Where this
  document now departs from #284's full-screen destination, the section says so
  and names #326.

[CONTEXT.md](../../CONTEXT.md) defines the domain terms used here. ADRs
[0001](../adr/0001-gplv3-and-embed-3dmloader.md),
[0002](../adr/0002-standalone-reimplementation-not-fork.md),
[0003](../adr/0003-junctions-over-symlinks-and-copy.md),
[0004](../adr/0004-conservative-auto-update-defaults.md), and
[0005](../adr/0005-importer-origin-conduit-not-maintainer.md) remain binding.
Existing behavior cited below is a preservation requirement, not a new product
policy. No contradiction was found between the accepted layouts and these ADRs.

## Reference assets and their limits

| Asset | Accepted view | Concrete coverage |
| --- | --- | --- |
| [Main-screen prototype](https://github.com/Derek-X-Wang/gmm/blob/prototype/283-main-screen/prototype/283-main-screen.html) on `prototype/283-main-screen` | Candidate `a` | Populated and empty Library; three installed Games; Variants; one shared-hash Conflict; Mod update; inline reinstall recovery; compact expandable attention; Launch in the Game header. |
| [Settings round-2 prototype](https://github.com/Derek-X-Wang/gmm/blob/prototype/284-settings-round2/prototype/284-settings-round2.html) on `prototype/284-settings-round2` | Candidate `b` | Both arrivals; Healthy and Degraded; all six Game pages; GMM page; independent return target; readable, unreadable, missing Model Importer, and absent Game states. |

These are static references for hierarchy, spacing, grouping, and navigation
(#283, #284). Their controls do not validate paths, launch, install, move files,
or persist settings. Do not merge the HTML or use its sample versions, origins,
paths, preview notices, control header, or timestamps as application data. Its
warning styling is a reference only; the human decision on #285 governs the
attention indicator's loud shape below.

The #326 shell decisions supersede parts of both frames. In the main-screen
prototype, the window bar and the strip's position below the tabs no longer
apply. In the Settings prototype, the full-screen frame and **Back to Mods** no
longer apply. Its Games list, GMM entry, page content, and grouping still apply
inside the dialog. The shell comparison canvas used in #326 shows OK / Cancel /
Apply on board B. That point is out of date; the Close-only decision governs
(#326).

## Application shell

The maintainer chose the broad reading of #326: a redesign pass toward a native
Windows application, not a menu bar added to the current screen. This section
records what was decided. It does not change what the application reports or
how Settings is organised (#326).

### Native menu and window identity

Global actions move from in-page chrome into a native menu (#326). The global
actions in the window bar today are the Settings control and **Run setup
again**. Both become menu items. The Settings item opens the Settings dialog on
its GMM page, as the window-bar control did (arrival contract below). **Run
setup again** keeps its existing behavior: it is unavailable while a reset is
pending, and a failure stays visible as a persistent error on the main screen,
not inside the menu.

The in-page window bar's `GMM` wordmark goes. The OS title bar already carries
the application's name, so showing it twice adds nothing (#326). With the
wordmark and both global actions gone, the window bar has no content left and
is not part of the main-screen hierarchy below.

Which menus exist, their names and order, and whether any other item appears
in them is not yet decided. See [Open shell decisions](#open-shell-decisions).

### Settings as a modal dialog

Settings opens as a **modal dialog** over the main screen (#326). The
maintainer chose it over the full-screen destination, a separate window, and a
docked panel. It is drawn inside the main window, not as a separate OS window.
It opens from the menu's Settings item. The existing status-bar Settings link
and Add game also open it, on a Game page (arrival contract below). While it is
open, the main screen stays in place behind it and does not accept input.
Closing the dialog shows the main screen exactly as it was left.

This replaces the full-screen destination of #284 round 2. That round chose a
full screen so Settings would not show Launch or the Mod list while the user
configures. The dialog keeps that rule for its own content: it contains no
Launch control and no Mod list. Everything else from #284 round 2 stands
inside the dialog: the list of all six Games, the separate GMM entry, one page
at a time, and a Game selection independent of the main screen.

The dialog dismisses with **Close only**. It has no OK, Cancel, or Apply
(#326). Settings changes continue to commit as they do today: each editor
commits its own change through its own mutation. The dialog is a container,
not a transaction. Nothing in it may claim a reversal the code cannot perform.

The reason is architectural. A toggle changed inside the dialog is already
saved before the user reaches any button. A Cancel would therefore have to
abandon changes that are already committed, which the code cannot do. A Cancel
that does not cancel is the same defect as the controls and messages this
repository has removed for claiming more than they establish. Windows 11's own
Settings also applies changes at once and has no OK or Cancel (#326).

Transactional settings editing is not rejected; it is a separate decision that
#326 did not take. If buffered edits with a real Cancel are wanted later, that
decision must account for the controls that are operations rather than
settings, such as Re-detect, Install, and Browse. Those cannot be buffered
(#326).

### Status bar

The per-Game status strip's content moves to a **status bar at the bottom of
the window** (#326). It still reports the Game and the Model Importer as
separately labelled facts, as #324 also requires. Its payload, wording, path
states, read-only rule, and Settings link are unchanged; see
[Per-Game status bar](#per-game-status-bar).

### The docked panel goes to the selected Mod

The docked panel was a candidate for Settings in #326 and lost to the modal
dialog. It is not discarded. It is reassigned to the selected Mod's detail in
[#327](https://github.com/Derek-X-Wang/gmm/issues/327). The split is the
ordinary desktop one: a dialog configures the application; a panel follows the
selection (#326). This document does not specify that panel. Do not reopen
the Settings presentation to reclaim it.

### Open shell decisions

The maintainer has **not** decided the following. They are recorded here so
that no ticket treats them as settled. Items marked *needed for coherence* were
not raised as open in #326, but this document cannot specify them without a
decision.

| Open item | What is known | Source |
| --- | --- | --- |
| Window default size, minimum size, and remembered size and position | The window is configured at 800x600, with no minimum and no remembered size or position. | #326 |
| Keyboard access and accelerators | Menu accelerators, standard Windows shortcuts, and keyboard-only paths to each surface. The existing Escape return from Settings is preserved (arrival contract below); nothing else is decided. | #326 |
| Per-flow evidence from the Windows playbook | #326 carries a read-only evidence playbook for a Windows session. Its results inform the two items above. | #326 |
| Menu structure | Which menus exist, their names and order, and whether **Check for updates**, Diagnostics, or Quit appear as menu items in addition to their current homes. Only the move of Settings and **Run setup again** is decided. | Raised in #326, not settled |
| Dialog size and behavior in a small window | Whether the dialog is resizable, how large it is relative to the window, and how it fits at the minimum window size. Depends on window sizing. | *needed for coherence* |
| Status bar presentation of long content | The bar must carry every fact in the per-Game payload, including the list of **Missing Game entries:**. How a list that exceeds one line is shown is not decided. No fact may be dropped silently. | #324, *needed for coherence* |
| Mod inspector and the main-screen no-sidebar rule | #327 adds a docked panel beside the Mod list. This document says the main screen has no sidebar or dock (#282, #283). #327 must reconcile the two when it is specified. | #327, *needed for coherence* |

A visual pass (how the application looks, as opposed to how native it feels)
is a separate axis and is not part of #326 (#326).

## Main-screen information architecture

The main screen manages Mods for one Game: browse the Library, enable/disable,
review Conflicts, and launch (#282). Candidate A gives the Mod list the first
large content area. There is no main-screen sidebar, session dock, or collapsible
setup disclosure (#282, #283).

Use this hierarchy from candidate A (#283), as revised by #326:

1. Native menu, above the page content: the global Settings item and **Run
   setup again** (#326).
2. Game header: the selected Game name and a prominent primary
   `Launch <Game name>` action.
3. Global header line: the compact expandable all-Games attention indicator
   and Game Session context.
4. Horizontal tabs for installed Games, with Add game beside the tabs.
5. Conditional recovery/setup notices, scoped to the affected Game or operation.
6. Mods: populated Library or the import-led empty state.
7. `ModUpdatesPanel` for this Game, after the Mod content.
8. Status bar at the bottom of the window: the selected Game's read-only
   status, with its Settings link (#326).

Candidate A put a window bar first, with the GMM wordmark and the global
Settings control, and placed the status strip directly below the tabs (#283).
#326 removes the window bar and moves the strip's content to the bottom status
bar. See [Application shell](#application-shell).

Conditional notices must not bring the configuration panels back above the Mod
list (#282). A per-Mod recovery warning stays on that Mod's row. A Game-level
Model Importer recovery summary can point to its Settings page; its full
configuration and recovery controls belong with `ImporterPanel` in Settings.
Existing `StartupReconcileNotice`, interrupted-launch notices, and skipped-setup
resume controls remain available (#282). A healthy completed startup needs no
recovery card. A reconcile failure names the affected Game and the error and
must not claim its Junctions were repaired.

### Per-Game status bar

#283 introduced this report as a thin status strip below the Game tabs. #326
moves its content to a status bar at the bottom of the window. Only its
position changes; the payload, wording, and path-state rules below stand.

The read-only status bar links to that Game's Settings page (#283), which
opens in the Settings dialog. The human decisions on #285 and #324 set its
payload: **`Game and Model Importer (<Importer>)`**, with a labelled Game fact,
Model Importer name and its healthy / unhealthy / unverified state, the saved
Game path, the verified installed version, and a compact pin marker when an
Importer Pin applies. Importer Origin detail stays in Settings and does not
appear in the status bar. The status bar contains no path editor,
Install/Update, rollback, Importer Pin control, Origin editor, or Library
mutation control.

When the path is unreadable, the line leads with **Cannot tell — install path
unreadable**. Keep the saved path, report Model Importer state as unverified,
never say “not installed,” and never infer a version. When the Game is readable
but its Model Importer is confirmed absent, keep the status bar visible with
**Model Importer not installed** and its Settings link. That link is the route
to fixing this state (#285 human decision).

A saved Game directory confirmed missing has its own wording: **Game: Saved
installation cannot be found**, identifying the missing directory and retaining
the saved path and Settings link. An existing saved directory with missing
required Game entries instead says **Game: Required Game files or directories
not found** and lists **Missing Game entries:** with the missing paths (#324).
Both report Model Importer state as unverified. Do not reuse “no saved path” or
“Model Importer not installed.”
All status-bar states follow the filesystem and path-state contract below; only
verified installed facts may supply an installed version (#285 human decision).

### Surface ownership

| Surface | Destination and scope | Source |
| --- | --- | --- |
| `ModList`, Variants, Conflicts, per-Mod recovery | Main, selected Game; preserve the existing list internals. | #282, #283 |
| `SessionBanner`, `LaunchGameButton` | Main; session facts remain cross-Game, Launch names the selected Game. | #282, #283 |
| `ModUpdatesPanel`, Mod update badges | Main; retain the existing weekly-check preference and explicit apply behavior. | #282, ADR 0004 |
| `AttentionView` | Compact expandable indicator in the main global header line; all Games, not just visible tabs. | #282, #283 |
| `Settings` install-path editor, `RebuildJunctions` | Selected Game's Settings page. | #282, #284 |
| `ImporterPanel`, `ImporterOriginPanel` | Selected Game's Settings page, including Origin Proposals and Origin Dismissals. | #282, #284, ADR 0005 |
| `LibraryPathsPanel` | Global root on GMM; one override on each Game page. | #284 round 2 |
| All-Games importer-recommendations switch | GMM page, with a named link from each Game's Importer Origin group. | #284 candidate B, ADR 0005 |
| `NetworkPanel`, `UpdatesPanel`, `Diagnostics` | GMM page. | #282, #284 |
| `LibraryAuditWarning` | Conditional recovery surface on its Game; retain its existing attachment to the Game install-path group when that group moves to Settings. | #282 recovery preservation, #284 |
| `WindowBar` (Settings control, **Run setup again**, `GMM` wordmark) | Removed from the page. Settings and **Run setup again** become native menu items; the wordmark is dropped. | #326 |
| `GameStatusStrip` | Status bar at the bottom of the window, selected Game; payload and wording unchanged. | #283, #285, #324, #326 |
| `SettingsDestination` | Modal Settings dialog over the main screen, dismissed with Close only. | #284 round 2, #326 |

### Main-surface states

Moving these surfaces preserves their existing operation states (#282, #285).
They can coexist: a populated Library can have a Conflict, an unavailable Mod,
an update-check error, and an active Game Session at the same time.

| Surface | States and required result |
| --- | --- |
| Selected Game/inventory | Registry/path reads pending, available, or failed; zero configured paths; one or more configured paths; selected Game changes. Show read failures as unavailable, with Settings reachable. Never render an absent Game as an installed tab. |
| Status bar | Checking, verified Game/Model Importer facts, or unavailable check, plus the four path cases below and distinct reports for a missing saved Game directory and missing required Game entries. Keep read-only and preserve the decided payload and wording above; the status bar stays visible when a readable Game has no Model Importer (#285 human decision). |
| Launch | Ready action, launching, active Game Session, returned session, or rejected launch. Preserve `launch_game` and its existing AV-pattern guidance via `av_guidance`; keep other command failures visible near Launch. Pending/session locks do not remove the primary action's identity. |
| Game Session context | Reading/unavailable, no active session, active session, cleaned stale session, or interrupted launch reservation. Continue existing session-event refresh while the Settings dialog is open, so closing it does not fabricate an ended session. |
| Mods/Variants | Mod query pending/error, confirmed empty, or populated; existing enabled/disabled choices; Variants absent, loading/error, or available; Variant change pending/success/failure; recovery quarantine. Retain existing controls and their individual gates. |
| Conflicts | Checking/error, verified none, or verified conflicts. A failed check is not a no-Conflict result. Preserve existing row disclosures; do not choose a winner. |
| Mod updates | Global weekly preference loading/error or enabled/disabled; Game check pending/error or successful with no updates/available updates; row update checking preference; apply pending/success/failure. A failed check is not “current,” and failed apply feedback survives a disappearing badge. |
| Setup/reconcile notices | Setup completed or skipped; resume/reset pending/rejected; startup reconciliation pending/finished with per-Game failures. Successful completion may remove a notice; an operation error still needs stable feedback. |

## Game navigation and independent Settings context

Settings opens as a modal dialog over the main screen (#326). The dialog
contains no Launch control or Mod list (#284 round 2). The main screen stays
behind it, unchanged and not accepting input, until the dialog closes (#326).
Keep the Settings title and the **Close** action in the dialog. Down its left
side, list every supported Game under Games. Each row has the Game name and one
line of state. Put **GMM settings** below that list under its own Application
heading. Show one selected page at a time (#284 candidate B).

#284 round 2 made Settings a full-screen destination that replaced the main
screen completely, with a **Back to Mods** action and no main-screen backdrop.
Its purpose was to keep Launch and the Mod list out of Settings. #326 replaces
that presentation with the dialog. The dialog's own content still excludes
both, and the main screen behind it cannot be used until the dialog closes.

All six titles are reachable: Genshin Impact (`gimi`), Honkai: Star Rail (`srmi`),
Zenless Zone Zero (`zzmi`), Wuthering Waves (`wwmi`), Honkai Impact 3rd (`himi`),
and Arknights Endfield (`efmi`). Use the existing supported-Game registry and
display names; an absent Game's row remains selectable (#284). The sidebar is
inside Settings only. It is not shared permanent chrome (#282, #284).

### Arrival and return contract

| Action | Destination | Effect on main-screen selection |
| --- | --- | --- |
| Status-bar Settings link for Game X | Settings dialog, Game X page. | None. |
| Menu Settings item | Settings dialog, GMM page. | None; global arrival implies no selected Game page. |
| Add game, then choose Game X | Settings dialog, Game X page, with its install-path editor reachable. | None; do not create a main-screen tab by selecting it. |
| Select a Game row in Settings | That Game's Settings page. | None, for either arrival route. |
| Select GMM settings or a named scope link | The named Settings page. | None. |
| Close, or the existing Escape return | Dialog closes; main shows the Game left on entry. | Restore that selection and the user's place in its Mod list. |

The first two routes are binding (#284 round 2). #326 moves the global entry
point from the window bar to the menu and keeps its GMM-page arrival. Add game
is the existing setup affordance routed to the now-decided destination (#285),
which now opens in the dialog (#326). Retain its supported-Game chooser rather
than silently choosing a Game. Do not adopt
candidate A's shared selection or candidate C's route-dependent policy (#284).
Navigation within Settings can change its own page and Game selection only.
Its header and form labels name the Game being configured. Candidate B's Back
action named the main-screen return Game. The dialog's action is **Close**
(#326), and the return Game stays visible behind the dialog.

### Installed-only tabs and adding a Game

The asymmetry is deliberate: main lists installed Games; Settings lists all six
(#284). For navigation, preserve the existing installed-tab definition: a
successfully read, non-empty saved Game install path. This is configured
membership, not a fresh assertion that the filesystem is healthy. A saved path
that is unreadable or now missing retains its Game's tab and repair route.
Neither failure nor a missing Model Importer removes that tab (#284 prototype
and #285 uncertainty requirement).

Setting a valid install path is how an absent Game gains a main-screen tab
(#284). The change refreshes tab membership after the backend accepts it; a
failed save does not add a tab. Adding a Game while in Settings does not select
it on main. Closing the dialog still returns to the prior Game. If no Game was
selected because there were no configured paths, use the existing
first-installed-Game fallback after paths load; this is initialization, not
shared Settings selection.

While saved paths load, report that the inventory is being checked. If one read
fails, name the affected Game and retain any last known configured membership;
do not label it absent or show a successful zero-installed-Games answer. Keep
known tabs usable and keep global Settings available for every Game. With all
reads successful and no saved paths, show the no-Game setup landing and Add game,
not an invented installed Game (#282, #284; existing `GameTabs` behavior).

## Filesystem and path-state contract

These are the prototype's four named states, not four interchangeable messages
(#284 candidate B; #285). The brief's three-way distinction refers to the three
non-healthy cases. Keep both Game state and Model Importer state in the data used
to render them.

| Prototype state | Evidence | Settings Game-row line | Game page behavior | Main tab |
| --- | --- | --- | --- | --- |
| `healthy` | Game path is readable and Model Importer presence is verified. | `<Importer> installed · Game readable` | Show saved path and verified installed facts; show existing configuration controls. | Present. |
| `unreadable` | A saved path cannot be checked: permission error, sharing violation, I/O failure, unavailable storage, or other uncertainty. | `Cannot tell · path unreadable; Model Importer unverified` | Keep the saved path. Say the Game and Model Importer cannot be verified. Show the actual failure and path-repair route; infer no installed version. | Retained. |
| `missing` | Game executable/path is readable; Model Importer files are confirmed absent. | `Game readable · Model Importer not installed` | Retain the Game path; offer the existing explicit install action. No installed version or pin-to-current is inferred. | Present. |
| `absent` | No saved install path. The prototype also has no detected path. | `Absent · no saved path` | Show the empty path editor and detection/picker routes. A readable Game path is needed before Model Importer installation. | Absent. |

“No saved path” is a fact about configuration, not proof that the Game exists
nowhere on disk. Auto-detection failure does not strengthen it into that claim.
Similarly, a recorded Importer Origin or version alone is not proof of readable
Model Importer files (#284, #285; ADR 0005).

Only an explicit `NotFound` proves filesystem absence. Report two distinct Game
path states (#324): `savedPathNotFound` means the **saved Game directory
itself** is confirmed absent; `requiredEntriesNotFound` means that directory
exists but required Game files or directories are confirmed missing. Recheck the
saved directory before reporting missing required entries; if it vanished,
report the missing saved directory, and keep a failed recheck as uncertainty.
Retain the saved path and configured tab in both states, identify the missing
paths, and keep the Model Importer unverified. Only the first state says the
saved installation cannot be found. Settings must name the Game in that claim,
including beneath a Model Importer heading: **Saved Game installation cannot be
found**. Settings and the status bar label required-entry details **Missing Game
entries:**. Do not call either state prototype `missing`: that state means a
readable Game with no Model Importer. Do not erase the setting or relabel it
`absent`. If Model Importer inspection fails while the Game is readable, keep
the verified Game fact and report Model Importer state as unverified; it is not
the `missing` case (#284, #285; repository filesystem-uncertainty rule).

Apply the same discipline to Library paths, Junctions, backups, recovery
directories, and status queries. Loading is not absence. A failed read is not
an empty list, healthy status, completed cleanup, or “up to date.” A previous
successful result may be retained only as a clearly identified previous result
while a new check is pending or failed. Never replace current uncertainty with
a plausible version or default path (#282 recovery rules; #285).

## Settings pages and their states

Use candidate B's content grouping (#284): Game path first, Model Importer and
Importer Origin groups next, then the Game's Library path. On GMM, put the
global Library root first, then Network, GMM updates, importer recommendations,
and Diagnostics. The reference uses full-width path groups and paired smaller
cards when space permits; a narrow dialog wraps the forms while retaining the
left navigation. The modal dialog is the only Settings shape (#326); do not
introduce another. The dialog's size relative to the window is an
[open shell decision](#open-shell-decisions).

### Per-Game page

| Group | Required content and controls | States to preserve |
| --- | --- | --- |
| Game install path | Named Game, path-state explanation, `PathField`, Apply/Enter, folder picker, Auto-detect/Re-detect; `RebuildJunctions` and its result. | Initial read pending/error; no saved path; saved/manual or detected path; draft; picker cancelled; save/detect pending; accepted or rejected save; detection finds a path or returns none; rebuild pending, success, or partial/failure report. Path checks follow the preceding table. |
| Model Importer | Verified installed facts, latest-release/update information, existing install/apply-update, rollback and Importer Pin controls; `LoaderVersionNote`; full importer recovery warning. | Checking; installed; confirmed absent; filesystem unverified; release/update check failed; available update; pinned/unpinned; install/rollback/pin pending and outcome; no backup to roll back to; pending recovery, described below. A failed upstream check is not “current.” |
| Importer Origin | Origin in effect and precedence layer; installed origin versus future install target; inline Origin Proposal, reason and replacement disclosure; decline/undo; override editor and clear action. Named link to GMM recommendations. | `ImporterOriginStatus` loading/error; `inEffect`/`noneInEffect`; installed `known`/`unknown`/`unreadable`; override `notSet`/`set`/`unreadable`; proposal absent/present; dismissals empty/present/unreadable; recommendation-list unusable; each action pending/success/failure. Keep each existing error rather than hiding it behind an absent proposal. |
| Library for this Game | Effective path, whether inherited or overridden, existing path editor/picker, Reset override, and `View global Library root` link. Retain overlap/move/Junction-restore reports in the affected scope. | Resolving/error; inherited or overridden; dirty draft; move/reset pending; accepted move; refused move; overlap repaired by setting-only change; partial Junction restoration. No override means Reset is unavailable, not that the Library is absent. |
| Conditional Library audit | Existing `LibraryAuditWarning` and its inspection, recover/name, delete confirmation, and duplicate-ownership review flows. | No findings; unreferenced folders; duplicate records; blocked duplicate choice; confirmation open/cancelled; action pending; recovered/deleted/resolved; rejection or partial filesystem result. Audit unavailable is not a clean audit. |

This relocates existing surfaces without changing their semantics (#282, #284).
In particular, Importer Pin suppresses version prompts only; it does not suppress
Origin Proposals. Unknown installed origin is not “not installed” and is not a
new proactive warning. Update stays on the installed origin. An explicit origin
change invalidates the install and clears its pin. A proposal makes the
replacement explicit; dismissals remain reversible on this Game's page
(ADR 0005). A future-install origin must never stand in for installed evidence.

### GMM page and Library scope split

| Group | Required content and controls | States to preserve |
| --- | --- | --- |
| Global Library root | Effective/default root and global override; existing editor/picker and Reset to default. Named links to per-Game Library pages. | Resolving/error; default/override; draft; moving/resetting; successful move; refused move; overlap-setting repair; per-Mod Junction restoration failures. |
| Network | Existing proxy URL, optional username/password, Save, Test connection. | Configuration loading/error; unset/set; edits; save/test pending; success/failure. Saved password is not exposed; retain the existing successful-save draft clearing and diagnostics redaction. |
| GMM updates | Existing signed update check, progress and Restart to apply action. | Existing `UpdateState`: `idle`, `checking`, `up-to-date`, `available`, `downloading`, `installed`, `error`. Show download progress if known; error is not “latest.” |
| Importer recommendations | The all-Games preference and its explanation; links to Game-specific proposals/dismissals/overrides. | Loading/error, enabled/disabled, preference mutation pending/success/failure. Turning off removes the recommendation layer, not just prompts; it preserves dismissals. |
| Diagnostics | Read-only log directory and existing local export action. | Log directory loading/error/available; export dialog cancelled; exporting; exported artifact result; export failure. No background upload or editable log path. |

The global Library root is edited only here; each per-Game override is edited
only on its Game's page (#284 round 2). The Game page states whether it follows
the root and links to it. The GMM page names the scope of inherited changes and
links back to Game overrides. Do not recreate an all-six-overrides editor on
GMM or a global-root editor beside a Game override.

Preserve `LibraryPaths` and `MoveReport` semantics: a safe change moves affected
bytes and restores Junctions; an overlap repair changes the setting and leaves
the old tree untouched. Report failed Junction restores individually. A Game
override affects that Game only; global changes affect inheriting Games.
Preserve refusal for non-NTFS targets and existing recovery/ownership guards
(#284 reference behavior; ADR 0003). No navigation action causes a move.

The Loader remains embedded in GMM; an informational upstream Loader version
does not create a separate Apply action. A newer embedded Loader arrives with
a GMM release (ADR 0001; existing `LoaderVersionStatus`). Model Importer and Mod
updates still require explicit action under the existing defaults (ADR 0004).
This layout change does not alter update policy or add tier switches.

## Attention, recovery, and operation availability

### Attention indicator states

The all-Games `AttentionReport` is an explicit-refresh snapshot, with its check
time and the notice that each action checks its own availability. Preserve this
contract from `AttentionView` and the mutation audit. Its header placement,
compact collapsed form, and ability to expand are settled (#282, #283).

The human decision on #285 keeps the same compact footprint when loud: a
warning icon, warning border/tint, explicit **Needs attention** text, and a
short finding count/summary. Do not auto-expand or use larger or filled heavy
styling. The full report stays behind user expansion. The count describes
**findings**, never a count of broken Mods: an active Game Session also makes
the report unsafe, so wording that implies every finding is a broken Mod would
be false in that state.

| State | Presentation and behavior |
| --- | --- |
| Initial check pending | Compact checking/unavailable state; no “nothing needs attention” claim. Disable duplicate refresh while checking. |
| Successful report, `safe_to_proceed` true | Quiet compact indicator; expansion can show the clean report and check time. |
| Successful report, `safe_to_proceed` false | Loud state, only in this case (#282 constraint 5); use the compact warning presentation decided above (#285 human decision). User expansion exposes the existing findings and their scope. |
| Check failed | Explicit unavailable/error result with Refresh attention. Do not synthesize either boolean result. A retained previous snapshot is labelled as previous. |
| Refresh pending | State that a refresh is underway; any retained report stays a dated snapshot. Do not claim recovery completed until the check succeeds. |
| Expanded | Preserve all non-empty report groups, path/recovery evidence, guidance, check time, and refresh; collapse returns to the header indicator. Expansion itself performs no recovery. |

Rust's `safe_to_proceed` is frontend `safeToProceed`. Do not derive it again from
the selected Game, a count, a Conflict, or an unreadable install path. Conflicts,
available updates, and an ordinary path check failure do not independently
trigger loud attention. In particular, the static prototype's degraded warning
does not override the boolean contract (#282, #285).
Pending and error states remain explicit; they do not borrow a loud trigger
of their own and must not manufacture a false report (#285 human decision).

### Recovery placement and states

These preserve the existing domain safety boundaries while moving surfaces
(#282, #285; CONTEXT.md). Keep the backend's recorded reason and exact paths.

| Recovery kind | Location and required states |
| --- | --- |
| Interrupted reinstall | Inline on its Mod row. Distinguish `junctionWithdrawn` from withdrawal failure: the latter says the Mod **may still be loading** and gives the reason. Saved enabled choice is unchanged. Retry pending, recovered, already recovered, still quarantined, and rejected retry retain their existing feedback. |
| Enable/disable transition | Inline on its Mod row. Show intended action, Junction path, reason, and automatic-startup retry guidance. If `ownerUncertain`, retain the explicit confirmation/producer-retirement action; do not assert the producer exited. Pending/failure/result remain visible. |
| Model Importer evacuation | Full warning on the Game's Settings page; conditional main summary points there. Preserve `retry`, `retireProducer`, and `acknowledgeAndRelease` action distinctions. Releasing a block after identity uncertainty is not a claim that files were restored. Preserve both recorded locations and confirmation text. |
| Interrupted Game launch | Main Game Session area, with missing or recorded child PID distinguished. Keep reservation until the user explicitly confirms the Game is closed and retires it. Show retirement pending/failure; refreshed removal is not proof of process identity. |
| Active Game Session | Main session context names the running Game and retains the keep-GMM-open/Mod-lock explanation. No active session is distinct from session lookup unavailable. A stale session cleaned by the backend gets the existing dismissible ended-unexpectedly notice. |
| Staged Library operation | Expanded attention names operation, Game, staged path, and recovery error or active-owner explanation. Preserve restart-recovery guidance and the affected Game's conditional Library audit route. Do not delete partial bytes from the indicator. |
| Unreferenced folders/duplicate records | Expanded attention summarizes them; the existing per-Game audit owns inspection and confirmed recovery/deletion/duplicate resolution. No inferred name or silently selected duplicate winner. |
| Library root/Mod path overlap | Expanded attention summarizes the overlap; the relevant Library group owns root repair and recorded-path guidance. Distinguish configured root overlap from stranded Mod records. Repairing the root alone does not clear stranded records. |
| Startup Junction reconciliation | Main conditional notice names per-Game failures; other Games are checked independently. Preserve finished/pending state and never report a failed Game as repaired. |

After a recovery action, keep result/error feedback outside any conditional row,
badge, proposal, or notice that the refresh can remove. Preserve stable focus
and the existing live-region announcements. Do not erase an error when the
Settings dialog opens, closes, or changes page, or when a refreshed row
disappears (#285 invalidation constraint and the committed mutation audit).

### Per-surface gates

`safe_to_proceed` is deliberately CLI-shaped. It is a reporting input for the
indicator, **not a global disable switch** (#285). False can describe an active
Game Session or one unreferenced folder. Neither justifies disabling navigation,
Settings, diagnostics, every Game, or every Mod.

Keep existing operation gates and backend checks (#282, #285): Launch respects
session/launch reservation and affected Game recovery; Mod controls respect
session and that Mod's quarantine/transition; importer changes respect that
Game's evacuation; Library operations respect overlapping roots and witnesses
for owned paths. A readable Game with no Model Importer can be configured.
Unreadable state must not invite an install that overwrites unverified files.
Unknown gate queries are not successful checks. Backend refusals remain visible
on the responsible surface; frontend summaries never replace those checks.

## Data contracts and refresh discipline

Reuse existing domain types and Tauri commands. This is a layout specification,
not a new Rust/TypeScript API design (#285). Existing read contracts include
`GameSummary`/`list_supported_games`, `get_game_install_path`, `LibraryPaths`/
`get_library_paths`, `UpdateStatus`/`check_importer_update`,
`ImporterOriginStatus`/`importer_origin_status`, `AttentionReport`/
`attention_status`, `SessionInfo`/`current_session`, and
`get_startup_reconcile_status`. Existing mutation routes include
`set_game_install_path`, `detect_game_install_path`, `set_library_root`,
`set_library_path_for_game`, `install_importer`, `rollback_importer`,
`set_importer_pinned`, and the existing Origin/recovery/Mod commands.

`get_game_install_path` reads the saved setting; it does not prove present
filesystem health. `UpdateStatus.installedVersion` and Origin bookkeeping also
do not provide that proof. Implementation needs a **fallible read-only Game and
Model Importer path-state check**, preserving the uncertainty boundary of the
existing `GameProfile` validator. Name and provide that capability without
calling `set_game_install_path` merely to inspect a path or inventing success
from nullable metadata. This document does not prescribe a new command
signature or domain type (#284 path states; #285).

The [committed mutation audit](../../src/test/path-mutation-audit.md) is the
required refresh inventory (#285). Moving/splitting a surface must carry its
mutation policy, parent callback, error retention, and query keys. In particular:

| Mutation family | Queries to preserve |
| --- | --- |
| Game path save/detection | `installPath/<Game>` and `onboarding/installPaths` on settlement; tabs and Settings share these reads. Refresh the new path-state projection when its underlying path changes. |
| Importer install/rollback/recovery | All `importer` reads and `importerOrigin/<Game>` on settlement, including latest-release data and both main/Settings projections. |
| Origin acceptance/override/dismissal | Preserve each audit row's settlement or atomic-success policy; refresh Origin and importer reads. Global recommendation changes refresh **all Games**. |
| Global/per-Game Library move/reset | Broad `libraryPaths`, `mods`, `libraryAudit`, `conflicts` on settlement, including cached inactive Games. |
| Audit recovery/delete/duplicate resolution | Broad audit, Mods, Conflicts, Mod updates, and Library paths on settlement. |
| Mod enable/disable and recovery | Mods and Conflicts for the affected Game on settlement; preserve Variant and update dependencies for their own actions. |
| Import/adoption | Mods, audit, and Mod updates for the Game on settlement. |
| Launch | Session and interrupted-launch reads on settlement. |
| Proxy save, setup completion/resume | Existing proxy/onboarding refresh and failure retention. |
| Attention | Mount snapshot and explicit refresh only; do not silently add automatic invalidation or polling. |

Success-only refresh remains valid for the audit's atomic writes; operations
that can persist filesystem/recovery state on rejection retain `onSettled`.
Splitting components requires updating that existing inventory's source rows
and retaining their coverage during implementation. Those changes are not part
of this document-only draft. Preserve cached data across navigation; remounting
or using zero stale time is not a substitute for correct invalidation (#285).

## First-run and empty-state landing

The wizard itself is unchanged (#282). After it completes, the main screen
lands on the applicable installed Game. When that Game's successfully loaded
Mod list is empty, candidate A's import content occupies the main content area:
**Adopt**, **Import ZIP**, **GameBanana**, followed by the ZIP drop zone. Each
route names the selected Game and uses its existing command and confirmation
flow (#282, #283). This is not a small toolbar above an empty table.

A Mod query that is pending or failed does not show this successful empty state.
A populated result keeps the existing import toolbar, drop zone, rows,
Variants, Conflict disclosures, and inline recovery. `ModUpdatesPanel` remains
after the content in both empty and populated views (#283). Import pending,
cancelled, success, and failure retain their existing behavior and feedback.

If setup was skipped, retain Resume setup and Run setup again behavior and
their persistent errors. If no install path exists, lead with Add game/setup
access rather than a “setup complete” claim for an arbitrary Game. Opening an
absent Game's Settings page supplies its path editor without requiring a tab
(#282, #284). Creating its tab does not bypass the wizard or install a Model
Importer implicitly.

## Non-goals

The following remain outside this effort (#282, #285):

- Redesigning Mod rows, enabled/disabled grouping, search, sorting, bulk actions,
  or Conflict resolution. v1 warns; users disable a conflicting Mod. It does
  not select a winner (CONTEXT.md).
- Redesigning the first-run wizard or its path-entry flow.
- Deciding the mechanical file split of `src/App.tsx` or defining component code.
- New backend safety policy, Library layout, Source/Variant semantics, update
  policy, Model Importer maintenance, or Loader distribution (ADRs 0001–0005).
- Merging prototypes, creating implementation tickets, closing #285 or the
  map, or enabling automatic merge before human review.

The shell change adds these (#326):

- Transactional settings editing: buffered edits with a real Cancel. It is a
  separate decision, not rejected, and must account for operations that cannot
  be buffered.
- A visual pass. How the application looks is a separate axis from how native
  it feels.
- The selected Mod's docked detail panel, which belongs to #327.

## Implementation acceptance scenarios

These are future implementation checks, not claims that this draft ran the app
(#285: moved components keep coverage and existing tests stay passing).

| Scenario | Required observation |
| --- | --- |
| Three configured Games out of six | Three horizontal main tabs; six selectable Settings rows; no main sidebar. |
| Either Settings arrival | Status-bar link opens the dialog on its Game; the menu Settings item opens it on GMM; neither dialog contains Launch or a Mod list (#326). |
| Switch Game or GMM page inside Settings | Main selection and list position stay unchanged when the dialog closes. |
| Settings dialog open | The main screen stays behind the dialog and accepts no input; the dialog's only dismiss action is **Close**; no OK, Cancel, or Apply exists (#326). |
| Change a setting, then Close | The change was committed by its own editor before Close; Close reverts nothing and nothing claims it did. A test fails if a control's label promises a reversal the implementation does not perform (#326). |
| Native menu | Settings and **Run setup again** are menu items that dispatch their existing actions; **Run setup again** is unavailable while a reset is pending, and its failure stays visible on the main screen (#326). |
| Application identity | The page shows no `GMM` wordmark and no in-page window bar; the OS title bar carries the name (#326). |
| Status bar | The selected Game's status sits at the bottom of the window, with Game and Model Importer as separately labelled facts and every payload state and wording above (#324, #326). |
| Add absent Game, accept/reject path | Its Settings editor is reachable without a tab; accepted save adds membership; rejection does not; previous main selection is preserved. |
| Healthy/unreadable/missing/absent | Render four distinct outcomes from the path table; keep unreadable saved paths; do not infer version/absence. |
| Saved Game file `NotFound`, or another I/O error | Keep saved configuration; identify confirmed missing file versus cannot-tell failure. Neither becomes “no saved path.” |
| Global root versus Game override | Editors appear only on their proper pages; links name the other scope; move results and inactive-Game refresh remain correct. |
| Populated Library | Variants, one Conflict, a Mod update, and one recovery warning remain usable under their own gates. |
| Confirmed empty Library versus loading/error | Only the successful empty result shows the three import routes as primary content. |
| Attention true/false/error/refresh | Quiet/loud/unavailable/checking are distinct; only false triggers loud; all-Games findings remain a dated explicit-refresh snapshot. |
| One quarantined Mod or active session | Existing scoped locks apply; unrelated navigation/settings are usable; `safe_to_proceed` is not a global gate. |
| Recovery action removes its own row | Stable focus and outcome/error survive disappearance; every affected cached query refreshes on the proper success/settlement boundary. |
| Narrow window and light/dark appearance | Accepted hierarchy and Settings list remain usable; paths/forms wrap without horizontal overflow, as the reference assets demonstrate. The minimum window size that defines "narrow" is an open shell decision (#326). |
