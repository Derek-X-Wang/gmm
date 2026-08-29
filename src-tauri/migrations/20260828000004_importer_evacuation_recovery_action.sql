-- Distinguish recoveries that can change on retry from identity loss that
-- requires the user to acknowledge and release the importer-operation block.

ALTER TABLE importer_evacuations
ADD COLUMN recovery_action TEXT
CHECK (recovery_action IS NULL OR recovery_action IN ('retry', 'release'));

-- The preceding migration was introduced by the same unshipped feature, but
-- its corpus fixture may carry a populated failure. Preserve that state as a
-- retryable failure rather than making the validated row unreadable.
UPDATE importer_evacuations
SET recovery_action = 'retry'
WHERE recovery_error IS NOT NULL;
