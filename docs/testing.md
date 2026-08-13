# Testing patterns

GMM ships five layers of automated coverage. Layers 1–2 run anywhere;
layers 3–5 are Windows-specific and exist so nobody has to sit at a
Windows box to know whether a change works.

| Layer | Where it runs | What it proves |
|---|---|---|
| 1. Core integration | any host | business logic against a temp SQLite + Library |
| 2. IPC contract | any host | Tauri command wire shapes (serde) |
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
cd src-tauri
cargo test --test migrations -- --ignored --exact regenerate_the_migration_corpus
git add tests/fixtures/migrations
```

It rewrites every fixture from the checked-in migration SQL, so run it
only when the corpus is genuinely out of date — and check the diff:
existing fixtures changing means a shipped migration changed.

## 2. Tauri command IPC contract (`src-tauri/tests/commands_ipc.rs`)

The Tauri `#[tauri::command]` macro generates a runtime wrapper that
uses serde for both the incoming Args struct and the returned value.
Driving that wrapper through `tauri::test::get_ipc_response` requires
synthesising a `Context<MockRuntime>` that carries the real ACL
capabilities — historically painful (see issue #26 body).

Pragmatic alternative: route the **same Args + return types** the
command body uses through `serde_json` and call the Core method
directly. The wire shape is identical; we just skip the runtime.

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

When adding a new command, extend `tests/commands_ipc.rs` with two
assertions per shape:

1. **Args deserialise.** Build a `serde_json::json!({ … })` value
   matching the JS-side shape and `from_value` it into the Args
   struct.
2. **Return serialises.** Run the Core method through whatever setup
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

Three drift guards keep it honest, all in `tests/ipc_contract.rs`: the
marker command must still be registered and still invoked by the
frontend, its body must still call `record_ipc_ready`, and the
PowerShell must still grep the same literal as
`diagnostics::IPC_READY_MARKER`.

If you move the marker to a different command, move all four together.

On failure it dumps the msiexec verbose log, GMM's own JSON logs, and
recent Application event-log errors — so a Windows-less maintainer can
still diagnose from the CI output. Artifacts are uploaded too.

This is the layer that would have caught a broken bundle, a missing
WebView2 dependency, or a migration that fails on a clean machine.

## 6. Signed-update round trip (`.github/scripts/updater-e2e.ps1`)

`tests/updater_config.rs` checks that `tauri.conf.json` *says* the right
things — a well-formed pubkey, HTTPS endpoints,
`createUpdaterArtifacts: true`. That is shape only, and the flag was
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
  with the throwaway key, serves the newer one's `latest.json` over
  `127.0.0.1`, downloads it back, verifies it via the ignored
  `the_bundled_artifact_verifies_against_the_key_that_signed_it` test,
  then installs the update over the older build and asserts the version
  moved, the app still starts (via the IPC readiness marker), and
  `%APPDATA%\GMM` survived.

Run it by hand from a Windows checkout with `pwsh
.github/scripts/updater-e2e.ps1`. **It is not wired into CI yet** — see
the follow-up issue; `.github/workflows/` is maintained by hand.

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

Windows-only, from a Windows host:

```bash
cargo xtask test-loader       # 3dmloader FFI smoke
```

CI gates merge on all of the above plus the installer smoke. The AFK
runner runs the host-runnable ones before pushing; you should too, and
add `cargo xwin clippy` when you touch anything `cfg(windows)`.
