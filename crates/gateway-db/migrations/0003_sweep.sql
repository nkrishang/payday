-- Sweep (execute) phase: transitions an invoice funded -> deploying -> fulfilled,
-- or -> blocked when the payment cannot be delivered. The backend key calls
-- PaymentFactory.execute, which CREATE3-deploys the Payment contract at the
-- payment address; that deployment sweeps the funds to the beneficiary.

-- Allow the new terminal 'blocked' status. A blocked invoice needs manual
-- intervention: either the beneficiary rejected the native transfer (the deploy
-- reverts permanently), or transient sweep errors exhausted the retry budget.
ALTER TABLE invoices
    DROP CONSTRAINT invoices_status_valid;

ALTER TABLE invoices
    ADD CONSTRAINT invoices_status_valid
        CHECK (status IN ('created', 'funded', 'deploying', 'fulfilled', 'failed', 'blocked'));

-- Sweep bookkeeping columns. All internal to the indexer, NULL/0 until the
-- invoice reaches the sweep phase.
--   execute_tx_hash:    hash of the successful execute tx we sent (32 bytes).
--                       NULL when the payment was found already-executed
--                       on-chain (a third party, or our own recovered tx).
--   fulfilled_at_block: block height at which the sweep was confirmed.
--   sweep_attempts:     count of transient (non-contract) failures so far;
--                       drives exponential backoff and the retry ceiling.
--   last_attempt_at:    timestamp of the most recent sweep attempt; with
--                       sweep_attempts it gates the next eligible retry.
--   blocked_reason:     why an invoice was blocked ('receiver_rejected' or
--                       'max_retries_exceeded'). NULL unless status = 'blocked'.
ALTER TABLE invoices
    ADD COLUMN execute_tx_hash    BYTEA,
    ADD COLUMN fulfilled_at_block BIGINT,
    ADD COLUMN sweep_attempts     INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN last_attempt_at    TIMESTAMPTZ,
    ADD COLUMN blocked_reason     TEXT;

ALTER TABLE invoices
    ADD CONSTRAINT invoices_execute_tx_hash_length
        CHECK (execute_tx_hash IS NULL OR octet_length(execute_tx_hash) = 32);

ALTER TABLE invoices
    ADD CONSTRAINT invoices_fulfilled_at_block_non_negative
        CHECK (fulfilled_at_block IS NULL OR fulfilled_at_block >= 0);

ALTER TABLE invoices
    ADD CONSTRAINT invoices_sweep_attempts_non_negative
        CHECK (sweep_attempts >= 0);
