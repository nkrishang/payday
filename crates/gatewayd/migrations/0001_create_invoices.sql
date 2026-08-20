CREATE TABLE invoices (
    id UUID PRIMARY KEY,

    idempotency_key TEXT NOT NULL,

    chain_id BIGINT NOT NULL,
    factory_address BYTEA NOT NULL,
    token_address BYTEA NOT NULL,
    token_decimals SMALLINT NOT NULL,
    beneficiary_address BYTEA NOT NULL,

    amount TEXT NOT NULL,
    salt BYTEA NOT NULL,
    payment_address BYTEA NOT NULL,

    status TEXT NOT NULL,

    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT invoices_idempotency_key_length
        CHECK (octet_length(idempotency_key) BETWEEN 1 AND 255),

    CONSTRAINT invoices_idempotency_key_unique
        UNIQUE (idempotency_key),

    CONSTRAINT invoices_factory_address_length
        CHECK (octet_length(factory_address) = 20),

    CONSTRAINT invoices_token_address_length
        CHECK (octet_length(token_address) = 20),

    CONSTRAINT invoices_token_decimals_range
        CHECK (token_decimals BETWEEN 0 AND 255),

    CONSTRAINT invoices_beneficiary_address_length
        CHECK (octet_length(beneficiary_address) = 20),

    CONSTRAINT invoices_amount_positive
        CHECK (amount ~ '^[0-9]+$' AND amount != '0'),

    CONSTRAINT invoices_salt_length
        CHECK (octet_length(salt) = 32),

    CONSTRAINT invoices_payment_address_length
        CHECK (octet_length(payment_address) = 20),

    CONSTRAINT invoices_status_valid
        CHECK (status IN ('created', 'funded', 'deploying', 'fulfilled', 'failed'))
);
