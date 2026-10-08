# GMM CLI

`gmm-cli` lets a local agent inspect GMM and apply Mods discovered elsewhere.

GMM performs no Mod search by design. Discovery on a Source is the caller's
job; GMM applies the discovered Mod by URL or submission ID.

The Windows MSI installs `gmm-cli.exe` beside `GMM.exe`, by default at
`%ProgramFiles%\GMM\gmm-cli.exe` (normally
`C:\Program Files\GMM\gmm-cli.exe`). If you choose a different install
directory, use `gmm-cli.exe` in that directory.

Configure the agent's executable path once with this full path. The installer
does not modify `PATH`: an agent can invoke the installed executable directly,
without elevation to edit `PATH` or leaving a `PATH` entry after uninstall.
For example, in PowerShell (close the GMM window first):

```powershell
& "$env:ProgramFiles\GMM\gmm-cli.exe" status
```

The CLI is built afresh with each app bundle and carries the same Windows
product/file version as the app. For development, build it with
`cargo build --workspace` from `src-tauri/`. To build a Windows MSI from source,
use `pnpm tauri build --config src-tauri/tauri.bundle.conf.json`; release and
installer CI use this same packaging config. It prepares Tauri's
target-triple-suffixed sidecar, which the MSI installs as `gmm-cli.exe`.

Every invocation prints one JSON object on one line. Success is
`{"ok":true,"result":{...}}`; failure is
`{"ok":false,"error":{"kind":"other","message":"..."}}`.
Errors reuse GMM's command-error classification, including
`invalidActiveVariant` and `alreadyRunning`. Exit statuses follow the probe:
0 success, 1 invalid usage, 2 operation failure or refusal.

`--data-dir PATH` defaults to the same resolved OS data directory plus `GMM`
as the app. Every valid invocation attempts to take that directory's instance lock;
close the GMM window first. Usage errors are returned before state is opened.

The CLI refuses both lock contention and lock I/O failures. The window app
deliberately fails open on lock I/O failures (for example, an unopenable lock
file during antivirus interference), logging the failure and starting without
a lock. It still refuses detected contention. The shared lock therefore does
not guarantee exclusion when the app has started without it; close that app
before using the CLI against the same data directory.

| Command | Required options | Result |
| --- | --- | --- |
| `games` | None | Supported Games, detected paths, configured install paths |
| `mods` | Optional `--game CODE` | Mods with enabled state, Variants, active selection |
| `importer` | `--game CODE` | Installed version, installed Origin, Importer Pin |
| `status` | None | All attention reports and `safeToProceed` |
| `variants` | `--mod-id ID` | Variants and active selection |
| `conflicts` | `--game CODE` | Conflicting hashes and contributing Mods |
| `import` | `--game CODE --source URL_OR_ID` | Imported GameBanana Mod |
| `import-zip` | `--game CODE --archive PATH --name NAME` | Imported local Mod |
| `adopt` | `--game CODE --from PATH --name NAME` | Adopted folder Mod |
| `enable` | `--mod-id ID` | Updated Mod |
| `disable` | `--mod-id ID` | Updated Mod |
| `set-variant` | `--mod-id ID --variant-id ID` | Updated Mod |
| `launch` | `--game CODE` | Completed Game Session |

Game codes are `gimi`, `srmi`, `zzmi`, `wwmi`, `himi`, and `efmi`.
Options may appear before or after the command. Duplicate, unknown, and missing
options are usage errors. Mod operations infer the Game from the persisted Mod.
Game paths and Model Importers are configured in the window; installing an
Importer, browsing Sources, recovery actions, and other window operations are
outside this command surface.

All seven state-changing commands accept `--dry-run`. The result describes the
requested action, destination, relevant identifiers, confirmation requirement,
and current attention report. It performs no download, extraction, copy,
Junction change, launch, migration, or recovery. A fresh dry run creates no
database or Library; taking the required lock can create the data directory
and its empty `instance.lock`. Source import previews cannot know the upstream
archive contents or metadata until the real download.

`enable`, `disable`, and `set-variant` require `--confirm` for a real operation.
They are the explicit Game-directory write class: enable creates a Junction,
disable removes one, and Variant selection can retarget an enabled Junction.
The Variant command uses this gate even when the Mod is currently disabled,
so its confirmation rule does not depend on mutable enabled state.

Before a real state-changing operation, an unsafe status refuses the command.
Inspect `status` and resolve the issue in the window first. After explicit
user agreement, `--allow-attention` bypasses this aggregate refusal; it does
not bypass any Core ownership, recovery, or Game Session guard. An unreadable
sub-report remains an error when the aggregate guard is used.

Status includes pending staged Library imports and adoptions in
`stagedLibraryOperations`, naming the witness ID, Game, operation, staged path,
and any recovery error. These witnesses make `safeToProceed` false even when
their owned partial directory is hidden from the unreferenced-directory audit.

Inspection never runs startup recovery or migrations. Existing databases open
read-only, and a stale, failed, or unrecognised migration record is refused;
open the app to migrate it before inspecting. Real writes use a distinct
non-recovering Core constructor that migrates before operating. This preserves
pending witnesses for `status` instead of silently repairing them on open.

`launch` shares the app's reservation, injection, Loader ownership and exit
watcher. The CLI stays running and holds its instance lock for the entire Game
Session, then prints the final JSON result after the Game exits. Keep the
invocation alive while playing. Native injection requires Windows and the
existing Loader resolution rules (`GMM_LOADER_DLL`, beside the executable,
or the vendored development copy).

Headless launch supplies no session-event adapter and instantiates no Tauri
runtime. The Windows CI loader diagnostic rejects GUI system imports in both
debug and release CLI executables, so the CLI no longer needs the app's
Common-Controls v6 manifest. The window app still embeds its own manifest.

The CLI releases its own launch reservation on an ordinary launch failure and
clears its active Game Session when the exit watcher observes the Game ending.
Ctrl+C or force-killing the CLI can prevent that cleanup. It does not prove the
Game exited: dropping a child-process handle does not terminate the Game. The
CLI uses dead-PID recovery rather than a signal handler: before a real
state-changing command, it checks the recorded Game PID and clears the session
only if that process has exited, using the same liveness check as the window.
This also handles an interrupted caller whose Game exits later. Close the Game,
then retry `launch`, `enable`, or `disable`; no window is needed for a dead
active session. A live or inaccessible process remains a blocker, and the
refusal tells you to close the Game and retry. `--allow-attention` cannot bypass
that guard.

`status` adds `activeSessionLiveness`: `null` with no active session, or an
object with `state` (`live` or `stale`) and `remedy`. An inaccessible PID is
conservatively reported as live. The persisted `activeSession` and aggregate
`safeToProceed` retain their existing meaning: a stale record still makes the
aggregate false until a real write clears it. Status and dry runs read liveness
without deleting records or repairing Library witnesses.

Real writes also prune abandoned launch reservations when recorded process
identities prove them finished. If a crash happened before the child PID was
recorded, recovery cannot prove whether a Game was spawned; open the window and
use its explicit retirement action after confirming the Game is closed.

Example workflow:

```sh
gmm-cli status
gmm-cli import --game gimi --source 123456 --dry-run
gmm-cli import --game gimi --source 123456
gmm-cli mods --game gimi
gmm-cli enable --mod-id MOD_ID --dry-run
gmm-cli enable --mod-id MOD_ID --confirm
gmm-cli conflicts --game gimi
gmm-cli launch --game gimi
```
