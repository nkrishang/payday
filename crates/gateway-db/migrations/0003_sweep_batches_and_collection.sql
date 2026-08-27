-- Invoice lifecycle gains `expired` (chain time passed the deadline while the
-- invoice was open) and `recovered` (the balance was routed to the recovery
-- address). The never-used `failed` state is removed.
ALTER TABLE invoices DROP CONSTRAINT invoices_status_valid;
ALTER TABLE invoices ADD CONSTRAINT invoices_status_valid
    CHECK (status IN ('created', 'funded', 'deploying', 'fulfilled', 'recovered', 'expired', 'blocked'));

-- One row per helper transaction nonce. Replacements for the same nonce append
-- to `tx_hashes`; the partial unique index enforces the signer's single
-- in-flight batch, which is what keeps the nonce stream simple.
CREATE TABLE sweep_batches (
    id UUID PRIMARY KEY,
    chain_id BIGINT NOT NULL,
    nonce BIGINT NOT NULL,
    gas_limit BIGINT NOT NULL,
    max_fee_per_gas TEXT NOT NULL,
    max_priority_fee_per_gas TEXT NOT NULL,
    tx_hashes BYTEA[] NOT NULL,
    raw_transactions BYTEA[] NOT NULL,
    submitted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    broadcast_at TIMESTAMPTZ,
    mined_tx_hash BYTEA,
    mined_block BIGINT,
    mined_block_hash BYTEA,
    mined_transaction_index BIGINT,
    resolution TEXT,
    resolved_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT sweep_batches_nonce_non_negative CHECK (nonce >= 0),
    CONSTRAINT sweep_batches_gas_limit_positive CHECK (gas_limit > 0),
    CONSTRAINT sweep_batches_fees_numeric
        CHECK (max_fee_per_gas ~ '^[0-9]+$' AND max_priority_fee_per_gas ~ '^[0-9]+$'),
    CONSTRAINT sweep_batches_tx_hashes_present CHECK (cardinality(tx_hashes) >= 1),
    CONSTRAINT sweep_batches_transactions_complete
        CHECK (cardinality(raw_transactions) = cardinality(tx_hashes)),
    CONSTRAINT sweep_batches_mined_complete
        CHECK ((mined_tx_hash IS NULL) = (mined_block IS NULL)
            AND (mined_block IS NULL) = (mined_block_hash IS NULL)
            AND (mined_block_hash IS NULL) = (mined_transaction_index IS NULL)),
    CONSTRAINT sweep_batches_mined_tx_hash_length
        CHECK (mined_tx_hash IS NULL OR octet_length(mined_tx_hash) = 32),
    CONSTRAINT sweep_batches_mined_block_non_negative
        CHECK (mined_block IS NULL OR mined_block >= 0),
    CONSTRAINT sweep_batches_mined_block_hash_length
        CHECK (mined_block_hash IS NULL OR octet_length(mined_block_hash) = 32),
    CONSTRAINT sweep_batches_mined_transaction_index_non_negative
        CHECK (mined_transaction_index IS NULL OR mined_transaction_index >= 0),
    CONSTRAINT sweep_batches_resolution_complete CHECK ((resolution IS NULL) = (resolved_at IS NULL)),
    CONSTRAINT sweep_batches_resolution_valid
        CHECK (resolution IS NULL OR resolution IN ('finalized', 'reverted', 'abandoned'))
);

CREATE UNIQUE INDEX sweep_batches_open ON sweep_batches (chain_id) WHERE resolved_at IS NULL;

-- In-flight sweep state moves to the batch; the invoice keeps the block at
-- which its funds were last fully drained and how many nonzero observations
-- are still sitting at the address.
ALTER TABLE invoices
    DROP COLUMN sweep_submitted_at,
    DROP COLUMN sweep_detected_at_block,
    DROP COLUMN sweep_detected_at_block_hash,
    ADD COLUMN sweep_batch_id UUID REFERENCES sweep_batches(id),
    ADD COLUMN drained_at_block BIGINT,
    ADD COLUMN drained_at_transaction_index BIGINT,
    ADD COLUMN uncollected_count INTEGER NOT NULL DEFAULT 0,
    ADD CONSTRAINT invoices_drained_at_block_non_negative
        CHECK (drained_at_block IS NULL OR drained_at_block >= 0),
    ADD CONSTRAINT invoices_drained_at_transaction_index_non_negative
        CHECK (drained_at_transaction_index IS NULL OR drained_at_transaction_index >= 0),
    ADD CONSTRAINT invoices_drain_position_complete
        CHECK ((drained_at_block IS NULL) = (drained_at_transaction_index IS NULL)),
    ADD CONSTRAINT invoices_uncollected_count_non_negative CHECK (uncollected_count >= 0);

ALTER TABLE invoices RENAME COLUMN fulfilled_at_block TO resolved_at_block;
ALTER TABLE invoices RENAME CONSTRAINT invoices_fulfilled_at_block_non_negative TO invoices_resolved_at_block_non_negative;

-- The sweep queue and the expiry watch are the two scans the worker runs on
-- every pass; both must stay proportional to open work, not invoice history.
DROP INDEX invoices_active_payment_address;
CREATE INDEX invoices_sweep_queue ON invoices (chain_id, id) WHERE uncollected_count > 0;
CREATE INDEX invoices_expiry_watch ON invoices (chain_id, expiration_timestamp)
    WHERE status IN ('created', 'funded');
CREATE INDEX invoices_in_flight ON invoices (sweep_batch_id) WHERE sweep_batch_id IS NOT NULL;

-- Every nonzero observation is either still at the address or was drained by
-- a sweep at `collected_at_block`. `late` replaces `blocked`: such transfers
-- are no longer parked for manual review but recovered automatically.
ALTER TABLE payment_observations
    DROP CONSTRAINT payment_observations_disposition_valid,
    ADD COLUMN collected_at_block BIGINT,
    ADD COLUMN collected_at_transaction_index BIGINT,
    ADD COLUMN block_timestamp BIGINT NOT NULL,
    ADD CONSTRAINT payment_observations_collected_at_block_non_negative
        CHECK (collected_at_block IS NULL OR collected_at_block >= 0),
    ADD CONSTRAINT payment_observations_collected_at_transaction_index_non_negative
        CHECK (collected_at_transaction_index IS NULL OR collected_at_transaction_index >= 0),
    ADD CONSTRAINT payment_observations_collection_position_complete
        CHECK ((collected_at_block IS NULL) = (collected_at_transaction_index IS NULL)),
    ADD CONSTRAINT payment_observations_block_timestamp_non_negative
        CHECK (block_timestamp >= 0);
UPDATE payment_observations SET disposition = 'late' WHERE disposition = 'blocked';
ALTER TABLE payment_observations
    ADD CONSTRAINT payment_observations_disposition_valid
        CHECK (disposition IN ('credited', 'late', 'error'));

DROP INDEX payment_observations_manual_review;
CREATE INDEX payment_observations_uncollected ON payment_observations (invoice_id, block_number)
    WHERE collected_at_block IS NULL AND disposition <> 'error';
CREATE INDEX payment_observations_errors ON payment_observations (observed_at) WHERE disposition = 'error';
