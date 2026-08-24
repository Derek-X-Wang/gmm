-- Keep a failed startup release visible and actionable without discarding the
-- durable evidence that explains why its staging witness survived (#187).

ALTER TABLE staged_library_operations ADD COLUMN recovery_error TEXT;
ALTER TABLE staged_library_operations ADD COLUMN recovery_attempted_at TEXT;
ALTER TABLE staged_library_operations
    ADD COLUMN recovery_attempts INTEGER NOT NULL DEFAULT 0;
