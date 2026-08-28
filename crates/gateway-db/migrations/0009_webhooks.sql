-- Durable, account-scoped webhook delivery.
CREATE EXTENSION IF NOT EXISTS pgcrypto;

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
    event_type TEXT NOT NULL CHECK (event_type IN ('payment.paid','payment.settled','payment.expired','payment.refunded','payment.needs_attention','webhook.test')),
    payload JSONB NOT NULL,
    fanout BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- Lifecycle replay is idempotent, while test events neither need an invoice nor
-- consume the lifecycle uniqueness slot.
CREATE UNIQUE INDEX webhook_lifecycle_event_once
    ON webhook_events(invoice_id, event_type) WHERE invoice_id IS NOT NULL AND fanout;

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

CREATE FUNCTION enqueue_payment_webhook() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE kind TEXT; public_status TEXT; occurred TIMESTAMPTZ; event_uuid UUID;
BEGIN
  IF NEW.status IS NOT DISTINCT FROM OLD.status THEN RETURN NEW; END IF;
  kind := CASE NEW.status WHEN 'funded' THEN 'payment.paid' WHEN 'fulfilled' THEN 'payment.settled'
    WHEN 'expired' THEN 'payment.expired' WHEN 'recovered' THEN 'payment.refunded'
    WHEN 'blocked' THEN 'payment.needs_attention' END;
  public_status := CASE NEW.status WHEN 'funded' THEN 'paid' WHEN 'fulfilled' THEN 'settled'
    WHEN 'expired' THEN 'expired' WHEN 'recovered' THEN 'refunded'
    WHEN 'blocked' THEN 'needs_attention' END;
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
          'received',NEW.confirmed_received,'reference',NEW.reference,'metadata',NEW.metadata))))
  ON CONFLICT(invoice_id,event_type) WHERE invoice_id IS NOT NULL AND fanout DO NOTHING;
  RETURN NEW;
END $$;
CREATE TRIGGER invoice_webhook_transition AFTER UPDATE OF status ON invoices
FOR EACH ROW EXECUTE FUNCTION enqueue_payment_webhook();

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
