-- Cancellation is customer intent, not an on-chain lifecycle transition. The
-- immutable Payment contract continues to use its encoded amount and expiry.
ALTER TABLE invoices ADD COLUMN cancellation_requested_at TIMESTAMPTZ;
ALTER TABLE invoices ADD COLUMN memo TEXT;
ALTER TABLE invoices ADD COLUMN expiration_intent TEXT;
UPDATE invoices SET expiration_intent = 'in:' || expires_in_secs::text;
ALTER TABLE invoices ALTER COLUMN expiration_intent SET NOT NULL;
ALTER TABLE invoices ADD CONSTRAINT invoices_expiration_intent_format
    CHECK (expiration_intent ~ '^(at|in):[0-9]+$');
ALTER TABLE invoices ADD CONSTRAINT invoices_memo_length
    CHECK (memo IS NULL OR octet_length(memo) <= 2000);
