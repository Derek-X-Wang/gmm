# Loader subsystem

How GMM gets a Model Importer DLL into a running gacha-game process.

## Why it exists

3dmigoto-derived Model Importers (GIMI, SRMI, ZZMI, WWMI, HIMI, EFMI) install a `d3d11.dll` proxy plus a `d3dx.ini` config inside the game directory. The `d3dx.ini` references a separate _loader_ executable that must be running at game-launch time to install a CBT hook and inject the importer into the game process. XXMI Launcher uses `XXMI Launcher.exe` for this role; GMM uses `gmm.exe` (per ADR 0002 — standalone reimplementation).

The actual injection logic — Win32 hook + named-mutex handshake + DLL-into-process injection — lives in `3dmloader.dll`. We vendor that DLL (see [`vendor/3dmloader/README.md`](../../vendor/3dmloader/README.md)) and bind it from Rust via the `gmm-loader` crate.

## Crate layout

The loader lives in the `src-tauri/crates/` workspace alongside the helpers needed to smoke-test it without a real game:

| Crate       | Type     | Platform  | Role |
|-------------|----------|-----------|------|
| `gmm-loader`| lib      | all       | Public Rust API. `Loader::load`, `Loader::hook`, `HookSession::wait_for_injection`, `HookSession::unhook`, `Loader::inject`. Real impl on Windows; uniform-error stub elsewhere so the rest of the codebase compiles. |
| `victim`    | bin      | Windows*  | Controlled target process. Creates a window with class `GMM-LOADER-TEST-VICTIM`, idles, exits cleanly on `WM_CLOSE` or after 30 s. Non-Windows stub `main` exits with code 64 so the workspace builds end-to-end on Linux CI. |
| `noop_dll`  | cdylib   | Windows*  | DLL whose `DllMain` returns `TRUE` and does nothing else. The hook injects this into `victim`. |
| `xtask`     | bin      | all       | `cargo xtask test-loader` orchestrates the smoke test. |

`*` `victim` and `noop_dll` only do anything meaningful on Windows. They compile on every platform so the workspace stays cross-platform-buildable.

## Entry points

`3dmloader.dll` exposes four C functions. Signatures from upstream's `XXMI-Launcher/src/xxmi_launcher/core/utils/dll_injector.py`:

```c
int HookLibrary(LPCWSTR dll_to_inject_path,
                HHOOK*  out_hook_handle,
                HANDLE* out_named_mutex);

int WaitForInjection(LPCWSTR dll_to_inject_path,
                     LPCWSTR target_process_name,
                     int     timeout_secs);

int UnhookLibrary(HHOOK*  in_out_hook_handle,
                  HANDLE* in_out_named_mutex);

int Inject(DWORD   target_pid,
           LPCWSTR dll_path,
           int     flags);
```

All four return `0` on success. The upstream library does not publish a stable error-code table; we surface non-zero statuses verbatim through `Error::NonZeroStatus { symbol, status }`. XXMI's Python wrapper documents these meaningful values for `HookLibrary`:

| status | meaning |
|-------:|---------|
| 0      | success |
| 100    | another instance of the loader is already hooked |
| 200    | failed to `LoadLibraryW` the supplied DLL — DLL missing or invalid |
| 300    | DLL missing the entry point upstream expects |
| 400    | failed to install the CBT hook |

`HookLibrary`'s first argument is **the DLL to inject**, not a window class. 3dmloader watches all window creations in every process; when a window appears, it `LoadLibraryW`'s the configured DLL inside that process. `WaitForInjection` then blocks until the DLL has loaded into a process whose name matches `target_process_name` (substring match).

## Argument lifetimes

| Argument                                  | Owner during call | Notes |
|-------------------------------------------|-------------------|-------|
| `LPCWSTR` strings                         | Rust caller       | We always pass pointers into a Rust-owned `Vec<u16>` that contains a single NUL terminator and lives for the entire call. Interior NUL bytes are rejected by `to_wide_nul` with `Error::InvalidPath`. |
| `HHOOK* out_hook_handle`, `HANDLE* out_named_mutex` (Hook) | Rust caller       | Pointers to fields of the freshly-built `HookSession`. After a successful Hook call those fields hold valid handles until Unhook zeroes them. |
| Same two pointers (Unhook)                | Rust caller       | Borrowed mutable into the live `HookSession` value. After the call both fields are zeroed by upstream; subsequent Unhook is a no-op. |
| `DWORD target_pid` (Inject)               | Rust caller       | Raw integer; the OS process is the actual resource and is the caller's responsibility. |
| `int timeout_ms`, `int flags`             | Rust caller       | Plain integers. |

## Panic / cleanup contract

The hook + named-mutex handles live inside `HookSession`. Its `Drop` impl calls `UnhookLibrary` unconditionally — including when the surrounding code is unwinding from a panic. Internally `HookSession` checks whether the handles are null (a successful explicit `unhook()` zeroes them and then `mem::forget`s the value) to avoid double-unhook.

`std::mem::forget` on a `HookSession` is the only way to bypass cleanup, and the public API never does this. Callers can `mem::forget` it themselves but we treat that as a documentation issue, not a soundness bug — the upstream library tolerates orphaned hooks long enough for the process to exit.

The `Loader` value owns the loaded `HMODULE`. Its `Drop` impl calls `FreeLibrary`. Multiple `HookSession`s derived from the same `Loader` share the underlying `LoadedDll` through an `Arc`, so freeing happens when the last consumer drops.

## Why this is GPLv3

`3dmloader.dll` is a fork of `bo3b/3Dmigoto`, both GPLv3. We dynamically link to `3dmloader.dll` from `gmm-loader`, which in turn is linked into `gmm.exe`. Under any plausible reading of GPLv3, that makes the entire binary GPLv3 — see ADR 0001 for the decision rationale.

We accept this on purpose: every Model Importer GMM is built to manage is GPLv3 (and is itself a 3dmigoto fork). A permissive GMM in the middle of a GPLv3 sandwich buys nothing and complicates redistribution.

## Audit guidance

When reviewing `crates/loader/src/windows.rs`, focus the `unsafe` audit on three regions:

1. **`Loader::load`'s FFI symbol resolution.** Pre-condition: `dll_path` is a valid UTF-16 NUL-terminated buffer (validated in `to_wide_nul`). Post-condition: each of the four function pointers points at a real export in the loaded DLL or `Error::MissingSymbol` is returned. The `std::mem::transmute::<*const (), FnXxx>` calls assume upstream's ABI matches our typedef — verified manually against `dll_injector.py` and the pinned `XXMI-Libs-Package` v0.8.8 release.
2. **`Loader::hook`'s call.** Pre-condition: `target_wide` is well-formed; `hook` and `mutex` are valid mutable references to a local stack frame. Post-condition: on success, `HookSession` is constructed with the returned handle values; on failure, no resources are leaked.
3. **`Drop for LoadedDll`.** Calls `FreeLibrary(self.handle)`. Safety follows from the invariant that `self.handle` was returned by `LoadLibraryW` in `Loader::load` and has not been freed elsewhere — `HMODULE` lives inside an `Arc<LoadedDll>` so no other path can free it.

There is no `unsafe` reachable from any public method **except** through these FFI calls. All other public functions return owned values (`Vec<u16>`, `PathBuf`, `Result<…>`) or borrowed `&Path`. No raw pointer escapes the crate.

## Smoke test

The end-to-end smoke runs in two places:

- `cargo xtask test-loader` on a developer's Windows host — fastest iteration.
- CI's `build (windows-latest)` matrix entry — gates merges to `main`.

The test is intentionally tiny:

1. Load `vendor/3dmloader/3dmloader.dll` via `Loader::load`.
2. `loader.hook(noop_dll_path)` installs the CBT hook configured to inject `noop_dll.dll` into any process that creates a window.
3. Spawn `victim.exe`. It creates a hidden window; 3dmloader observes the creation and `LoadLibraryW`s `noop_dll.dll` inside victim's address space.
4. `session.wait_for_injection("victim", 15)` waits up to 15 s for `noop_dll.dll` to land in a process whose name contains `"victim"`.
5. Drop `HookSession` (unhook) and `Loader` (FreeLibrary). Confirm `victim` exits cleanly.

The smoke does **not** assert anything inside `victim`'s memory space — we trust `wait_for_injection`'s success as proof. If a future regression makes that signal unreliable we can extend `victim` to write a sentinel value to a named-pipe on `DLL_PROCESS_ATTACH`.

## How to upgrade `3dmloader.dll`

See [`vendor/3dmloader/README.md`](../../vendor/3dmloader/README.md) for the version-bump procedure. Any change to the four entry points means `crates/loader/src/windows.rs` needs the matching change here.
