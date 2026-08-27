ALTER TABLE invoices ADD COLUMN expires_in_secs BIGINT;

UPDATE invoices
SET expires_in_secs = GREATEST(
    1,
    expiration_timestamp - extract(epoch FROM created_at)::BIGINT
);

ALTER TABLE invoices
    ALTER COLUMN expires_in_secs SET NOT NULL,
    ADD CONSTRAINT invoices_expires_in_secs_positive CHECK (expires_in_secs > 0);

CREATE INDEX invoices_account_created_id_desc
    ON invoices (account_id, created_at DESC, id DESC);
