-- Transaction execution state, owned by gum-signers. gum-server reads
-- `execution.executor_status` and `execution.signer_status` for its
-- operator status endpoint and writes nothing here.
--
-- The model has two levels. A *job* is one command from the bus (a sweep
-- batch or a withdrawal step) and is the unit of
-- idempotency: its id is the command's job id, so a redelivered command
-- finds its job and does nothing. A *transaction* is one (signer, nonce)
-- slot spent on a job; every same-nonce replacement is an *attempt* under
-- it. Signed bytes are persisted before they are broadcast, so a restart
-- can always resend exactly what may already be in a mempool instead of
-- signing something new for a nonce that may already be taken.

CREATE SCHEMA execution;

CREATE TABLE execution.jobs (
    id UUID PRIMARY KEY,
    kind TEXT NOT NULL,
    chain_id BIGINT NOT NULL,
    correlation_id UUID NOT NULL,
    -- The bus message that carried the command; the causation of every
    -- event the job publishes.
    command_message_id UUID,
    -- The command as received, decoded again on every pass: the job's
    -- inputs are immutable and live nowhere else in this schema.
    command JSONB NOT NULL,
    state TEXT NOT NULL DEFAULT 'queued',
    -- Transient failures before a transaction exists (RPC down, KMS
    -- throttled) count here; the job is retried after `next_attempt_at`.
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error TEXT,
    accepted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at TIMESTAMPTZ,
    -- The event published when the job resolved, for operators.
    resolution JSONB,

    CONSTRAINT execution_jobs_kind
        CHECK (kind IN ('sweep_batch', 'withdrawal_step', 'onboarding_payment')),
    CONSTRAINT execution_jobs_state
        CHECK (state IN ('queued', 'executing', 'finalized', 'abandoned', 'rejected')),
    CONSTRAINT execution_jobs_resolved
        CHECK ((resolved_at IS NULL) = (state IN ('queued', 'executing')))
);

CREATE INDEX execution_jobs_open ON execution.jobs (chain_id, next_attempt_at) WHERE resolved_at IS NULL;
CREATE INDEX execution_jobs_by_correlation ON execution.jobs (correlation_id);

CREATE TABLE execution.transactions (
    id UUID PRIMARY KEY,
    job_id UUID NOT NULL REFERENCES execution.jobs(id),
    chain_id BIGINT NOT NULL,
    signer BYTEA NOT NULL,
    nonce BIGINT NOT NULL,
    target BYTEA NOT NULL,
    calldata BYTEA NOT NULL,
    value TEXT NOT NULL DEFAULT '0',
    gas_limit BIGINT NOT NULL,
    -- prepared: signed bytes durable, not known to have left the node.
    -- broadcast: at least one attempt was accepted by a node.
    -- mined: a receipt was seen for one attempt; awaiting finality.
    -- finalized | reverted: the receipt is final (succeeded / failed).
    -- abandoned: the nonce was consumed by none of the attempts.
    state TEXT NOT NULL DEFAULT 'prepared',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- When the newest attempt was broadcast (or prepared, before that):
    -- the clock the pending timeout runs on.
    submitted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    broadcast_at TIMESTAMPTZ,
    mined_tx_hash BYTEA,
    mined_block BIGINT,
    mined_block_hash BYTEA,
    mined_transaction_index BIGINT,
    mined_succeeded BOOLEAN,
    -- Set once the lane has replaced past the configured limit; one alert.
    stall_reported_at TIMESTAMPTZ,
    resolved_at TIMESTAMPTZ,

    CONSTRAINT execution_transactions_signer_length CHECK (octet_length(signer) = 20),
    CONSTRAINT execution_transactions_target_length CHECK (octet_length(target) = 20),
    CONSTRAINT execution_transactions_nonce_non_negative CHECK (nonce >= 0),
    CONSTRAINT execution_transactions_gas_limit_positive CHECK (gas_limit > 0),
    CONSTRAINT execution_transactions_value_numeric CHECK (value ~ '^[0-9]+$'),
    CONSTRAINT execution_transactions_state
        CHECK (state IN ('prepared', 'broadcast', 'mined', 'finalized', 'reverted', 'abandoned')),
    CONSTRAINT execution_transactions_resolved
        CHECK ((resolved_at IS NULL) = (state IN ('prepared', 'broadcast', 'mined'))),
    CONSTRAINT execution_transactions_mined_complete
        CHECK ((mined_tx_hash IS NULL) = (mined_block IS NULL)
            AND (mined_block IS NULL) = (mined_block_hash IS NULL)
            AND (mined_block_hash IS NULL) = (mined_transaction_index IS NULL)
            AND (mined_transaction_index IS NULL) = (mined_succeeded IS NULL)),
    CONSTRAINT execution_transactions_mined_lengths
        CHECK ((mined_tx_hash IS NULL OR octet_length(mined_tx_hash) = 32)
            AND (mined_block_hash IS NULL OR octet_length(mined_block_hash) = 32)),
    -- One nonce is spent once.
    CONSTRAINT execution_transactions_nonce_once UNIQUE (chain_id, signer, nonce)
);

-- One transaction in flight per signer: the invariant that keeps the nonce
-- stream trivially correct (the next nonce is always the mined count).
CREATE UNIQUE INDEX execution_transactions_open_lane
    ON execution.transactions (chain_id, signer) WHERE resolved_at IS NULL;
CREATE INDEX execution_transactions_by_job ON execution.transactions (job_id);

-- Every signed transaction ever produced for a slot, oldest first.
-- Replacements keep nonce, target, calldata, value and gas limit and raise
-- only the fees.
CREATE TABLE execution.transaction_attempts (
    transaction_id UUID NOT NULL REFERENCES execution.transactions(id),
    replacement_number INTEGER NOT NULL,
    tx_hash BYTEA NOT NULL,
    raw_transaction BYTEA NOT NULL,
    max_fee_per_gas TEXT NOT NULL,
    max_priority_fee_per_gas TEXT NOT NULL,
    prepared_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    broadcast_at TIMESTAMPTZ,

    PRIMARY KEY (transaction_id, replacement_number),
    CONSTRAINT execution_transaction_attempts_hash_length CHECK (octet_length(tx_hash) = 32),
    CONSTRAINT execution_transaction_attempts_hash_unique UNIQUE (tx_hash),
    CONSTRAINT execution_transaction_attempts_fees_numeric
        CHECK (max_fee_per_gas ~ '^[0-9]+$' AND max_priority_fee_per_gas ~ '^[0-9]+$')
);

-- Chain-fault controls the server sent us. A resume is retained as a
-- tombstone so a delayed retry of the matching halt cannot reactivate it.
-- In-flight transactions keep being reconciled while any fault is open.
CREATE TABLE execution.chain_halts (
    fault_id UUID PRIMARY KEY,
    chain_id BIGINT NOT NULL,
    reason TEXT,
    halted_at TIMESTAMPTZ,
    resumed_at TIMESTAMPTZ,
    CONSTRAINT execution_chain_halts_has_control
        CHECK (halted_at IS NOT NULL OR resumed_at IS NOT NULL)
);
CREATE INDEX execution_chain_halts_open ON execution.chain_halts (chain_id)
    WHERE halted_at IS NOT NULL AND resumed_at IS NULL;

-- Health the signers publish for operators (read by gum-server's status
-- endpoint). One row per chain worker and one per pool signer.
CREATE TABLE execution.executor_status (
    chain_id BIGINT PRIMARY KEY,
    state TEXT NOT NULL,
    detail TEXT,
    heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT execution_executor_status_state CHECK (state IN ('running', 'halted', 'failing'))
);

CREATE TABLE execution.signer_status (
    chain_id BIGINT NOT NULL,
    signer BYTEA NOT NULL,
    balance_wei TEXT,
    low_balance BOOLEAN NOT NULL DEFAULT false,
    error TEXT,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (chain_id, signer),
    CONSTRAINT execution_signer_status_signer_length CHECK (octet_length(signer) = 20),
    CONSTRAINT execution_signer_status_balance_numeric
        CHECK (balance_wei IS NULL OR balance_wei ~ '^[0-9]+$')
);
