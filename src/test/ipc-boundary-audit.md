# IPC boundary structural audit — #438

The conditional error gate inventories conditional `useQuery` observers. It guards
all reads of `error`, `isError`, `isLoadingError`, `isRefetchError`, `failureReason`,
`failureCount` and `status`, including `status === "error"`. These observer values
can retain failure state after `enabled` becomes false. All status reads are
conservatively guarded, even comparisons to other statuses.

The mutation scope gate inventories every `useMutation`. It follows local bodies
from `onMutate`, `onError`, `onSuccess` and `onSettled`. It also scans **direct cache
writes anywhere in the options object**, including `mutationFn` progress callbacks.
This closes that hole without classifying every backend API call in mutationFn as
an opaque callback. Invalidation requests a fresh read; it is not a direct write.
Any `.mutate` / `.mutateAsync` call with a second argument fails closed. Move those
callbacks to inventoried hook options; per-call options are not analysed.

A single writer must serialize repeated invocations and mounted instances.
Writers with the same literal array head must share one literal scope ID or one
scope template with a nonempty literal head. Templates must have identical syntax
(taking literal text and expression trees into account, ignoring trivia).

Rule 1 supports a per-site review allowance in inline query options:
`meta: { conditionalErrorBoundaryReason: "A specific reviewed reason" }`.
The allowance cell must exactly match that literal reason; absent metadata requires
`none`. Empty, nonliteral and unmatched allowances fail. The reason column must
explain why the consumer remains safe. This deliberately bypasses Rule 1 syntax
analysis at that site; review must check the actual guard and consumers. Use it
for guarded destructuring, custom hooks returning observers, and guarded effects
when direct analysis cannot prove safety. Destructured bindings use their binding
pattern in the site key. Negated-disjunction early returns such as
`if (!open || !mod) return null` for `enabled: open && Boolean(mod)` are supported
without an allowance.

Rule 2 opaque callback handoffs are listed by exact callee name below. Their bodies
remain a review boundary; this is a declared exception, not inferred safety.
Opaque mutation keys/options have no production allowance.

The query evidence ratchet inventories every `useQuery` and `useQueries` reading by
how its owner consumes it. Its rule number is 3 and its own table is below.

| Rule | Site | Opaque callback allowance | Reason |
| --- | --- | --- | --- |
| 1 | GamePresets::currentMods | none | Every observer status and error read is under recapturing, the same boolean that enables the current-setup confirmation query. |
| 1 | ModDetailPanel::remembered | none | The early return when closed gates the width-read error on open. |
| 1 | ModDetailPanel::updates | none | The upstream notice shares hasUpstream with enabled. |
| 1 | OnboardingWizard / ImporterStep::installPaths | none | No observer error render; per-Game result errors are separate data. |
| 2 | App / MainApp::reopenSetup | onResumeWizard, onSetupFailure | Parent callbacks reset wizard/feedback state and report setup failure. Settlement refreshes onboarding status; neither callback installs cache data. |
| 2 | App / useSessionPresentation::retireLaunch | commandFailureMessage, game.toUpperCase, retirement.finish, retirement.start | commandFailureMessage and toUpperCase format messages; retirement.start/finish publish local recovery feedback. Settlement refreshes session and launch reservations. |
| 2 | App / useLaunchGame::launch | guidance.refetch | Error handling refetches exclusion guidance from its query; this refresh installs no mutation return value. |
| 2 | App / ModUpdatesPanel::check | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | App / ModUpdatesPanel::toggleGlobal | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | GamePresets::rename | setAction | Only clears local action state and invalidates presets/game; no direct cache write. |
| 2 | GamePresets::remove | setAction | Only clears local action state and invalidates presets/game; no direct cache write. |
| 2 | GamePresets::recapture | setAction | Only clears local action state and invalidates presets/game; no direct cache write. |
| 2 | GamePresets::save | setName | Successful atomic capture clears only the local name draft and invalidates presets/game; it installs no cache data. The listing query is unconditional. |
| 2 | GamePresets::apply | (old?.mods ?? []).map, commandFailureMessage, old?.results.find, setApplyState | Message formatting and local apply state only; no direct cache writer. Settlement unconditionally refreshes Conflicts and affected Library reads. Scope conflicts-${game} serializes operations within this Game. |
| 2 | Diagnostics::exportBundle | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | ImporterOriginPanel::accept | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | ImporterOriginPanel::decline | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | ImporterOriginPanel::undo | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | ImporterOriginPanel / OverrideEditor::save | onChanged, setAssetPattern, setOwner, setRepo | Draft setters clear local fields; parent onChanged is ImporterOriginPanel.invalidate and only refreshes queries. |
| 2 | ImporterOriginPanel / OverrideEditor::clear | onChanged | Parent onChanged is ImporterOriginPanel.invalidate and only refreshes queries. |
| 2 | ImporterPanel::install | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | ImporterPanel::rollback | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | ImporterPanel::pin | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | ImporterPanel::retireEvacuation | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | ImporterPanel::retryEvacuation | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | ImporterRecommendations::toggleRecommendations | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | LibraryAuditWarning::reveal | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | LibraryAuditWarning::recover | reportRef.current?.focus, setFeedback, setOpen | publishFeedback moves focus and updates local feedback/open state; refresh only invalidates queries. |
| 2 | LibraryAuditWarning::remove | reportRef.current?.focus, setFeedback, setOpen | publishFeedback moves focus and updates local feedback/open state; refresh only invalidates queries. |
| 2 | LibraryAuditWarning::resolveDuplicates | reportRef.current?.focus, setFeedback, setOpen | publishFeedback moves focus and updates local feedback/open state; refresh only invalidates queries. |
| 2 | LibraryPathEditors / GlobalLibraryRootEditor::setRoot | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | LibraryPathEditors / GameLibraryOverrideEditor::setPerGame | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | ModDetailPanel::save | none | Direct cache writer; literal scope serializes repeated calls and instances. |
| 2 | ModList / ModUpdateBadge::apply | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | ModList / ModUpdateBadge::toggle | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | ModList::toggle | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | ModList::retryRecovery | commandFailureMessage, sectionRef.current?.focus, setRecoveryFeedback | commandFailureMessage formats the failure; focus and feedback setters affect local UI. Settlement refreshes Mods and Conflicts. |
| 2 | ModList::retireEnabledTransition | commandFailureMessage, retirement.finish, retirement.start | Message formatting and retirement.start/finish publish local recovery feedback. Settlement refreshes Mods and Conflicts. |
| 2 | ModList::releaseEnabledTransition | commandFailureMessage, retirement.finish, retirement.start | Message formatting and retirement.start/finish publish local release feedback. Settlement refreshes Mods and Conflicts. |
| 2 | ModList / AdoptButton::adopt | onAdopted, setName, setOpen_, setPicked | Draft setters only change local UI; onAdopted/onImported is ModList.invalidate, which only refreshes Mods, audit and update rows. |
| 2 | ModList / GameBananaImport::ingest | onImported, setInput, setOpen | Draft setters only change local UI; onAdopted/onImported is ModList.invalidate, which only refreshes Mods, audit and update rows. |
| 2 | ModList / VariantSelector::switchVariant | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | ModList / ImportZipButton::importMutation | onImported, setName, setOpen_, setPicked | Draft setters only change local UI; onAdopted/onImported is ModList.invalidate, which only refreshes Mods, audit and update rows. |
| 2 | ModList / ZipDropZone::importMutation | onImported, setName, setPendingPath | Draft setters only change local UI; onAdopted/onImported is ModList.invalidate, which only refreshes Mods, audit and update rows. |
| 2 | ModUpdateBatchAction::apply | commandFailureMessage, old.mods.map, old.results.find, setBatches, update | Direct Conflicts writer uses conflicts-${game} to serialize within each Game. Message formatting and array map/find read local results; setBatches and update change local batch presentation. |
| 2 | OnboardingWizard::close | onDone, onFailure | Parent onDone clears wizard state and invalidates path/importer queries; onFailure records local failure feedback. Neither installs a direct cache value. |
| 2 | OnboardingWizard / DetectStep::setPath | setOverrides | setOverrides only stores local path overrides. Settlement refreshes install and path status queries. |
| 2 | OnboardingWizard / LibraryStep::setRoot | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | SettingsDestination / NetworkPanel::save | setClearPassword, setPassword | Setters clear the local password draft and checkbox. Settlement refreshes proxyConfig. |
| 2 | SettingsDestination / NetworkPanel::test | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | SettingsDestination / GamePathPage::setPath | setDetectFailed, setLastSource | Setters update local path-source feedback. Settlement only refreshes install and path status queries. |
| 2 | SettingsDestination / GamePathPage::detect | setDetectFailed, setLastSource | Setters update local path-source feedback. Settlement only refreshes install and path status queries. |
| 2 | SettingsDestination / RebuildJunctions::rebuild | none | No direct cache write in success/settlement; refresh or local UI only. |

## Rule 3 — presence-keyed query readings — #482

React Query keeps the last value when a refresh fails, and keeps it with
`status: "success"` when a refresh is paused because the app is offline. A surface
that states a claim from `data`, `isLoading`, `isSuccess` or any other observer
property therefore presents an old read as a current one. `readEvidence` in
`src/queryEvidence.ts` is the shared reading rule that separates the four cases.

Rule 3 does not forbid a presence-keyed reading. It freezes the set. Every
`useQuery`/`useQueries` call whose result is consumed, and which is not routed
entirely through `readEvidence`, carries exactly one row below naming the **exact
observer properties its owner keys off** and why asserting them there is honest. A
new reading fails the gate until it has a row, a new property on an inventoried
reading fails until the row names it, and a row for a reading that no longer exists
fails as stale. Converting a reading to `readEvidence` and deleting its row leaves
the gate green, so #483 can remove these rows one at a time.

Only three consumption shapes count as routed through the evidence rule: the
observer passed to `readEvidence`, a `.refetch` control (asking for another read
claims nothing about the value already held), and a `useQueries` collection mapped
through `readEvidence`. Everything else is recorded, and anything the gate cannot
follow — an alias, an element access, the observer handed to another function or
component, a destructuring rest element — is recorded as `<forwarded>` rather than
assumed safe.

The standard each reason has to meet: the surface may assert its last value when a
value one refresh out of date leads the user to the same correct action, and the
surface does not use it to say that something is absent, zero, safe, verified, up to
date or finished. A reason that could not be written honestly says so in the row and
is listed under the findings below; those are defects for #483, not fixed here.

| Rule | Site | Presence-keyed reads | Reason |
| --- | --- | --- | --- |
| 3 | App / MainApp::mods | data | Pairs update rows with Mod names in the Mod updates panel only. The Mod list beside it owns the Library reading and renders its failure, so the empty fallback drops update rows rather than claiming an empty Library. |
| 3 | App / MainApp::installPaths | <forwarded> | The tab strip reads this collection through readEvidence. The one element forwarded to GameStatusStrip is the saved install path, a stored setting only the user changes, and the strip labels it "Last saved Game path" whenever that read failed or is in flight. |
| 3 | App / useSessionPresentation::session | data, isSuccess | The running banner appears only for a Session a read established, and the Mod lock it explains is enforced in the Core. A Session that ended while the refresh was paused leaves the banner withholding editing rather than permitting it. |
| 3 | App / useSessionPresentation::interruptedLaunches | data | No honest reason. An unread list renders no warnings at all, an interrupted launch reservation needs the user's explicit confirmation, and nothing on the main screen says this read failed. Defect for #483. |
| 3 | App / useLaunchGame::guidance | data | Disabled until a launch fails with an AV-pattern error, then refetched. Absent guidance falls back to the plain failure notice, so this read decides how much help text is shown and claims nothing about the Game. |
| 3 | App / ModUpdatesPanel::global | data | No honest reason. The checkbox is `global.data ?? true`, which states that the weekly check is enabled without having read the preference. ImporterRecommendations is the honest treatment of the same preference shape. Defect for #483. |
| 3 | App / ModUpdatesPanel::rows | data, isSuccess | The count beside the heading is a hint and is omitted at zero, and isSuccess is forwarded to ModUpdateBatchAction as `verified`, which disables Apply all and says "Available Mod updates are unverified". A paused refresh keeps isSuccess true; closing that is a migration step. |
| 3 | AttentionView::report | data, dataUpdatedAt, error, isError, isFetching, isSuccess | Every claim is dated. The report renders dataUpdatedAt as "Checked at", says "Previous snapshot" while refetching or after a failure, requires an explicit safeToProceed boolean before saying anything about safety, and states that each action checks its own availability. |
| 3 | Diagnostics::logDir | data, error, isError, isFetching, isPending, isSuccess | The log directory is a fixed per-install path. Pending, failed and "from the previous check" are each rendered, Export is disabled unless the last read succeeded, and the Core re-reads the directory when the bundle is written. |
| 3 | GamePresets::currentMods | data, error, isFetching, isSuccess | Enabled only while the re-capture dialog is open. Confirm requires isSuccess with no refresh in flight, so a failed read leaves the dialog unable to re-capture and says so instead of capturing a guessed setup. |
| 3 | GamePresets::presets | data, error, isPending, isSuccess | A saved preset list changes only through this panel's own mutations, which invalidate it. "No saved presets for this Game" is gated on isSuccess, and a failed read renders "Could not list presets." |
| 3 | GameRecoveryNotice::recovery | data, error, isError, isFetching | No honest reason for the unestablished case. A failed read renders the failure and a refetch is marked as the previous report, but a paused or never-completed read renders nothing, which on the main screen reads as "no Model Importer recovery outstanding". Defect for #483. |
| 3 | GameStatusStrip::status | data, error, isFetching | The summary line is all this strip claims, and every branch it cannot verify says "Cannot tell" or "unverified". A failed or in-flight read replaces the line rather than dressing it, and the path is labelled "Last saved Game path" in both. |
| 3 | ImporterOriginPanel::path | data, isError, isFetching | The filesystem reading is dropped unless the last read succeeded and no refresh is in flight, so Install availability and the files description fall back to unavailable rather than to the previous answer. |
| 3 | ImporterOriginPanel::status | data, error, isError, isFetching, isPending, isSuccess | Pending and failed reads each get their own line, a retained Origin is labelled "from the previous check", and an Importer Origin is a recorded setting that only an explicit origin change moves. |
| 3 | ImporterPanel::path | data, error, isError, isFetching, isPending | The path line is "Checking Game and Model Importer…" while in flight and "Cannot tell · Model Importer filesystem unverified" on a failure or an absent read, and the importer action stays "unavailable" unless a successful read said readable. |
| 3 | ImporterPanel::release | data, error, isError, isFetching, isPending | The line reads "checking…", "check failed" or "No release available" from the read's own outcome, with the failure named beside it. A retained tag is only ever the last release GMM saw, and installing fetches again. |
| 3 | ImporterPanel::update | data, error, isError, isFetching, isPending | The update badge and the verified installed version both require a read that succeeded with no refresh in flight, and the backend's own checkError is honoured, so an unavailable check shows no badge rather than "up to date". |
| 3 | ImporterPanel::evacuation | data, error, isError, isFetching | The recovery gate fails closed on an unread value: `evacuation.data !== null` is true while data is undefined, so importer actions stay blocked until a read says no evacuation is outstanding. |
| 3 | ImporterRecommendations::preference | data, error, isError, isFetching, isPending, isSuccess | The toggle is rendered only once the preference has been read, is marked "from the previous check" after a failure, and is disabled unless the last read succeeded, so GMM never shows a setting it has not read. |
| 3 | LibraryAuditWarning::audit | data, error, isError, isFetching, isPending | An absent report renders "Checking Library folders…" or the audit-unavailable notice with a refresh button, and a retained report is labelled "from the previous Library check". The audit is advisory and every action on it re-validates in the Core. |
| 3 | LibraryPathEditors / GlobalLibraryRootEditor::paths | data, error, isError | Library paths are stored settings that change only through this editor's own move mutation. A failed read replaces the whole editor with "Library settings are unavailable until GMM can read their current paths" and an absent one says "Resolving paths…". |
| 3 | LibraryPathEditors / GameLibraryOverrideEditor::paths | data, error, isError | The same stored settings and the same unavailable treatment as the global editor, scoped to one Game's override. |
| 3 | ModDetailPanel / ModScreenshotFigure::screenshot | data, isPending | Every state is a statement about the image and never about the Mod. "No screenshot available" always carries its reason, and with no value that reason is "GMM could not ask for the screenshot.", so an unread image never reads as a Mod without one. |
| 3 | ModDetailPanel::remembered | data, error | A remembered panel width is a preference with a 320px default. An unread width opens the panel at that default and renders the read failure, and neither states anything about the Mod. |
| 3 | ModDetailPanel::updates | data, error | The only value taken from the list is the row's upstreamVersion, shown under the label "Last seen upstream", which is a past reading by construction. The failed read renders under the same guard that enables the query. |
| 3 | ModList / ModUpdateBadge::rows | data | No honest reason. A missing row renders no badge and no per-Mod check toggle, which is exactly how a Mod with no upstream update renders, and ModList shows no notice for this read. The same claim as #471. Defect for #483. |
| 3 | ModList::mods | data, error, isLoading, isSuccess | No honest reason. The heading states a Mod count from the last read while the failure notice beside it says GMM could not read the Library. This is the instance measured on #419. Defect for #483. |
| 3 | ModList::conflicts | data, error | Conflicts are advisory in v1, and the failure notice says in so many words that this is not a "no conflicts" result. The per-Mod counts and the unscanned badge come from the backend's own omitted entries. A paused read still shows zero conflicts unmarked; closing that is a migration step. |
| 3 | ModList / VariantSelector::v | data, isFetching | No honest reason. An unread Variant list renders no selector, which is indistinguishable from a single-folder Mod, and nothing reports the failed read. Defect for #483. |
| 3 | OnboardingWizard / WelcomeStep::guidance | data, error, isError | The AV guidance block is extra reading shown only when it was read, with the failure rendered instead when it was not. The step's own acknowledgement does not depend on it. |
| 3 | OnboardingWizard / DetectStep::detect | data, error, isError, isFetching, isLoading | Detection has its own loading and error screens and a Re-scan control. A detected path is only ever a suggestion the user accepts, browses past or skips before anything is written. |
| 3 | OnboardingWizard / LibraryStep::paths | data, error, isError | The same stored Library paths as the Settings editor, with the same unavailable placeholder and failure notice. The step writes the root the user picks rather than deriving anything from the read. |
| 3 | OnboardingWizard / ImporterStep::supported | data, error, isError, isLoading | The supported-Game registry is static at build time and cached with staleTime Infinity. The step renders its own loading and error states and installs nothing without a registry. |
| 3 | OnboardingWizard / ImporterStep::installPaths | data, isLoading | The query keeps each Game's failure inside its own value, which the step lists as install-path failures, so an unreadable Game is reported rather than silently dropped from the install candidates. |
| 3 | SessionCheckNotice / useSessionCheck::session | <forwarded> | The notice itself is on readEvidence. The raw observer is returned so App's Launch gate can key off isSuccess and data, which this file's own comment records as the remaining migration step; that gate blocks Launch on a failed read. |
| 3 | SettingsDestination::statuses | <forwarded> | Each element feeds one navigation sub-label, which renders "Checking · Previous: …" while refetching and "Cannot tell · path check unavailable" when the read failed or is absent. The Game's own page re-states the same reading in full. |
| 3 | SettingsDestination / ApplicationVersion::version | data, error, isPending | The version of the running build cannot change while GMM is open, and an unread version renders the read failure rather than a number. |
| 3 | SettingsDestination / GamePathExplanation::status | data, error, isError, isFetching, isPending | Pending and failed reads replace the line with "Cannot tell · path check unavailable", and a retained reading is labelled as previous in both the in-flight and the failed case. |
| 3 | SettingsDestination / NetworkPanel::cfg | data, error, isPending | The claim about a saved proxy password already goes through readEvidence. The presence-keyed reads left seed the editable URL and username drafts and render this read's own pending and failure lines, and a draft is not a claim. |
| 3 | SettingsDestination / GamePathPage::pathQuery | data, error, isError, isPending | The saved install path is a stored setting the user edits on this page. An unread path leaves the field empty with "Saved path unavailable" and disables every path control, and saving writes what the user picked rather than what was read. |

### What rule 3 found

45 readings in 17 production files produce a value. Four are already routed entirely
through `readEvidence` and carry no row: `App::onboarding`,
`App / GameRegistryGate::games`, `App / StartupReconcileNotice::status` and
`LoaderVersionNote::loader`. `src/App.tsx` is mixed, as #475 left it, and
`SessionCheckNotice` and `SettingsDestination` each consume one reading through
`readEvidence` while still forwarding or presence-keying another. The remaining 41
are inventoried above. A `useQuery` whose result is discarded is not a reading and is
outside the rule; `src/App.tsx` has one, kept mounted only to observe the session.

Six rows could not be given an honest reason. Each states something GMM has not
established, and none is fixed here:

- `App / useSessionPresentation::interruptedLaunches` — no warnings for an unread
  reservation list.
- `App / ModUpdatesPanel::global` — `?? true` invents an unread preference.
- `GameRecoveryNotice::recovery` — a paused or never-completed read renders silence
  on the main screen.
- `ModList / ModUpdateBadge::rows` — a Mod with an unread update row renders as a
  Mod with no update.
- `ModList::mods` — the heading's count contradicts the failure notice beside it,
  the instance measured on #419.
- `ModList / VariantSelector::v` — an unread Variant list renders as a Mod with no
  Variants.

Three rows name a residual gap narrower than a defect: `App / ModUpdatesPanel::rows`,
`ModList::conflicts` and `SessionCheckNotice / useSessionCheck::session` all handle a
failed refresh and are blind only to a paused one.

### Rule 3 limits

The gate is syntax-only and has no type checker. It follows uses of the reading's own
binding inside its owner and nothing further: a reading passed to another function or
stored in an alias is recorded as `<forwarded>`, and what that consumer then claims is
a review boundary, as `App / MainApp::installPaths`,
`SessionCheckNotice / useSessionCheck::session` and `SettingsDestination::statuses`
show. A reading consumed through a wrapper hook in another module is invisible for the
same reason `binding()` cannot cross module boundaries. The observer property list is
fixed: a property React Query adds later is recorded as `<forwarded>` until it is
listed, which fails closed. `useInfiniteQuery` and `useSuspenseQuery` are not
enumerated, because production source uses neither. A row proves that a human read the
site, not that the site is safe.

## Findings and limits

The current source has three conditional `useQuery` observers. Widening the failure
surface found zero additional production violations. `data` is deliberately excluded:
a disabled observer also retains data, but unconditional cached-data presentation is
legitimate and common. This rule targets stale failure messaging, not all stale data.

Widening Rule 2 found zero new unscoped cache writers. It required seven exact
callback inventory updates (MainApp reopenSetup, session retireLaunch, launch,
ModList retryRecovery/retireEnabledTransition, batch apply and wizard close).
These are reviewed message formatting, local UI/recovery state or query refreshes.

Finding 3 uses option A: batch scope is `conflicts-${game}`. Per-Game batch state
and batchId-based stops support independent Games. A shared literal scope would
queue unrelated Games. The frontend test "runs different Games concurrently and
queues same-Game batch writers across mounted instances" exercises the production
component with the real Query scheduler and controlled IPC promises. Template
matching proves agreement of syntax, not the semantic relation between dynamic
scope partitions and key suffixes; review must verify that relation. Identifier
names in templates must agree across writers, even if differently named variables
would have equal values. An identical template does not prove equal bindings.

A cache write behind a helper imported from another module is invisible because
`binding()` does not cross module boundaries. An explained allowance row can
therefore legitimize a real violation; the table does **not** prove safety. Local
callback bodies are followed lexically. The options scan sees direct writes but
does not follow mutationFn helper bodies. Review must inspect those handoffs.
Pure calls such as commandFailureMessage, Array.map and Array.find add inventory
noise. A future separate change could distinguish reviewed pure calls from opaque
handoffs using a narrow, tested classification; this round keeps exact lists.

`shape()` compares identifier text, not binding identity. A shadowed `mod` in a
`.map()` callback can satisfy a guard about the `mod` prop. This syntax-only gate
has no type checker and does not claim whole-program or binding-safe proof.

`useQueries`, `useInfiniteQuery` and `useSuspenseQuery` are outside Rule 1, as are
unconditional queries and manually triggered `enabled: false` queries.
Specifically, App.tsx around line 141 renders `path.error` under `path.isError`
from a `useQueries` collection. Adding `enabled` there can reproduce the #432
class outside this gate; review must cover that site. Other hook factories and
aliased observer forwarding need explicit review as well.

Inventory rows tolerate leading whitespace. Opaque option spreads, computed
options, unnamed owners, indirect observer aliases and unresolved enabled callbacks
fail closed unless a specific Rule 1 reason is declared and matched in the audit.
These tests do not validate IPC or Windows runtime.

## Conflict omission causes — #444

`detect_conflicts` now returns `omitted` entries with `mod_id` and `cause`
(`inFlight` or `unestablished`), sorted by Mod id. This is a read-only shape
change with no command, query, mutation, or opaque callback added. ModList
uses the cause for the row and partial-count tooltip. ModUpdateBatchAction
formats the same cause-specific explanation from the omission entries. GamePresets renders the
backend's neutral incomplete-reading explanation; it does not infer recovery
from omissions. Existing inventory rows and totals remain unchanged.
