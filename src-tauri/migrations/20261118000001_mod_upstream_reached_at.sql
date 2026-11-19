-- Per-Mod update-check cadence (issue #471).
--
-- upstream_reached_at: when GameBanana last answered about this Mod, as
-- RFC 3339. An answer that the submission is gone counts; a transport
-- failure does not. NULL means never answered, so the Mod is due. The
-- weekly check (ADR 0004) is due for a Game when any of its polled Mods
-- is due, and a scheduled pass polls only the Mods that are due.
--
-- Not backfilled from the per-Game record below: that record could not
-- say which Mods a partial check reached, which was the defect. Every
-- Mod is therefore due once after this migration, which costs one
-- metadata call per Mod and applies nothing.
--
-- The per-Game record `mod_updates.last_check_at.<game>` is replaced by
-- this column and deleted, so no stale value can disagree with it.

ALTER TABLE mods ADD COLUMN upstream_reached_at TEXT;

DELETE FROM settings WHERE key LIKE 'mod_updates.last_check_at.%';
