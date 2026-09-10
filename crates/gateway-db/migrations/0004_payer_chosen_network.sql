-- The payer chooses the network they pay on. A deposit request commits to
-- every supported network at issuance (in its canonical snapshot); the chain,
-- factory, and token become binding-time facts, written together with the
-- payer's wallet, the salt, and the address, and immutable from then on.
--
-- Pre-release cutover: rows issued under the single-chain schema (snapshot
-- `payday.invoice`, generation-1 addresses) are not carried forward. Wipe
-- them before applying this migration; the constraint below would refuse an
-- unbound row that still names a chain.

ALTER TABLE invoices
    ALTER COLUMN chain_id DROP NOT NULL,
    ALTER COLUMN factory_address DROP NOT NULL,
    ALTER COLUMN token_address DROP NOT NULL;

ALTER TABLE invoices DROP CONSTRAINT invoices_binding_complete;
ALTER TABLE invoices ADD CONSTRAINT invoices_binding_complete CHECK (
    (
        payer_wallet IS NULL AND payer_attestation IS NULL AND wallet_bound_at IS NULL
        AND recovery_address IS NULL AND salt IS NULL AND payment_address IS NULL
        AND chain_id IS NULL AND factory_address IS NULL AND token_address IS NULL
    )
    OR
    (
        payer_wallet IS NOT NULL AND payer_attestation IS NOT NULL AND wallet_bound_at IS NOT NULL
        AND recovery_address = payer_wallet AND salt IS NOT NULL AND payment_address IS NOT NULL
        AND chain_id IS NOT NULL AND factory_address IS NOT NULL AND token_address IS NOT NULL
    )
);

-- The three network columns leave the issuance group and join the binding
-- group: settable once, from NULL, together with the rest of the binding.
CREATE OR REPLACE FUNCTION reject_invoice_issuance_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.account_id IS DISTINCT FROM OLD.account_id
     OR NEW.idempotency_key IS DISTINCT FROM OLD.idempotency_key
     OR NEW.customer_id IS DISTINCT FROM OLD.customer_id
     OR NEW.issuer_id IS DISTINCT FROM OLD.issuer_id
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

-- An unbound request has no chain, so no chain's clock expires it through
-- `invoices_expiry_watch`; every chain's reconciler expires unbound requests
-- against its own finalized time through this index instead.
CREATE INDEX invoices_unbound_expiry ON invoices (expiration_timestamp)
    WHERE chain_id IS NULL AND status = 'created';

-- The wallet challenge is minted for one chain: the attestation is rebuilt
-- from the session's record when the signature comes back, so the payer
-- cannot sign under one chain's domain and bind another.
ALTER TABLE payer_sessions
    ADD COLUMN wallet_nonce_chain_id BIGINT,
    DROP CONSTRAINT payer_sessions_wallet_challenge_complete,
    ADD CONSTRAINT payer_sessions_wallet_challenge_complete CHECK (
        (wallet_nonce IS NULL) = (wallet_nonce_expires_at IS NULL)
        AND (wallet_nonce IS NULL) = (wallet_nonce_chain_id IS NULL)
    );
