# Testing patterns

GMM ships two layers of automated coverage. Both run on macOS dev
hosts so the same workflow works locally + in CI (Linux runner).

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

## Running the suite

```bash
cd src-tauri
cargo fmt --check
cargo clippy --all-targets --no-deps -- -D warnings
cargo test                # all integration tests
cargo test --test conflicts   # one file at a time when iterating
```

Frontend type-check + build:

```bash
pnpm install --frozen-lockfile
pnpm tsc --noEmit
pnpm build
```

CI gates merge on all six commands above. The AFK runner runs them
locally before pushing; you should too.
