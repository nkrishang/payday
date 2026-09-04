-- Which issuer identity a request was issued under.
--
-- The invoice already snapshots the issuer party, and that snapshot is what the
-- document says and what the attribution hash commits to — renaming an identity
-- must never change an invoice already issued. What was missing is the other
-- half: a durable handle for "this request belongs to that identity", which
-- survives the identity being renamed, moved to another mailbox, or pointed at
-- different wallets. It is the same shape the customer link has on the billed
-- side, and it is immutable for the same reason.
--
-- The foreign key takes no delete action, so an identity that history refers to
-- cannot simply vanish; the API refuses to delete one and says why.
ALTER TABLE invoices ADD COLUMN issuer_id UUID;

ALTER TABLE invoices ADD CONSTRAINT invoices_issuer_fk
    FOREIGN KEY (account_id, issuer_id) REFERENCES issuers(account_id, id);

CREATE INDEX invoices_account_issuer_created
    ON invoices(account_id, issuer_id, created_at DESC)
    WHERE issuer_id IS NOT NULL;

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

DROP TRIGGER invoice_issuance_immutable ON invoices;
CREATE TRIGGER invoice_issuance_immutable
BEFORE UPDATE OF
    account_id, idempotency_key, customer_id, issuer_id, chain_id, factory_address,
    token_address, token_decimals, beneficiary_address, expiration_timestamp,
    expires_in_secs, expiration_intent, recovery_address, amount, net_amount,
    salt, payment_address, issuer, bill_to, notes, heading, memo, reference,
    payer_policy_mode, expected_email, expected_identity, issuance_snapshot,
    attribution_version, attribution_nonce, attribution_hash
ON invoices
FOR EACH ROW EXECUTE FUNCTION reject_invoice_issuance_mutation();
