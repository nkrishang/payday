-- Merchant sessions: the API-first payer mode. The merchant's own application
-- has authenticated the payer, names them by its own `payer_reference`, and
-- opens the hosted checkout for them with a single-use client secret minted
-- by the merchant API. Exchanging that secret is the verification: it mints
-- a payer session carrying the merchant-session fact and completes the
-- invoice's verification, exactly as a proven mailbox does for
-- `verified_email`. Payday has no production data, so the constraints are
-- simply replaced.

-- ---------------------------------------------------------------------------
-- Invoices: a third mode with its own assertion. `payer_reference` is the
-- merchant's identifier for the authenticated payer, opaque to Payday.
-- ---------------------------------------------------------------------------
ALTER TABLE invoices
    ADD COLUMN payer_reference TEXT CHECK (
        payer_reference IS NULL OR octet_length(payer_reference) BETWEEN 1 AND 128
    );

ALTER TABLE invoices
    DROP CONSTRAINT invoices_payer_policy_mode_valid,
    DROP CONSTRAINT invoices_payer_policy_shape;

ALTER TABLE invoices
    ADD CONSTRAINT invoices_payer_policy_mode_valid CHECK (
        payer_policy_mode IN ('permissionless', 'verified_email', 'merchant_session')
    ),
    ADD CONSTRAINT invoices_payer_policy_shape CHECK (
        (payer_policy_mode = 'permissionless'
            AND expected_email IS NULL AND payer_reference IS NULL)
        OR
        (payer_policy_mode = 'verified_email'
            AND expected_email IS NOT NULL AND payer_reference IS NULL)
        OR
        (payer_policy_mode = 'merchant_session'
            AND expected_email IS NULL AND payer_reference IS NOT NULL)
    );

-- The assertion is an issuance field: immutable like the rest.
DROP TRIGGER invoice_issuance_immutable ON invoices;

CREATE OR REPLACE FUNCTION reject_invoice_issuance_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
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
     OR NEW.payer_reference IS DISTINCT FROM OLD.payer_reference
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
    account_id, idempotency_key, customer_id, issuer_id, chain_id, factory_address,
    token_address, token_decimals, beneficiary_address, expiration_timestamp,
    expires_in_secs, expiration_intent, recovery_address, amount, net_amount,
    salt, payment_address, issuer, bill_to, notes, heading, memo, reference,
    payer_policy_mode, expected_email, payer_reference, issuance_snapshot,
    attribution_version, attribution_nonce, attribution_hash
ON invoices
FOR EACH ROW EXECUTE FUNCTION reject_invoice_issuance_mutation();

-- ---------------------------------------------------------------------------
-- Webhooks: the payment object gains the merchant's own payer reference, so
-- the app that opened the checkout can credit the right ledger straight from
-- `payment.paid` and `payment.settled`. It is the merchant's identifier,
-- not the payer's data; the expected email stays out as before.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION webhook_payment_object(invoice invoices) RETURNS JSONB
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
        'likely_unsolicited_at', invoice.likely_unsolicited_at)
$$;

-- ---------------------------------------------------------------------------
-- Payer sessions establish one more fact: the merchant's app opened them.
-- ---------------------------------------------------------------------------
ALTER TABLE payer_sessions ADD COLUMN merchant_session_verified_at TIMESTAMPTZ;

-- ---------------------------------------------------------------------------
-- Verification attempts: a merchant-session exchange is recorded as an
-- approved attempt of its own kind, by the merchant rather than a provider.
-- ---------------------------------------------------------------------------
ALTER TABLE payer_verifications
    DROP CONSTRAINT payer_verifications_kind_check,
    DROP CONSTRAINT payer_verifications_provider_check;

ALTER TABLE payer_verifications
    ADD CONSTRAINT payer_verifications_kind_check CHECK (
        kind IN ('email', 'merchant_session')
    ),
    ADD CONSTRAINT payer_verifications_provider_check CHECK (
        provider IN ('auth0', 'merchant')
    );

-- ---------------------------------------------------------------------------
-- Client secrets: minted by the merchant API for one invoice, returned once,
-- stored hashed, and spent by exactly one exchange. `used_at` and the
-- session it minted are set together.
-- ---------------------------------------------------------------------------
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
