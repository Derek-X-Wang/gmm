-- User-confirmed release of an enable/disable transition GMM cannot finish.
--
-- A released row is no longer a recovery witness. It gives up the recorded
-- Junction and Library identity and stops blocking launch and Library
-- mutations, but it stays on the Mod as the durable statement that GMM does
-- not know whether that Mod's Junction matches its stored `enabled` flag.
-- The row is removed only when the user explicitly enables or disables the
-- Mod again and that work succeeds.

ALTER TABLE enabled_transitions ADD COLUMN released_at TEXT;
