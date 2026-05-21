# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

GMM uses the **single-context layout**: one `CONTEXT.md` and one `docs/adr/` at the repo root.

## Before exploring, read these

- **`CONTEXT.md`** at the repo root — the domain glossary.
- **`docs/adr/`** — read ADRs that touch the area you're about to work in.

If any of these files don't exist, **proceed silently**. Don't flag their absence; don't suggest creating them upfront. The producer skill (`/grill-with-docs`) creates them lazily when terms or decisions actually get resolved.

## File structure

```
/
├── CONTEXT.md
├── docs/adr/
│   ├── 0001-gplv3-and-embed-3dmloader.md
│   ├── 0002-standalone-reimplementation-not-fork.md
│   ├── 0003-junctions-over-symlinks-and-copy.md
│   └── 0004-conservative-auto-update-defaults.md
└── src/
```

This repo is a single application (Tauri shell + Rust backend + React frontend); there is no `CONTEXT-MAP.md` and no per-context ADR directories. If GMM ever grows additional independent contexts (e.g. a separate server-side mod-index service), revisit this layout.

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in `CONTEXT.md`. Don't drift to synonyms the glossary explicitly avoids.

For GMM specifically: prefer `Mod` (not "package" or "skin"), `Variant` (not "preset"), `Library` (not "mod folder" or "store"), `Junction` (not "symlink" or "link"), `Loader` (not "injector"), `Game Session` (not "play session" or "runtime"), `Importer Pin` (not "version lock"), `Model Importer` (not "3dmigoto" or "DLL bundle" — the importer is built on top of 3dmigoto but is the user-facing concept).

If the concept you need isn't in the glossary yet, that's a signal — either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `/grill-with-docs`).

## Flag ADR conflicts

If your output contradicts an existing ADR, surface it explicitly rather than silently overriding:

> _Contradicts ADR-0001 (GMM is GPLv3, embed 3dmloader.dll) — but worth reopening because…_
