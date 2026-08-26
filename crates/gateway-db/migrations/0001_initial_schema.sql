CREATE TABLE accounts (
    id UUID PRIMARY KEY,
    api_key_hash BYTEA NOT NULL UNIQUE,
    api_key_hint TEXT NOT NULL,
    api_key_generation BIGINT NOT NULL DEFAULT 1,
    key_created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    key_rotated_at TIMESTAMPTZ,
    disabled_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT accounts_api_key_hash_length CHECK (octet_length(api_key_hash) = 32),
    CONSTRAINT accounts_api_key_generation_positive CHECK (api_key_generation > 0),
    CONSTRAINT accounts_api_key_hint_length CHECK (octet_length(api_key_hint) BETWEEN 1 AND 32)
);

CREATE TABLE account_identities (
    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,
    account_id UUID NOT NULL REFERENCES accounts(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (issuer, subject),
    CONSTRAINT account_identities_issuer_length CHECK (octet_length(issuer) BETWEEN 1 AND 2048),
    CONSTRAINT account_identities_subject_length CHECK (octet_length(subject) BETWEEN 1 AND 255)
);

CREATE TABLE account_key_authentication_events (
    account_id UUID NOT NULL REFERENCES accounts(id),
    event_id TEXT NOT NULL,
    used_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (account_id, event_id),
    CONSTRAINT account_key_authentication_events_event_id_length
        CHECK (octet_length(event_id) BETWEEN 1 AND 255)
);

CREATE TABLE invoices (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id),
    idempotency_key TEXT NOT NULL,

    chain_id BIGINT NOT NULL,
    factory_address BYTEA NOT NULL,
    token_address BYTEA NOT NULL,
    token_decimals SMALLINT NOT NULL,
    beneficiary_address BYTEA NOT NULL,
    expiration_timestamp BIGINT NOT NULL,
    recovery_address BYTEA NOT NULL,

    amount TEXT NOT NULL,
    salt BYTEA NOT NULL,
    payment_address BYTEA NOT NULL,
    status TEXT NOT NULL,

    confirmed_received TEXT NOT NULL DEFAULT '0',
    funded_at_block BIGINT,
    funded_at_block_hash BYTEA,
    observed_amount TEXT,

    sweep_submitted_at TIMESTAMPTZ,
    sweep_detected_at_block BIGINT,
    sweep_detected_at_block_hash BYTEA,
    execute_tx_hash BYTEA,
    fulfilled_at_block BIGINT,
    sweep_attempts INTEGER NOT NULL DEFAULT 0,
    last_attempt_at TIMESTAMPTZ,
    blocked_reason TEXT,

    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT invoices_idempotency_key_length
        CHECK (octet_length(idempotency_key) BETWEEN 1 AND 255),
    CONSTRAINT invoices_account_id_idempotency_key_unique
        UNIQUE (account_id, idempotency_key),
    CONSTRAINT invoices_factory_address_length
        CHECK (octet_length(factory_address) = 20),
    CONSTRAINT invoices_token_address_length
        CHECK (octet_length(token_address) = 20),
    CONSTRAINT invoices_token_decimals_range
        CHECK (token_decimals BETWEEN 0 AND 255),
    CONSTRAINT invoices_beneficiary_address_length
        CHECK (octet_length(beneficiary_address) = 20),
    CONSTRAINT invoices_expiration_timestamp_non_negative
        CHECK (expiration_timestamp >= 0),
    CONSTRAINT invoices_recovery_address_length
        CHECK (octet_length(recovery_address) = 20),
    CONSTRAINT invoices_amount_positive
        CHECK (amount ~ '^[0-9]+$' AND amount != '0'),
    CONSTRAINT invoices_salt_length
        CHECK (octet_length(salt) = 32),
    CONSTRAINT invoices_payment_address_length
        CHECK (octet_length(payment_address) = 20),
    CONSTRAINT invoices_status_valid
        CHECK (status IN ('created', 'funded', 'deploying', 'fulfilled', 'failed', 'blocked')),
    CONSTRAINT invoices_confirmed_received_numeric
        CHECK (confirmed_received ~ '^[0-9]+$'),
    CONSTRAINT invoices_funded_at_block_non_negative
        CHECK (funded_at_block IS NULL OR funded_at_block >= 0),
    CONSTRAINT invoices_funded_at_block_hash_length
        CHECK (funded_at_block_hash IS NULL OR octet_length(funded_at_block_hash) = 32),
    CONSTRAINT invoices_observed_amount_numeric
        CHECK (observed_amount IS NULL OR observed_amount ~ '^[0-9]+$'),
    CONSTRAINT invoices_sweep_detected_at_block_non_negative
        CHECK (sweep_detected_at_block IS NULL OR sweep_detected_at_block >= 0),
    CONSTRAINT invoices_sweep_detected_at_block_hash_length
        CHECK (sweep_detected_at_block_hash IS NULL OR octet_length(sweep_detected_at_block_hash) = 32),
    CONSTRAINT invoices_sweep_detection_complete
        CHECK ((sweep_detected_at_block IS NULL) = (sweep_detected_at_block_hash IS NULL)),
    CONSTRAINT invoices_execute_tx_hash_length
        CHECK (execute_tx_hash IS NULL OR octet_length(execute_tx_hash) = 32),
    CONSTRAINT invoices_fulfilled_at_block_non_negative
        CHECK (fulfilled_at_block IS NULL OR fulfilled_at_block >= 0),
    CONSTRAINT invoices_sweep_attempts_non_negative
        CHECK (sweep_attempts >= 0)
);

CREATE UNIQUE INDEX invoices_chain_token_payment_address_unique
    ON invoices (chain_id, token_address, payment_address);

CREATE INDEX invoices_active_payment_address
    ON invoices (chain_id, token_address, payment_address)
    WHERE status = 'created';

CREATE INDEX invoices_account_id_id ON invoices (account_id, id);

CREATE TABLE indexer_cursor (
    chain_id BIGINT PRIMARY KEY,
    token_address BYTEA NOT NULL,
    last_block BIGINT NOT NULL,
    last_block_hash BYTEA NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT indexer_cursor_token_address_length
        CHECK (octet_length(token_address) = 20),
    CONSTRAINT indexer_cursor_last_block_non_negative
        CHECK (last_block >= 0),
    CONSTRAINT indexer_cursor_last_block_hash_length
        CHECK (octet_length(last_block_hash) = 32)
);

CREATE TABLE payment_observations (
    chain_id BIGINT NOT NULL,
    token_address BYTEA NOT NULL,
    block_number BIGINT NOT NULL,
    block_hash BYTEA NOT NULL,
    transaction_hash BYTEA NOT NULL,
    transaction_index BIGINT NOT NULL,
    log_index BIGINT NOT NULL,
    sender_address BYTEA NOT NULL,
    recipient_address BYTEA NOT NULL,
    invoice_id UUID NOT NULL REFERENCES invoices(id),
    amount TEXT NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),

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
