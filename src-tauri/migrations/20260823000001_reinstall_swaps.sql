-- Durable witness for an in-progress GameBanana Mod reinstall (#166).
--
-- A Windows directory replacement needs two renames: old -> quarantine,
-- then staged -> live. `reinstall_swaps` bridges that filesystem gap with
-- SQLite's atomic commit. While a row exists, startup deterministically rolls
-- the reinstall back to the old tree. The successful metadata/Variant update
-- deletes the row in the same transaction, after which startup keeps the live
-- replacement and treats the old tree as an ordinary delete quarantine.
--
-- This is recovery state only. It is never a second source of truth for a
-- Mod, and only reinstall plus startup recovery may read it.

CREATE TABLE IF NOT EXISTS reinstall_swaps (
    token            TEXT PRIMARY KEY,
    mod_id           TEXT NOT NULL UNIQUE REFERENCES mods(id) ON DELETE CASCADE,
    game_code        TEXT NOT NULL REFERENCES games(code),
    library_path     TEXT NOT NULL,
    staged_path      TEXT NOT NULL,
    quarantine_path  TEXT NOT NULL,
    old_identity     TEXT NOT NULL,
    staged_identity  TEXT NOT NULL,
    created_at       TEXT NOT NULL
);
