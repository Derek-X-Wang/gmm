-- Durable witness for evacuating an existing Model Importer (#227).
--
-- The row commits before the first importer entry leaves the game directory.
-- Presence means rollback: startup restores every completely evacuated entry
-- and keeps the row when filesystem uncertainty prevents a safe decision.

CREATE TABLE IF NOT EXISTS importer_evacuations (
    token                  TEXT PRIMARY KEY,
    game_code              TEXT NOT NULL UNIQUE REFERENCES games(code),
    game_path              TEXT NOT NULL,
    game_identity          TEXT NOT NULL,
    backup_path            TEXT NOT NULL UNIQUE,
    backup_identity        TEXT NOT NULL,
    backup_root_identity   TEXT NOT NULL,
    entries_json           TEXT NOT NULL,
    owner_pid              INTEGER NOT NULL,
    owner_started_at       INTEGER,
    owner_active           INTEGER NOT NULL CHECK (owner_active IN (0, 1)),
    created_at             TEXT NOT NULL,
    recovery_error         TEXT,
    recovery_attempted_at  TEXT,
    recovery_attempts      INTEGER NOT NULL DEFAULT 0
);
