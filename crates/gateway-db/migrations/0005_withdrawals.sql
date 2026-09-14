-- Merchant withdrawals: the whole USDC balance of the account's Payday wallet
-- on every chain, moved to one destination the merchant names. One row per
-- withdrawal, one leg per source chain. The merchant authorizes each leg
-- with an EIP-3009 signature (crates/gateway-core/src/withdrawal_authorization.rs)
-- and gateway-indexer relays it: a same-chain `transferWithAuthorization`, or
-- `WithdrawalForwarder.bridge` (CCTP burn) followed, once Circle attests, by
-- `MessageTransmitterV2.receiveMessage` on the destination chain.

CREATE TABLE withdrawals (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id),
    idempotency_key TEXT NOT NULL,
    -- The Payday wallet the legs are signed from, as it was at creation.
    wallet_address TEXT NOT NULL,
    destination_chain_id BIGINT NOT NULL,
    destination_address TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Exactly one of these is set once every leg is terminal, or the
    -- merchant cancelled before anything was relayed.
    completed_at TIMESTAMPTZ,
    cancelled_at TIMESTAMPTZ,
    failed_at TIMESTAMPTZ,

    CONSTRAINT withdrawals_idempotency UNIQUE (account_id, idempotency_key),
    CONSTRAINT withdrawals_wallet_address_shape CHECK (wallet_address ~ '^0x[0-9a-fA-F]{40}$'),
    CONSTRAINT withdrawals_destination_address_shape CHECK (destination_address ~ '^0x[0-9a-fA-F]{40}$'),
    CONSTRAINT withdrawals_one_ending CHECK (num_nonnulls(completed_at, cancelled_at, failed_at) <= 1)
);

-- One withdrawal in flight per account: the legs snapshot the balances, and
-- two overlapping snapshots would authorize the same funds twice.
CREATE UNIQUE INDEX withdrawals_open ON withdrawals (account_id)
    WHERE completed_at IS NULL AND cancelled_at IS NULL AND failed_at IS NULL;
CREATE INDEX withdrawals_by_account ON withdrawals (account_id, created_at DESC, id DESC);

CREATE TABLE withdrawal_legs (
    id UUID PRIMARY KEY,
    withdrawal_id UUID NOT NULL REFERENCES withdrawals(id),
    -- Display order: the chain registry's order.
    position SMALLINT NOT NULL,
    kind TEXT NOT NULL,
    source_chain_id BIGINT NOT NULL,
    destination_chain_id BIGINT NOT NULL,
    -- Base units, decimal.
    amount TEXT NOT NULL,
    state TEXT NOT NULL,

    -- The authorization the merchant signs, complete enough to rebuild the
    -- typed data and verify the signature without a chain read.
    token_address TEXT NOT NULL,
    domain_name TEXT NOT NULL,
    domain_version TEXT NOT NULL,
    authorization_kind TEXT NOT NULL,
    -- The payee: the destination (transfer) or the forwarder (bridge).
    authorization_to TEXT NOT NULL,
    nonce BYTEA NOT NULL,
    -- Bridge legs only: the salt behind the nonce's destination commitment.
    salt BYTEA,
    -- Unix seconds; the token refuses the authorization from then on.
    valid_before BIGINT NOT NULL,
    signature BYTEA,
    authorized_at TIMESTAMPTZ,

    -- The relayer's one in-flight transaction for this leg, in
    -- sweep_batches' shape: one signer nonce, every same-nonce replacement
    -- appended, the receipt once seen. Cleared when the step resolves.
    step_chain_id BIGINT,
    step_nonce BIGINT,
    step_gas_limit BIGINT,
    step_max_fee_per_gas TEXT,
    step_max_priority_fee_per_gas TEXT,
    step_tx_hashes BYTEA[],
    step_raw_transactions BYTEA[],
    step_submitted_at TIMESTAMPTZ,
    step_broadcast_at TIMESTAMPTZ,
    step_mined_tx_hash BYTEA,
    step_mined_block BIGINT,
    step_mined_block_hash BYTEA,

    -- Outcomes.
    transfer_tx_hash BYTEA,
    burn_tx_hash BYTEA,
    mint_tx_hash BYTEA,
    attestation_message BYTEA,
    attestation BYTEA,
    attestation_next_check_at TIMESTAMPTZ,
    failure_reason TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT withdrawal_legs_position UNIQUE (withdrawal_id, position),
    CONSTRAINT withdrawal_legs_kind CHECK (kind IN ('transfer', 'bridge')),
    CONSTRAINT withdrawal_legs_kind_matches_chains
        CHECK ((kind = 'transfer') = (source_chain_id = destination_chain_id)),
    CONSTRAINT withdrawal_legs_amount_positive CHECK (amount ~ '^[0-9]+$' AND amount != '0'),
    CONSTRAINT withdrawal_legs_state CHECK (state IN (
        'awaiting_signature', 'authorized', 'relaying', 'burned', 'attested', 'minting',
        'completed', 'failed', 'expired', 'cancelled')),
    CONSTRAINT withdrawal_legs_authorization_kind
        CHECK (authorization_kind IN ('transfer', 'receive')
            AND (authorization_kind = 'transfer') = (kind = 'transfer')),
    CONSTRAINT withdrawal_legs_addresses_shape
        CHECK (token_address ~ '^0x[0-9a-fA-F]{40}$' AND authorization_to ~ '^0x[0-9a-fA-F]{40}$'),
    CONSTRAINT withdrawal_legs_nonce_length CHECK (octet_length(nonce) = 32),
    CONSTRAINT withdrawal_legs_salt CHECK ((salt IS NULL) = (kind = 'transfer')
        AND (salt IS NULL OR octet_length(salt) = 32)),
    CONSTRAINT withdrawal_legs_signature CHECK ((signature IS NULL) = (authorized_at IS NULL)
        AND (signature IS NULL OR octet_length(signature) = 65)),
    CONSTRAINT withdrawal_legs_signed_before_relay
        CHECK (state IN ('awaiting_signature', 'expired', 'cancelled') OR signature IS NOT NULL),
    CONSTRAINT withdrawal_legs_step_complete CHECK (
        (step_chain_id IS NULL) = (step_nonce IS NULL)
        AND (step_nonce IS NULL) = (step_gas_limit IS NULL)
        AND (step_gas_limit IS NULL) = (step_max_fee_per_gas IS NULL)
        AND (step_max_fee_per_gas IS NULL) = (step_max_priority_fee_per_gas IS NULL)
        AND (step_max_priority_fee_per_gas IS NULL) = (step_tx_hashes IS NULL)
        AND (step_tx_hashes IS NULL) = (step_raw_transactions IS NULL)
        AND (step_raw_transactions IS NULL) = (step_submitted_at IS NULL)),
    CONSTRAINT withdrawal_legs_step_only_while_relaying
        CHECK (step_chain_id IS NULL OR state IN ('relaying', 'minting')),
    CONSTRAINT withdrawal_legs_step_fees_numeric CHECK (
        (step_max_fee_per_gas IS NULL OR step_max_fee_per_gas ~ '^[0-9]+$')
        AND (step_max_priority_fee_per_gas IS NULL OR step_max_priority_fee_per_gas ~ '^[0-9]+$')),
    CONSTRAINT withdrawal_legs_step_transactions_complete CHECK (
        step_tx_hashes IS NULL
        OR (cardinality(step_tx_hashes) >= 1 AND cardinality(step_raw_transactions) = cardinality(step_tx_hashes))),
    CONSTRAINT withdrawal_legs_step_mined_complete CHECK (
        (step_mined_tx_hash IS NULL) = (step_mined_block IS NULL)
        AND (step_mined_block IS NULL) = (step_mined_block_hash IS NULL)
        AND (step_mined_tx_hash IS NULL OR step_chain_id IS NOT NULL)),
    CONSTRAINT withdrawal_legs_hash_lengths CHECK (
        (step_mined_tx_hash IS NULL OR octet_length(step_mined_tx_hash) = 32)
        AND (step_mined_block_hash IS NULL OR octet_length(step_mined_block_hash) = 32)
        AND (transfer_tx_hash IS NULL OR octet_length(transfer_tx_hash) = 32)
        AND (burn_tx_hash IS NULL OR octet_length(burn_tx_hash) = 32)
        AND (mint_tx_hash IS NULL OR octet_length(mint_tx_hash) = 32)),
    CONSTRAINT withdrawal_legs_attestation_complete
        CHECK ((attestation_message IS NULL) = (attestation IS NULL)),
    CONSTRAINT withdrawal_legs_failure_reason
        CHECK ((failure_reason IS NOT NULL) = (state = 'failed'))
);

-- The relayer signs with one key per chain and keeps one transaction in
-- flight on it; sweep_batches_open enforces the same for sweeps, and the
-- indexer checks both before it signs anything.
CREATE UNIQUE INDEX withdrawal_steps_open ON withdrawal_legs (step_chain_id) WHERE step_chain_id IS NOT NULL;
CREATE INDEX withdrawal_legs_by_withdrawal ON withdrawal_legs (withdrawal_id, position);
CREATE INDEX withdrawal_legs_relayable ON withdrawal_legs (source_chain_id, created_at) WHERE state = 'authorized';
CREATE INDEX withdrawal_legs_mintable ON withdrawal_legs (destination_chain_id, created_at) WHERE state = 'attested';
CREATE INDEX withdrawal_legs_attesting ON withdrawal_legs (source_chain_id, attestation_next_check_at) WHERE state = 'burned';
CREATE INDEX withdrawal_legs_expiring ON withdrawal_legs (valid_before) WHERE state IN ('awaiting_signature', 'authorized');
