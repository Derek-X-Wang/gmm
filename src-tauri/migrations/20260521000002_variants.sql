-- Variants: zero-or-more mutually exclusive presets per Mod
-- (CONTEXT.md § Variant, issue #14 slice 5).
--
-- A Mod with zero rows in mod_variants behaves exactly as before; the
-- junction targets library_path. With 2+ rows, the junction targets
-- library_path/<variant.subpath> where variant matches active_variant_id.

CREATE TABLE IF NOT EXISTS mod_variants (
    id       TEXT PRIMARY KEY,
    mod_id   TEXT NOT NULL REFERENCES mods(id) ON DELETE CASCADE,
    name     TEXT NOT NULL,
    subpath  TEXT NOT NULL,
    UNIQUE (mod_id, name)
);

CREATE INDEX IF NOT EXISTS idx_mod_variants_mod_id ON mod_variants(mod_id);

ALTER TABLE mods ADD COLUMN active_variant_id TEXT REFERENCES mod_variants(id);
