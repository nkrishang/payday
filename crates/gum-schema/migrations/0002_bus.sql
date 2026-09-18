-- The message bus: durable, at-least-once delivery between the services,
-- in Postgres. See crates/gum-bus for the client and docs/architecture.md
-- for why Postgres and not a broker.
--
-- A producer publishes a message *in the same transaction* as the domain
-- change that justifies it (the outbox); the insert fans out one delivery
-- row per subscribed consumer. A consumer claims a delivery with
-- `FOR UPDATE SKIP LOCKED`, holds a lease while it works, and acknowledges
-- it *in the same transaction* as its own state change (the inbox). A crash
-- anywhere in between leaves the lease to expire and the delivery to be
-- claimed again; that is why every handler is idempotent.

CREATE SCHEMA bus;

-- Static topology: who receives what. Rows are part of the schema because
-- the set of services is fixed; a new consumer is a new row here.
CREATE TABLE bus.subscriptions (
    consumer TEXT NOT NULL,
    topic TEXT NOT NULL,
    PRIMARY KEY (consumer, topic)
);

INSERT INTO bus.subscriptions (consumer, topic) VALUES
    ('gum-signers', 'execution.commands'),
    ('gum-signers', 'chain.control'),
    ('gum-server', 'execution.events');

-- Immutable log of everything published. `deduplication_key` is the
-- producer's idempotency key within the topic: publishing the same key
-- twice with the same payload is a no-op, with a different payload an
-- error (`payload_hash` is what tells them apart).
CREATE TABLE bus.messages (
    id UUID PRIMARY KEY,
    topic TEXT NOT NULL,
    kind TEXT NOT NULL,
    schema_version SMALLINT NOT NULL,
    payload JSONB NOT NULL,
    payload_hash BYTEA NOT NULL,
    producer TEXT NOT NULL,
    deduplication_key TEXT NOT NULL,
    correlation_id UUID NOT NULL,
    causation_id UUID,
    published_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT bus_messages_dedup UNIQUE (topic, deduplication_key),
    CONSTRAINT bus_messages_payload_hash_length CHECK (octet_length(payload_hash) = 32)
);

CREATE INDEX bus_messages_by_correlation ON bus.messages (correlation_id, published_at);
CREATE INDEX bus_messages_by_topic ON bus.messages (topic, published_at);

-- One row per (message, consumer). `pending` rows with `available_at <= now()`
-- and no live lease are claimable. `attempt_count` counts claims; each
-- failure pushes `available_at` out with exponential backoff and jitter, and
-- after the consumer's attempt limit the row is parked as `dead` for an
-- operator (`gum-server bus dead`, `gum-server bus retry <message>`).
CREATE TABLE bus.deliveries (
    message_id UUID NOT NULL REFERENCES bus.messages(id),
    consumer TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    attempt_count INTEGER NOT NULL DEFAULT 0,
    available_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_token UUID,
    leased_until TIMESTAMPTZ,
    last_error TEXT,
    last_attempt_at TIMESTAMPTZ,
    done_at TIMESTAMPTZ,

    PRIMARY KEY (message_id, consumer),
    CONSTRAINT bus_deliveries_state CHECK (state IN ('pending', 'done', 'dead')),
    CONSTRAINT bus_deliveries_lease_complete CHECK ((lease_token IS NULL) = (leased_until IS NULL))
);

-- The claim scan: pending rows of one consumer, oldest available first.
CREATE INDEX bus_deliveries_claimable
    ON bus.deliveries (consumer, available_at) WHERE state = 'pending';
CREATE INDEX bus_deliveries_dead ON bus.deliveries (consumer, last_attempt_at) WHERE state = 'dead';

-- Fan out a published message to every subscribed consumer, and wake any
-- consumer blocked in LISTEN. Doing this in a trigger means a producer can
-- never forget the fan-out, and that it commits with the message.
CREATE FUNCTION bus.fan_out() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO bus.deliveries (message_id, consumer)
    SELECT NEW.id, subscription.consumer
    FROM bus.subscriptions subscription
    WHERE subscription.topic = NEW.topic;
    PERFORM pg_notify('bus_' || replace(NEW.topic, '.', '_'), NEW.id::text);
    RETURN NEW;
END $$;

CREATE TRIGGER bus_messages_fan_out AFTER INSERT ON bus.messages
FOR EACH ROW EXECUTE FUNCTION bus.fan_out();
