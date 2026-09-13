-- A relay step's transaction can revert for reasons no retry fixes only
-- sometimes: `receiveMessage` reverts while Circle's transmitter is paused,
-- a burn reverts under a transient token outage, and an authorization that
-- somebody else consumed makes every further attempt revert. Those legs'
-- obligations survive the revert (an unused authorization, an attestation
-- Circle has already signed), so the relayer puts the step back in its
-- queue after a backoff instead of failing the leg outright, and only a
-- run of reverts makes the failure permanent. The count and the earliest
-- next attempt travel with the leg.

ALTER TABLE withdrawal_legs
    ADD COLUMN step_reverts SMALLINT NOT NULL DEFAULT 0,
    ADD COLUMN step_retry_at TIMESTAMPTZ;
