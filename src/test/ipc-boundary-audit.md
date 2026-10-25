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

| Rule | Site | Opaque callback allowance | Reason |
| --- | --- | --- | --- |
| 1 | ModDetailPanel::remembered | none | The early return when closed gates the width-read error on open. |
| 1 | ModDetailPanel::updates | none | The upstream notice shares hasUpstream with enabled. |
| 1 | OnboardingWizard / ImporterStep::installPaths | none | No observer error render; per-Game result errors are separate data. |
| 2 | App / MainApp::reopenSetup | onResumeWizard, onSetupFailure | Parent callbacks reset wizard/feedback state and report setup failure. Settlement refreshes onboarding status; neither callback installs cache data. |
| 2 | App / useSessionPresentation::retireLaunch | commandFailureMessage, game.toUpperCase, retirement.finish, retirement.start | commandFailureMessage and toUpperCase format messages; retirement.start/finish publish local recovery feedback. Settlement refreshes session and launch reservations. |
| 2 | App / useLaunchGame::launch | guidance.refetch | Error handling refetches exclusion guidance from its query; this refresh installs no mutation return value. |
| 2 | App / ModUpdatesPanel::check | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | App / ModUpdatesPanel::toggleGlobal | none | No direct cache write in success/settlement; refresh or local UI only. |
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
