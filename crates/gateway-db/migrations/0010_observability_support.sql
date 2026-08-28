ALTER TABLE accounts ADD COLUMN email TEXT;
ALTER TABLE accounts ADD CONSTRAINT accounts_email_length
    CHECK (email IS NULL OR octet_length(email) BETWEEN 3 AND 254);

CREATE TABLE notification_outbox (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id),
    invoice_id UUID NOT NULL REFERENCES invoices(id),
    reason TEXT NOT NULL,
    email TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    delivered_at TIMESTAMPTZ,
    missing_email_reported_at TIMESTAMPTZ,
    leased_until TIMESTAMPTZ,
    last_error TEXT
);
CREATE INDEX notification_outbox_pending ON notification_outbox (next_attempt_at)
    WHERE (email IS NOT NULL AND delivered_at IS NULL)
       OR (email IS NULL AND missing_email_reported_at IS NULL);

CREATE TABLE api_status (
    chain_id BIGINT PRIMARY KEY,
    heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Preserve the existing lifecycle event contract while enriching the
-- needs-attention event with actionable, payer-safe support guidance.
CREATE OR REPLACE FUNCTION enqueue_payment_webhook() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE kind TEXT; public_status TEXT; occurred TIMESTAMPTZ; event_uuid UUID; attention BOOLEAN;
BEGIN
  attention := NEW.blocked_reason IS NOT NULL AND OLD.blocked_reason IS NULL;
  IF NEW.status IS NOT DISTINCT FROM OLD.status AND NOT attention THEN RETURN NEW; END IF;
  kind := CASE WHEN attention THEN 'payment.needs_attention' ELSE CASE NEW.status
    WHEN 'funded' THEN 'payment.paid' WHEN 'fulfilled' THEN 'payment.settled'
    WHEN 'expired' THEN 'payment.expired' WHEN 'recovered' THEN 'payment.refunded' END END;
  public_status := CASE WHEN attention THEN 'needs_attention' ELSE CASE NEW.status
    WHEN 'funded' THEN 'paid' WHEN 'fulfilled' THEN 'settled'
    WHEN 'expired' THEN 'expired' WHEN 'recovered' THEN 'refunded' END END;
  IF kind IS NULL THEN RETURN NEW; END IF;
  event_uuid := gen_random_uuid();
  occurred := COALESCE(CASE WHEN NEW.status='funded' THEN NEW.paid_at
                            WHEN NEW.status='expired' THEN NEW.expired_at
                            WHEN NEW.status IN ('fulfilled','recovered') THEN NEW.settled_at END,
                       now());
  INSERT INTO webhook_events(id, account_id, invoice_id, event_type, payload)
  VALUES (event_uuid, NEW.account_id, NEW.id, kind,
    jsonb_build_object('version','2026-08-01','id',event_uuid,'type',kind,
      'occurred_at',occurred,'data',jsonb_build_object('payment',
        jsonb_build_object('id',NEW.id,'status',public_status,'amount',NEW.amount,
          'received',NEW.confirmed_received,'reference',NEW.reference,'metadata',NEW.metadata) ||
        CASE WHEN NEW.status='blocked' THEN jsonb_build_object('attention',jsonb_build_object(
          'reason_code',NEW.blocked_reason,
          'message','Automatic payout requires a manual review. Funds remain safe.',
          'action','Contact Payday support and provide the payment ID.'))
        ELSE '{}'::jsonb END)))
  ON CONFLICT(invoice_id,event_type) WHERE invoice_id IS NOT NULL AND fanout DO NOTHING;
  RETURN NEW;
END $$;

DROP TRIGGER invoice_webhook_transition ON invoices;
CREATE TRIGGER invoice_webhook_transition AFTER UPDATE OF status, blocked_reason ON invoices
FOR EACH ROW EXECUTE FUNCTION enqueue_payment_webhook();
