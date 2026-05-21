# GMM — Agent Instructions

Gacha Mod Manager: a Tauri + Rust + React/TypeScript Windows desktop mod manager for the XXMI-family of 3dmigoto-based gacha-game model importers (GIMI, SRMI, ZZMI, WWMI, HIMI, EFMI).

GMM is GPLv3 and embeds `3dmloader.dll` via Rust FFI. See `docs/adr/0001-gplv3-and-embed-3dmloader.md`.

## Required reading before any work

- `CONTEXT.md` — domain glossary (Model Importer, Library, Mod, Variant, Junction, Loader, Source, Importer Pin, Game Session, Conflict).
- `docs/adr/` — past architectural decisions. Always check before contradicting them.

## Agent skills

### Issue tracker

GitHub Issues on `Derek-X-Wang/gmm` via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default canonical vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context layout: one `CONTEXT.md` + one `docs/adr/` at repo root. See `docs/agents/domain.md`.
