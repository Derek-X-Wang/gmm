-- Per-mod update tracking (issue #23, slice 13c).
--
-- upstream_version: the most recent version string observed at the
-- upstream platform during the weekly check. NULL until the first
-- poll. Compared against mods.version (the version we actually
-- installed) to decide whether a badge shows.
--
-- update_check_enabled: per-mod opt-out flag. The user can turn off
-- the weekly check for a specific mod without touching the global
-- toggle in settings.

ALTER TABLE mods ADD COLUMN upstream_version       TEXT;
ALTER TABLE mods ADD COLUMN update_check_enabled   INTEGER NOT NULL DEFAULT 1;
