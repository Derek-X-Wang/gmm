-- Key/value settings. Used initially for Library path overrides
-- (slice 15 in CONTEXT.md / issue #8); future slices add more keys.
--
-- Schema versioning is handled by sqlx migrations; do not edit this
-- file after it has shipped — write a new migration instead.

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT
);
