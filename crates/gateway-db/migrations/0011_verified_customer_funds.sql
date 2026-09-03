-- Verified customer funds: the consolidated pre-production schema for the
-- invoice document, payer policy, verification state, attachments,
-- attribution commitment, and the platform recovery ledger
-- (features/product-plan.md §8). Payday has no production data, so the
-- whole product schema lands in this one migration; squash it into the
-- baseline before the first production release.
--
-- Invoice document, policy, and attribution columns stay nullable while the
-- slices that populate them are developed. Before release, reset
-- pre-production data and make issuer, bill_to, issuance_snapshot, and the
-- attribution columns NOT NULL.

-- ---------------------------------------------------------------------------
-- Accounts: the dashboard provisions an account on first sign-in without
-- issuing an API key, so a missing key no longer implies revocation. A
-- revoked account still has no key.
-- ---------------------------------------------------------------------------
ALTER TABLE accounts DROP CONSTRAINT accounts_revoked_key_state;
ALTER TABLE accounts ADD CONSTRAINT accounts_revoked_key_state
    CHECK (key_revoked_at IS NULL OR api_key_hash IS NULL);

-- ---------------------------------------------------------------------------
-- Customers: merchant-owned counterparty records. Issued invoices snapshot
-- the party they were billed to instead of depending on these mutable rows.
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
-- Invoice document, payer policy, verification completion, and attribution.
-- ---------------------------------------------------------------------------

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

-- The merchant's asserted legal identity for `verified_identity`: the minimal
-- fields the identity vendor matches against.
CREATE FUNCTION expected_identity_valid(value JSONB) RETURNS BOOLEAN
LANGUAGE SQL IMMUTABLE PARALLEL SAFE
AS $$
    SELECT value IS NOT NULL
       AND jsonb_typeof(value) = 'object'
       AND jsonb_typeof(value->'first_name') = 'string'
       AND jsonb_typeof(value->'last_name') = 'string'
       AND octet_length(value->>'first_name') BETWEEN 1 AND 255
       AND octet_length(value->>'last_name') BETWEEN 1 AND 255
$$;

ALTER TABLE invoices
    ADD COLUMN customer_id UUID,
    ADD COLUMN issuer JSONB,
    ADD COLUMN bill_to JSONB,
    ADD COLUMN notes TEXT,
    ADD COLUMN heading TEXT,
    ADD COLUMN payer_policy_mode TEXT NOT NULL DEFAULT 'permissionless',
    ADD COLUMN expected_email TEXT,
    ADD COLUMN expected_identity JSONB,
    ADD COLUMN verification_completed_at TIMESTAMPTZ,
    ADD COLUMN issuance_snapshot JSONB,
    ADD COLUMN attribution_version SMALLINT,
    ADD COLUMN attribution_nonce BYTEA,
    ADD COLUMN attribution_hash BYTEA,
    ADD COLUMN likely_unsolicited_at TIMESTAMPTZ;

ALTER TABLE invoices
    -- The composite reference keeps a customer link inside its own account.
    ADD CONSTRAINT invoices_customer_same_account
        FOREIGN KEY (account_id, customer_id) REFERENCES customers(account_id, id),
    ADD CONSTRAINT invoices_payer_policy_mode_valid CHECK (
        payer_policy_mode IN (
            'permissionless',
            'verified_email',
            'verified_identity',
            'verified_identity_unattributed'
        )
    ),
    ADD CONSTRAINT invoices_payer_policy_shape CHECK (
        (payer_policy_mode = 'permissionless'
            AND expected_email IS NULL AND expected_identity IS NULL)
        OR
        (payer_policy_mode = 'verified_email'
            AND expected_email IS NOT NULL AND expected_identity IS NULL)
        OR
        (payer_policy_mode = 'verified_identity'
            AND expected_email IS NOT NULL AND expected_identity IS NOT NULL)
        OR
        (payer_policy_mode = 'verified_identity_unattributed'
            AND expected_email IS NOT NULL AND expected_identity IS NULL)
    ),
    ADD CONSTRAINT invoices_expected_email_length CHECK (
        expected_email IS NULL OR octet_length(expected_email) BETWEEN 3 AND 254
    ),
    ADD CONSTRAINT invoices_attribution_complete CHECK (
        (attribution_version IS NULL
            AND attribution_nonce IS NULL
            AND attribution_hash IS NULL
            AND issuance_snapshot IS NULL)
        OR
        (attribution_version = 1
            AND octet_length(attribution_nonce) = 32
            AND octet_length(attribution_hash) = 32
            AND jsonb_typeof(issuance_snapshot) = 'object')
    ),
    ADD CONSTRAINT invoices_notes_length CHECK (
        notes IS NULL OR octet_length(notes) <= 4000
    ),
    ADD CONSTRAINT invoices_heading_length CHECK (
        heading IS NULL OR octet_length(heading) <= 200
    ),
    ADD CONSTRAINT invoices_issuer_valid
        CHECK (issuer IS NULL OR invoice_party_valid(issuer)),
    ADD CONSTRAINT invoices_bill_to_valid
        CHECK (bill_to IS NULL OR invoice_party_valid(bill_to)),
    ADD CONSTRAINT invoices_expected_identity_valid
        CHECK (expected_identity IS NULL OR expected_identity_valid(expected_identity));

CREATE INDEX invoices_customer_created
    ON invoices(account_id, customer_id, created_at DESC)
    WHERE customer_id IS NOT NULL;
CREATE INDEX invoices_verification_pending
    ON invoices(account_id, payer_policy_mode, expiration_timestamp)
    WHERE verification_completed_at IS NULL
      AND payer_policy_mode <> 'permissionless';
CREATE INDEX invoices_likely_unsolicited
    ON invoices(account_id, likely_unsolicited_at DESC)
    WHERE likely_unsolicited_at IS NOT NULL;

-- An issued invoice is an immutable snapshot. Its chain parameters, amount,
-- parties (including the customer link), policy, and attribution material
-- can only change through cancellation and reissuance; the trigger refuses
-- any update that touches them. Lifecycle columns (status, received, sweep
-- state, verification completion) remain mutable.
CREATE FUNCTION reject_invoice_issuance_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.account_id IS DISTINCT FROM OLD.account_id
     OR NEW.idempotency_key IS DISTINCT FROM OLD.idempotency_key
     OR NEW.customer_id IS DISTINCT FROM OLD.customer_id
     OR NEW.chain_id IS DISTINCT FROM OLD.chain_id
     OR NEW.factory_address IS DISTINCT FROM OLD.factory_address
     OR NEW.token_address IS DISTINCT FROM OLD.token_address
     OR NEW.token_decimals IS DISTINCT FROM OLD.token_decimals
     OR NEW.beneficiary_address IS DISTINCT FROM OLD.beneficiary_address
     OR NEW.expiration_timestamp IS DISTINCT FROM OLD.expiration_timestamp
     OR NEW.expires_in_secs IS DISTINCT FROM OLD.expires_in_secs
     OR NEW.expiration_intent IS DISTINCT FROM OLD.expiration_intent
     OR NEW.recovery_address IS DISTINCT FROM OLD.recovery_address
     OR NEW.amount IS DISTINCT FROM OLD.amount
     OR NEW.net_amount IS DISTINCT FROM OLD.net_amount
     OR NEW.salt IS DISTINCT FROM OLD.salt
     OR NEW.payment_address IS DISTINCT FROM OLD.payment_address
     OR NEW.issuer IS DISTINCT FROM OLD.issuer
     OR NEW.bill_to IS DISTINCT FROM OLD.bill_to
     OR NEW.notes IS DISTINCT FROM OLD.notes
     OR NEW.heading IS DISTINCT FROM OLD.heading
     OR NEW.memo IS DISTINCT FROM OLD.memo
     OR NEW.reference IS DISTINCT FROM OLD.reference
     OR NEW.payer_policy_mode IS DISTINCT FROM OLD.payer_policy_mode
     OR NEW.expected_email IS DISTINCT FROM OLD.expected_email
     OR NEW.expected_identity IS DISTINCT FROM OLD.expected_identity
     OR NEW.issuance_snapshot IS DISTINCT FROM OLD.issuance_snapshot
     OR NEW.attribution_version IS DISTINCT FROM OLD.attribution_version
     OR NEW.attribution_nonce IS DISTINCT FROM OLD.attribution_nonce
     OR NEW.attribution_hash IS DISTINCT FROM OLD.attribution_hash
  THEN
    RAISE EXCEPTION 'invoice % issuance fields are immutable; cancel and reissue instead', OLD.id
      USING ERRCODE = 'integrity_constraint_violation';
  END IF;
  RETURN NEW;
END $$;

CREATE TRIGGER invoice_issuance_immutable
BEFORE UPDATE OF
    account_id, idempotency_key, customer_id, chain_id, factory_address,
    token_address, token_decimals, beneficiary_address, expiration_timestamp,
    expires_in_secs, expiration_intent, recovery_address, amount, net_amount,
    salt, payment_address, issuer, bill_to, notes, heading, memo, reference,
    payer_policy_mode, expected_email, expected_identity, issuance_snapshot,
    attribution_version, attribution_nonce, attribution_hash
ON invoices
FOR EACH ROW EXECUTE FUNCTION reject_invoice_issuance_mutation();

-- ---------------------------------------------------------------------------
-- PDF attachments: staged by presigned upload, finalized after the malware
-- scan, and bound to exactly one invoice at issuance.
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
    -- replace what the invoice committed to. NULL on unversioned buckets.
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
-- and an attached document can never move to another invoice.
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
-- Payer state: opaque sessions, verification attempts, and the
-- merchant-scoped credential reuse ledger. No vendor-extracted identity.
-- ---------------------------------------------------------------------------
CREATE TABLE payer_sessions (
    id UUID PRIMARY KEY,
    token_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    payer_ref BYTEA CHECK (payer_ref IS NULL OR octet_length(payer_ref) = 32),
    email_verified_at TIMESTAMPTZ,
    document_verified_at TIMESTAMPTZ,
    liveness_verified_at TIMESTAMPTZ,
    identity_matched_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    CHECK (expires_at > created_at)
);

CREATE INDEX payer_sessions_invoice ON payer_sessions(invoice_id);
CREATE INDEX payer_sessions_expiry ON payer_sessions(expires_at);

CREATE TABLE payer_verifications (
    id UUID PRIMARY KEY,
    invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    payer_session_id UUID NOT NULL REFERENCES payer_sessions(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('email', 'identity')),
    status TEXT NOT NULL CHECK (
        status IN (
            'pending', 'approved', 'declined', 'in_review',
            'expired', 'abandoned', 'review_required'
        )
    ),
    provider TEXT NOT NULL CHECK (provider IN ('auth0', 'didit', 'manual')),
    provider_reference TEXT,
    provider_event_id TEXT,
    payer_ref BYTEA CHECK (payer_ref IS NULL OR octet_length(payer_ref) = 32),
    expected_identity_hash BYTEA CHECK (
        expected_identity_hash IS NULL OR octet_length(expected_identity_hash) = 32
    ),
    document_status TEXT,
    liveness_status TEXT,
    identity_match_status TEXT,
    risk_codes TEXT[] NOT NULL DEFAULT '{}',
    country_code TEXT CHECK (
        country_code IS NULL OR country_code ~ '^[A-Z]{2,3}$'
    ),
    attempt_number SMALLINT NOT NULL DEFAULT 1 CHECK (attempt_number BETWEEN 1 AND 2),
    verified_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    -- Reconciliation of hosted identity sessions: when to ask the provider
    -- next, the lease a worker holds while asking, how many times a decision
    -- has been fetched (drives the in-review backoff), and how many fetches
    -- failed in a row (drives the operational backoff; never a decline).
    next_poll_at TIMESTAMPTZ,
    leased_until TIMESTAMPTZ,
    poll_count INTEGER NOT NULL DEFAULT 0 CHECK (poll_count >= 0),
    provider_failures INTEGER NOT NULL DEFAULT 0 CHECK (provider_failures >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (kind <> 'identity' OR payer_ref IS NOT NULL)
);

CREATE UNIQUE INDEX payer_verifications_provider_reference
    ON payer_verifications(provider, provider_reference)
    WHERE provider_reference IS NOT NULL;
CREATE INDEX payer_verifications_poll
    ON payer_verifications(next_poll_at)
    WHERE status IN ('pending', 'in_review');
CREATE INDEX payer_verifications_invoice
    ON payer_verifications(invoice_id, created_at DESC);
CREATE INDEX payer_verifications_session
    ON payer_verifications(payer_session_id);

CREATE TABLE payer_credentials (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    payer_ref BYTEA NOT NULL CHECK (octet_length(payer_ref) = 32),
    kind TEXT NOT NULL CHECK (kind IN ('document_liveness', 'matched_identity')),
    expected_identity_hash BYTEA CHECK (
        expected_identity_hash IS NULL OR octet_length(expected_identity_hash) = 32
    ),
    verified_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (kind = 'document_liveness' AND expected_identity_hash IS NULL)
        OR
        (kind = 'matched_identity' AND expected_identity_hash IS NOT NULL)
    )
);

CREATE UNIQUE INDEX payer_generic_credential
    ON payer_credentials(account_id, payer_ref, kind)
    WHERE kind = 'document_liveness';
CREATE UNIQUE INDEX payer_matched_credential
    ON payer_credentials(account_id, payer_ref, expected_identity_hash)
    WHERE kind = 'matched_identity';

-- ---------------------------------------------------------------------------
-- Manual review and identity webhook deduplication.
-- ---------------------------------------------------------------------------
CREATE TABLE verification_reviews (
    id UUID PRIMARY KEY,
    verification_id UUID NOT NULL REFERENCES payer_verifications(id),
    requested_by_account_id UUID REFERENCES accounts(id),
    decision TEXT CHECK (decision IS NULL OR decision IN ('approved', 'declined')),
    reviewer TEXT,
    note TEXT CHECK (note IS NULL OR octet_length(note) <= 2000),
    requested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    decided_at TIMESTAMPTZ,
    CHECK ((decision IS NULL) = (decided_at IS NULL))
);

CREATE INDEX verification_reviews_verification
    ON verification_reviews(verification_id, requested_at DESC);

CREATE TABLE identity_webhook_receipts (
    provider TEXT NOT NULL,
    event_id TEXT NOT NULL,
    provider_reference TEXT NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (provider, event_id)
);

-- ---------------------------------------------------------------------------
-- Recovery ledger: every amount the platform recovery wallet received on an
-- invoice's behalf, so manual review has a complete record.
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
-- Webhooks: verification and recovery events, and payer-safe payloads.
-- ---------------------------------------------------------------------------
ALTER TABLE webhook_events DROP CONSTRAINT webhook_events_event_type_check;
ALTER TABLE webhook_events ADD CONSTRAINT webhook_events_event_type_check CHECK (
    event_type IN (
        'payment.paid', 'payment.settled', 'payment.expired', 'payment.refunded',
        'payment.needs_attention', 'payment.likely_unsolicited',
        'payment.recovered_funds', 'verification.approved',
        'verification.declined', 'webhook.test'
    )
);

-- Lifecycle events happen once per invoice. Recovery events are one per
-- ledger row, and an invoice can recover funds more than once (an overpayment
-- at settlement, then a late transfer), so they are keyed by the ledger row
-- instead of the invoice.
DROP INDEX webhook_lifecycle_event_once;
CREATE UNIQUE INDEX webhook_lifecycle_event_once
    ON webhook_events(invoice_id, event_type)
    WHERE invoice_id IS NOT NULL AND fanout AND event_type <> 'payment.recovered_funds';

-- The public status vocabulary of the webhook envelope; `refunded` is kept
-- for the existing `payment.refunded` contract.
CREATE FUNCTION webhook_public_status(status TEXT, confirmed_received TEXT, blocked_reason TEXT)
RETURNS TEXT LANGUAGE SQL IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE
        WHEN blocked_reason IS NOT NULL OR status = 'blocked' THEN 'needs_attention'
        WHEN status = 'created' AND confirmed_received = '0' THEN 'awaiting_payment'
        WHEN status = 'created' THEN 'partially_paid'
        WHEN status IN ('funded', 'deploying') THEN 'paid'
        WHEN status = 'fulfilled' THEN 'settled'
        WHEN status = 'expired' THEN 'expired'
        WHEN status = 'recovered' THEN 'refunded'
    END
$$;

-- The payment object shared by every invoice event. It carries the policy
-- mode, verification completion, and unsolicited-funding timestamps only:
-- never the expected email, the expected identity, or any payer assertion.
CREATE FUNCTION webhook_payment_object(invoice invoices) RETURNS JSONB
LANGUAGE SQL STABLE AS $$
    SELECT jsonb_build_object(
        'id', invoice.id,
        'status', webhook_public_status(invoice.status, invoice.confirmed_received, invoice.blocked_reason),
        'amount', invoice.amount,
        'received', invoice.confirmed_received,
        'reference', invoice.reference,
        'metadata', invoice.metadata,
        'payer_policy_mode', invoice.payer_policy_mode,
        'verification_completed_at', invoice.verification_completed_at,
        'likely_unsolicited_at', invoice.likely_unsolicited_at)
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
    WHERE invoice_id IS NOT NULL AND fanout AND event_type <> 'payment.recovered_funds'
    DO NOTHING;
END $$;

CREATE OR REPLACE FUNCTION enqueue_payment_webhook() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE kind TEXT; occurred TIMESTAMPTZ; attention BOOLEAN; payment JSONB;
BEGIN
  payment := webhook_payment_object(NEW);
  attention := NEW.blocked_reason IS NOT NULL AND OLD.blocked_reason IS NULL;

  IF NEW.status IS DISTINCT FROM OLD.status OR attention THEN
    kind := CASE WHEN attention THEN 'payment.needs_attention' ELSE CASE NEW.status
      WHEN 'funded' THEN 'payment.paid' WHEN 'fulfilled' THEN 'payment.settled'
      WHEN 'expired' THEN 'payment.expired' WHEN 'recovered' THEN 'payment.refunded' END END;
    IF kind IS NOT NULL THEN
      occurred := CASE WHEN NEW.status = 'funded' THEN NEW.paid_at
                       WHEN NEW.status = 'expired' THEN NEW.expired_at
                       WHEN NEW.status IN ('fulfilled', 'recovered') THEN NEW.settled_at END;
      PERFORM enqueue_invoice_webhook_event(NEW.account_id, NEW.id, kind, occurred,
        jsonb_build_object('payment', payment ||
          CASE WHEN attention OR NEW.status = 'blocked' THEN jsonb_build_object('attention', jsonb_build_object(
            'reason_code', NEW.blocked_reason,
            'message', 'Automatic payout requires a manual review. Funds remain safe.',
            'action', 'Contact Payday support and provide the payment ID.'))
          ELSE '{}'::jsonb END));
    END IF;
  END IF;

  IF NEW.verification_completed_at IS NOT NULL AND OLD.verification_completed_at IS NULL THEN
    PERFORM enqueue_invoice_webhook_event(NEW.account_id, NEW.id, 'verification.approved',
      NEW.verification_completed_at, jsonb_build_object('payment', payment));
  END IF;

  IF NEW.likely_unsolicited_at IS NOT NULL AND OLD.likely_unsolicited_at IS NULL THEN
    PERFORM enqueue_invoice_webhook_event(NEW.account_id, NEW.id, 'payment.likely_unsolicited',
      NEW.likely_unsolicited_at, jsonb_build_object('payment', payment));
  END IF;

  RETURN NEW;
END $$;

DROP TRIGGER invoice_webhook_transition ON invoices;
CREATE TRIGGER invoice_webhook_transition
AFTER UPDATE OF status, blocked_reason, verification_completed_at, likely_unsolicited_at ON invoices
FOR EACH ROW EXECUTE FUNCTION enqueue_payment_webhook();

-- One event per ledger row, written in the transaction that records the recovery.
CREATE FUNCTION enqueue_recovered_funds_webhook() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE invoice invoices;
BEGIN
  SELECT * INTO invoice FROM invoices WHERE id = NEW.invoice_id;
  PERFORM enqueue_invoice_webhook_event(invoice.account_id, NEW.invoice_id, 'payment.recovered_funds',
    NEW.recovered_at,
    jsonb_build_object(
      'payment', webhook_payment_object(invoice),
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
