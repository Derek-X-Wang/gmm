-- GameBanana ingest (issue #21, slice 11) records provenance + the
-- bits the UI needs to render a "View on GameBanana" link and a
-- version badge. All five columns are NULL for mods whose `source`
-- is `manual` or `local`.

ALTER TABLE mods ADD COLUMN gamebanana_id INTEGER;
ALTER TABLE mods ADD COLUMN source_url    TEXT;
ALTER TABLE mods ADD COLUMN author        TEXT;
ALTER TABLE mods ADD COLUMN version       TEXT;
ALTER TABLE mods ADD COLUMN screenshot_url TEXT;
