# Testing patterns

GMM ships five layers of automated coverage. Layers 1–2 run anywhere;
layers 3–5 are Windows-specific and exist so nobody has to sit at a
Windows box to know whether a change works.

| Layer | Where it runs | What it proves |
|---|---|---|
| 1. Core integration | any host | business logic against a temp SQLite + Library |
| 2. IPC contract | any host | Backend serde shapes plus frontend `invoke` envelopes |
| 3. Frontend component | any host (jsdom) | React state machines + gating |
| 4. Windows-gated Rust | Windows CI | junctions, registry, loader FFI, full vertical |
| 5. Installer smoke | Windows CI | the MSI a user actually downloads |

## 1. Core integration tests (`src-tauri/tests/*.rs`)

The bulk of the suite. Each test drives `Core` directly against a
temp-directory SQLite + Library tree. No Tauri runtime, no network
unless we stand up a `mockito` server in the test.

Pattern:

```rust
use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

#[tokio::test]
async fn some_behaviour() {
    let tmp = TempDir::new().unwrap();
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.unwrap();
    // … drive Core methods + assert on disk + DB …
}
```

Add a new test file per feature. Existing ones to use as templates:

- `tests/zip_import.rs` — slice 1b (filesystem ingest hardening)
- `tests/reconcile.rs` — slice 1c (junction reconciliation)
- `tests/variants.rs` — slice 5 (multi-variant detection)
- `tests/conflicts.rs` — slice 12 (3dmigoto INI parser)
- `tests/gamebanana.rs` + `tests/mod_updates.rs` — slices 11 + 13c (network
  via [`mockito`](https://docs.rs/mockito); the production code accepts an
  `Endpoints` test seam so tests inject the mock server URL)
- `tests/migrations.rs` — the migration corpus, below

### The migration corpus

Every other test starts from `Core::new` against an empty file, so the
migrations only ever run against nothing. A migration that breaks on
real rows — a `NOT NULL` column with no default, a `UNIQUE` index
existing data violates — passes the whole suite and bricks the install
on first launch. GMM self-updates, so it reaches users on its own.

`tests/fixtures/migrations/NNN_<name>.db` holds a populated SQLite file
with migrations `1..=NNN` applied: a Game with an install path, two Mods
(one enabled, one not, one GameBanana and one local) with their Junction
directory names, a Library root override, a Variant with an active
selection, and update state. `tests/migrations.rs` opens each through
the real startup path and asserts the data is all still there, that
reopening is a no-op, and that an interrupted migration leaves a
database the next startup can finish.

The `.db` files are checked in as binaries deliberately. They carry the
`_sqlx_migrations` rows — including sqlx's checksum of each migration's
SQL — exactly as written at generation time, so **editing an
already-shipped migration makes the tests fail with the same
`VersionMismatch` a user's install would hit.**

**Adding a migration means adding a fixture.** The generator is an
ignored test in the same file, so the seed data and the assertions
cannot drift apart:

```bash
cargo xtask migration-fixture
git add src-tauri/tests/fixtures/migrations
```

The generator creates only the newest missing fixture. Existing fixtures are
immutable historical evidence: a normal run fails rather than overwriting one,
and `SHA256SUMS` pins their exact committed bytes in the migration test suite.
On pull requests, CI also requires `REGENERATIONS.md` to change when an existing
checksum is changed or removed, so the ordinary review path cannot accept a
hurried rewrite without a stated reason. Appending the checksum for a genuinely
new fixture does not require a regeneration entry. This is not tamper-proof: a
determined person can edit the fixture, checksum, and record together, and
review must still judge the explanation. An exceptional repair requires both
an explicit target and a recorded reason:

```bash
cargo xtask migration-fixture --regenerate-existing NNN --reason "why the historical artifact is invalid"
```

That path records the supplied reason in `REGENERATIONS.md`. See
`tests/fixtures/migrations/PROVENANCE.md` for the verified origin and limits of
the current corpus.

## 2. Tauri command IPC contract (`src-tauri/tests/commands_ipc.rs`)

The Tauri `#[tauri::command]` macro generates a runtime wrapper that
uses serde for both the incoming Args struct and the returned value.
Driving that wrapper through `tauri::test::get_ipc_response` requires
synthesising a `Context<MockRuntime>` that carries the real ACL
capabilities — historically painful (see issue #26 body).

The backend half routes the **same Args + return types** the command body
uses through `serde_json` and calls the Core method directly. Because that
cannot see how the frontend called `invoke`, `src/api.test.ts` asserts each
current `*Args` command's real frontend envelope, including `ProxyArgs`.
`tests/ipc_contract.rs` then parses the actual frontend outer key and actual
Rust parameter identifier and compares them directly, with no expected-name
table. Renaming `args` to `request` on only one side therefore fails the
host-runnable contract test. These tests still skip Tauri's runtime and ACL
enforcement; registration is covered separately in `tests/ipc_contract.rs`.

```rust
use gmm_lib::commands::AdoptArgs;
use serde_json::json;

#[test]
fn adopt_args_deserialises_from_camel_case_json() {
    let args: AdoptArgs = serde_json::from_value(json!({
        "game": "gimi",
        "sourcePath": "/tmp/my-mod",
        "name": "My Mod",
    })).unwrap();
    assert_eq!(args.source_path.to_string_lossy(), "/tmp/my-mod");
}
```

When adding a new `*Args` command, extend `tests/commands_ipc.rs` with the
backend shape assertions and `src/api.test.ts` with the frontend envelope
assertion. The cross-source outer-name check has no hand-maintained command
list: it inventories every `*Args` struct declaration, requires each type to
be the unqualified parameter of exactly one `#[tauri::command] pub async fn`,
and checks every frontend `invoke` callsite it finds for that command. Its
source parser deliberately supports the repository's current declaration
style: `*Args` declarations must remain in `src-tauri/src/commands.rs`.
Moving a declaration to another module, using a different visibility or sync
function, or qualifying or wrapping the Rust type fails with an actionable
diagnostic instead of silently dropping that command. On the frontend, the
scanner checks every callsite it recognises: a double-quoted command-name
literal with an inline object envelope. A recognised callsite with a
non-inline envelope fails and tells the developer to inline it or extend the
scanner; other command-name syntax is outside this test's coverage. Supporting
an excluded form requires extending the scanner alongside the syntax change.
It does not replace the explicit serde-shape and frontend API assertions below,
or prove Tauri runtime/ACL behaviour:

1. **Args deserialise.** Build a `serde_json::json!({ … })` value
   matching the JS-side shape and `from_value` it into the Args
   struct.
2. **Frontend invokes the real envelope.** Mock `@tauri-apps/api/core`, call
   the exported API function, and assert the exact command name plus outer
   object passed to `invoke`.
3. **Return serialises.** Run the Core method through whatever setup
   makes sense, `to_value` the result, and assert the JSON keys are
   the camelCase / snake_case the frontend expects.

For commands that emit user-facing error strings (e.g.
`set_mod_enabled` when no install path is set), extract the literal
as a `pub const` in `commands.rs` and assert against it in the test.
That way the wire copy can't drift without a corresponding test
update.

### Commands with orchestration worth testing

Wire shapes are enough for the commands that are one `await` over a
Core method. `launch_game` is not one of those: it spawns a process,
holds it in an RAII guard across several fallible steps, claims the
Game Session, installs the live session, emits events, and spawns the
exit watcher. None of that is reachable through serde.

The pattern for a command like that is to move the orchestration into
a plain function generic over the Tauri runtime and leave a shell
behind:

```rust
#[tauri::command]
pub async fn launch_game(
    app: AppHandle,
    core: State<'_, Core>,
    runtime: State<'_, SessionRuntime>,
    game: GameCode,
) -> Result<SessionInfo, String> {
    launch::launch(&app, &core, &runtime, game, &LaunchOptions::default())
        .await
        .map(|outcome| outcome.info)
}
```

`runtime::launch::launch` is then callable from a test with a
`tauri::test::mock_app()` handle, which is a real `Emitter` — so the
emitted events are the production ones, asserted by name **and order**
via `app.listen`. Two things make the flow testable without weakening
it:

- **`LaunchOptions`** carries the timings that would otherwise be
  hard-coded (injection timeout, inject settle, watcher poll). Prod
  passes `default()`; tests shrink them so a timeout path costs
  seconds.
- **`LaunchOutcome::watcher`** hands back the exit watcher's
  `JoinHandle`. Prod drops it (the task is detached); tests `.await`
  it and get deterministic teardown instead of sleeping.

**Windows gotcha.** A test binary that builds a Tauri `App` needs the
Common-Controls v6 manifest linked into it, or it dies at load with
`STATUS_ENTRYPOINT_NOT_FOUND` (exit code `0xc0000139`) before running a
single case. tauri-build links the manifest with
`cargo:rustc-link-arg-bins`, which covers bins and nothing else
([tauri#13419](https://github.com/tauri-apps/tauri/issues/13419)), so
`src-tauri/build.rs` embeds `windows-app-manifest.xml` itself via plain
`rustc-link-arg`. Don't remove that without checking `cargo test` still
runs on Windows.

Coverage lands in two files:

- `tests/launch_command.rs` — host-runnable. Everything that fails
  *before* the first Windows-only call: double-launch refusal, unset
  install path, missing game exe, missing Model Importer. Each asserts
  the same three cleanup properties (nothing persisted, nothing live,
  nothing emitted).
- `tests/launch_command_windows.rs` — Windows CI. Spawn onwards: the
  happy path plus watcher teardown, injection timeout, the two states a
  dead watcher leaves in the live-session slot, and the watcher
  finishing when the slot is cleared under it. One Game per test, since
  the process-snapshot assertions match on executable name, and the CBT
  hook is process-global so those tests serialise on a mutex.

## 3. Frontend component tests (`src/**/*.test.{ts,tsx}`)

vitest + `@testing-library/react` under jsdom. The whole `./api` module
is mocked per test file, so nothing reaches a real backend — a test that
accidentally invokes for real hits the 5 s timeout instead of hanging.

Use the shared harness so TanStack Query is wired with retries off
(a rejected mutation surfaces immediately instead of backing off):

```tsx
import { renderWithQuery } from "./test/harness";

vi.mock("./api", () => ({ markOnboardingComplete: (...a) => spy(...a) }));

it("gates Continue on the AV acknowledgement", async () => {
  renderWithQuery(<OnboardingWizard onDone={vi.fn()} />);
  expect(screen.getByRole("button", { name: /continue/i })).toBeDisabled();
  await userEvent.click(await screen.findByRole("checkbox"));
  expect(screen.getByRole("button", { name: /continue/i })).toBeEnabled();
});
```

Anything rendered behind a query (`guidance.data ? … : "Loading…"`) needs
`findBy*`, not `getBy*` — the element does not exist on first paint.

## 4. Windows-gated Rust tests

Files that open with `#![cfg(windows)]` compile away everywhere else and
run only on the `windows-latest` CI matrix entry:

- `tests/session_smoke.rs` — per-game session round-trips
- `tests/e2e_windows.rs` — Core + Loader against a fake game install
  (detect → importer install → adopt → junction → hook → inject →
  lock → teardown), assembled from `Core` calls rather than driven
  through the command layer
- `tests/launch_command_windows.rs` — the `launch_game` orchestration
  itself: ChildGuard cleanup, the atomic session claim, event order,
  the exit watcher
- `tests/registry_windows.rs` — real `HKCU` uninstall entries driving
  `detect_from_registry`, cleaned up by a `Drop` guard

The fake-game fixture reuses the crates slices 4a/4b already built:
`victim.exe` becomes `GenshinImpact.exe` (it creates a real window, so
the CBT hook fires) and `noop_dll.dll` becomes `d3d11.dll` (it exports
`CBTProc`, which is the symbol 3dmloader resolves). That is why an
end-to-end injection test is possible without shipping a gacha game to
a CI runner.

**These need `cargo build --workspace` first** — they shell out to
`target/debug/victim.exe` and `noop_dll.dll`. CI orders the steps
accordingly.

### Checking Windows code from a non-Windows host

`cargo check` on macOS/Linux silently skips every `cfg(windows)` file,
so a typo in one of them would only surface after an ~8 minute CI round
trip. [`cargo-xwin`](https://github.com/rust-cross/cargo-xwin) fixes
that — it downloads the MSVC headers/libs and cross-compiles locally:

```bash
cargo install cargo-xwin
rustup target add x86_64-pc-windows-msvc

cd src-tauri
cargo xwin clippy --workspace --all-targets \
    --target x86_64-pc-windows-msvc -- -D warnings
```

This compiles the Windows-only test binaries and catches type errors,
missing imports, and clippy lints that the host build never sees. It
cannot *run* them — that still needs the Windows runner — but it turns
"push and wait 8 minutes" into a ~30 second local loop.

## 5. Installer smoke (`.github/scripts/installer-smoke.ps1`)

Runs in its own `installer` CI job. Builds the real MSI, installs it
silently, launches the installed exe, and asserts the app reached a
working state:

1. `msiexec /i /quiet` exits 0
2. the installed `GMM.exe` exists
3. launching it creates `%APPDATA%\GMM\gmm.db` (migrations ran)
4. it creates `%APPDATA%\GMM\logs\*.log` (tracing up)
5. that log carries the **IPC readiness marker**
6. the process survives startup (no crash loop)
7. the seeded DB contains all six game codes
8. `msiexec /x` uninstalls cleanly

### The IPC readiness marker

Steps 3, 4, 6 and 7 all pass on a build whose UI is completely broken:
the Rust side runs migrations and starts logging whether or not the
WebView can reach it, so "denied by the ACL" and "not registered in
`generate_handler![]`" both look like success. `tests/ipc_contract.rs`
catches *name* mismatches statically but cannot see either of those.

Step 5 closes that: `is_onboarding_complete` — the App router's own
query, first invoke of every session on both the wizard and main-app
branches — calls `diagnostics::record_ipc_ready()`, and the smoke waits
for the resulting line. Reaching that call requires the WebView to boot,
the IPC channel to come up, the command to be registered, and the ACL to
allow it.

Drift guards keep it honest, all in `tests/ipc_contract.rs`: the marker
command must still be registered and still invoked by the frontend, its
body must still call `record_ipc_ready`, **all three** PowerShell scripts
must still grep the same literal as `diagnostics::IPC_READY_MARKER`, and
the two scripts that launch the app more than once must require a *new*
marker rather than any marker.

That last one is not hypothetical. GMM's logs are not cleared between
launches, so "does the marker appear anywhere" is satisfied instantly by
the previous launch's line — which would make every startup check after
the first one pass without the process under test having done anything,
including when it crashed. `Get-IpcMarkerCount` compares a count taken
before starting the app with one taken after.

If you move the marker to a different command, move all four together.

On failure it dumps the msiexec verbose log, GMM's own JSON logs, and
recent Application event-log errors — so a Windows-less maintainer can
still diagnose from the CI output. Artifacts are uploaded too.

The manifest fixture also keeps an asynchronous read pending on the accepted
connection while its response is withheld. The fixture withholds the final body
byte, checks the read after flushing the incomplete response prefix, and checks
again immediately before that byte releases the response. If GMM closes the
request before release, including in the final response-preparation window, the
smoke reports a `PRODUCT` failure: abandoning the in-flight refresh is
application behavior, not a fixture-server outage. The fixture does not check
after the complete response because a correct client may close normally then.

This is the layer that would have caught a broken bundle, a missing
WebView2 dependency, or a migration that fails on a clean machine.

## 6. Signed-update round trip (`.github/scripts/updater-e2e.ps1`)

`tests/updater_config.rs` checks that `tauri.conf.json` *says* the right
things — a well-formed pubkey, HTTPS endpoints,
`createUpdaterArtifacts: true`, and the MSI-only bundle target. That is shape
only, and the updater-artifact flag was
missing from the very first release tag, so installers shipped with no
update path at all.

Two layers cover the rest:

- **`tests/updater_signature.rs`** (host-runnable). Generates a
  throwaway keypair with `tauri signer generate`, signs with
  `tauri signer sign`, and re-runs the exact verification
  `tauri-plugin-updater` performs (`PublicKey::decode` →
  `Signature::decode` → `verify(data, sig, true)`). Asserts a tampered
  artifact, a tampered signature, and a signature from a different key
  are all refused. The real signing key is a release secret and is never
  read.
- **`.github/scripts/updater-e2e.ps1`** (Windows). Builds two versions
  with the throwaway key, enumerates signatures across the entire bundle,
  and requires exactly one signed artifact. It constructs a test-only
  `latest.json` for that sole artifact, serves it over `127.0.0.1`, logs the
  local URL's artifact name as diagnostics, downloads it back, and verifies it
  via the ignored
  `the_bundled_artifact_verifies_against_the_key_that_signed_it` test. It then
  installs the update over the older build and asserts the version moved, the
  app still starts (via the IPC readiness marker), and `%APPDATA%\GMM` survived.

The local `latest.json` is not production's manifest oracle: this script writes
it from the artifact it already selected. Production manifest generation
belongs to `tauri-apps/tauri-action`. The statement that alpha2 advertised MSI
comes from manual inspection of that published release's real `latest.json`,
not from this round trip. The automated protection here is the independent
count of signatures emitted from the shipped config: anything other than one
fails before the local manifest is constructed.

Runs in its own `updater` CI job on every pull request, alongside
`installer`, and its result gates `check`. You can also run it by hand
from a Windows checkout with `pwsh .github/scripts/updater-e2e.ps1`.

It is a separate job because it builds the bundle twice — version N and
version N+1 — which is the expensive part; behind the matrix it would
add that time to every PR's critical path instead of running alongside
it. On failure the job uploads `ci-diagnostics/` (the msiexec verbose
logs and GMM's own JSON logs) as an artifact.

**No release secret is involved.** The script generates its own minisign
keypair with `tauri signer generate` for the length of the job and never
reads `TAURI_SIGNING_PRIVATE_KEY`, which stays release-only. That is
what lets this run on a fork's pull request.

Two things about that script are worth knowing before you edit it.

**GMM ships only the MSI installer.** The installer smoke and lifecycle jobs
exercise MSI installation, upgrade, repair, downgrade refusal, and uninstall,
so `bundle.targets` is deliberately `["msi"]` and users auto-update through
that MSI artifact. The updater test searches the whole bundle root and requires
exactly one signature; adding another installer target without extending the
product's lifecycle contract fails the job rather than silently leaving a
shipped updater artifact unverified. The
`v0.1.0-alpha.2` release exposed this gap by emitting both MSI and NSIS.
The target is Windows-specific, so release bundles must be built on Windows;
a macOS development host should not treat bare `pnpm tauri build` as installer
verification.

**The updater artifact is found via its `.sig`, not by extension.** What
the bundler signs has changed shape across Tauri versions — v1 and early
v2 signed a zipped installer, the 2.11 line this repo pins signs the raw
`.msi`. Asserting a fixed extension is how the script came to fail on
every correct build. It now takes the `*.sig` the bundler emitted and
derives the artifact beside it.

**The throwaway build sets `dangerousInsecureTransportProtocol`.** A
release build of `tauri-plugin-updater` refuses a non-HTTPS endpoint
outright, so an app pointed at `http://127.0.0.1` dies at startup with
exit code 101 before drawing a window. The flag lives only in the
per-build override the script writes; the shipped `tauri.conf.json`
keeps HTTPS-only endpoints, and `updater_config.rs` asserts it. Transport
security is not what this test covers — the signature is, and that is
unaffected by how the bytes arrived.

## 7. Installer lifecycle (`.github/scripts/installer-lifecycle.ps1`)

`installer-smoke.ps1` covers a clean machine. This covers the path every
*existing* user takes: upgrade, downgrade refusal, repair, uninstall (#57,
#141).

Runs as a second step in the same `updater` job and **reuses the two MSIs
`updater-e2e.ps1` already built**. A sibling job would have to run
`tauri build --release` twice more for coverage that overlaps, which
roughly doubles the Windows CI bill.

1. install 9.9.0, launch it, seed realistic state
2. assert exactly one Add/Remove Programs entry and **zero** startup
   registrations
3. upgrade to 9.9.1; assert one entry, one install directory, and that
   `GMM.exe`'s bytes actually changed
4. assert every seeded invariant survived
5. run the 9.9.0 MSI over 9.9.1; require Windows Installer exit code 1603
   and the `A newer version of GMM is already installed.` launch-condition
   message, then recheck version 9.9.1, the one Add/Remove Programs entry,
   the executable bytes, a working launch, and every seeded invariant
6. delete `GMM.exe`, run `msiexec /f`, assert it comes back
   byte-identical and user data is untouched
7. uninstall; assert the documented policy — install directory gone,
   `%APPDATA%\GMM` and Junctions kept

The refused-downgrade verbose log is retained in `ci-diagnostics/`. The exit
code is deliberately pinned rather than checked only for non-zero: a missing
MSI, malformed command line, or locked file must not impersonate the downgrade
guarantee.

### Why there is a fixture binary

The hard part of testing an upgrade is not running `msiexec` twice, it is
having something real to preserve. A canary text file proves almost
nothing: it is not written through GMM's code, not in the database, and
not a Junction.

`src-tauri/crates/lifecycle-fixture/` seeds the four things that matter
through `Core`'s own API — a Library entry, an enabled Mod, a live
Junction into a game directory, and an **Importer Pin** — then re-checks
them. The pin gets its own assertion rather than riding on a generic "app
data preserved" check because ADR 0004 makes it the escape hatch during a
ban-wave window: losing it silently is an account-safety regression.

The Junction is checked by **reading through it**, not by `exists()`. A
directory that is still there but no longer points at the Library passes
an existence check and loads nothing, which is exactly the failure an
upgrade could introduce.

`verify` reports every failure rather than stopping at the first, because
a Windows-less maintainer reads that CI log once.

It is test-only and never shipped — `tauri build` bundles only the `gmm`
binary, and `updater_config.rs` asserts the app package declares exactly
one. Helper binaries live in their own crate under `crates/` for that
reason.

## Running the suite

```bash
cd src-tauri
cargo fmt --check
cargo clippy --workspace --all-targets --no-deps -- -D warnings
cargo build --workspace       # required before the Windows-gated tests
cargo test --workspace
cargo test --test conflicts   # one file at a time when iterating
```

Frontend:

```bash
pnpm install --frozen-lockfile
pnpm tsc --noEmit
pnpm test                     # vitest
pnpm build
```

CI scripts (PowerShell 7, PSScriptAnalyzer 1.25.0, ShellCheck, and
actionlint 1.7.12):

```bash
pwsh -NoProfile -Command \
  'Set-PSRepository PSGallery -InstallationPolicy Trusted; Install-Module PSScriptAnalyzer -RequiredVersion 1.25.0 -Scope CurrentUser -Force'
pwsh -NoProfile -File .github/scripts/lint-ci-scripts.ps1
pwsh -NoProfile -File .github/scripts/test-lint-ci-scripts.ps1
.github/scripts/parse-shell-scripts.sh
.github/scripts/test-parse-shell-scripts.sh
find .github/scripts -type f -name '*.sh' -exec shellcheck {} +
.github/scripts/test-dead-origin-issues.sh
actionlint
```

The CI job runs these static checks on Ubuntu and is part of the aggregate
`check`; it neither waits for nor executes on a Windows runner.

Windows-only, from a Windows host:

```bash
cargo xtask test-loader       # 3dmloader FFI smoke
```

CI gates merge on all of the above plus the installer smoke. The AFK
runner runs the host-runnable ones before pushing; you should too, and
add `cargo xwin clippy` when you touch anything `cfg(windows)`.
