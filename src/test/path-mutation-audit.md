# Path input and frontend mutation audit — #269 / #271

## Result and scope

The first-run Library field refreshes after successful and rejected moves. The
four user-settable path fields accept drafts and commit only on Enter, Apply, or
a completed folder pick. Each route invokes the same existing backend mutation;
there is no frontend path validation, trimming, or implicit reset to default.
The diagnostics log directory stays read-only.

The inventory contains **43 `useMutation` call sites in 12 files** and is now
enforced by `mutation-invalidation-boundary.test.ts` under `pnpm test`. The gate
parses production TypeScript/TSX under `src/`, including new files, and requires
exactly one row per call site and no stale rows. The original audit traced each
mutation to its IPC command,
its displayed query state, its success/settlement callbacks, and any parent
invalidation callback. Read-only backend inspection distinguished atomic setting
writes from operations that can leave filesystem or recovery state after failure.
The original audit changed no backend files. #292 adds a shared Core blocking
boundary for path-state inspection. No shared frontend invalidation module was introduced.

Reconciliation for #287 found the same 41 call sites, with no additional missing
invalidation. The reported 46 text occurrences were 41 calls, four imports, and
one mention in this document. The three intentional no-refresh sites now carry
literal `meta.noInvalidationReason` values matching their existing decisions.

The #301 finished-layout sweep reconciled all 41 sites in 10 files against the
table below. No missing, duplicated or stale rows were found. MainApp's Launch
and session hooks, retained Settings pages, extracted Library editors and the
shared import settlement callbacks still map to their recorded owners and query
keys. The gate and rendered invalidation suite remain complementary: the gate
checks declarations and inventory, while rendered tests check the affected
cached reads and surviving feedback. The known pin/recommendations path-state
over-refresh from #301's scope comment remains conservative and unchanged.

## Complete mutation inventory

`Settled` means invalidation runs after either success or rejection. Existing
success-only invalidation was retained when the relevant persisted write is a
single statement or transaction and no additional displayed query was missing.

To add a mutation, use a named binding in a named component/function and inline
options without spreads. Record either an `onSuccess`/`onSettled` invalidation
callback or a non-empty literal `meta.noInvalidationReason`, then add its row.
Rows use the source path relative to `src/` without the extension, followed by
` / Component` when the function name differs, and the mutation binding name.
This distinguishes same-named bindings in different components or files.

The gate recognizes visible `invalidateQueries` calls, local callback references,
and handoffs to `onX` component callback parameters. It rejects callbacks that
only change local state. This is structural evidence of a recorded decision;
review still checks parent callback behavior, query keys, and whether success or
settlement is correct. It does not prove runtime invalidation or follow semantic
aliases/type information. Tests and test support under `src/test/` are excluded
from the production inventory.

| File / component | Mutation | Result and displayed state |
| --- | --- | --- |
| App / MainApp | reopenSetup | Changed: settled `onboarding/status`; setup error survives a route change after partial settings writes. |
| App / useSessionPresentation | retireLaunch | Unchanged: transactional claim retirement already refreshes `interruptedSessionLaunches` and `session` on success. |
| App / useLaunchGame | launch | Unchanged: already settles `session` and `interruptedSessionLaunches`. |
| App / ModUpdatesPanel | check | Changed: settles `modUpdates/game`; a multi-Mod batch can persist earlier upstream versions before rejecting. |
| App / ModUpdatesPanel | toggleGlobal | Unchanged: one setting write already refreshes `modUpdates/globalEnabled` on success. |
| ModDetailPanel | save | Atomic panel-width setting write after 250 ms without changes or on blur; serial requests preserve settled input order, and success updates `modPanelWidth` directly from the bounded input. Literal `meta.noInvalidationReason` records why no query invalidation is needed. |
| ModUpdateBatchAction | apply | Settles Mods/update rows/Conflicts/Variants/attention/audit after success or rejection; publishes backend Conflict findings with omissions marked unverified and unconditionally refreshes mounted Conflict observers. Unknown results and errors survive refreshed update rows; preflight refusals do not invent Unknown outcomes. |
| ModList / ModUpdateBadge | apply | Unchanged: already settles `mods/game`, `modUpdates/game`, `variants/modId`, and `conflicts/game`; error remains outside the disappearing badge. |
| ModList / ModUpdateBadge | toggle | Unchanged: one Mod setting write already refreshes `modUpdates/game` on success. |
| SettingsDestination / NetworkPanel | save | Changed: settles `proxyConfig`; URL, username, and password are separate writes. Success still clears the password draft. |
| SettingsDestination / NetworkPanel | test | Unchanged: connection probe; no persisted query state to refresh. |
| ImporterPanel | install | Changed helper: already settled, now also refreshes `gamePathStatus/game`, `importerOrigin/game` and all `importer` queries, including latest release; stable install feedback remains on the Game page. |
| ImporterPanel | rollback | Changed: same expanded helper, including `gamePathStatus/game`, runs on settlement; filesystem restoration can fail before provenance is recorded; null success explicitly reports no rollback backup. |
| ImporterPanel | pin | Atomic success policy retained; shared helper now also refreshes path-state/Origin/latest-release surfaces. No separate missing failure invalidation found for this single setting write. |
| ImporterPanel | retireEvacuation | Changed helper: existing settlement includes path-state/Origin/latest-release state restored by recovery; outcome/error stays outside the removable warning. |
| ImporterPanel | retryEvacuation | Changed helper: existing settlement includes path-state/Origin/latest-release state restored by recovery; outcome/error stays outside the removable warning. |
| LibraryPathEditors / GlobalLibraryRootEditor | setRoot | Relocated: move/reset settles broad `libraryPaths`, `mods`, `libraryAudit`, and `conflicts`, including cached inactive Games, on success or rejection; relocation can persist paths or recovery state before failing. |
| LibraryPathEditors / GameLibraryOverrideEditor | setPerGame | Relocated: move/reset retains the same broad settlement keys, including cached inactive Games, on success or rejection; the displayed Game does not narrow refresh. |
| Diagnostics | exportBundle | Unchanged: writes a user-selected diagnostics artifact; displayed log-directory query does not change. |
| SettingsDestination / GamePathPage | setPath | Changed: settles `gamePathStatus/game`, `installPath/game` and the wizard's cached `onboarding/installPaths` aggregate. Success still updates the manual-path badge. |
| SettingsDestination / GamePathPage | detect | Changed: same path-state/install-path settlement refresh; successful automatic detection persists a Game path. |
| SettingsDestination / RebuildJunctions | rebuild | Changed: settles `mods/game`; quarantine Junction withdrawal can record changed recovery state. The rebuild summary still uses mutation data. |
| ModList | toggle | Unchanged: already settles `mods/game` and `conflicts/game`. |
| ModList | retryRecovery | Unchanged: already settles `mods/game` and `conflicts/game`; persistent feedback survives disappearing recovery controls. |
| ModList | retireEnabledTransition | Unchanged: already settles `mods/game` and `conflicts/game`. |
| ModList / AdoptButton | adopt | Existing settlement callback retained; parent refresh now also includes `modUpdates/game` alongside `mods/game` and `libraryAudit/game`. Shared import callback. |
| ModList / GameBananaImport | ingest | Changed parent callback: settles `modUpdates/game` as well as Mods/audit; fresh cached update rows otherwise omit a newly imported Mod. |
| ModList / VariantSelector | switchVariant | Unchanged: transactional active-Variant update already refreshes `variants/modId` and `conflicts/game` on success. Existing refetch guard stays intact. |
| ModList / ImportZipButton | importMutation | Existing settlement callback retained; same expanded parent callback as adoption/GameBanana import. |
| ModList / ZipDropZone | importMutation | Existing settlement callback retained; same expanded parent callback. |
| ImporterOriginPanel | accept | Changed: settles `gamePathStatus/game`, `importerOrigin/game` and `importer`; proposal installation can leave recovery state after rejection. Error is outside the conditional proposal. |
| ImporterOriginPanel | decline | Existing success boundary retained; shared helper adds path-state to Origin/importer refresh after atomic dismissal. Its error notice now stays outside the proposal with the install error. |
| ImporterOriginPanel | undo | Existing success boundary retained; shared helper adds path-state to Origin/importer refresh after atomic dismissal restoration. |
| ImporterRecommendations | toggleRecommendations | Changed: successful global preference write refreshes all `gamePathStatus` and `importer` queries as well as `importerOrigin`; toggling can change the resolved install/release target. |
| ImporterOriginPanel / OverrideEditor | save | Existing transactional success boundary retained; `onChanged` calls the parent's expanded path-state/Origin/importer invalidation for override/install/pin changes. Success still clears editor drafts. |
| ImporterOriginPanel / OverrideEditor | clear | Unchanged: same transactional write and parent invalidation. |
| LibraryAuditWarning | reveal | Unchanged: reveals a folder; no persisted query state changes. |
| LibraryAuditWarning | recover | Changed: settles audit/Mods/Conflicts/update rows/Library paths. Recovery can rename a folder before later work fails. |
| LibraryAuditWarning | remove | Changed: same settlement refresh; delete can quarantine a directory before failure. |
| LibraryAuditWarning | resolveDuplicates | Changed: same broad settlement refresh, including cached Games whose rejected records are removed. Error remains visible if the last audit row disappears. |
| OnboardingWizard | close | Changed: settles `onboarding/status`; completion writes two settings. App retains errors if refreshed status changes the route. Standalone wizard retains its own error notice. |
| OnboardingWizard / DetectStep | setPath | Changed: settles `gamePathStatus/game`, `installPath/game` and cached `onboarding/installPaths`; successful row override remains local. |
| OnboardingWizard / LibraryStep | setRoot | Changed: missing invalidation fixed with settlement refresh of Library paths, Mods, audit, and Conflicts. |

The wizard's sequential `ImporterStep.installOne` is not a mutation hook, but it
also writes persisted state. Its `finally` block invalidates `gamePathStatus/game`,
`importer` and `importerOrigin/game` after each attempted installation. App also
refreshes all `gamePathStatus` queries when setup completes.

`AttentionView` intentionally remains an explicit-refresh, time-stamped snapshot
per its existing product contract. Its query is not automatically invalidated.
Static registry, AV guidance, loader version, startup reconcile report, and
informational diagnostics queries have no missing writer among these hooks.

## Settings destination relocation — #292

`GamePathPage.setPath`, `GamePathPage.detect`, `RebuildJunctions.rebuild`,
`NetworkPanel.save`, and `NetworkPanel.test` moved from `App` to
`SettingsDestination`. Their settlement/success policies and existing query keys
are preserved. Path save/detection additionally settle `gamePathStatus/<Game>`.
`LibraryAuditWarning` stays in its own file with the same four inventory rows
and broad settlement refresh. Its attachment moved with the path editor; initial
loading and unavailable checks are explicit, and retained findings on query
failure are identified as previous.

The destination retains visited Settings pages, including their child mutation
observers and feedback, across page changes and Back/Escape. Inactive pages use
`hidden` and are not exposed to keyboard or accessibility navigation. Main
content unmounts when Settings opens; its selected Game, Mod query observer,
session-event owner, and return anchor remain above the destination switch.
No new mutation hook or blanket gate exception was added.

## Per-Game importer relocation — #294

`ImporterPanel` moved into its own module. Its five mutation policies are
unchanged: install, rollback, retry and retirement settle all `importer` reads,
this Game's `importerOrigin` and `gamePathStatus`; the atomic pin write refreshes
those reads on success. The two groups now follow the Game install-path group
inside each visited Settings page. Their live observers, drafts and operation
feedback survive hidden pages and Close. Recovery errors and outcomes
remain outside the warning that settlement can remove. Origin acceptance still
settles, while override/dismissal writes retain their atomic-success boundary.

Importer and Origin install actions require a successful readable Game and a
verified installed or confirmed-absent Model Importer. Filesystem failure never
supplies an installed version or an install invitation. Upstream check failures
remain distinct from a current version. The frontend boundary tests exercise
active projections and fresh inactive caches; the inventory gate alone does not
prove these query keys or runtime policies.

## GMM settings groups relocation — #296

`Diagnostics.exportBundle` moved from `App` into its own module, retaining the
local artifact command and explicit no-invalidation decision. The global
`toggleRecommendations` moved from `ImporterOriginPanel` into
`ImporterRecommendations`. Its atomic-success boundary and all-Game
`importerOrigin`, `importer`, and `gamePathStatus` prefixes are unchanged; it
also refreshes the dedicated global preference query. Two-Game behavioral
regressions assert refreshed Origin, release, update and path projections in
both Games' DOM under the production 30-second cache window, and observe both
Games' evacuation checks re-run. Narrowing any Game prefix to either one Game,
or the importer prefix to latest/update/evacuation alone, is tested independently
by source mutation.

The GMM page retains its Library root first and Network second, followed by
GMM updates, importer recommendations and Diagnostics. `ModUpdatesPanel` stays
on the main screen. The three preference tests previously bound to the per-Game
checkbox (write, structured failure, pending/success) now exercise the global
module, including the saved checkbox value and other-Game refresh behavior.
No inventory gate or per-Game action coverage was weakened.

## Main-screen hierarchy — #293

The inventory still contains 41 call sites in nine files. `SessionBanner.retireLaunch`
and `LaunchGameButton.launch` now live in the same source file under
`useSessionPresentation` and `useLaunchGame`. The hooks keep their existing
success/settlement policies while separating the global session context and
Game-header action from conditional operation notices. They stay observed
across Settings navigation. No mutation moved to a new file or gained a new
invalidation exception.

The new status strip reads `gamePathStatus/<Game>` and the recovery summary
reads `importer/evacuation/<Game>`, sharing the Settings keys and existing
refresh boundaries. The strip contains only a Settings route; recovery actions
remain in Settings. Rendered tests cover retained configured tabs, named
failures, explicit checking, confirmed-empty navigation, the eight content
slots, individual Mod gates, and both Settings arrival/return paths. Behavioral
source mutations remove each Mod/Conflict/Launch refresh independently.

## Observed regression evidence

Tests were written and run before each fix. Exact observed failures included:

- #269: `findByDisplayValue("D:\\Mods\\Library")` could not find the refreshed
  wizard path after either successful or rejected folder changes: **2 failed**
  before invalidation, then **2 passed**.
- Audit: Settings Library moves did not show `D:\\Library` or refreshed disabled
  Mod state; failed batch checking did not show `Update → 3.0`; failed rebuild did
  not show the new reinstall recovery region: **4 failed**, then passed.
- Audit: `queryByText(oldAuditPath)` remained present after rejected recovery,
  delete, or duplicate resolution; the rejected Origin-install button also
  remained present: **4 regressions failed**, then passed. Failures remain visible
  after refreshed rows disappear.
- Audit: `findByText("Example/Restored-Package")` failed after ordinary importer
  installation and rollback: **2 failed**, then passed.
- Audit: `findByDisplayValue("http://persisted-proxy:8080")` and refreshed setup
  route headings were missing after partial Network/setup writes: **3 failed**,
  then **3 passed**, with original errors retained.
- Audit: `findByRole("button", { name: "Update → 4.0" })` was missing for a newly
  imported GameBanana Mod when the QueryClient used production's **30-second
  stale time**: **1 failed**, then passed after refreshing update rows. The App
  regression suite now uses that production cache window; zero-stale-time mounts
  had concealed this gap.
- #271: **9 typed/pasted valid and refused-path tests failed** on the picker-only
  fields before implementation; the edit action reported that `clear()` requires
  an editable field. All nine then passed. A subsequent temporary `readOnly`
  source mutation produced **4 failures** at the explicit assertion
  `expect(field).not.toHaveAttribute("readonly")`; removing that mutation returned
  all four to green.

Typed input tests also assert no command call during editing and exactly one
call after Enter. Refused paths surface the exact backend message, including a
Library/backups overlap refusal. Picker tests cover all four field types, draft
replacement by a completed pick, unchanged dialog options, cancellation, and
pending-move control disabling. Diagnostics remains read-only.

## Final verification and limits

- VERIFIED — original #269/#271 audit: `pnpm test`, **157 passed, 0 failed, 14 files**.
- VERIFIED — #292: full frontend suite, TypeScript check and production build;
  all five existing Rust AST gates and the path-status runtime boundary gate.
- VERIFIED — `pnpm build`: TypeScript and Vite production build succeeded.
- VERIFIED — `git diff --check`: no whitespace errors.
- NOT-RUN — native Windows/Tauri GUI, real user-drive path validation, and
  screen-reader/scroll geometry checks.
- VERIFIED — #292: `cargo build --workspace` followed by all Rust workspace
  targets, plus Clippy with warnings denied and Rust formatting. Four existing
  live/fixture tests remain ignored; native GUI limits above still apply.

The IPC boundary is mocked for component tests. These tests prove frontend
command routing, cache refresh, error retention, and explicit input commitment;
they do not prove a native Library move or Windows folder validation.
Typed and picked paths use the same backend command, including its existing
fallible GameProfile validation. Rust path-status suites inspect temporary
filesystem fixtures; they do not validate a real user-selected Windows drive.
No new client validation
was added; native rejection of a nonexistent Game path remains an existing
backend limitation, tracked separately in #276.

## Confirmed-empty import landing — #300

`ModList` and its Mod-row and import controls moved into their own module.
All ten moved mutation sites retain their policies and query keys. Imports
and adoption use one parent callback for `mods/game`, `libraryAudit/game`, and
`modUpdates/game` on settlement in both layouts. Controls retain their mounted
identity when the first Mod changes the landing into the populated toolbar.
Only a successful empty Mod query shows the import landing; pending and failed
reads keep explicit loading or error feedback. The inventory now contains
41 call sites in ten files.

## Selected Mod inspector — #327

The panel adds one width-setting mutation in its own component. The preference
lives in `mods.panelWidthCssPx`; it does not change Mod data or native window
geometry. Requests run in order, successful writes update the cached width,
and read or write errors remain visible in the inspector. The inventory now
contains 43 call sites in 12 files.
