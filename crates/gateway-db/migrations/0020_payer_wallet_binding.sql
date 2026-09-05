-- A deposit request no longer has a payment address at issuance. The payer
-- attests, in their session and once the request's policy is satisfied, the
-- wallet they will pay from (an EIP-712 signature). Only then are the
-- recovery term (that wallet), the salt (derived from the attribution hash
-- and the attestation's signing digest), and the CREATE3 address derived
-- and written, together, once. The platform recovery wallet is gone: excess
-- and late funds return to the payer's own wallet.
--
-- Numbered after both 0018 migrations (account wallet, merchant session),
-- which it extends: the merchant-session fields it re-declares below are
-- carried forward unchanged.
--
-- Pre-release reset: every existing request commits to the retired platform
-- recovery wallet through a nonce-derived salt that the new scheme cannot
-- re-derive, so those rows and everything hanging off them are deleted
-- rather than carried as unverifiable history (as 0017 did for the removed
-- identity modes). The few dependents without ON DELETE CASCADE go first.
DELETE FROM onboarding_demo_payments;
DELETE FROM notification_outbox;
DELETE FROM payment_observations;
DELETE FROM invoices;

-- ---------------------------------------------------------------------------
-- Invoices: the binding columns exist only once a payer wallet is bound.
-- ---------------------------------------------------------------------------
DROP TRIGGER invoice_issuance_immutable ON invoices;

ALTER TABLE invoices
    ALTER COLUMN recovery_address DROP NOT NULL,
    ALTER COLUMN salt DROP NOT NULL,
    ALTER COLUMN payment_address DROP NOT NULL,
    DROP COLUMN attribution_nonce,
    ALTER COLUMN issuance_snapshot SET NOT NULL,
    ALTER COLUMN attribution_version SET NOT NULL,
    ALTER COLUMN attribution_hash SET NOT NULL,
    ADD COLUMN payer_wallet BYTEA,
    ADD COLUMN payer_attestation JSONB,
    ADD COLUMN wallet_bound_at TIMESTAMPTZ;

ALTER TABLE invoices
    ADD CONSTRAINT invoices_payer_wallet_length
        CHECK (payer_wallet IS NULL OR octet_length(payer_wallet) = 20),
    -- All of the binding or none of it, and the recovery term is the wallet.
    ADD CONSTRAINT invoices_binding_complete CHECK (
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
    ADD CONSTRAINT invoices_funds_need_binding CHECK (
        payment_address IS NOT NULL
        OR (status IN ('created', 'expired') AND confirmed_received = '0' AND uncollected_count = 0)
    );

-- Issuance fields are immutable from the start; the binding fields are
-- immutable from the moment they are set, and are set all at once.
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
-- Payer sessions carry the one-time wallet challenge; verification attempts
-- record the wallet attestation next to the email code.
-- ---------------------------------------------------------------------------
ALTER TABLE payer_sessions
    ADD COLUMN wallet_nonce BYTEA CHECK (wallet_nonce IS NULL OR octet_length(wallet_nonce) = 32),
    ADD COLUMN wallet_nonce_expires_at TIMESTAMPTZ,
    ADD CONSTRAINT payer_sessions_wallet_challenge_complete
        CHECK ((wallet_nonce IS NULL) = (wallet_nonce_expires_at IS NULL));

ALTER TABLE payer_verifications
    DROP CONSTRAINT payer_verifications_kind_check,
    DROP CONSTRAINT payer_verifications_provider_check,
    ADD CONSTRAINT payer_verifications_kind_check CHECK (
        kind IN ('email', 'wallet', 'merchant_session')
    ),
    ADD CONSTRAINT payer_verifications_provider_check CHECK (
        provider IN ('auth0', 'payday', 'merchant')
    );

-- ---------------------------------------------------------------------------
-- Webhooks: the request becomes payable when its wallet is bound, and the
-- payment object names the wallet and the address.
-- ---------------------------------------------------------------------------
ALTER TABLE webhook_events DROP CONSTRAINT webhook_events_event_type_check;
ALTER TABLE webhook_events ADD CONSTRAINT webhook_events_event_type_check CHECK (
    event_type IN (
        'payment.paid', 'payment.settled', 'payment.expired', 'payment.refunded',
        'payment.needs_attention', 'payment.likely_unsolicited',
        'payment.recovered_funds', 'payment.ready', 'verification.approved', 'webhook.test'
    )
);

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
        'likely_unsolicited_at', invoice.likely_unsolicited_at,
        'payer_wallet', CASE WHEN invoice.payer_wallet IS NULL THEN NULL
                             ELSE '0x' || encode(invoice.payer_wallet, 'hex') END,
        'address', CASE WHEN invoice.payment_address IS NULL THEN NULL
                        ELSE '0x' || encode(invoice.payment_address, 'hex') END,
        'wallet_bound_at', invoice.wallet_bound_at)
$$;

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

  IF NEW.wallet_bound_at IS NOT NULL AND OLD.wallet_bound_at IS NULL THEN
    PERFORM enqueue_invoice_webhook_event(NEW.account_id, NEW.id, 'payment.ready',
      NEW.wallet_bound_at, jsonb_build_object('payment', payment));
  END IF;

  IF NEW.likely_unsolicited_at IS NOT NULL AND OLD.likely_unsolicited_at IS NULL THEN
    PERFORM enqueue_invoice_webhook_event(NEW.account_id, NEW.id, 'payment.likely_unsolicited',
      NEW.likely_unsolicited_at, jsonb_build_object('payment', payment));
  END IF;

  RETURN NEW;
END $$;

DROP TRIGGER invoice_webhook_transition ON invoices;
CREATE TRIGGER invoice_webhook_transition
AFTER UPDATE OF status, blocked_reason, verification_completed_at, wallet_bound_at, likely_unsolicited_at ON invoices
FOR EACH ROW EXECUTE FUNCTION enqueue_payment_webhook();
