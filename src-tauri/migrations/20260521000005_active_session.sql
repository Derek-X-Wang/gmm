-- Slice 4b (#12): persist the currently-active GameSession so a GMM crash
-- mid-session can be detected on next startup. Singleton row enforced by
-- the id-must-be-1 CHECK constraint.

CREATE TABLE IF NOT EXISTS active_session (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    game_code  TEXT NOT NULL REFERENCES games(code),
    pid        INTEGER NOT NULL,
    started_at TEXT NOT NULL
);
