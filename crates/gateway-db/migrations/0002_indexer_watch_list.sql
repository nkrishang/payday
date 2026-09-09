-- The indexer's transfer signal subscribes the node to USDC transfers whose
-- recipient is a payment address still worth watching: every open deposit
-- request, every address with uncollected funds, and every address touched
-- recently (a late transfer to a settled address is forwarded by the sweep
-- worker, so it earns a fast wake-up for a while). The list is fingerprinted
-- on a short timer, so its predicate must be served by partial indexes rather
-- than a scan of invoice history. `invoices_sweep_queue` (uncollected_count
-- > 0) already exists; these two cover the other branches.
CREATE INDEX invoices_watch_open ON invoices (chain_id, token_address)
    WHERE payment_address IS NOT NULL AND status NOT IN ('fulfilled', 'recovered');
CREATE INDEX invoices_watch_recent ON invoices (chain_id, token_address, updated_at)
    WHERE payment_address IS NOT NULL;
