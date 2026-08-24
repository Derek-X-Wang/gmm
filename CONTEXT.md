# GMM — Domain Glossary

Gacha Mod Manager: a Windows desktop mod manager for 3dmigoto-based gacha-game mods. Standalone reimplementation of XXMI Launcher with a central mod library, GameBanana ingest, and easy enable/disable via NTFS junctions.

## Terms

### Model Importer
The per-game configuration installed into a game directory: `d3dx.ini`, `Core/`, `ShaderFixes/`, and an empty `Mods/`. It contains **no compiled binaries** — the DLLs it drives (`d3d11.dll`, `d3dcompiler_47.dll`) ship with the Loader package, not here. At runtime the Loader patches the game and the Model Importer's configuration recursively scans `<Game>/Mods/` for `.ini` mod definitions. One importer per supported game: GIMI (Genshin), SRMI (Star Rail), ZZMI (ZZZ), WWMI (Wuthering Waves), HIMI (Honkai Impact 3rd), EFMI (Endfield). Distributed as GitHub release ZIPs; GMM downloads and extracts on demand. Because a Model Importer is configuration and HLSL rather than a build artifact, fixing a broken one is a text edit, not a compile.

### Game
One of the six supported titles. Each Game has its own Model Importer, its own Library subdirectory, its own GameBanana category, and its own install-path detection logic.

### Library
The central on-disk storage for all imported mods, located outside any game folder (default `%AppData%\GMM\library\<game>\<mod-id>\`). Source of truth for the user's mod collection. Backed up / portable independent of game installs. Enable/disable never changes these bytes, but GMM can permanently delete one explicitly confirmed unreferenced Library folder through the recovery audit.

### Mod
The unit of enable/disable. Owns: id (local), optional source (GameBanana submission), Game, display name, author, version, enabled flag, optional active variant. When enabled, exactly one NTFS Junction is created from `<Game>/Mods/<sanitized-name>/` to the Mod's effective Library path.

### Variant
A subfolder within a Mod representing one of several mutually exclusive presets (e.g. hair colors, costume options). A Mod has zero variants (single-folder mod) or two or more variants (radio-selected). Switching variants re-targets the Mod's Junction.

### Junction
An NTFS directory junction linking an enabled Mod's Library path into the game's `Mods/` directory. Chosen over symlinks because junctions require no admin rights or Developer Mode. Disabling a Mod = remove its Junction; the Library copy is untouched.

### Patch / Override
Mods that depend on or modify another Mod. **Out of scope for v1.**

### Loader
`3dmloader.dll` from `SpectrumQT/XXMI-Libs-Package` (GPLv3, forked from `bo3b/3Dmigoto`). The hook/inject library responsible for getting a Model Importer DLL into the game process at runtime. GMM embeds it directly via Rust FFI; the GMM process itself holds the loader for the lifetime of a modded game session. The Model Importer's `d3dx.ini` `loader:` setting points at GMM's exe rather than XXMI's. See ADR 0001.

### Source
The origin of a Mod's files. Possible values v1: `gamebanana` (URL-paste or 1-click import in future), `local` (user-supplied ZIP via drop-zone), `manual` (user constructed in-place outside GMM and adopted). Source determines update-check behaviour and provenance UI.

### Library Layout
On-disk shape: `%AppData%\GMM\library\<game-code>\<mod-id>\<variant-or-root>\…`. Game codes match Model Importer slugs: `gimi`, `srmi`, `zzmi`, `wwmi`, `himi`, `efmi`. Mod IDs are local ULIDs, not GameBanana IDs (a Mod can be re-imported from a different Source). A Mod's Library path final component **is its Mod ID**. Recovery preserves a valid orphan directory ULID as the recovered Mod ID, and refuses recovery if another Mod already claims that valid ID. Only when the final component is not a valid ULID does recovery generate a fresh ULID and rename the directory so the invariant still holds. User can override the global Library root and each per-game subpath in Settings.

GMM reserves `.gmm-delete-<token>` directories and their paired `.gmm-delete-<token>.intent` files as interrupted-delete quarantine state in every effective per-game Library root. The intent records the quarantined directory's filesystem identity; startup recursively removes a quarantine only when that identity still matches, and removes a stranded intent without touching an intact pre-rename directory.

GameBanana reinstall also reserves `.gmm-reinstall-<token>` as a same-root staging name. A matching row in the internal `reinstall_swaps` recovery table is a durable commit witness, never Mod metadata: while it exists, startup restores the old intent-backed quarantine and removes the staged/new tree; its deletion commits atomically with replacement metadata and Variants, after which the live replacement wins and startup may purge the old quarantine.

Library relocation is refused while a reinstall witness exists under the subtree being moved. Same-volume rename would preserve the witness identities, but cross-volume relocation copies directories and gives them new identities; the user must let an active reinstall settle and then retry the move. If startup cannot safely recover an interrupted reinstall, the witness remains the durable owner of both possible byte trees and that one Mod is quarantined as unusable. GMM still starts for every other Mod and Game, explains that a temporary lock may clear while moved/deleted paths or permissions need intervention, and offers an in-app retry of the same verified rollback. Ordinary delete cleanup cannot reclaim either witnessed identity while the quarantine remains.

### Importer Origin
Where a Game's Model Importer comes from: either a GitHub release origin (an `owner`/`repo` pair plus a release-asset match) or a user-supplied local ZIP. Not to be confused with **Source**, which is about a Mod's files; Importer Origin is about the per-game importer package. Resolved through three layers — the user's per-game override, GMM's curated `recommended-importers.json` recommendation, then the compiled-in default — and recorded per install, where `unknown` is a valid value for importers installed outside GMM. A recommendation decides the origin for an install that does not exist yet; an existing install's origin changes only through an Origin Proposal the user accepts, or an override they set. Update never switches origin. Changing a Game's Importer Origin invalidates its install and clears its Importer Pin. See ADR 0005.

### Origin Proposal
GMM's offer to move a Game onto a different Importer Origin, shown on that Game's own surface with the recommendation's optional reason and what accepting will replace. Accepting installs from the proposed origin through the ordinary backup-and-rollback path; it is the only act that moves an existing install, and the only way an `unknown` origin becomes known. There is deliberately no way to record an origin without installing. Declining records an **Origin Dismissal** and touches nothing.

### Origin Dismissal
A declined Origin Proposal, remembered per Game and keyed by **the origin it proposed**, compared case-insensitively. A later proposal of a different origin still prompts; the same origin stays quiet. Dismissals are listed and reversible on the affected Game's surface, and survive the recommendations switch being turned off and back on. Distinct from opting out: a dismissal is a judgement about one proposal, the switch is a standing preference.

### Importer Pin
A per-game `pinned_version` setting that suppresses Model Importer **version** update prompts for that game. Used by users during ban-wave windows or when a new importer release breaks a mod they care about. It does not suppress Importer Origin recommendations: a pin says "don't move me to a newer build of this package", not "stop telling me the package's source has died". Changing a game's Importer Origin clears its pin, because a version string taken against one origin means nothing against another. See ADR 0004 and ADR 0005.

### Game Session
The window of time between GMM spawning the game process and the game process exiting. During a Game Session, GMM holds the Loader in-process, mod enable/disable is locked, and the UI shows a "Game running" banner. Mods can only be toggled outside Game Sessions.

### Conflict
Two enabled Mods bind the same 3dmigoto resource hash (`[TextureOverride…]`, `[ResourceOverride…]`, etc.). GMM detects conflicts by parsing INIs at enable time and builds a hash-to-mods map. v1 surfaces conflicts as warnings without resolving a winner; users disable one of the conflicting Mods to clear the warning. Priority-order resolution (MO2-style) is deferred to v1.1.
