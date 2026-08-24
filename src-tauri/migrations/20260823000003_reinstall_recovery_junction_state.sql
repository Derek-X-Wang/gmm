-- A quarantined reinstall can outlive a failed attempt to withdraw its
-- deployment Junction. Persist what GMM actually established so startup,
-- retry, reconcile, and rebuild do not confuse "quarantined" with "absent
-- from the game".

ALTER TABLE reinstall_swaps
    ADD COLUMN junction_withdrawn INTEGER NOT NULL DEFAULT 0
        CHECK (junction_withdrawn IN (0, 1));
ALTER TABLE reinstall_swaps ADD COLUMN junction_withdrawal_error TEXT;
