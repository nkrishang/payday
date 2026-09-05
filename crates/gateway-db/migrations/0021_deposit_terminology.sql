-- Deposits and deposit requests are the product's two primitives. The API,
-- the SDK, the dashboard, and the docs now speak only of them; this
-- migration brings the webhook contract, which the database itself writes,
-- into the same vocabulary. Internal tables keep the invoice naming: that
-- terminology stops at the HTTP seam, and the schema is not that seam.
--
--   payment.paid               -> deposit_request.deposited
--   payment.settled            -> deposit_request.settled
--   payment.expired            -> deposit_request.expired
--   payment.refunded           -> deposit_request.returned
--   payment.needs_attention    -> deposit_request.needs_attention
--   payment.likely_unsolicited -> deposit_request.likely_unsolicited
--   payment.recovered_funds    -> deposit_request.recovered_funds
--   payment.ready              -> deposit_request.ready
--   data.payment               -> data.deposit_request
--
-- The public status vocabulary inside the envelope follows the API's:
-- awaiting_deposit, partially_deposited, deposited, settled, expired,
-- returned, needs_attention. `refunded`, which the envelope alone used for
-- what the API called `returned`, is gone.

-- ---------------------------------------------------------------------------
-- Existing rows: pre-release data, rewritten so the queue never carries two
-- vocabularies at once.
-- ---------------------------------------------------------------------------
ALTER TABLE webhook_events DROP CONSTRAINT webhook_events_event_type_check;
DROP INDEX webhook_lifecycle_event_once;

UPDATE webhook_events SET event_type = CASE event_type
    WHEN 'payment.paid' THEN 'deposit_request.deposited'
    WHEN 'payment.settled' THEN 'deposit_request.settled'
    WHEN 'payment.expired' THEN 'deposit_request.expired'
    WHEN 'payment.refunded' THEN 'deposit_request.returned'
    WHEN 'payment.needs_attention' THEN 'deposit_request.needs_attention'
    WHEN 'payment.likely_unsolicited' THEN 'deposit_request.likely_unsolicited'
    WHEN 'payment.recovered_funds' THEN 'deposit_request.recovered_funds'
    WHEN 'payment.ready' THEN 'deposit_request.ready'
    ELSE event_type END
WHERE event_type LIKE 'payment.%';

UPDATE webhook_events SET payload = jsonb_set(payload, '{type}', to_jsonb(event_type))
WHERE payload ? 'type';

UPDATE webhook_events
SET payload = jsonb_set(
    payload #- '{data,payment}',
    '{data,deposit_request}',
    payload #> '{data,payment}')
WHERE payload #> '{data,payment}' IS NOT NULL;

UPDATE webhook_events
SET payload = jsonb_set(payload, '{data,deposit_request,status}', to_jsonb(
    CASE payload #>> '{data,deposit_request,status}'
        WHEN 'awaiting_payment' THEN 'awaiting_deposit'
        WHEN 'partially_paid' THEN 'partially_deposited'
        WHEN 'paid' THEN 'deposited'
        WHEN 'refunded' THEN 'returned'
        ELSE payload #>> '{data,deposit_request,status}' END))
WHERE payload #>> '{data,deposit_request,status}' IS NOT NULL;

ALTER TABLE webhook_events ADD CONSTRAINT webhook_events_event_type_check CHECK (
    event_type IN (
        'deposit_request.deposited', 'deposit_request.settled', 'deposit_request.expired',
        'deposit_request.returned', 'deposit_request.needs_attention',
        'deposit_request.likely_unsolicited', 'deposit_request.recovered_funds',
        'deposit_request.ready', 'verification.approved', 'webhook.test'
    )
);

CREATE UNIQUE INDEX webhook_lifecycle_event_once
    ON webhook_events(invoice_id, event_type)
    WHERE invoice_id IS NOT NULL AND fanout AND event_type <> 'deposit_request.recovered_funds';

-- ---------------------------------------------------------------------------
-- The envelope's status vocabulary, now identical to the API's.
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION webhook_public_status(status TEXT, confirmed_received TEXT, blocked_reason TEXT)
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

-- The deposit request object shared by every event. Same fields as before:
-- the policy mode, the merchant's own payer reference, verification
-- completion, the unsolicited-funding timestamp, and the bound wallet and
-- address; never the expected email or any payer assertion.
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

CREATE OR REPLACE FUNCTION enqueue_invoice_webhook_event(
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

CREATE OR REPLACE FUNCTION enqueue_payment_webhook() RETURNS trigger LANGUAGE plpgsql AS $$
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

CREATE OR REPLACE FUNCTION enqueue_recovered_funds_webhook() RETURNS trigger LANGUAGE plpgsql AS $$
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

DROP FUNCTION webhook_payment_object(invoices);
