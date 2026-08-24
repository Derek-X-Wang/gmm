-- A reinstall witness remains the durable owner when verified startup
-- rollback cannot finish (#179). These fields record the failed attempt so
-- the affected Mod can be quarantined in-app without weakening the witness or
-- treating an active reinstall as a failure.

ALTER TABLE reinstall_swaps ADD COLUMN recovery_error TEXT;
ALTER TABLE reinstall_swaps ADD COLUMN recovery_attempted_at TEXT;
ALTER TABLE reinstall_swaps ADD COLUMN recovery_attempts INTEGER NOT NULL DEFAULT 0;
