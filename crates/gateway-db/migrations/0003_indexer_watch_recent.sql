-- no-transaction
-- Serves the recently-touched branch of the indexer's watch list; see
-- 0002_indexer_watch_open.sql for why this is a single-statement partial
-- index built CONCURRENTLY outside a transaction and without
-- `IF NOT EXISTS`.
CREATE INDEX CONCURRENTLY invoices_watch_recent ON invoices (chain_id, token_address, updated_at)
    WHERE payment_address IS NOT NULL;
