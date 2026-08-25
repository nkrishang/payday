ALTER TABLE payment_observations
    DROP CONSTRAINT payment_observations_amount_positive,
    ADD CONSTRAINT payment_observations_amount_numeric
        CHECK (amount ~ '^[0-9]+$'),
    ADD COLUMN disposition TEXT NOT NULL DEFAULT 'credited',
    ADD COLUMN disposition_reason TEXT,
    ADD CONSTRAINT payment_observations_disposition_valid
        CHECK (disposition IN ('credited', 'error', 'blocked')),
    ADD CONSTRAINT payment_observations_disposition_reason_valid
        CHECK (
            (disposition = 'credited' AND disposition_reason IS NULL)
            OR (disposition != 'credited' AND disposition_reason IS NOT NULL)
        );

CREATE INDEX payment_observations_manual_review
    ON payment_observations (observed_at)
    WHERE disposition IN ('error', 'blocked');
