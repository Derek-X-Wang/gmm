# 0003 — NTFS junctions for the Library → Game overlay

Date: 2026-05-20
Status: Accepted

## Context

Enabled Mods need to appear inside `<Game>/Mods/` because 3dmigoto-derived Model Importers scan that path for `.ini` files. GMM's Library lives elsewhere (`%AppData%\GMM\library\…` by default, user-overridable). We need a mechanism to project enabled Mods from Library into `<Game>/Mods/` without copying gigabytes per toggle.

The Model Importer only reads from `Mods/`; it never writes back. This rules out the harder case (write-back reconciliation) that motivates true VFS solutions like ModOrganizer 2's USVFS.

## Considered alternatives

- **NTFS symbolic links.** Native Windows feature. Functionally equivalent to junctions for our purposes but require either administrative privileges or Windows 10 Developer Mode to create. We are not willing to demand either from end users.
- **File copies.** Move enabled Mod contents into `<Game>/Mods/` and remove on disable. Safe, dumb, doubles disk usage for every enabled Mod, slow for large mods. Unacceptable for users with 100+ enabled mods.
- **Full USVFS-style hooked-syscall VFS** (MO2 approach). Hooks `Nt*` calls in the game process to fabricate a virtual directory tree. Solves write-back. Massive engineering investment for a problem we do not have. Anti-cheat-adjacent.
- **In-place mods (no Library).** Skip the overlay; mods live in `<Game>/Mods/` directly, enable/disable via `DISABLED` filename prefix. Loses the Library abstraction (portability, backup, multi-game inventory, GameBanana ingest pipeline).

## Decision

GMM creates an NTFS directory junction for each enabled Mod, pointing from `<Game>/Mods/<sanitized-mod-name>/` to the Mod's effective path in the Library (which may be the Mod root or a Variant subfolder). Disable removes the junction. The Library copy is the source of truth and is never relocated by enable/disable.

We use the Rust `std::os::windows::fs::symlink_dir` API with the directory-junction flavour where possible (via `junction` crate or a thin FFI to `CreateSymbolicLinkW` with `SYMBOLIC_LINK_FLAG_DIRECTORY` plus a junction fallback path).

## Consequences

- No admin / Dev Mode requirement at runtime.
- Library is portable: moving the Library directory and recreating junctions reproduces the deployed state.
- Mods on a different volume than the game still work (junctions span volumes; symlinks also do, but junctions don't need elevated rights).
- A user running on a non-NTFS volume (e.g. exFAT external drive) for either the Library or the game install cannot use GMM as-is. We accept this; we will detect at first-run and refuse with a clear error.
- We must own a sanitisation rule for Mod display names → junction directory names (NTFS-reserved chars stripped, deduped, max length checked).
