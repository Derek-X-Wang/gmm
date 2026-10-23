# egui batch-stop accessibility spike

## Result

**GREEN for the tested UI idiom on egui / egui_kittest 0.37.0.** All 26 widget assertions use the AccessKit tree with role plus name and retain their original checks. All five original stop scenarios pass. This is evidence for a tractable UI port, not a migration or a complete accessibility assessment.

The standalone library uses only local contracts and a recorded stub service. Run it independently:

```sh
cd spike/egui-modlist
cargo build
cargo test
```

Rust 1.95 is required by the pinned dependencies. No GMM Core, Tauri, eframe, window, graphics backend, or JavaScript runtime is involved. The tests execute the actual egui view headlessly; they do not build a separate accessibility tree for tests. Native screen-reader speech is **NOT-RUN**.

## A. Assertion mapping

There are **32 `expect` sites** in `src/ModUpdateBatchStop.test.tsx`, including the two shared setup checks. Line numbers below refer to that original file. `B(name)` means `get_by_role_and_label(Role::Button, name)`. `S(text)` and `A(text)` use the corresponding role plus exact accessible name, check the actual node value, and retain the original bare-role uniqueness check. They also verify live metadata. `V(role, text)` checks role plus name, node value, not-hidden state, positive bounds, and intersection with the harness viewport. `N(name)` is `query_by_role_and_label(Role::Button, name).is_none()`.

**Classification caveat:** the requested definition of SURVIVED requires role plus name. Six original assertions concern callback/service/cache spies rather than widgets, so they cannot meet that definition. They are marked **WEAKENED** under that literal rule, even though their local stub checks preserve the original recorded-call conditions. They are not evidence of lost widget-query capability. No UI assertion falls back to raw painted text, and none is IMPOSSIBLE in this spike.

| Original assertion | egui_kittest form | SURVIVED / WEAKENED / IMPOSSIBLE | note |
| --- | --- | --- | --- |
| L28 Apply button is enabled | `!B("Apply all Mod updates").accesskit_node().is_disabled()` | SURVIVED | Shared setup, repeated for all five scenarios |
| L30 progress is a function | `service.on_progress.is_some()`; callback type is `Box<dyn FnMut(ModUpdateProgress)>` | WEAKENED | Non-widget setup; callback is actually registered by Apply and invoked by the stub |
| L43 Stop is enabled | `!B("Stop after this Mod").accesskit_node().is_disabled()` | SURVIVED | Present and enabled are distinct checks |
| L46 Stop becomes disabled | `B("Stop after this Mod").accesskit_node().is_disabled()` | SURVIVED | Node must still exist |
| L47 waiting-for-Current text is visible | `V(Paragraph, "Stop requested. Waiting for Current to finish; later Mods will not start.")` | SURVIVED | Role, name, value, visibility and viewport bounds |
| L48 exactly one Stop call with Apply's batch ID | `assert_eq!(stops, [Command::Stop { batch_id: apply_id }])` | WEAKENED | Non-widget service spy; preserves count, command and exact token |
| L49 First: Updated is visible | `V(ListItem, "First: Updated")` | SURVIVED | Actual rendered result node |
| L51 Stop absent during scan | `N("Stop after this Mod")` | SURVIVED | Absent, not disabled |
| L59 status contains Batch stopped | `S("Batch stopped.")` | SURVIVED | Exact value is stronger than substring; unique Status retained |
| L60 Current: Updated is visible | `V(ListItem, "Current: Updated")` | SURVIVED | Actual rendered result node |
| L61 unique Later: Skipped result includes untouched-Library reason | `V(ListItem, full_skipped_text)` plus unique prefix count | SURVIVED | Full exact string; excludes inline TextRun duplicates from widget count |
| L62 unique alert contains Conflicts unverified | `A(full_conflict_error)` | SURVIVED | Exact error, unique Alert, assertive live metadata |
| L63 zero-Conflicts success text is absent | role+name Paragraph absence AND all-tree label/value absence | SURVIVED | Extra all-role check prevents a wrong-role copy from escaping the assertion |
| L64 conflicts/gimi query is invalidated | recorded `Command::Invalidate(["conflicts", "gimi"])` exists | WEAKENED | Non-widget cache spy; no actual TanStack Query runtime |
| L65 incomplete conflicts are not cached | exact `Command::SetConflicts { game, report }` is absent | WEAKENED | Non-widget cache spy; preserves report-value comparison |
| L76 unique status contains Checking Conflicts… 3 of 3 completed | `S("Checking Conflicts… 3 of 3 completed.")` | SURVIVED | Exact text, Status role, live metadata |
| L77 Stop absent during final scan | `N("Stop after this Mod")` | SURVIVED | Role plus exact name |
| L78 no Stop command was sent | `assert_eq!(stops.len(), 0)` | WEAKENED | Non-widget service spy |
| L81 Apply is enabled after completion | `!B("Apply all Mod updates").accesskit_node().is_disabled()` | SURVIVED | Role plus exact name |
| L83 second Apply sends a second command | recorded Apply-command count equals 2 | WEAKENED | Non-widget service spy; also checks a fresh batch token |
| L84 fresh batch Stop is enabled | `!B("Stop after this Mod").accesskit_node().is_disabled()` | SURVIVED | Before any new progress event |
| L85 unique status contains Starting updates… 0 of 3 completed | `S("Starting updates… 0 of 3 completed.")` | SURVIVED | Progress reset preserved |
| L92 unique alert contains exact stop-delivery error | `A("Stop request unverified: connection lost The batch may continue. Try again.")` | SURVIVED | Error is rendered; retry remains possible |
| L94 retry Stop is enabled | `!B("Stop after this Mod").accesskit_node().is_disabled()` | SURVIVED | Distinct from absent |
| L96 retried Stop becomes disabled | `B("Stop after this Mod").accesskit_node().is_disabled()` | SURVIVED | Also records a second delivery with the same batch token |
| L97 no alert after retry | role+name error absence AND zero Alert nodes | SURVIVED | Keeps original all-alert absence, not only expected-name absence |
| L103 no stale Stop on another Game | `N("Stop after this Mod")` after selecting srmi | SURVIVED | Same persistent view state; does not recreate component |
| L107 unique status contains Batch finished | `S("Batch finished.")` | SURVIVED | Late Stop does not force a stopped result |
| L108 no Stop after completion | `N("Stop after this Mod")` | SURVIVED | Role plus exact name |
| L119 unique status still contains Batch finished after delayed stop error | `S("Batch finished.")` | SURVIVED | Error delivered only after batch completion |
| L120 no alert after delayed stop error | role+name error absence AND zero Alert nodes | SURVIVED | All-alert absence retained |
| L121 no Stop after delayed stop error | `N("Stop after this Mod")` | SURVIVED | Role plus exact name |

The original Unicode ellipsis (`…`) and em dash (`—`) are retained. Additional tests cover every Apply-disable gate and the pending Apply name. The original five scenario names have direct counterparts in `tests/batch_stop.rs`; the sixth test covers gates explicitly requested by the dispatch.

## B. Five capability answers

1. **Role plus accessible name: yes.** `h.get_by_role_and_label(Role::Button, "Stop after this Mod")`. Despite the method's word “label”, this queries AccessKit's computed accessible name for a Button, not text painted on the screen. The independent-name probe below verifies this distinction.
2. **Disabled versus absent: yes.** `h.get_by_role_and_label(Role::Button, "Stop after this Mod").accesskit_node().is_disabled()`. `get` first requires exactly one present node; the property then checks its disabled state. A second pointer click on the disabled widget produces no extra Stop call.
3. **Absent by role plus name: yes.** `h.query_by_role_and_label(Role::Button, "Stop after this Mod").is_none()`. Tests demonstrate withdrawal during scanning, another Game, and completion. Bare-role Alert absence is also retained so another error cannot hide behind a different name.
4. **Status / aria-live: expressible and readable, but not automatic for labels.** A plain `ui.label` produces no Status node. The production view's `text` helper annotates that actual rendered widget with `Role::Status`, `Live::Polite`, and live-atomic metadata through `Context::accesskit_node_builder`. Alerts get `Role::Alert` and `Live::Assertive`. The tests inspect both the role/name/value and live properties. This is the low-level AccessKit equivalent, not a built-in `ui.status` widget. Actual OS adapter announcements, timing and screen-reader behavior remain **NOT-RUN**.
5. **Independent accessible name: yes.** `ui.button("×").accessible_name("Close Mod details")`. The probe finds `Role::Button + "Close Mod details"` and finds no Button named `"×"`. The visible button still uses the cross glyph.

Relevant APIs, verified against the downloaded pinned sources: [egui Response::accessible_name](https://docs.rs/egui/0.37.0/egui/struct.Response.html#method.accessible_name), [Context::accesskit_node_builder](https://docs.rs/egui/0.37.0/egui/struct.Context.html#method.accesskit_node_builder), [kittest Queryable](https://docs.rs/kittest/0.5.0/kittest/trait.Queryable.html), and [AccessKit Node](https://docs.rs/accesskit/0.25.1/accesskit/struct.Node.html).

## C. Size comparison

Physical lines including comments, blank lines and rustfmt formatting:

| Scope | Lines |
| --- | ---: |
| Original TSX component | 140 in this checkout (dispatch described 139) |
| Rust view, state and lifecycle logic, including semantic-text helper (`src/view.rs`) | 375 |
| Of the above, `show` rendering method | 162 |
| Local contracts and recorded service (`src/model.rs`) | 107 |
| Module declarations (`src/lib.rs`) | 2 |
| Five scenario ports plus Apply-gate test | 368 |
| Four capability probes | 87 |

The Rust view/controller is about 2.7 times the TSX physical line count. The total library is 484 lines, including the local types/service; the TSX count excludes its imported API and query-library implementation. This is not a normalized language comparison. No CSS file was added. The view uses egui defaults; the original muted/small typography, full GMM theme and visual parity were not reproduced, so zero CSS lines do **not** establish zero equivalent styling cost.

## D. Friction and limits

- **Semantics need deliberate markup.** Buttons expose role/name/disabled automatically. Status, Alert, Paragraph, List and ListItem annotations were added to actual rendered widgets. No test-only nodes or separate tree are used. Status and alert accessible names equal their displayed text, with value checks retained independently. A migration would need a shared semantic-widget layer and an audit of callers.
- **Label storage differs.** AccessKit Label stores text as value, and kittest special-cases that role when matching labels. The view uses explicit Paragraph/ListItem/Status/Alert roles with real names instead of relying on text-only Label lookups. A generic tree prefix query initially counted both a semantic result and its inline TextRun; the uniqueness check excludes TextRun fragments, while retaining the full result's role/name/value assertion.
- **Immediate-mode state ownership changed.** React state, mutation lifecycle callbacks and the requested-stop ref became one persistent view/controller with per-Game batches, one pending Game, a batch-token set, and explicit completion/failure handlers. The stub registers a real typed progress callback; it queues events that the view drains before drawing. Finish/stop-failure delivery is manually ordered, matching the original deferred-promise test scenarios. No async executor, Tauri transport, Core, or real cache implementation is tested. The stub counter is a local substitute for UUIDs.
- **Text input:** a named TextInput accepts basic text events, updates its state, and exposes the new value. Native IME composition, CJK entry, selection, clipboard, undo, keyboard navigation quality, and platform accessibility interaction are **NOT-RUN**. This batch view itself has no text field.
- **Long lists / `show_rows`:** a 10,000-row probe exposes the first rendered slice and no node named `Mod 9999`. Fewer than 20 button nodes are present in a 300-by-120 harness. This is a concrete limitation for whole-Library accessibility queries: absence from a virtualized tree cannot prove absence from the data. Scrolling to an offscreen item and measuring performance were **NOT-RUN**. The ported three-result list does not use virtualization.
- **Visibility:** the text assertions check accessibility hidden state and widget bounds against a known viewport, alongside exact content. They do not verify pixels, occlusion by other windows, CSS behavior, or native screen-reader announcements.
- **Timing:** `h.run()` settles frame updates rather than waiting for React's scheduler. Delayed stop errors and batch completion are injected in the original order. Real cross-thread races and frame wake-up integration are outside this standalone stub.
- **Test API learning:** disabled is on `node.accesskit_node()`, not directly on egui_kittest's wrapper Node. The initial compile attempt exposed this and was corrected without changing assertions.

## E. Verdict and verification

**GREEN — the tested accessibility query/assert idiom ports, with explicit semantic annotations.**
All 26 widget assertions retain role plus name and their enabled, disabled, absent, content or visibility checks.
All five original scenarios pass; no widget assertion is weakened or impossible.
Six non-widget spies are separately labeled under the strict requested rubric; their recorded conditions remain tested.
Status/live metadata and independent accessible names are verified in the actual AccessKit tree.
Native speech, input quality and full-app rendering are not established by this spike.
Virtualization needs a different navigation strategy before querying offscreen rows.
This evidence supports a tractable migration investigation, not approval to migrate GMM.

**VERIFIED:** independent `cargo build`, `cargo test` (10 passed, 0 failed), and formatting checks; all changes confined to new files under `spike/egui-modlist`.

Three temporary source mutations caused the intended assertions to fail: replacing Status with Paragraph failed Status-role uniqueness; removing Stop's disabled condition failed the disabled property assertion; removing scan withdrawal failed role+name absence. All mutations were restored before final build/test. These checks each ran in seconds, so no check met the five-minute wefty threshold.

**NOT-RUN:** existing repository test suites (explicitly excluded), native Windows UI/assistive technology, actual OS live announcements, full GMM styling, real service/cache integration, and long-list performance.
