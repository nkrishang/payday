-- Cross-chain payments through Relay (relay.link). The hosted checkout lets
-- a payer whose USDC sits on another chain pay a deposit request anyway:
-- gatewayd asks Relay for an exact-output quote whose recipient is the
-- payment address, the payer sends the quote's transactions from the wallet
-- they attested, and Relay's solver delivers USDC to the payment address on
-- the request's chain. One row per quote; the row is what lets the indexer
-- attribute the solver's transfer to the attested wallet instead of
-- flagging it as likely unsolicited.
--
--   quoted ─▶ sent ─▶ filled
--   quoted ─▶ expired          (never reported as sent)
--   sent   ─▶ failed | refunded
CREATE TABLE relay_intents (
    id UUID PRIMARY KEY,
    invoice_id UUID NOT NULL REFERENCES invoices(id),
    -- Relay's request id, bytes32.
    request_id BYTEA NOT NULL,
    origin_chain_id BIGINT NOT NULL,
    -- The invoice's chain, restated so the destination chain's worker can
    -- scan its own intents without joining invoices.
    destination_chain_id BIGINT NOT NULL,
    origin_currency BYTEA NOT NULL,
    -- The attested wallet the quote was made for: Relay's `user`.
    payer_wallet BYTEA NOT NULL,
    -- Base units, decimal: what the payer sends on the origin chain, and
    -- exactly what lands on the payment address (EXACT_OUTPUT).
    quoted_in_amount TEXT NOT NULL,
    quoted_out_amount TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'quoted',
    -- The origin transaction the page reported, then the one Relay reports.
    origin_tx_hash BYTEA,
    -- The destination transactions Relay reports for the fill.
    fill_tx_hashes BYTEA[] NOT NULL DEFAULT '{}',
    -- Relay's own last word on the request, for operators.
    relay_status TEXT,
    -- How the origin sender was established: `receipt` (the origin chain is
    -- one this deployment serves and the transfer was read from it) or
    -- `relay_api` (Relay's request record names the depositor).
    attribution_source TEXT,
    next_check_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    sent_at TIMESTAMPTZ,
    resolved_at TIMESTAMPTZ,
    -- A quote nobody sends is forgotten after this.
    expires_at TIMESTAMPTZ NOT NULL,

    CONSTRAINT relay_intents_request_id_unique UNIQUE (request_id),
    CONSTRAINT relay_intents_request_id_length CHECK (octet_length(request_id) = 32),
    CONSTRAINT relay_intents_origin_currency_length CHECK (octet_length(origin_currency) = 20),
    CONSTRAINT relay_intents_payer_wallet_length CHECK (octet_length(payer_wallet) = 20),
    CONSTRAINT relay_intents_amounts_numeric
        CHECK (quoted_in_amount ~ '^[0-9]+$' AND quoted_out_amount ~ '^[0-9]+$'),
    CONSTRAINT relay_intents_status_valid
        CHECK (status IN ('quoted', 'sent', 'filled', 'failed', 'refunded', 'expired')),
    CONSTRAINT relay_intents_origin_tx_hash_length
        CHECK (origin_tx_hash IS NULL OR octet_length(origin_tx_hash) = 32),
    CONSTRAINT relay_intents_attribution_source_valid
        CHECK (attribution_source IS NULL OR attribution_source IN ('receipt', 'relay_api'))
);

-- One origin transaction pays for one intent.
CREATE UNIQUE INDEX relay_intents_origin_tx
    ON relay_intents (origin_chain_id, origin_tx_hash)
    WHERE origin_tx_hash IS NOT NULL;
-- The crediting path's lookup: intents that may explain a solver's transfer.
CREATE INDEX relay_intents_by_invoice_open
    ON relay_intents (invoice_id)
    WHERE status IN ('sent', 'filled');
-- The destination worker's poll.
CREATE INDEX relay_intents_polling
    ON relay_intents (destination_chain_id, next_check_at)
    WHERE status = 'sent';
-- Forgetting unsent quotes.
CREATE INDEX relay_intents_quoted ON relay_intents (expires_at) WHERE status = 'quoted';
-- The page's view: the newest intent for its request.
CREATE INDEX relay_intents_by_invoice ON relay_intents (invoice_id, created_at DESC);

-- A transfer from a wallet other than the attested one, while an intent the
-- payer reported as sent is pending, is parked rather than flagged: the
-- poller either attributes it to the intent (relay_intent_id) or unparks it
-- into the ordinary likely-unsolicited path.
ALTER TABLE payment_observations
    ADD COLUMN relay_intent_id UUID REFERENCES relay_intents(id),
    ADD COLUMN relay_parked_at TIMESTAMPTZ;
CREATE INDEX payment_observations_relay_parked
    ON payment_observations (invoice_id)
    WHERE relay_parked_at IS NOT NULL;
