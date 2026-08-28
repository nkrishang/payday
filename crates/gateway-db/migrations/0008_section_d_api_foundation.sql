-- Public payment fields remain attached to invoices so the existing on-chain
-- worker can continue to treat invoices as its source of truth.
ALTER TABLE invoices
    ADD COLUMN reference TEXT,
    ADD COLUMN metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN paid_at TIMESTAMPTZ,
    ADD COLUMN expired_at TIMESTAMPTZ,
    ADD COLUMN settlement_tx_hash BYTEA,
    ADD COLUMN fee_amount TEXT NOT NULL DEFAULT '0',
    ADD COLUMN net_amount TEXT;

UPDATE invoices SET net_amount = amount;
CREATE FUNCTION payment_metadata_values_fit(value JSONB) RETURNS BOOLEAN
LANGUAGE SQL IMMUTABLE PARALLEL SAFE
AS $$ SELECT count(*) <= 16
          AND COALESCE(bool_and(octet_length(entry.value::text) <= 512), true)
      FROM jsonb_each(value) entry $$;
ALTER TABLE invoices
    ALTER COLUMN net_amount SET NOT NULL,
    ADD CONSTRAINT invoices_reference_length
        CHECK (reference IS NULL OR char_length(reference) <= 128),
    ADD CONSTRAINT invoices_metadata_object
        CHECK (jsonb_typeof(metadata) = 'object'),
    ADD CONSTRAINT invoices_metadata_limits
        CHECK (payment_metadata_values_fit(metadata)),
    ADD CONSTRAINT invoices_settlement_tx_hash_length
        CHECK (settlement_tx_hash IS NULL OR octet_length(settlement_tx_hash) = 32),
    ADD CONSTRAINT invoices_fee_amount_zero
        CHECK (fee_amount = '0'),
    ADD CONSTRAINT invoices_net_amount_matches_amount
        CHECK (net_amount = amount);

CREATE INDEX invoices_account_status_created_page
    ON invoices (account_id, status, created_at DESC, id DESC);
CREATE INDEX invoices_account_reference_created_page
    ON invoices (account_id, reference, created_at DESC, id DESC) WHERE reference IS NOT NULL;

-- The committed range timestamp makes the cursor a complete finalized-head
-- projection for API `as_of` and freshness reporting.
ALTER TABLE indexer_cursor
    ADD COLUMN last_block_timestamp BIGINT,
    ADD CONSTRAINT indexer_cursor_last_block_timestamp_non_negative
        CHECK (last_block_timestamp IS NULL OR last_block_timestamp >= 0);

-- Existing rows predate this projection. New cursor writes always populate it.

-- Finality is an independently observed boundary, not an alias for the last
-- successfully indexed range.
CREATE TABLE indexer_status (
    chain_id BIGINT NOT NULL,
    token_address BYTEA NOT NULL,
    finalized_block BIGINT NOT NULL CHECK (finalized_block >= 0),
    finalized_block_hash BYTEA NOT NULL CHECK (octet_length(finalized_block_hash) = 32),
    finalized_block_timestamp BIGINT NOT NULL CHECK (finalized_block_timestamp >= 0),
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (chain_id, token_address),
    CHECK (octet_length(token_address) = 20)
);

-- The sweeper reports its own health. Queue depth alone cannot distinguish an
-- idle healthy worker from one that is paused or failing.
CREATE TABLE sweeper_status (
    chain_id BIGINT PRIMARY KEY,
    state TEXT NOT NULL CHECK (state IN ('running', 'paused', 'degraded')),
    heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
