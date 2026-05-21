# GMM — Gacha Mod Manager

Tauri + Rust + React/TypeScript Windows desktop mod manager for the
XXMI-family of 3dmigoto-based gacha-game model importers (GIMI, SRMI,
ZZMI, WWMI, HIMI, EFMI). Standalone reimplementation; see
[`docs/adr/0002-standalone-reimplementation-not-fork.md`](docs/adr/0002-standalone-reimplementation-not-fork.md).

GMM is GPLv3 and embeds `3dmloader.dll` via Rust FFI. See
[`docs/adr/0001-gplv3-and-embed-3dmloader.md`](docs/adr/0001-gplv3-and-embed-3dmloader.md).

## Antivirus and SmartScreen

GMM is unsigned and behaves like a DLL injector at launch time — it
holds `3dmloader.dll` in process, installs a Win32 CBT hook, and calls
`LoadLibraryW` on a `d3d11.dll` proxy from inside the game's address
space. None of this is malicious (it is exactly what loading a
3dmigoto-derived Model Importer requires), but Windows Defender,
third-party antivirus products, and SmartScreen all flag the pattern.

**Why the binary is unsigned.** An Authenticode certificate is a
recurring cost we have deferred until v1 user numbers justify it; the
XXMI ecosystem GMM grew out of has the same gap. Each release page on
GitHub publishes a SHA-256 digest so you can verify the binary against
the release before adding any exclusion.

**How to add an exclusion** in Windows Defender:
*Settings → Privacy & security → Windows Security → Virus & threat
protection → Manage settings → Add or remove exclusions → Folder*.
Add the GMM install directory (default `%LocalAppData%\Programs\GMM\`)
and the GMM data directory (default `%AppData%\GMM\`). Restart GMM
afterwards — Defender does not re-evaluate running processes until they
relaunch. Most third-party antivirus products (Norton, Bitdefender,
Avast, AVG, ESET, Kaspersky) expose the same concept under *Exceptions*
or *Trusted folders*; see the canonical guide for product-specific
menu paths.

**If the binary was auto-quarantined**, restore it from your AV's
quarantine UI **before** adding the exclusion (otherwise the restore
is re-quarantined immediately). For Windows Defender: *Windows Security
→ Virus & threat protection → Protection history → Actions → Restore*.
Then add the exclusion using the steps above and reinstall the affected
Model Importer from inside GMM (*Model Importer panel → Reinstall
importer*).

**SmartScreen on first launch.** Click *More info* on the "Windows
protected your PC" prompt, then click *Run anyway*. The prompt does not
repeat for subsequent launches of the same installer. We deliberately
do **not** ship a manifest workaround or compatibility-shim trick to
hide GMM from SmartScreen — the right fix is signing, which we will do
when funding allows.

The canonical, full-length version of this guide — including
product-specific exclusion menus and the in-app launch-error copy — is
at [`docs/antivirus-and-smartscreen.md`](docs/antivirus-and-smartscreen.md).
When GMM's Launch action fails with an OS error string matching a known
AV / SmartScreen pattern, the in-app error renders the same exclusion
instructions inline instead of just the raw error.

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

## Updates and signature verification

GMM checks for new releases on every launch through
[`tauri-plugin-updater`](https://v2.tauri.app/plugin/updater/). The
`tauri.conf.json`'s `plugins.updater.endpoints` points at
`https://github.com/Derek-X-Wang/gmm/releases/latest/download/latest.json`;
when a new tag is published, that URL resolves to a signed manifest
produced by `.github/workflows/release.yml`.

**Signing.** Each release is signed with a minisign keypair generated
once via `pnpm tauri signer generate`. The public half is embedded in
the binary (see `plugins.updater.pubkey` in `src-tauri/tauri.conf.json`);
the private half lives only as a repo secret (`TAURI_SIGNING_PRIVATE_KEY`
plus `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) and on the maintainer's
backups. `tauri-plugin-updater` refuses to install any release whose
signature does not verify against the embedded public key.

**On-demand checks.** Settings → Updates → *Check for updates* runs
the same flow the auto-check does — fetch `latest.json`, verify the
signature, download + install if newer, prompt to relaunch.

**Releasing.** Push a tag matching `v*` (e.g. `v0.5.0-alpha.1`); the
`release.yml` workflow builds the MSI on `windows-latest`, signs the
manifest, and drafts a GitHub Release with the binary, `.sig`, and
`latest.json`. The maintainer reviews the draft (smoke test on a real
Windows machine, double-check the changelog) and clicks Publish; only
after Publish does the `latest.../latest.json` URL resolve.

**If the key rotates** (lost, leaked, or maintainer turnover), every
existing GMM install becomes unable to install future updates and must
be reinstalled manually from a fresh GitHub Release. There is no
key-rotation flow in v1.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
