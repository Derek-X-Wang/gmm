# GMM — Domain Glossary

Gacha Mod Manager: a Windows desktop mod manager for 3dmigoto-based gacha-game mods. Standalone reimplementation of XXMI Launcher with a central mod library, GameBanana ingest, and easy enable/disable via NTFS junctions.

## Terms

### Model Importer
A 3dmigoto-derived DLL bundle (`d3d11.dll` + INIs) installed into a game directory. At runtime it patches the game and recursively scans `<Game>/Mods/` for `.ini` mod definitions. One importer per supported game: GIMI (Genshin), SRMI (Star Rail), ZZMI (ZZZ), WWMI (Wuthering Waves), HIMI (Honkai Impact 3rd), EFMI (Endfield). Distributed as GitHub release ZIPs; GMM downloads and extracts on demand.

### Game
One of the six supported titles. Each Game has its own Model Importer, its own Library subdirectory, its own GameBanana category, and its own install-path detection logic.

### Library
The central on-disk storage for all imported mods, located outside any game folder (default `%AppData%\GMM\library\<game>\<mod-id>\`). Source of truth for the user's mod collection. Backed up / portable independent of game installs.

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
On-disk shape: `%AppData%\GMM\library\<game-code>\<mod-id>\<variant-or-root>\…`. Game codes match Model Importer slugs: `gimi`, `srmi`, `zzmi`, `wwmi`, `himi`, `efmi`. Mod IDs are local ULIDs, not GameBanana IDs (a Mod can be re-imported from a different Source). User can override the global Library root and each per-game subpath in Settings.

### Importer Pin
A per-game `pinned_version` setting that suppresses Model Importer update prompts for that game. Used by users during ban-wave windows or when a new importer release breaks a mod they care about. See ADR 0004.

### Game Session
The window of time between GMM spawning the game process and the game process exiting. During a Game Session, GMM holds the Loader in-process, mod enable/disable is locked, and the UI shows a "Game running" banner. Mods can only be toggled outside Game Sessions.

### Conflict
Two enabled Mods bind the same 3dmigoto resource hash (`[TextureOverride…]`, `[ResourceOverride…]`, etc.). GMM detects conflicts by parsing INIs at enable time and builds a hash-to-mods map. v1 surfaces conflicts as warnings without resolving a winner; users disable one of the conflicting Mods to clear the warning. Priority-order resolution (MO2-style) is deferred to v1.1.
