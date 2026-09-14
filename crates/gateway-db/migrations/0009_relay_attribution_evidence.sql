-- Attribution evidence, tightening what `filled` means: Payday verified the
-- origin payment itself, from the origin chain's receipt, or it did not fill
-- the intent at all. Relay's record of a depositor and the page's reported
-- hash are hints for finding the evidence, never the evidence.
--
-- `origin_tx_hash` keeps the page's report (an unverified hint). The verified
-- origin transaction is its own column, and only it is unique: a reported
-- hash nobody verified must not squat on another payment's transaction.
ALTER TABLE relay_intents
    ADD COLUMN verified_origin_tx_hash BYTEA,
    ADD CONSTRAINT relay_intents_verified_origin_tx_hash_length
        CHECK (
            verified_origin_tx_hash IS NULL
            OR octet_length(verified_origin_tx_hash) = 32
        );

DROP INDEX relay_intents_origin_tx;

CREATE UNIQUE INDEX relay_intents_verified_origin_tx
    ON relay_intents (origin_chain_id, verified_origin_tx_hash)
    WHERE verified_origin_tx_hash IS NOT NULL;

-- A filled intent was verified through a receipt, or it is not filled.
-- Rows from before this migration were filled on Relay's word; withdraw
-- their attribution (balances and the sticky unsolicited flags stay) and
-- let them verify again or time out like any other sent intent.
UPDATE payment_observations o
SET relay_intent_id = NULL,
    relay_parked_at = COALESCE(o.relay_parked_at, now())
FROM relay_intents r
WHERE o.relay_intent_id = r.id
  AND r.status = 'filled';

UPDATE relay_intents
SET status = 'sent',
    attribution_source = NULL,
    verified_origin_tx_hash = NULL,
    resolved_at = NULL,
    next_check_at = now(),
    sent_at = COALESCE(sent_at, created_at)
WHERE status = 'filled';

ALTER TABLE relay_intents DROP CONSTRAINT relay_intents_attribution_source_valid;

ALTER TABLE relay_intents
    ADD CONSTRAINT relay_intents_verified_fill CHECK (
        (
            status = 'filled'
            AND attribution_source IS NOT DISTINCT FROM 'receipt'
            AND verified_origin_tx_hash IS NOT NULL
        )
        OR (
            status <> 'filled'
            AND attribution_source IS NULL
            AND verified_origin_tx_hash IS NULL
        )
    );

-- The reconciliation scan: unreported quotes whose exact amount somebody's
-- transfer may have already paid (the solver can land before the report).
CREATE INDEX relay_intents_reconciliation
    ON relay_intents (destination_chain_id, next_check_at, expires_at)
    WHERE status IN ('quoted', 'expired');
