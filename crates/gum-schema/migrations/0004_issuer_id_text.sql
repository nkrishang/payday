-- The deposit request's `issuer_id` becomes an opaque, merchant-supplied
-- string instead of a reference to a saved issuer identity, and the saved
-- identities feature (issuers, payout addresses, their association table)
-- and the onboarding demo payments are removed with it.
--
-- Existing public ids are serialized as `iss_<uuid>` in both the HTTP API
-- and webhook payloads (see `webhook_deposit_request_object`), so every
-- existing value is prefixed with `iss_` on the way to TEXT to keep the
-- wire form identical. New values are stored and returned verbatim.
--
-- The issuance-immutability trigger must be dropped for the ALTER: it is an
-- `UPDATE OF issuer_id` trigger, and PostgreSQL refuses to ALTER COLUMN TYPE
-- under one. It is recreated identically; `IS DISTINCT FROM` compares TEXT
-- exactly as it compared UUID.
--
-- Runs in the one transaction sqlx wraps every migration in.

DROP TRIGGER invoice_issuance_immutable ON invoices;

ALTER TABLE invoices DROP CONSTRAINT invoices_issuer_fk;
DROP INDEX invoices_account_issuer_created;

ALTER TABLE invoices
    ALTER COLUMN issuer_id TYPE TEXT
    USING CASE WHEN issuer_id IS NULL THEN NULL ELSE 'iss_' || issuer_id::text END;

ALTER TABLE invoices ADD CONSTRAINT invoices_issuer_id_length
    CHECK (issuer_id IS NULL OR octet_length(issuer_id) BETWEEN 1 AND 255);

CREATE INDEX invoices_account_issuer_created
    ON invoices(account_id, issuer_id, created_at DESC, id DESC)
    WHERE issuer_id IS NOT NULL;

CREATE OR REPLACE FUNCTION reject_invoice_issuance_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.account_id IS DISTINCT FROM OLD.account_id
     OR NEW.idempotency_key IS DISTINCT FROM OLD.idempotency_key
     OR NEW.customer_id IS DISTINCT FROM OLD.customer_id
     OR NEW.issuer_id IS DISTINCT FROM OLD.issuer_id
     OR NEW.currency IS DISTINCT FROM OLD.currency
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
     OR NEW.chain_id IS DISTINCT FROM OLD.chain_id
     OR NEW.factory_address IS DISTINCT FROM OLD.factory_address
     OR NEW.token_address IS DISTINCT FROM OLD.token_address
  ) THEN
    RAISE EXCEPTION 'invoice % payer wallet binding is immutable once set', OLD.id
      USING ERRCODE = 'integrity_constraint_violation';
  END IF;
  RETURN NEW;
END $$;

CREATE TRIGGER invoice_issuance_immutable
BEFORE UPDATE OF
    account_id, idempotency_key, customer_id, issuer_id, chain_id, factory_address,
    token_address, currency, token_decimals, beneficiary_address, expiration_timestamp,
    expires_in_secs, expiration_intent, recovery_address, amount, net_amount,
    salt, payment_address, issuer, bill_to, notes, heading, memo, reference,
    payer_policy_mode, expected_email, payer_reference, issuance_snapshot,
    attribution_version, attribution_hash,
    payer_wallet, payer_attestation, wallet_bound_at
ON invoices
FOR EACH ROW EXECUTE FUNCTION reject_invoice_issuance_mutation();

-- The webhook object now returns `issuer_id` exactly as stored: an existing
-- `iss_<uuid>` value is already prefixed by the migration above, and a new
-- merchant-supplied value must not be transformed.
CREATE OR REPLACE FUNCTION webhook_deposit_request_object(invoice invoices) RETURNS JSONB
LANGUAGE SQL STABLE AS $$
    SELECT jsonb_build_object(
        'id', 'dr_' || invoice.id::text,
        'status', webhook_public_status(invoice.status, invoice.confirmed_received, invoice.attention_reason),
        'currency', invoice.currency,
        'amount', webhook_decimal_amount(invoice.amount, invoice.token_decimals),
        'amount_base_units', invoice.amount,
        'received', webhook_decimal_amount(invoice.confirmed_received, invoice.token_decimals),
        'received_base_units', invoice.confirmed_received,
        'heading', invoice.heading,
        'reference', invoice.reference,
        'metadata', invoice.metadata,
        'customer_id', 'cus_' || invoice.customer_id::text,
        'issuer_id', invoice.issuer_id,
        'payer_policy_mode', invoice.payer_policy_mode,
        'payer_reference', invoice.payer_reference,
        'verification_completed_at', webhook_rfc3339(invoice.verification_completed_at),
        'likely_unsolicited_at', webhook_rfc3339(invoice.likely_unsolicited_at),
        'payer_wallet', CASE WHEN invoice.payer_wallet IS NULL THEN NULL
                             ELSE '0x' || encode(invoice.payer_wallet, 'hex') END,
        'address', CASE WHEN invoice.payment_address IS NULL THEN NULL
                        ELSE '0x' || encode(invoice.payment_address, 'hex') END,
        'chain_id', CASE
            WHEN invoice.chain_id IS NOT NULL THEN invoice.chain_id::text
            WHEN jsonb_array_length(invoice.issuance_snapshot->'networks') = 1
                THEN invoice.issuance_snapshot->'networks'->0->>'chain_id'
            ELSE NULL END,
        'wallet_bound_at', webhook_rfc3339(invoice.wallet_bound_at),
        'expires_at', webhook_rfc3339(to_timestamp(invoice.expiration_timestamp)),
        'created_at', webhook_rfc3339(invoice.created_at))
$$;

-- The saved-identity feature is gone. `invoices` no longer references
-- `issuers`; the association table goes first as the only remaining FK.
DROP TABLE issuer_payout_addresses;
DROP TABLE issuers;
DROP TABLE payout_addresses;

-- The onboarding demo flow is removed in the same release; the deployment
-- drains its jobs before applying this migration.
DROP TABLE onboarding_demo_payments;
