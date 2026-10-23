# IPC boundary structural audit — #438

The conditional error gate inventories conditional `useQuery` observers only.
The mutation scope gate inventories every `useMutation` and examines direct
`setQueryData` / `setQueriesData` writes reached from success or settlement.
Invalidation requests a fresh read and is not a direct cache write.

A single writer must also serialize repeated invocations and mounted instances.
Writers with the same literal array head must share one literal scope ID; this
conservative grouping does not assume dynamic suffixes are disjoint.

Opaque callback handoffs are listed by exact callee name below. Their bodies
remain a review boundary; this is a declared exception, not inferred safety.
Opaque keys/options have no production allowance. The gates fail on them.

| Rule | Site | Opaque callback allowance | Reason |
| --- | --- | --- | --- |
| 1 | ModDetailPanel::remembered | none | The early return when closed gates the width-read error on open. |
| 1 | ModDetailPanel::updates | none | The upstream notice shares hasUpstream with enabled. |
| 1 | OnboardingWizard / ImporterStep::installPaths | none | No observer error render; per-Game result errors are separate data. |
| 2 | App / MainApp::reopenSetup | onResumeWizard | Parent onResumeWizard only resets setup feedback and selects the wizard. Settlement refreshes onboarding status. |
| 2 | App / useSessionPresentation::retireLaunch | game.toUpperCase, retirement.finish | String formatting and useRecoveryOutcomes.finish publish local feedback. Settlement refreshes session and launch reservations. |
| 2 | App / useLaunchGame::launch | none | No direct cache write in success/settlement; refresh or local UI only. |
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
| 2 | ModList::retryRecovery | sectionRef.current?.focus, setRecoveryFeedback | Focus and recovery feedback setters affect local UI. Settlement refreshes Mods and Conflicts. |
| 2 | ModList::retireEnabledTransition | retirement.finish | useRecoveryOutcomes.finish publishes local feedback. Settlement refreshes Mods and Conflicts. |
| 2 | ModList / AdoptButton::adopt | onAdopted, setName, setOpen_, setPicked | Draft setters only change local UI; onAdopted/onImported is ModList.invalidate, which only refreshes Mods, audit and update rows. |
| 2 | ModList / GameBananaImport::ingest | onImported, setInput, setOpen | Draft setters only change local UI; onAdopted/onImported is ModList.invalidate, which only refreshes Mods, audit and update rows. |
| 2 | ModList / VariantSelector::switchVariant | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | ModList / ImportZipButton::importMutation | onImported, setName, setOpen_, setPicked | Draft setters only change local UI; onAdopted/onImported is ModList.invalidate, which only refreshes Mods, audit and update rows. |
| 2 | ModList / ZipDropZone::importMutation | onImported, setName, setPendingPath | Draft setters only change local UI; onAdopted/onImported is ModList.invalidate, which only refreshes Mods, audit and update rows. |
| 2 | ModUpdateBatchAction::apply | setBatches, update | Direct Conflicts writer now shares scope conflicts. setBatches and its update parameter only change local batch presentation. |
| 2 | OnboardingWizard::close | onDone | Parent onDone clears wizard state and invalidates path/importer queries; it installs no direct cache value. |
| 2 | OnboardingWizard / DetectStep::setPath | setOverrides | setOverrides only stores local path overrides. Settlement refreshes install and path status queries. |
| 2 | OnboardingWizard / LibraryStep::setRoot | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | SettingsDestination / NetworkPanel::save | setClearPassword, setPassword | Setters clear the local password draft and checkbox. Settlement refreshes proxyConfig. |
| 2 | SettingsDestination / NetworkPanel::test | none | No direct cache write in success/settlement; refresh or local UI only. |
| 2 | SettingsDestination / GamePathPage::setPath | setDetectFailed, setLastSource | Setters update local path-source feedback. Settlement only refreshes install and path status queries. |
| 2 | SettingsDestination / GamePathPage::detect | setDetectFailed, setLastSource | Setters update local path-source feedback. Settlement only refreshes install and path status queries. |
| 2 | SettingsDestination / RebuildJunctions::rebuild | none | No direct cache write in success/settlement; refresh or local UI only. |

## Findings and limits

No conditional-query error violation was present. The current source has three
conditional `useQuery` observers. The manually triggered `enabled: false` query
and the plural `useQueries` collection are outside Rule 1.

The one serialization finding was `ModUpdateBatchAction::apply`: it publishes
`["conflicts", selected]` directly without a scope. Adding `scope: { id:
"conflicts" }` serializes repeated batches across mounted instances. This groups
all Games conservatively. The width writer already had its correct scope.

The table declares external/component callback and UI method boundaries rather
than proving their implementations with type or whole-program analysis. Adding,
removing or renaming such a call requires an exact table update and review of its
reason. Local function bodies are followed lexically; opaque keys, option
spreads, computed options, unnamed observers, indirect error aliases and
unresolved enabled callbacks fail closed. Dynamic key suffixes are deliberately
not used to prove separation. These tests do not validate IPC or Windows runtime.
