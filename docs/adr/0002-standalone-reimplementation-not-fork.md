# 0002 — GMM is a standalone reimplementation, not a fork of XXMI Launcher

Date: 2026-05-20
Status: Accepted

## Context

GMM's feature surface overlaps significantly with XXMI Launcher: detect game installs, download Model Importer packages, lay them out in the game directory, run as the loader process. XXMI is mature and battle-tested. The fastest way to ship is to fork XXMI and add features on top.

GMM's chosen stack is Tauri + Rust + React/TypeScript. XXMI is Python + customtkinter. The stacks are incompatible — a "fork" would in practice mean either rewriting everything anyway, or bundling XXMI's Python runtime as an opaque subprocess and treating GMM as a shell around it.

## Considered alternatives

- **Fork XXMI in-place.** Continue in Python with customtkinter. Cheapest implementation but loses the small-binary, web-UI, fast-iteration benefits we picked Tauri for. Also locks GMM into XXMI's UI architecture, which is the part we most want to redesign.
- **Bundle XXMI Python runtime as a subprocess.** Tauri shell drives an embedded XXMI. Adds 50-80 MB of Python runtime to the binary, defeating Tauri's footprint advantage. Double-process complexity for cross-cutting features like progress reporting.
- **Sit alongside XXMI.** User installs XXMI separately; GMM contributes only the mod-library and GameBanana layers. Drops the auto-updater and importer-install feature scope entirely.

## Decision

GMM is a standalone reimplementation in Rust (backend) + TypeScript (UI). It downloads the same public Model Importer release ZIPs that XXMI consumes (`*-Package` repos under SpectrumQT / leotorrez orgs) and the same `XXMI-Libs-Package` loader, but does not vendor or link XXMI Launcher's source code. We may study XXMI source for behavioural reference (both GPLv3) but write all GMM logic ourselves.

## Consequences

- We re-pay XXMI's per-game install / detection cost in Rust. Estimated weeks of work across the six v1 games. This is the biggest single up-front investment in the project.
- Bug-fix velocity is independent of XXMI's release cadence. Conversely, we do not benefit from upstream fixes for free.
- Game-detection heuristics, importer-install flows, and per-game quirks (FPS unlock, HDR toggle, anti-cheat handling) must be documented in GMM's own per-game modules in `src-tauri/src/games/<game>.rs` — we lose XXMI's existing Python implementations as reference but write our own pattern.
- We retain freedom to redesign all UX without negotiating XXMI's existing structure.
