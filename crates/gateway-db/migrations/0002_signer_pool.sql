-- Signer pool: several funded keys per chain, one helper transaction in
-- flight per key. Every open batch and relay step records the address that
-- signed it, and the open-row uniqueness moves from "per chain" to "per
-- chain and signer".
--
-- A batch open while this migration runs gets the zero address; the worker
-- halts on it ("signed outside the pool") until an operator sets the column
-- to the address that signed it or resolves the row by hand. Deploy with no
-- open batch (docs/production-runbook.md) and nothing needs doing.

ALTER TABLE sweep_batches
    ADD COLUMN signer BYTEA NOT NULL DEFAULT '\x0000000000000000000000000000000000000000',
    ADD CONSTRAINT sweep_batches_signer_length CHECK (octet_length(signer) = 20);
ALTER TABLE sweep_batches ALTER COLUMN signer DROP DEFAULT;

DROP INDEX sweep_batches_open;
CREATE UNIQUE INDEX sweep_batches_open ON sweep_batches (chain_id, signer) WHERE resolved_at IS NULL;

ALTER TABLE withdrawal_legs ADD COLUMN step_signer BYTEA;
ALTER TABLE withdrawal_legs DROP CONSTRAINT withdrawal_legs_step_complete;
ALTER TABLE withdrawal_legs ADD CONSTRAINT withdrawal_legs_step_complete CHECK (
    (step_chain_id IS NULL) = (step_signer IS NULL)
    AND (step_signer IS NULL OR octet_length(step_signer) = 20)
    AND (step_chain_id IS NULL) = (step_nonce IS NULL)
    AND (step_nonce IS NULL) = (step_gas_limit IS NULL)
    AND (step_gas_limit IS NULL) = (step_max_fee_per_gas IS NULL)
    AND (step_max_fee_per_gas IS NULL) = (step_max_priority_fee_per_gas IS NULL)
    AND (step_max_priority_fee_per_gas IS NULL) = (step_tx_hashes IS NULL)
    AND (step_tx_hashes IS NULL) = (step_raw_transactions IS NULL)
    AND (step_raw_transactions IS NULL) = (step_submitted_at IS NULL));

DROP INDEX withdrawal_steps_open;
CREATE UNIQUE INDEX withdrawal_steps_open ON withdrawal_legs (step_chain_id, step_signer) WHERE step_chain_id IS NOT NULL;
