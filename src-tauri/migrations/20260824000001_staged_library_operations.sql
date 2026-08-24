-- Durable ownership for adopt/import directories while their bytes are being
-- copied outside SQLite's Library writer fence (#187).
--
-- The row is committed before the producer writes its first file. Successful
-- import deletes it atomically with the Mod and Variant rows. A returned error
-- deletes it only after the staged filesystem identity is re-proved; a process
-- death leaves it for startup to release without discarding the staged bytes,
-- after which the ordinary Library audit can offer recovery or deletion.

CREATE TABLE IF NOT EXISTS staged_library_operations (
    id               TEXT PRIMARY KEY,
    game_code        TEXT NOT NULL REFERENCES games(code),
    operation        TEXT NOT NULL CHECK (operation IN ('adopt', 'import_zip')),
    staged_path      TEXT NOT NULL UNIQUE,
    staged_identity  TEXT NOT NULL UNIQUE,
    created_at       TEXT NOT NULL
);
