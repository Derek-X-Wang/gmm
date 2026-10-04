# GMM CLI

`gmm-cli` lets a local agent inspect GMM and apply Mods discovered elsewhere.
Build it with `cargo build --workspace` from `src-tauri/`. It is a workspace
binary; the installer continues to bundle only the app, whose name stays `gmm`.

Every invocation prints one JSON object on one line. Success is
`{"ok":true,"result":{...}}`; failure is
`{"ok":false,"error":{"kind":"other","message":"..."}}`.
Errors reuse GMM's command-error classification, including
`invalidActiveVariant` and `alreadyRunning`. Exit statuses follow the probe:
0 success, 1 invalid usage, 2 operation failure or refusal.

`--data-dir PATH` defaults to the same resolved OS data directory plus `GMM`
as the app. Every valid invocation takes that directory's instance lock;
close the GMM window first. Usage errors are returned before state is opened.

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
