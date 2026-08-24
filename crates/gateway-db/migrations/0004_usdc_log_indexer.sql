-- Funding is now derived from finalized USDC Transfer logs rather than native
-- balance reconciliation. The old height-only cursor is not reusable because it
-- did not identify the token or canonical block hash.
DROP TABLE indexer_cursor;

CREATE TABLE indexer_cursor (
    chain_id        BIGINT PRIMARY KEY,
    token_address   BYTEA NOT NULL,
    last_block      BIGINT NOT NULL,
    last_block_hash BYTEA NOT NULL,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT indexer_cursor_token_address_length
        CHECK (octet_length(token_address) = 20),
    CONSTRAINT indexer_cursor_last_block_non_negative
        CHECK (last_block >= 0),
    CONSTRAINT indexer_cursor_last_block_hash_length
        CHECK (octet_length(last_block_hash) = 32)
);

ALTER TABLE invoices
    ADD COLUMN confirmed_received TEXT NOT NULL DEFAULT '0',
    ADD COLUMN funded_at_block_hash BYTEA,
    ADD COLUMN sweep_submitted_at TIMESTAMPTZ,
    ADD COLUMN sweep_detected_at_block BIGINT,
    ADD COLUMN sweep_detected_at_block_hash BYTEA;

ALTER TABLE invoices
    ADD CONSTRAINT invoices_confirmed_received_numeric
        CHECK (confirmed_received ~ '^[0-9]+$'),
    ADD CONSTRAINT invoices_funded_at_block_hash_length
        CHECK (funded_at_block_hash IS NULL OR octet_length(funded_at_block_hash) = 32),
    ADD CONSTRAINT invoices_sweep_detected_at_block_non_negative
        CHECK (sweep_detected_at_block IS NULL OR sweep_detected_at_block >= 0),
    ADD CONSTRAINT invoices_sweep_detected_at_block_hash_length
        CHECK (sweep_detected_at_block_hash IS NULL OR octet_length(sweep_detected_at_block_hash) = 32),
    ADD CONSTRAINT invoices_sweep_detection_complete
        CHECK ((sweep_detected_at_block IS NULL) = (sweep_detected_at_block_hash IS NULL));

CREATE UNIQUE INDEX invoices_chain_token_payment_address_unique
    ON invoices (chain_id, token_address, payment_address);

CREATE INDEX invoices_active_payment_address
    ON invoices (chain_id, token_address, payment_address)
    WHERE status = 'created';

CREATE TABLE payment_observations (
    chain_id         BIGINT NOT NULL,
    token_address    BYTEA NOT NULL,
    block_number     BIGINT NOT NULL,
    block_hash       BYTEA NOT NULL,
    transaction_hash BYTEA NOT NULL,
    transaction_index BIGINT NOT NULL,
    log_index        BIGINT NOT NULL,
    sender_address   BYTEA NOT NULL,
    recipient_address BYTEA NOT NULL,
    invoice_id       UUID NOT NULL REFERENCES invoices(id),
    amount           TEXT NOT NULL,
    observed_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (chain_id, token_address, transaction_hash, log_index),
    CONSTRAINT payment_observations_token_address_length
        CHECK (octet_length(token_address) = 20),
    CONSTRAINT payment_observations_block_hash_length
        CHECK (octet_length(block_hash) = 32),
    CONSTRAINT payment_observations_transaction_hash_length
        CHECK (octet_length(transaction_hash) = 32),
    CONSTRAINT payment_observations_sender_address_length
        CHECK (octet_length(sender_address) = 20),
    CONSTRAINT payment_observations_recipient_address_length
        CHECK (octet_length(recipient_address) = 20),
    CONSTRAINT payment_observations_amount_positive
        CHECK (amount ~ '^[0-9]+$' AND amount != '0'),
    CONSTRAINT payment_observations_block_number_non_negative
        CHECK (block_number >= 0),
    CONSTRAINT payment_observations_transaction_index_non_negative
        CHECK (transaction_index >= 0),
    CONSTRAINT payment_observations_log_index_non_negative
        CHECK (log_index >= 0)
);

CREATE INDEX payment_observations_invoice_order
    ON payment_observations (invoice_id, block_number, transaction_index, log_index);
