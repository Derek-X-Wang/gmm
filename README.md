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

## Network / Proxy

GMM's outbound traffic (GitHub release downloads, GameBanana API) can
be routed through an HTTP or SOCKS5 proxy. Configure it under Settings →
Network. Expected URL formats:

- `http://host:port`
- `http://host:port` with optional username/password fields (Basic auth)
- `socks5://host:port`

The "Test connection" button issues a HEAD request to
`api.github.com` through the configured proxy and surfaces a clear
"Proxy unreachable" error if it cannot connect. The proxy password is
write-only in the UI and never read back — it lives in the local
SQLite settings table and is excluded from the diagnostics bundle.
Userinfo pasted into the URL field (e.g. `http://user:pass@proxy`) is
also redacted in the bundle as defence-in-depth.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
