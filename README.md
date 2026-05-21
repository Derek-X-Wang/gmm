# GMM — Gacha Mod Manager

Tauri + Rust + React/TypeScript Windows desktop mod manager for the
XXMI-family of 3dmigoto-based gacha-game model importers (GIMI, SRMI,
ZZMI, WWMI, HIMI, EFMI). Standalone reimplementation; see
[`docs/adr/0002-standalone-reimplementation-not-fork.md`](docs/adr/0002-standalone-reimplementation-not-fork.md).

GMM is GPLv3 and embeds `3dmloader.dll` via Rust FFI. See
[`docs/adr/0001-gplv3-and-embed-3dmloader.md`](docs/adr/0001-gplv3-and-embed-3dmloader.md).

## Diagnostics & privacy

GMM does not phone home. There is no telemetry, no crash reporter, no
background uploader. Structured logs are written **only** to your local
disk at:

- Windows: `%AppData%\GMM\logs\gmm-YYYY-MM-DD.log`
- macOS (dev): `~/Library/Application Support/GMM/logs/gmm-YYYY-MM-DD.log`
- Linux (dev): `~/.local/share/GMM/logs/gmm-YYYY-MM-DD.log`

Files are JSON-lines, one event per line. Daily rotation is automatic;
files older than 14 days are deleted on startup.

To share diagnostics on a bug report, open Settings → Diagnostics →
**Export diagnostics bundle**. The resulting `.zip` contains the last
7 days of logs plus a redacted `settings.json` snapshot. The proxy
URL's userinfo (e.g. `user:password@`) is redacted to `REDACTED@`;
library and game install paths are preserved because they are usually
required to reproduce path-dependent bugs.

You decide where the bundle is written and whether to share it. GMM
never uploads anything on your behalf.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
