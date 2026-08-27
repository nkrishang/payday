CREATE INDEX invoices_account_id_text_prefix
    ON invoices (account_id, (id::text) text_pattern_ops);

ALTER TABLE invoices ADD COLUMN settled_at TIMESTAMPTZ;
