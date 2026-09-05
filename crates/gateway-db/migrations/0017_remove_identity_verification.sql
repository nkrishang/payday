-- Identity verification is no longer part of the product. The payer policy
-- keeps two modes, permissionless and verified_email; the hosted document
-- and liveness check, its provider plumbing, credential reuse, and manual
-- review all go. Payday has no production data, so rows that only the
-- removed modes could have produced are deleted rather than preserved.

-- ---------------------------------------------------------------------------
-- Tables that existed only for the hosted identity check.
-- ---------------------------------------------------------------------------
DROP TABLE verification_reviews;
DROP TABLE payer_credentials;
DROP TABLE identity_webhook_receipts;

-- ---------------------------------------------------------------------------
-- Invoices: two policy modes, one assertion.
-- ---------------------------------------------------------------------------
DELETE FROM invoices
 WHERE payer_policy_mode IN ('verified_identity', 'verified_identity_unattributed');

ALTER TABLE invoices
    DROP CONSTRAINT invoices_payer_policy_mode_valid,
    DROP CONSTRAINT invoices_payer_policy_shape,
    DROP CONSTRAINT invoices_expected_identity_valid;

DROP TRIGGER invoice_issuance_immutable ON invoices;

ALTER TABLE invoices DROP COLUMN expected_identity;

DROP FUNCTION expected_identity_valid(JSONB);

ALTER TABLE invoices
    ADD CONSTRAINT invoices_payer_policy_mode_valid CHECK (
        payer_policy_mode IN ('permissionless', 'verified_email')
    ),
    ADD CONSTRAINT invoices_payer_policy_shape CHECK (
        (payer_policy_mode = 'permissionless' AND expected_email IS NULL)
        OR
        (payer_policy_mode = 'verified_email' AND expected_email IS NOT NULL)
    );

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
    payer_policy_mode, expected_email, issuance_snapshot,
    attribution_version, attribution_nonce, attribution_hash
ON invoices
FOR EACH ROW EXECUTE FUNCTION reject_invoice_issuance_mutation();

-- ---------------------------------------------------------------------------
-- Payer sessions establish one fact: the expected mailbox.
-- ---------------------------------------------------------------------------
ALTER TABLE payer_sessions
    DROP COLUMN document_verified_at,
    DROP COLUMN liveness_verified_at,
    DROP COLUMN identity_matched_at;

-- ---------------------------------------------------------------------------
-- Verification attempts are email codes: one kind, one provider, and the
-- statuses an email code can have. Everything the hosted session needed
-- (its reference, its facts, risk categories, resubmission count, and the
-- reconciler's polling state) goes.
-- ---------------------------------------------------------------------------
DELETE FROM payer_verifications WHERE kind <> 'email';

ALTER TABLE payer_verifications
    DROP CONSTRAINT payer_verifications_kind_check,
    DROP CONSTRAINT payer_verifications_status_check,
    DROP CONSTRAINT payer_verifications_provider_check,
    DROP CONSTRAINT payer_verifications_check,
    DROP COLUMN provider_reference,
    DROP COLUMN expected_identity_hash,
    DROP COLUMN document_status,
    DROP COLUMN liveness_status,
    DROP COLUMN identity_match_status,
    DROP COLUMN risk_codes,
    DROP COLUMN country_code,
    DROP COLUMN attempt_number,
    DROP COLUMN expires_at,
    DROP COLUMN next_poll_at,
    DROP COLUMN leased_until,
    DROP COLUMN poll_count,
    DROP COLUMN provider_failures;

ALTER TABLE payer_verifications
    ADD CONSTRAINT payer_verifications_kind_check CHECK (kind = 'email'),
    ADD CONSTRAINT payer_verifications_status_check CHECK (
        status IN ('pending', 'approved', 'abandoned')
    ),
    ADD CONSTRAINT payer_verifications_provider_check CHECK (provider = 'auth0');

-- ---------------------------------------------------------------------------
-- Webhooks: nothing declines a verification any more.
-- ---------------------------------------------------------------------------
DELETE FROM webhook_events WHERE event_type = 'verification.declined';

ALTER TABLE webhook_events DROP CONSTRAINT webhook_events_event_type_check;
ALTER TABLE webhook_events ADD CONSTRAINT webhook_events_event_type_check CHECK (
    event_type IN (
        'payment.paid', 'payment.settled', 'payment.expired', 'payment.refunded',
        'payment.needs_attention', 'payment.likely_unsolicited',
        'payment.recovered_funds', 'verification.approved', 'webhook.test'
    )
);
