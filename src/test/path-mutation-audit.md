# Path input and frontend mutation audit — #269 / #271

## Result and scope

The first-run Library field refreshes after successful and rejected moves. The
four user-settable path fields accept drafts and commit only on Enter, Apply, or
a completed folder pick. Each route invokes the same existing backend mutation;
there is no frontend path validation, trimming, or implicit reset to default.
The diagnostics log directory stays read-only.

The audit walked every TypeScript/TSX file under `src/` and found **41
`useMutation` calls in four files**. Each mutation was traced to its IPC command,
its displayed query state, its success/settlement callbacks, and any parent
invalidation callback. Read-only backend inspection distinguished atomic setting
writes from operations that can leave filesystem or recovery state after failure.
No backend files changed. No shared invalidation module was introduced.

## Complete mutation inventory

`Settled` means invalidation runs after either success or rejection. Existing
success-only invalidation was retained when the relevant persisted write is a
single statement or transaction and no additional displayed query was missing.

| File / component | Mutation | Result and displayed state |
| --- | --- | --- |
| App / MainApp | reopenSetup | Changed: settled `onboarding/status`; setup error survives a route change after partial settings writes. |
| App / SessionBanner | retireLaunch | Unchanged: transactional claim retirement already refreshes `interruptedSessionLaunches` and `session` on success. |
| App / LaunchGameButton | launch | Unchanged: already settles `session` and `interruptedSessionLaunches`. |
| App / ModUpdatesPanel | check | Changed: settles `modUpdates/game`; a multi-Mod batch can persist earlier upstream versions before rejecting. |
| App / ModUpdatesPanel | toggleGlobal | Unchanged: one setting write already refreshes `modUpdates/globalEnabled` on success. |
| App / ModUpdateBadge | apply | Unchanged: already settles `mods/game`, `modUpdates/game`, `variants/modId`, and `conflicts/game`; error remains outside the disappearing badge. |
| App / ModUpdateBadge | toggle | Unchanged: one Mod setting write already refreshes `modUpdates/game` on success. |
| App / NetworkPanel | save | Changed: settles `proxyConfig`; URL, username, and password are separate writes. Success still clears the password draft. |
| App / NetworkPanel | test | Unchanged: connection probe; no persisted query state to refresh. |
| App / ImporterPanel | install | Changed helper: already settled, now also refreshes `importerOrigin/game` and all `importer` queries, including latest release. |
| App / ImporterPanel | rollback | Changed: same expanded helper runs on settlement; filesystem restoration can fail before provenance is recorded. |
| App / ImporterPanel | pin | Atomic success policy retained; shared helper now also refreshes the Origin/latest-release surfaces. No separate missing failure invalidation found for this single setting write. |
| App / ImporterPanel | retireEvacuation | Changed helper: existing settlement now includes Origin/latest-release state restored by recovery. |
| App / ImporterPanel | retryEvacuation | Changed helper: existing settlement now includes Origin/latest-release state restored by recovery. |
| App / LibraryPathsPanel | setRoot | Changed: settles `libraryPaths`, `mods`, `libraryAudit`, and `conflicts`; relocation updates Mod paths and can fail to restore deployment. |
| App / LibraryPathsPanel | setPerGame | Changed: same settlement refresh; broad keys also cover cached inactive Game views. |
| App / Diagnostics | exportBundle | Unchanged: writes a user-selected diagnostics artifact; displayed log-directory query does not change. |
| App / Settings | setPath | Changed: settles `installPath/game` and the wizard's cached `onboarding/installPaths` aggregate. Success still updates the manual-path badge. |
| App / Settings | detect | Changed: same settlement refresh; successful automatic detection persists a Game path. |
| App / RebuildJunctions | rebuild | Changed: settles `mods/game`; quarantine Junction withdrawal can record changed recovery state. The rebuild summary still uses mutation data. |
| App / ModList | toggle | Unchanged: already settles `mods/game` and `conflicts/game`. |
| App / ModList | retryRecovery | Unchanged: already settles `mods/game` and `conflicts/game`; persistent feedback survives disappearing recovery controls. |
| App / ModList | retireEnabledTransition | Unchanged: already settles `mods/game` and `conflicts/game`. |
| App / AdoptButton | adopt | Existing settlement callback retained; parent refresh now also includes `modUpdates/game` alongside `mods/game` and `libraryAudit/game`. Shared import callback. |
| App / GameBananaImport | ingest | Changed parent callback: settles `modUpdates/game` as well as Mods/audit; fresh cached update rows otherwise omit a newly imported Mod. |
| App / VariantSelector | switchVariant | Unchanged: transactional active-Variant update already refreshes `variants/modId` and `conflicts/game` on success. Existing refetch guard stays intact. |
| App / ImportZipButton | importMutation | Existing settlement callback retained; same expanded parent callback as adoption/GameBanana import. |
| App / ZipDropZone | importMutation | Existing settlement callback retained; same expanded parent callback. |
| ImporterOriginPanel | accept | Changed: settles `importerOrigin/game` and `importer`; proposal installation can leave recovery state after rejection. Error is outside the conditional proposal. |
| ImporterOriginPanel | decline | Unchanged invalidation: atomic dismissal already refreshes Origin/importer on success. Its error notice now stays outside the proposal with the install error. |
| ImporterOriginPanel | undo | Unchanged: atomic dismissal restoration already refreshes Origin/importer on success. |
| ImporterOriginPanel | toggleRecommendations | Changed: successful global preference write refreshes all `importer` queries as well as `importerOrigin`; toggling can change the resolved install/release target. |
| ImporterOriginPanel / OverrideEditor | save | Unchanged: transactionally changes override/install/pin state; `onChanged` calls the parent's Origin/importer invalidation. Success still clears editor drafts. |
| ImporterOriginPanel / OverrideEditor | clear | Unchanged: same transactional write and parent invalidation. |
| LibraryAuditWarning | reveal | Unchanged: reveals a folder; no persisted query state changes. |
| LibraryAuditWarning | recover | Changed: settles audit/Mods/Conflicts/update rows/Library paths. Recovery can rename a folder before later work fails. |
| LibraryAuditWarning | remove | Changed: same settlement refresh; delete can quarantine a directory before failure. |
| LibraryAuditWarning | resolveDuplicates | Changed: same broad settlement refresh, including cached Games whose rejected records are removed. Error remains visible if the last audit row disappears. |
| OnboardingWizard | close | Changed: settles `onboarding/status`; completion writes two settings. App retains errors if refreshed status changes the route. Standalone wizard retains its own error notice. |
| OnboardingWizard / DetectStep | setPath | Changed: settles `installPath/game` and cached `onboarding/installPaths`; successful row override remains local. |
| OnboardingWizard / LibraryStep | setRoot | Changed: missing invalidation fixed with settlement refresh of Library paths, Mods, audit, and Conflicts. |

The wizard's sequential `ImporterStep.installOne` is not a mutation hook, but it
also writes persisted state. Its `finally` block now invalidates `importer` and
`importerOrigin/game` after each attempted installation.

`AttentionView` intentionally remains an explicit-refresh, time-stamped snapshot
per its existing product contract. Its query is not automatically invalidated.
Static registry, AV guidance, loader version, startup reconcile report, and
informational diagnostics queries have no missing writer among these hooks.

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

- VERIFIED — `pnpm test`: **157 passed, 0 failed, 14 files**.
- VERIFIED — `pnpm build`: TypeScript and Vite production build succeeded.
- VERIFIED — `git diff --check`: no whitespace errors.
- NOT-RUN — native Windows/Tauri GUI and actual filesystem path validation.
- NOT-RUN — Rust/backend tests; no backend files changed.

The IPC boundary is mocked for component tests. These tests prove frontend
command routing, cache refresh, error retention, and explicit input commitment;
they do not prove a native Library move or Windows folder validation.
Read-only inspection found that the existing `Core::set_game_install_path` stores
the supplied string without checking filesystem existence or the executable.
Typed and picked paths use that same unchanged command. No new client validation
was added; native rejection of a nonexistent Game path remains an existing
backend limitation, tracked separately in #276.
