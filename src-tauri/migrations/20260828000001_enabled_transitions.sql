-- Durable witness for a Mod enable/disable transition (#190).
--
-- The row commits before the Junction changes. A successful transition
-- deletes it atomically with the enabled flag update. Any later database,
-- filesystem, or process failure leaves enough state for startup to finish
-- the requested transition without guessing from a torn Junction/flag pair.

CREATE TABLE IF NOT EXISTS enabled_transitions (
    mod_id                 TEXT PRIMARY KEY REFERENCES mods(id) ON DELETE CASCADE,
    game_code              TEXT NOT NULL REFERENCES games(code),
    intended_enabled       INTEGER NOT NULL CHECK (intended_enabled IN (0, 1)),
    junction_path          TEXT NOT NULL UNIQUE,
    junction_target        TEXT NOT NULL,
    junction_parent_identity TEXT NOT NULL,
    junction_identity      TEXT,
    owner_pid              INTEGER NOT NULL,
    owner_started_at       INTEGER,
    owner_active           INTEGER NOT NULL CHECK (owner_active IN (0, 1)),
    created_at             TEXT NOT NULL,
    recovery_error         TEXT,
    recovery_attempted_at  TEXT,
    recovery_attempts      INTEGER NOT NULL DEFAULT 0
);
