-- Durable reservations for launches that have committed to starting a game
-- but have not yet won the singleton active_session row (#192).
--
-- More than one reservation is allowed: simultaneous launches retain the
-- existing spawn-then-claim behavior, so exactly one INSERT into
-- active_session wins and every loser still owns (and kills) its child. Every
-- Library writer treats any reservation as a session blocker. child_pid is
-- filled immediately after spawn so a replacement GMM process can keep an
-- orphaned reservation while that child is still alive, then retire it once
-- both the launcher and child are gone.

CREATE TABLE IF NOT EXISTS session_launch_claims (
    token       TEXT PRIMARY KEY,
    game_code   TEXT NOT NULL REFERENCES games(code),
    owner_pid   INTEGER NOT NULL,
    child_pid   INTEGER,
    started_at  TEXT NOT NULL
);
