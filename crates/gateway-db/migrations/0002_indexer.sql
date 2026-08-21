-- Indexer cursor: last processed block height, one row per chain.
-- Height-only for this milestone (no block hash / reorg detection yet).
CREATE TABLE indexer_cursor (
    chain_id   BIGINT PRIMARY KEY,
    last_block BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT indexer_cursor_last_block_non_negative
        CHECK (last_block >= 0)
);

-- Funding anchor columns on invoices. Internal indexer bookkeeping, not yet
-- exposed through the API. Populated when an invoice transitions to 'funded'.
--   funded_at_block: block height at which the balance was observed to be sufficient.
--   observed_amount: canonical base-unit balance seen at funding time (decimal string).
ALTER TABLE invoices
    ADD COLUMN funded_at_block BIGINT,
    ADD COLUMN observed_amount TEXT;

ALTER TABLE invoices
    ADD CONSTRAINT invoices_observed_amount_numeric
        CHECK (observed_amount IS NULL OR observed_amount ~ '^[0-9]+$');

ALTER TABLE invoices
    ADD CONSTRAINT invoices_funded_at_block_non_negative
        CHECK (funded_at_block IS NULL OR funded_at_block >= 0);
