-- Bind interrupted deployment recovery to the exact Library objects selected
-- when the witness committed. Nullable only so an already-written transition
-- from the immediately preceding schema migrates without inventing identity;
-- the validated loader treats NULL as corrupt durable state and refuses it.
ALTER TABLE enabled_transitions ADD COLUMN junction_target_identity TEXT;
ALTER TABLE enabled_transitions ADD COLUMN library_identity TEXT;
