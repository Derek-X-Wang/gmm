-- Saved complete Mod setups belong to one Game and contain identities only.
CREATE TABLE presets (
    id TEXT PRIMARY KEY,
    game_code TEXT NOT NULL REFERENCES games(code),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    created_at TEXT NOT NULL,
    UNIQUE (game_code, name)
);

-- Keep snapshot identities after Library removal, so apply can report missing
-- Mods or Variants instead of silently erasing them from a saved setup.
CREATE TABLE preset_entries (
    preset_id TEXT NOT NULL REFERENCES presets(id) ON DELETE CASCADE,
    mod_id TEXT NOT NULL,
    active_variant_id TEXT,
    PRIMARY KEY (preset_id, mod_id)
);
