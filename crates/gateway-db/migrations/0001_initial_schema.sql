-- Payday schema baseline.
--
-- This is the whole schema in one migration. It replaces the pre-release
-- chain of 21 incremental migrations, which was squashed on 2026-09-06 while
-- Payday had no production data to carry forward. Every table, constraint,
-- index, function, and trigger below is exactly what that chain ended with;
-- only the intermediate states are gone.
--
-- Internal tables keep the invoice naming (`invoices`, `payment_observations`,
-- `invoice_attachments`): the product speaks of deposits and deposit requests,
-- but that terminology stops at the HTTP seam and the schema is not that seam.
-- The webhook contract, which the database itself writes, uses the public
-- vocabulary.
--
-- Because the schema is embedded and applied at startup, a database that has
-- already run the old chain refuses this file (its `_sqlx_migrations` rows
-- disagree). Recreate the database empty instead; see
-- docs/production-runbook.md.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- ---------------------------------------------------------------------------
-- Validation helpers used by column constraints.
-- ---------------------------------------------------------------------------

-- Public metadata is a small object of short values.
CREATE FUNCTION payment_metadata_values_fit(value JSONB) RETURNS BOOLEAN
LANGUAGE SQL IMMUTABLE PARALLEL SAFE
AS $$ SELECT count(*) <= 16
          AND COALESCE(bool_and(octet_length(entry.value::text) <= 512), true)
      FROM jsonb_each(value) entry $$;

-- A party object is {name, email?, details?}: bounded free text that is
-- rendered verbatim and never parsed.
CREATE FUNCTION invoice_party_valid(value JSONB) RETURNS BOOLEAN
LANGUAGE SQL IMMUTABLE PARALLEL SAFE
AS $$
    SELECT value IS NOT NULL
       AND jsonb_typeof(value) = 'object'
       AND jsonb_typeof(value->'name') = 'string'
       AND octet_length(value->>'name') BETWEEN 1 AND 255
       AND (NOT value ? 'email'
            OR value->'email' = 'null'::jsonb
            OR (jsonb_typeof(value->'email') = 'string'
                AND octet_length(value->>'email') BETWEEN 3 AND 254))
       AND (NOT value ? 'details'
            OR value->'details' = 'null'::jsonb
            OR (jsonb_typeof(value->'details') = 'string'
                AND octet_length(value->>'details') <= 4000))
$$;

-- ---------------------------------------------------------------------------
-- Accounts. A merchant account is provisioned on first sign-in (Privy) and
-- may hold one server-side API key, one previous key during its rotation
-- grace period, and the embedded wallet Privy created for it. The dashboard
-- provisions an account without issuing a key, so a missing key does not
-- imply revocation; a revoked account still has no key.
-- ---------------------------------------------------------------------------
CREATE TABLE accounts (
    id UUID PRIMARY KEY,
    api_key_hash BYTEA UNIQUE,
    api_key_hint TEXT,
    api_key_generation BIGINT NOT NULL DEFAULT 1,
    key_created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    key_rotated_at TIMESTAMPTZ,
    disabled_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    previous_api_key_hash BYTEA UNIQUE,
    previous_api_key_expires_at TIMESTAMPTZ,
    key_revoked_at TIMESTAMPTZ,
    email TEXT,
    wallet_address TEXT,

    CONSTRAINT accounts_api_key_hash_length CHECK (octet_length(api_key_hash) = 32),
    CONSTRAINT accounts_api_key_generation_positive CHECK (api_key_generation > 0),
    CONSTRAINT accounts_api_key_hint_length CHECK (octet_length(api_key_hint) BETWEEN 1 AND 32),
    CONSTRAINT accounts_previous_api_key_hash_length
        CHECK (previous_api_key_hash IS NULL OR octet_length(previous_api_key_hash) = 32),
    CONSTRAINT accounts_previous_key_complete
        CHECK ((previous_api_key_hash IS NULL) = (previous_api_key_expires_at IS NULL)),
    CONSTRAINT accounts_current_key_complete
        CHECK ((api_key_hash IS NULL) = (api_key_hint IS NULL)),
    CONSTRAINT accounts_revoked_key_state
        CHECK (key_revoked_at IS NULL OR api_key_hash IS NULL),
    CONSTRAINT accounts_email_length
        CHECK (email IS NULL OR octet_length(email) BETWEEN 3 AND 254),
    CONSTRAINT accounts_wallet_address_shape
        CHECK (wallet_address IS NULL OR wallet_address ~ '^0x[0-9a-fA-F]{40}$')
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

-- ---------------------------------------------------------------------------
-- Customers: merchant-owned counterparty records. Issued deposit requests
-- snapshot the party they were billed to instead of depending on these
-- mutable rows.
-- ---------------------------------------------------------------------------
CREATE TABLE customers (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 255),
    email TEXT CHECK (email IS NULL OR octet_length(email) BETWEEN 3 AND 254),
    details TEXT CHECK (details IS NULL OR octet_length(details) <= 4000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, id)
);

CREATE INDEX customers_account_created
    ON customers(account_id, created_at DESC, id DESC);

-- ---------------------------------------------------------------------------
-- Issuer identities and payout addresses: the merchant's own side of a
-- deposit request, saved once instead of retyped per issuance. A request
-- still snapshots the party it was issued under and the address it settles
-- to. `email_verified_at` is the proof that the contact address is a mailbox
-- the merchant holds; clearing the address clears the proof with it
-- (enforced in the repository, which only ever writes them together).
-- ---------------------------------------------------------------------------
CREATE TABLE issuers (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 255),
    contact_email TEXT NOT NULL CHECK (octet_length(contact_email) BETWEEN 3 AND 254),
    details TEXT CHECK (details IS NULL OR octet_length(details) <= 4000),
    email_verified_at TIMESTAMPTZ,
    -- When the last code went out, so a merchant cannot use this route to
    -- post a mailbox a code every second.
    email_code_sent_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, id)
);

CREATE INDEX issuers_account_created ON issuers(account_id, created_at DESC, id DESC);

-- One name per identity, per account, the way a human reads it: case and
-- surrounding space are not a difference. The repository matches this
-- constraint by name (gateway_db::ISSUER_NAME_UNIQUE).
CREATE UNIQUE INDEX issuers_account_name_unique
    ON issuers(account_id, lower(btrim(name)));

-- Payout addresses are stored EIP-55 checksummed, which is also what makes
-- the per-account uniqueness meaningful. A label is a short handle read in a
-- picker, not free text.
CREATE TABLE payout_addresses (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    address TEXT NOT NULL CHECK (address ~ '^0x[0-9a-fA-F]{40}$'),
    label TEXT CHECK (label IS NULL OR label ~ '^[A-Za-z0-9][A-Za-z0-9 ._''&()-]{0,19}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, id),
    UNIQUE (account_id, address)
);

CREATE INDEX payout_addresses_account_created
    ON payout_addresses(account_id, created_at DESC, id DESC);

-- Which addresses an identity may settle to. Many-to-many: one company can
-- pay into several wallets, and one treasury wallet can serve several
-- identities. The composite foreign keys carry `account_id` into both sides,
-- so an association across two accounts cannot be written at all.
CREATE TABLE issuer_payout_addresses (
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    issuer_id UUID NOT NULL,
    payout_address_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (issuer_id, payout_address_id),
    FOREIGN KEY (account_id, issuer_id) REFERENCES issuers(account_id, id) ON DELETE CASCADE,
    FOREIGN KEY (account_id, payout_address_id)
        REFERENCES payout_addresses(account_id, id) ON DELETE CASCADE
);

CREATE INDEX issuer_payout_addresses_by_address
    ON issuer_payout_addresses(payout_address_id);

-- ---------------------------------------------------------------------------
-- Sweep batches: one row per helper transaction nonce. Replacements for the
-- same nonce append to `tx_hashes`; the partial unique index enforces the
-- signer's single in-flight batch, which is what keeps the nonce stream
-- simple.
-- ---------------------------------------------------------------------------
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

-- ---------------------------------------------------------------------------
-- Deposit requests (internally, invoices). One row per issued request: the
-- immutable issuance snapshot (chain parameters, amount, parties, policy,
-- attribution commitment), the payer wallet binding written once the payer
-- attests the wallet they will pay from, and the mutable lifecycle
-- (status, received amount, sweep state, verification completion).
--
-- A request has no payment address at issuance. The payer attests, in their
-- session and once the request's policy is satisfied, the wallet they will
-- pay from (an EIP-712 signature). Only then are the recovery term (that
-- wallet), the salt (derived from the attribution hash and the attestation's
-- signing digest), and the CREATE3 address derived and written, together,
-- once. Excess and late funds return to the payer's own wallet.
--
-- Internal statuses: created, funded, deploying, fulfilled, recovered,
-- expired, blocked. `webhook_public_status` maps them to the public
-- vocabulary.
-- ---------------------------------------------------------------------------
CREATE TABLE invoices (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id),
    idempotency_key TEXT NOT NULL,

    -- Chain parameters committed at issuance.
    chain_id BIGINT NOT NULL,
    factory_address BYTEA NOT NULL,
    token_address BYTEA NOT NULL,
    token_decimals SMALLINT NOT NULL,
    beneficiary_address BYTEA NOT NULL,
    expiration_timestamp BIGINT NOT NULL,
    -- Set with the wallet binding: always equal to payer_wallet.
    recovery_address BYTEA,

    amount TEXT NOT NULL,
    -- Set with the wallet binding.
    salt BYTEA,
    payment_address BYTEA,
    status TEXT NOT NULL,

    -- Funding and settlement, as observed on-chain.
    confirmed_received TEXT NOT NULL DEFAULT '0',
    funded_at_block BIGINT,
    funded_at_block_hash BYTEA,
    observed_amount TEXT,
    execute_tx_hash BYTEA,
    resolved_at_block BIGINT,
    sweep_attempts INTEGER NOT NULL DEFAULT 0,
    last_attempt_at TIMESTAMPTZ,
    blocked_reason TEXT,

    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Sweep state: the in-flight batch, the block at which the address was
    -- last fully drained, and how many nonzero observations still sit there.
    sweep_batch_id UUID REFERENCES sweep_batches(id),
    drained_at_block BIGINT,
    drained_at_transaction_index BIGINT,
    uncollected_count INTEGER NOT NULL DEFAULT 0,
    settled_at TIMESTAMPTZ,

    -- Expiry as requested (`at:<unix>` or `in:<secs>`) and as resolved.
    expires_in_secs BIGINT NOT NULL,
    -- Cancellation is customer intent, not an on-chain lifecycle transition.
    cancellation_requested_at TIMESTAMPTZ,
    memo TEXT,
    expiration_intent TEXT NOT NULL,

    -- Public fields.
    reference TEXT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    paid_at TIMESTAMPTZ,
    expired_at TIMESTAMPTZ,
    settlement_tx_hash BYTEA,
    fee_amount TEXT NOT NULL DEFAULT '0',
    net_amount TEXT NOT NULL,

    -- The document: parties, free text, and the customer link.
    customer_id UUID,
    issuer JSONB,
    bill_to JSONB,
    notes TEXT,
    heading TEXT,

    -- Payer policy: permissionless, verified_email (expected_email is the
    -- asserted mailbox), or merchant_session (payer_reference is the
    -- merchant's own opaque identifier for the authenticated payer).
    payer_policy_mode TEXT NOT NULL DEFAULT 'permissionless',
    expected_email TEXT,
    verification_completed_at TIMESTAMPTZ,

    -- The canonical issuance snapshot and the attribution commitment over it.
    issuance_snapshot JSONB NOT NULL,
    attribution_version SMALLINT NOT NULL,
    attribution_hash BYTEA NOT NULL,
    -- Set when funds arrive from a wallet other than the attested one.
    likely_unsolicited_at TIMESTAMPTZ,

    -- Which issuer identity the request was issued under. The FK takes no
    -- delete action, so an identity that history refers to cannot vanish.
    issuer_id UUID,
    payer_reference TEXT CHECK (
        payer_reference IS NULL OR octet_length(payer_reference) BETWEEN 1 AND 128
    ),

    -- The payer wallet binding, written all at once.
    payer_wallet BYTEA,
    payer_attestation JSONB,
    wallet_bound_at TIMESTAMPTZ,

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
        CHECK (status IN ('created', 'funded', 'deploying', 'fulfilled', 'recovered', 'expired', 'blocked')),
    CONSTRAINT invoices_confirmed_received_numeric
        CHECK (confirmed_received ~ '^[0-9]+$'),
    CONSTRAINT invoices_funded_at_block_non_negative
        CHECK (funded_at_block IS NULL OR funded_at_block >= 0),
    CONSTRAINT invoices_funded_at_block_hash_length
        CHECK (funded_at_block_hash IS NULL OR octet_length(funded_at_block_hash) = 32),
    CONSTRAINT invoices_observed_amount_numeric
        CHECK (observed_amount IS NULL OR observed_amount ~ '^[0-9]+$'),
    CONSTRAINT invoices_execute_tx_hash_length
        CHECK (execute_tx_hash IS NULL OR octet_length(execute_tx_hash) = 32),
    CONSTRAINT invoices_resolved_at_block_non_negative
        CHECK (resolved_at_block IS NULL OR resolved_at_block >= 0),
    CONSTRAINT invoices_sweep_attempts_non_negative
        CHECK (sweep_attempts >= 0),
    CONSTRAINT invoices_drained_at_block_non_negative
        CHECK (drained_at_block IS NULL OR drained_at_block >= 0),
    CONSTRAINT invoices_drained_at_transaction_index_non_negative
        CHECK (drained_at_transaction_index IS NULL OR drained_at_transaction_index >= 0),
    CONSTRAINT invoices_drain_position_complete
        CHECK ((drained_at_block IS NULL) = (drained_at_transaction_index IS NULL)),
    CONSTRAINT invoices_uncollected_count_non_negative CHECK (uncollected_count >= 0),
    CONSTRAINT invoices_expires_in_secs_positive CHECK (expires_in_secs > 0),
    CONSTRAINT invoices_expiration_intent_format
        CHECK (expiration_intent ~ '^(at|in):[0-9]+$'),
    CONSTRAINT invoices_memo_length
        CHECK (memo IS NULL OR octet_length(memo) <= 2000),
    CONSTRAINT invoices_reference_length
        CHECK (reference IS NULL OR char_length(reference) <= 128),
    CONSTRAINT invoices_metadata_object
        CHECK (jsonb_typeof(metadata) = 'object'),
    CONSTRAINT invoices_metadata_limits
        CHECK (payment_metadata_values_fit(metadata)),
    CONSTRAINT invoices_settlement_tx_hash_length
        CHECK (settlement_tx_hash IS NULL OR octet_length(settlement_tx_hash) = 32),
    CONSTRAINT invoices_fee_amount_zero
        CHECK (fee_amount = '0'),
    CONSTRAINT invoices_net_amount_matches_amount
        CHECK (net_amount = amount),
    -- The composite references keep customer and issuer links inside their
    -- own account.
    CONSTRAINT invoices_customer_same_account
        FOREIGN KEY (account_id, customer_id) REFERENCES customers(account_id, id),
    CONSTRAINT invoices_issuer_fk
        FOREIGN KEY (account_id, issuer_id) REFERENCES issuers(account_id, id),
    CONSTRAINT invoices_payer_policy_mode_valid CHECK (
        payer_policy_mode IN ('permissionless', 'verified_email', 'merchant_session')
    ),
    CONSTRAINT invoices_payer_policy_shape CHECK (
        (payer_policy_mode = 'permissionless'
            AND expected_email IS NULL AND payer_reference IS NULL)
        OR
        (payer_policy_mode = 'verified_email'
            AND expected_email IS NOT NULL AND payer_reference IS NULL)
        OR
        (payer_policy_mode = 'merchant_session'
            AND expected_email IS NULL AND payer_reference IS NOT NULL)
    ),
    CONSTRAINT invoices_expected_email_length CHECK (
        expected_email IS NULL OR octet_length(expected_email) BETWEEN 3 AND 254
    ),
    CONSTRAINT invoices_notes_length CHECK (
        notes IS NULL OR octet_length(notes) <= 4000
    ),
    CONSTRAINT invoices_heading_length CHECK (
        heading IS NULL OR octet_length(heading) <= 200
    ),
    CONSTRAINT invoices_issuer_valid
        CHECK (issuer IS NULL OR invoice_party_valid(issuer)),
    CONSTRAINT invoices_bill_to_valid
        CHECK (bill_to IS NULL OR invoice_party_valid(bill_to)),
    CONSTRAINT invoices_payer_wallet_length
        CHECK (payer_wallet IS NULL OR octet_length(payer_wallet) = 20),
    -- All of the binding or none of it, and the recovery term is the wallet.
    CONSTRAINT invoices_binding_complete CHECK (
        (
            payer_wallet IS NULL AND payer_attestation IS NULL AND wallet_bound_at IS NULL
            AND recovery_address IS NULL AND salt IS NULL AND payment_address IS NULL
        )
        OR
        (
            payer_wallet IS NOT NULL AND payer_attestation IS NOT NULL AND wallet_bound_at IS NOT NULL
            AND recovery_address = payer_wallet AND salt IS NOT NULL AND payment_address IS NOT NULL
        )
    ),
    -- Nothing can arrive at an address that does not exist: an unbound
    -- request only ever waits or expires.
    CONSTRAINT invoices_funds_need_binding CHECK (
        payment_address IS NOT NULL
        OR (status IN ('created', 'expired') AND confirmed_received = '0' AND uncollected_count = 0)
    )
);

CREATE UNIQUE INDEX invoices_chain_token_payment_address_unique
    ON invoices (chain_id, token_address, payment_address);
CREATE INDEX invoices_account_id_id ON invoices (account_id, id);
CREATE INDEX invoices_account_id_text_prefix
    ON invoices (account_id, (id::text) text_pattern_ops);
CREATE INDEX invoices_account_created_id_desc
    ON invoices (account_id, created_at DESC, id DESC);
CREATE INDEX invoices_account_status_created_page
    ON invoices (account_id, status, created_at DESC, id DESC);
CREATE INDEX invoices_account_reference_created_page
    ON invoices (account_id, reference, created_at DESC, id DESC) WHERE reference IS NOT NULL;
CREATE INDEX invoices_customer_created
    ON invoices(account_id, customer_id, created_at DESC)
    WHERE customer_id IS NOT NULL;
CREATE INDEX invoices_account_issuer_created
    ON invoices(account_id, issuer_id, created_at DESC)
    WHERE issuer_id IS NOT NULL;
CREATE INDEX invoices_verification_pending
    ON invoices(account_id, payer_policy_mode, expiration_timestamp)
    WHERE verification_completed_at IS NULL
      AND payer_policy_mode <> 'permissionless';
CREATE INDEX invoices_likely_unsolicited
    ON invoices(account_id, likely_unsolicited_at DESC)
    WHERE likely_unsolicited_at IS NOT NULL;

-- The sweep queue and the expiry watch are the two scans the worker runs on
-- every pass; both must stay proportional to open work, not history.
CREATE INDEX invoices_sweep_queue ON invoices (chain_id, id) WHERE uncollected_count > 0;
CREATE INDEX invoices_expiry_watch ON invoices (chain_id, expiration_timestamp)
    WHERE status IN ('created', 'funded');
CREATE INDEX invoices_in_flight ON invoices (sweep_batch_id) WHERE sweep_batch_id IS NOT NULL;

-- Issuance fields are immutable from the start; the binding fields are
-- immutable from the moment they are set, and are set all at once. Lifecycle
-- columns (status, received, sweep state, verification completion) remain
-- mutable. Changing anything else means cancelling and reissuing.
CREATE FUNCTION reject_invoice_issuance_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.account_id IS DISTINCT FROM OLD.account_id
     OR NEW.idempotency_key IS DISTINCT FROM OLD.idempotency_key
     OR NEW.customer_id IS DISTINCT FROM OLD.customer_id
     OR NEW.issuer_id IS DISTINCT FROM OLD.issuer_id
     OR NEW.chain_id IS DISTINCT FROM OLD.chain_id
     OR NEW.factory_address IS DISTINCT FROM OLD.factory_address
     OR NEW.token_address IS DISTINCT FROM OLD.token_address
     OR NEW.token_decimals IS DISTINCT FROM OLD.token_decimals
     OR NEW.beneficiary_address IS DISTINCT FROM OLD.beneficiary_address
     OR NEW.expiration_timestamp IS DISTINCT FROM OLD.expiration_timestamp
     OR NEW.expires_in_secs IS DISTINCT FROM OLD.expires_in_secs
     OR NEW.expiration_intent IS DISTINCT FROM OLD.expiration_intent
     OR NEW.amount IS DISTINCT FROM OLD.amount
     OR NEW.net_amount IS DISTINCT FROM OLD.net_amount
     OR NEW.issuer IS DISTINCT FROM OLD.issuer
     OR NEW.bill_to IS DISTINCT FROM OLD.bill_to
     OR NEW.notes IS DISTINCT FROM OLD.notes
     OR NEW.heading IS DISTINCT FROM OLD.heading
     OR NEW.memo IS DISTINCT FROM OLD.memo
     OR NEW.reference IS DISTINCT FROM OLD.reference
     OR NEW.payer_policy_mode IS DISTINCT FROM OLD.payer_policy_mode
     OR NEW.expected_email IS DISTINCT FROM OLD.expected_email
     OR NEW.payer_reference IS DISTINCT FROM OLD.payer_reference
     OR NEW.issuance_snapshot IS DISTINCT FROM OLD.issuance_snapshot
     OR NEW.attribution_version IS DISTINCT FROM OLD.attribution_version
     OR NEW.attribution_hash IS DISTINCT FROM OLD.attribution_hash
  THEN
    RAISE EXCEPTION 'invoice % issuance fields are immutable; cancel and reissue instead', OLD.id
      USING ERRCODE = 'integrity_constraint_violation';
  END IF;
  IF OLD.payment_address IS NOT NULL AND (
        NEW.payer_wallet IS DISTINCT FROM OLD.payer_wallet
     OR NEW.payer_attestation IS DISTINCT FROM OLD.payer_attestation
     OR NEW.wallet_bound_at IS DISTINCT FROM OLD.wallet_bound_at
     OR NEW.recovery_address IS DISTINCT FROM OLD.recovery_address
     OR NEW.salt IS DISTINCT FROM OLD.salt
     OR NEW.payment_address IS DISTINCT FROM OLD.payment_address
  ) THEN
    RAISE EXCEPTION 'invoice % payer wallet binding is immutable once set', OLD.id
      USING ERRCODE = 'integrity_constraint_violation';
  END IF;
  RETURN NEW;
END $$;

CREATE TRIGGER invoice_issuance_immutable
BEFORE UPDATE OF
    account_id, idempotency_key, customer_id, issuer_id, chain_id, factory_address,
    token_address, token_decimals, beneficiary_address, expiration_timestamp,
    expires_in_secs, expiration_intent, recovery_address, amount, net_amount,
    salt, payment_address, issuer, bill_to, notes, heading, memo, reference,
    payer_policy_mode, expected_email, payer_reference, issuance_snapshot,
    attribution_version, attribution_hash,
    payer_wallet, payer_attestation, wallet_bound_at
ON invoices
FOR EACH ROW EXECUTE FUNCTION reject_invoice_issuance_mutation();

-- ---------------------------------------------------------------------------
-- Indexer state. The cursor is the last committed finalized range (and a
-- complete finalized-head projection for API `as_of` and freshness
-- reporting); finality is an independently observed boundary, not an alias
-- for the last successfully indexed range; and the sweeper reports its own
-- health, because queue depth alone cannot distinguish an idle healthy
-- worker from one that is paused or failing.
-- ---------------------------------------------------------------------------
CREATE TABLE indexer_cursor (
    chain_id BIGINT PRIMARY KEY,
    token_address BYTEA NOT NULL,
    last_block BIGINT NOT NULL,
    last_block_hash BYTEA NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_block_timestamp BIGINT,

    CONSTRAINT indexer_cursor_token_address_length
        CHECK (octet_length(token_address) = 20),
    CONSTRAINT indexer_cursor_last_block_non_negative
        CHECK (last_block >= 0),
    CONSTRAINT indexer_cursor_last_block_hash_length
        CHECK (octet_length(last_block_hash) = 32),
    CONSTRAINT indexer_cursor_last_block_timestamp_non_negative
        CHECK (last_block_timestamp IS NULL OR last_block_timestamp >= 0)
);

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

CREATE TABLE sweeper_status (
    chain_id BIGINT PRIMARY KEY,
    state TEXT NOT NULL CHECK (state IN ('running', 'paused', 'degraded')),
    heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- Payment observations: every finalized USDC transfer into a payment
-- address, as the ledger. Every nonzero observation is either still at the
-- address or was drained by a sweep at `collected_at_block`. `late`
-- transfers are recovered automatically to the payer's wallet; `error` rows
-- await an operator.
-- ---------------------------------------------------------------------------
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
    disposition TEXT NOT NULL DEFAULT 'credited',
    disposition_reason TEXT,
    collected_at_block BIGINT,
    collected_at_transaction_index BIGINT,
    block_timestamp BIGINT NOT NULL,

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
    CONSTRAINT payment_observations_amount_numeric
        CHECK (amount ~ '^[0-9]+$'),
    CONSTRAINT payment_observations_block_number_non_negative
        CHECK (block_number >= 0),
    CONSTRAINT payment_observations_transaction_index_non_negative
        CHECK (transaction_index >= 0),
    CONSTRAINT payment_observations_log_index_non_negative
        CHECK (log_index >= 0),
    CONSTRAINT payment_observations_disposition_valid
        CHECK (disposition IN ('credited', 'late', 'error')),
    CONSTRAINT payment_observations_disposition_reason_valid
        CHECK (
            (disposition = 'credited' AND disposition_reason IS NULL)
            OR (disposition != 'credited' AND disposition_reason IS NOT NULL)
        ),
    CONSTRAINT payment_observations_collected_at_block_non_negative
        CHECK (collected_at_block IS NULL OR collected_at_block >= 0),
    CONSTRAINT payment_observations_collected_at_tx_index_non_negative
        CHECK (collected_at_transaction_index IS NULL OR collected_at_transaction_index >= 0),
    CONSTRAINT payment_observations_collection_position_complete
        CHECK ((collected_at_block IS NULL) = (collected_at_transaction_index IS NULL)),
    CONSTRAINT payment_observations_block_timestamp_non_negative
        CHECK (block_timestamp >= 0)
);

CREATE INDEX payment_observations_invoice_order
    ON payment_observations (invoice_id, block_number, transaction_index, log_index);
CREATE INDEX payment_observations_uncollected ON payment_observations (invoice_id, block_number)
    WHERE collected_at_block IS NULL AND disposition <> 'error';
CREATE INDEX payment_observations_errors ON payment_observations (observed_at) WHERE disposition = 'error';

-- ---------------------------------------------------------------------------
-- Recovery ledger: every amount returned to the payer's wallet on a
-- request's behalf, so review has a complete record.
-- ---------------------------------------------------------------------------
CREATE TABLE recovered_funds (
    id UUID PRIMARY KEY,
    invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    chain_id BIGINT NOT NULL,
    token_address BYTEA NOT NULL CHECK (octet_length(token_address) = 20),
    transaction_hash BYTEA NOT NULL CHECK (octet_length(transaction_hash) = 32),
    block_number BIGINT NOT NULL CHECK (block_number >= 0),
    amount TEXT NOT NULL CHECK (amount ~ '^[0-9]+$' AND amount <> '0'),
    reason TEXT NOT NULL CHECK (
        reason IN ('overpayment', 'expired', 'late_transfer')
    ),
    recovered_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(invoice_id, transaction_hash, reason)
);

-- ---------------------------------------------------------------------------
-- PDF attachments: staged by presigned upload, finalized after the malware
-- scan, and bound to exactly one deposit request at issuance.
-- ---------------------------------------------------------------------------
CREATE TABLE invoice_attachments (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    invoice_id UUID UNIQUE REFERENCES invoices(id) ON DELETE CASCADE,
    object_key TEXT NOT NULL UNIQUE,
    original_filename TEXT NOT NULL CHECK (
        octet_length(original_filename) BETWEEN 1 AND 255
    ),
    mime_type TEXT NOT NULL CHECK (mime_type = 'application/pdf'),
    byte_length BIGINT CHECK (
        byte_length IS NULL OR byte_length BETWEEN 1 AND 5242880
    ),
    sha256 BYTEA CHECK (sha256 IS NULL OR octet_length(sha256) = 32),
    -- The bucket version that was hashed at finalization. Every later read
    -- (retagging, download) pins it, so a second PUT to the key can never
    -- replace what the request committed to. NULL on unversioned buckets.
    version_id TEXT,
    status TEXT NOT NULL CHECK (
        status IN ('pending_upload', 'scanning', 'ready', 'rejected', 'attached')
    ),
    scan_result TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finalized_at TIMESTAMPTZ,
    attached_at TIMESTAMPTZ,
    CHECK (
        (status IN ('pending_upload', 'scanning')
            AND byte_length IS NULL AND sha256 IS NULL)
        OR
        (status IN ('ready', 'attached')
            AND byte_length IS NOT NULL
            AND sha256 IS NOT NULL
            AND scan_result = 'NO_THREATS_FOUND')
        OR status = 'rejected'
    )
);

CREATE INDEX invoice_attachments_stale
    ON invoice_attachments(created_at)
    WHERE invoice_id IS NULL;

-- The attachment commitment (object, length, hash) is fixed once finalized,
-- and an attached document can never move to another request.
CREATE FUNCTION reject_attachment_commitment_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF OLD.invoice_id IS NOT NULL AND NEW.invoice_id IS DISTINCT FROM OLD.invoice_id THEN
    RAISE EXCEPTION 'attachment % is bound to invoice % and cannot be moved', OLD.id, OLD.invoice_id
      USING ERRCODE = 'integrity_constraint_violation';
  END IF;
  IF OLD.sha256 IS NOT NULL AND (
       NEW.sha256 IS DISTINCT FROM OLD.sha256
       OR NEW.byte_length IS DISTINCT FROM OLD.byte_length
       OR NEW.version_id IS DISTINCT FROM OLD.version_id
       OR NEW.object_key IS DISTINCT FROM OLD.object_key)
  THEN
    RAISE EXCEPTION 'attachment % is finalized and its content commitment is immutable', OLD.id
      USING ERRCODE = 'integrity_constraint_violation';
  END IF;
  RETURN NEW;
END $$;

CREATE TRIGGER invoice_attachment_commitment_immutable
BEFORE UPDATE OF invoice_id, object_key, sha256, byte_length, version_id ON invoice_attachments
FOR EACH ROW EXECUTE FUNCTION reject_attachment_commitment_mutation();

-- ---------------------------------------------------------------------------
-- Payer state: opaque sessions, the facts each one has established (a
-- proven mailbox, a merchant-session exchange, the one-time wallet
-- challenge), verification attempts, and the single-use client secrets a
-- merchant's own application mints to open the checkout for an
-- authenticated payer.
-- ---------------------------------------------------------------------------
CREATE TABLE payer_sessions (
    id UUID PRIMARY KEY,
    token_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    payer_ref BYTEA CHECK (payer_ref IS NULL OR octet_length(payer_ref) = 32),
    email_verified_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    merchant_session_verified_at TIMESTAMPTZ,
    wallet_nonce BYTEA CHECK (wallet_nonce IS NULL OR octet_length(wallet_nonce) = 32),
    wallet_nonce_expires_at TIMESTAMPTZ,
    CHECK (expires_at > created_at),
    CONSTRAINT payer_sessions_wallet_challenge_complete
        CHECK ((wallet_nonce IS NULL) = (wallet_nonce_expires_at IS NULL))
);

CREATE INDEX payer_sessions_invoice ON payer_sessions(invoice_id);
CREATE INDEX payer_sessions_expiry ON payer_sessions(expires_at);

-- An email code (auth0), a wallet attestation (payday), or a merchant-session
-- exchange (merchant), each recorded as an attempt of its own kind.
CREATE TABLE payer_verifications (
    id UUID PRIMARY KEY,
    invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    payer_session_id UUID NOT NULL REFERENCES payer_sessions(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('email', 'wallet', 'merchant_session')),
    status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'abandoned')),
    provider TEXT NOT NULL CHECK (provider IN ('auth0', 'payday', 'merchant')),
    provider_event_id TEXT,
    payer_ref BYTEA CHECK (payer_ref IS NULL OR octet_length(payer_ref) = 32),
    verified_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX payer_verifications_invoice
    ON payer_verifications(invoice_id, created_at DESC);
CREATE INDEX payer_verifications_session
    ON payer_verifications(payer_session_id);

-- Client secrets: minted by the merchant API for one request, returned once,
-- stored hashed, and spent by exactly one exchange. `used_at` and the
-- session it minted are set together.
CREATE TABLE payer_client_secrets (
    id UUID PRIMARY KEY,
    invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    secret_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(secret_hash) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    payer_session_id UUID REFERENCES payer_sessions(id) ON DELETE SET NULL,
    CHECK (expires_at > created_at),
    CHECK (used_at IS NULL OR used_at >= created_at)
);

CREATE INDEX payer_client_secrets_invoice
    ON payer_client_secrets(invoice_id, created_at DESC);

-- ---------------------------------------------------------------------------
-- Onboarding: the one real, self-funded deposit request the walkthrough
-- issues per account. One row per account, ever: the primary key both
-- dedupes a retried request and bounds the feature to at most one on-chain
-- transfer per account. tx_hash starts NULL and is filled in once the
-- transfer broadcasts, so a crash in between leaves a retryable state.
-- ---------------------------------------------------------------------------
CREATE TABLE onboarding_demo_payments (
    account_id UUID PRIMARY KEY REFERENCES accounts(id),
    payment_id UUID NOT NULL REFERENCES invoices(id),
    tx_hash TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- Email notifications: an outbox drained by the API. A `merchant` row tells
-- the account's verified contact about a payout that needs attention; a
-- `payer` row sends the payer named on a deposit request, at the
-- `payer.email` given at issuance, their link to it. The dispatcher claims
-- only the recipients it has a sender configured for. A row the provider
-- rejects outright, or whose request no longer stands, is abandoned rather
-- than retried forever.
-- ---------------------------------------------------------------------------
CREATE TABLE notification_outbox (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id),
    invoice_id UUID NOT NULL REFERENCES invoices(id),
    recipient TEXT NOT NULL DEFAULT 'merchant' CHECK (recipient IN ('merchant', 'payer')),
    reason TEXT NOT NULL,
    email TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    delivered_at TIMESTAMPTZ,
    missing_email_reported_at TIMESTAMPTZ,
    abandoned_at TIMESTAMPTZ,
    leased_until TIMESTAMPTZ,
    last_error TEXT
);
CREATE INDEX notification_outbox_pending ON notification_outbox (next_attempt_at)
    WHERE abandoned_at IS NULL
      AND ((email IS NOT NULL AND delivered_at IS NULL)
        OR (email IS NULL AND missing_email_reported_at IS NULL));

-- ---------------------------------------------------------------------------
-- Webhooks: durable, account-scoped delivery. Events are written by the
-- triggers below in the same transaction as the state change they report;
-- deliveries fan out to every active endpoint and are leased by the API's
-- delivery worker.
-- ---------------------------------------------------------------------------
CREATE TABLE webhook_endpoints (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    secret_hash BYTEA NOT NULL CHECK (octet_length(secret_hash) = 32),
    secret_ciphertext BYTEA NOT NULL,
    secret_cipher_version SMALLINT NOT NULL DEFAULT 1 CHECK (secret_cipher_version = 1),
    secret_key_id TEXT NOT NULL DEFAULT 'primary',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX webhook_active_endpoint_url_once ON webhook_endpoints(account_id, url)
    WHERE disabled_at IS NULL;

CREATE TABLE webhook_events (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    invoice_id UUID REFERENCES invoices(id) ON DELETE CASCADE,
    event_type TEXT NOT NULL CHECK (
        event_type IN (
            'deposit_request.deposited', 'deposit_request.settled', 'deposit_request.expired',
            'deposit_request.returned', 'deposit_request.needs_attention',
            'deposit_request.likely_unsolicited', 'deposit_request.recovered_funds',
            'deposit_request.ready', 'verification.approved', 'webhook.test'
        )
    ),
    payload JSONB NOT NULL,
    fanout BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- Lifecycle events happen once per request, so replay is idempotent. Test
-- events need no request and take no uniqueness slot. Recovery events are one
-- per ledger row, and a request can recover funds more than once (an
-- overpayment at settlement, then a late transfer), so they are keyed by the
-- ledger row instead.
CREATE UNIQUE INDEX webhook_lifecycle_event_once
    ON webhook_events(invoice_id, event_type)
    WHERE invoice_id IS NOT NULL AND fanout AND event_type <> 'deposit_request.recovered_funds';

CREATE TABLE webhook_deliveries (
    id UUID PRIMARY KEY,
    event_id UUID NOT NULL REFERENCES webhook_events(id) ON DELETE CASCADE,
    endpoint_id UUID NOT NULL REFERENCES webhook_endpoints(id) ON DELETE CASCADE,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_until TIMESTAMPTZ,
    lease_token UUID,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','delivered','failed')),
    delivered_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(event_id, endpoint_id)
);
CREATE INDEX webhook_deliveries_ready ON webhook_deliveries(next_attempt_at)
    WHERE state = 'pending';

CREATE TABLE webhook_delivery_attempts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    delivery_id UUID NOT NULL REFERENCES webhook_deliveries(id) ON DELETE CASCADE,
    attempt_number INTEGER NOT NULL,
    attempted_at TIMESTAMPTZ NOT NULL,
    duration_ms BIGINT NOT NULL CHECK (duration_ms >= 0),
    status INTEGER,
    error TEXT,
    UNIQUE(delivery_id, attempt_number)
);

-- The public status vocabulary of the webhook envelope, identical to the
-- API's.
CREATE FUNCTION webhook_public_status(status TEXT, confirmed_received TEXT, blocked_reason TEXT)
RETURNS TEXT LANGUAGE SQL IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE
        WHEN blocked_reason IS NOT NULL OR status = 'blocked' THEN 'needs_attention'
        WHEN status = 'created' AND confirmed_received = '0' THEN 'awaiting_deposit'
        WHEN status = 'created' THEN 'partially_deposited'
        WHEN status IN ('funded', 'deploying') THEN 'deposited'
        WHEN status = 'fulfilled' THEN 'settled'
        WHEN status = 'expired' THEN 'expired'
        WHEN status = 'recovered' THEN 'returned'
    END
$$;

-- The deposit request object shared by every event: the policy mode, the
-- merchant's own payer reference, verification completion, the
-- unsolicited-funding timestamp, and the bound wallet and address; never the
-- expected email or any payer assertion.
CREATE FUNCTION webhook_deposit_request_object(invoice invoices) RETURNS JSONB
LANGUAGE SQL STABLE AS $$
    SELECT jsonb_build_object(
        'id', invoice.id,
        'status', webhook_public_status(invoice.status, invoice.confirmed_received, invoice.blocked_reason),
        'amount', invoice.amount,
        'received', invoice.confirmed_received,
        'reference', invoice.reference,
        'metadata', invoice.metadata,
        'payer_policy_mode', invoice.payer_policy_mode,
        'payer_reference', invoice.payer_reference,
        'verification_completed_at', invoice.verification_completed_at,
        'likely_unsolicited_at', invoice.likely_unsolicited_at,
        'payer_wallet', CASE WHEN invoice.payer_wallet IS NULL THEN NULL
                             ELSE '0x' || encode(invoice.payer_wallet, 'hex') END,
        'address', CASE WHEN invoice.payment_address IS NULL THEN NULL
                        ELSE '0x' || encode(invoice.payment_address, 'hex') END,
        'wallet_bound_at', invoice.wallet_bound_at)
$$;

CREATE FUNCTION enqueue_invoice_webhook_event(
    account UUID, invoice UUID, kind TEXT, occurred TIMESTAMPTZ, data JSONB
) RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE event_uuid UUID;
BEGIN
  event_uuid := gen_random_uuid();
  INSERT INTO webhook_events(id, account_id, invoice_id, event_type, payload)
  VALUES (event_uuid, account, invoice, kind,
    jsonb_build_object('version', '2026-08-01', 'id', event_uuid, 'type', kind,
      'occurred_at', COALESCE(occurred, now()), 'data', data))
  ON CONFLICT (invoice_id, event_type)
    WHERE invoice_id IS NOT NULL AND fanout AND event_type <> 'deposit_request.recovered_funds'
    DO NOTHING;
END $$;

-- Lifecycle, verification, readiness, and unsolicited-funding events, from
-- the request's own row changes.
CREATE FUNCTION enqueue_payment_webhook() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE kind TEXT; occurred TIMESTAMPTZ; attention BOOLEAN; deposit_request JSONB;
BEGIN
  deposit_request := webhook_deposit_request_object(NEW);
  attention := NEW.blocked_reason IS NOT NULL AND OLD.blocked_reason IS NULL;

  IF NEW.status IS DISTINCT FROM OLD.status OR attention THEN
    kind := CASE WHEN attention THEN 'deposit_request.needs_attention' ELSE CASE NEW.status
      WHEN 'funded' THEN 'deposit_request.deposited' WHEN 'fulfilled' THEN 'deposit_request.settled'
      WHEN 'expired' THEN 'deposit_request.expired' WHEN 'recovered' THEN 'deposit_request.returned' END END;
    IF kind IS NOT NULL THEN
      occurred := CASE WHEN NEW.status = 'funded' THEN NEW.paid_at
                       WHEN NEW.status = 'expired' THEN NEW.expired_at
                       WHEN NEW.status IN ('fulfilled', 'recovered') THEN NEW.settled_at END;
      PERFORM enqueue_invoice_webhook_event(NEW.account_id, NEW.id, kind, occurred,
        jsonb_build_object('deposit_request', deposit_request ||
          CASE WHEN attention OR NEW.status = 'blocked' THEN jsonb_build_object('attention', jsonb_build_object(
            'reason_code', NEW.blocked_reason,
            'message', 'Automatic payout requires a manual review. Funds remain safe.',
            'action', 'Contact Payday support and provide the deposit request ID.'))
          ELSE '{}'::jsonb END));
    END IF;
  END IF;

  IF NEW.verification_completed_at IS NOT NULL AND OLD.verification_completed_at IS NULL THEN
    PERFORM enqueue_invoice_webhook_event(NEW.account_id, NEW.id, 'verification.approved',
      NEW.verification_completed_at, jsonb_build_object('deposit_request', deposit_request));
  END IF;

  IF NEW.wallet_bound_at IS NOT NULL AND OLD.wallet_bound_at IS NULL THEN
    PERFORM enqueue_invoice_webhook_event(NEW.account_id, NEW.id, 'deposit_request.ready',
      NEW.wallet_bound_at, jsonb_build_object('deposit_request', deposit_request));
  END IF;

  IF NEW.likely_unsolicited_at IS NOT NULL AND OLD.likely_unsolicited_at IS NULL THEN
    PERFORM enqueue_invoice_webhook_event(NEW.account_id, NEW.id, 'deposit_request.likely_unsolicited',
      NEW.likely_unsolicited_at, jsonb_build_object('deposit_request', deposit_request));
  END IF;

  RETURN NEW;
END $$;

CREATE TRIGGER invoice_webhook_transition
AFTER UPDATE OF status, blocked_reason, verification_completed_at, wallet_bound_at, likely_unsolicited_at ON invoices
FOR EACH ROW EXECUTE FUNCTION enqueue_payment_webhook();

-- One event per ledger row, written in the transaction that records the
-- recovery.
CREATE FUNCTION enqueue_recovered_funds_webhook() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE invoice invoices;
BEGIN
  SELECT * INTO invoice FROM invoices WHERE id = NEW.invoice_id;
  PERFORM enqueue_invoice_webhook_event(invoice.account_id, NEW.invoice_id, 'deposit_request.recovered_funds',
    NEW.recovered_at,
    jsonb_build_object(
      'deposit_request', webhook_deposit_request_object(invoice),
      'recovery', jsonb_build_object(
        'id', NEW.id,
        'amount', NEW.amount,
        'reason', NEW.reason,
        'transaction_hash', '0x' || encode(NEW.transaction_hash, 'hex'),
        'block_number', NEW.block_number,
        'recovered_at', NEW.recovered_at)));
  RETURN NEW;
END $$;

CREATE TRIGGER recovered_funds_webhook AFTER INSERT ON recovered_funds
FOR EACH ROW EXECUTE FUNCTION enqueue_recovered_funds_webhook();

-- Every event fans out to the account's active endpoints as it is written.
CREATE FUNCTION fanout_webhook_event() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NOT NEW.fanout THEN RETURN NEW; END IF;
  INSERT INTO webhook_deliveries(id,event_id,endpoint_id)
    SELECT gen_random_uuid(),NEW.id,id FROM webhook_endpoints
    WHERE account_id=NEW.account_id AND disabled_at IS NULL;
  RETURN NEW;
END $$;

CREATE TRIGGER webhook_event_fanout AFTER INSERT ON webhook_events
FOR EACH ROW EXECUTE FUNCTION fanout_webhook_event();
