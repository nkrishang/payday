-- Tracks the one real, self-funded deposit request the onboarding walkthrough
-- issues per account (Payday billing itself, then paying itself, so a brand
-- new merchant sees the whole product work end to end before touching it).
--
-- One row per account, ever: the primary key on account_id both dedupes a
-- retried request for the same payment and bounds the feature to at most one
-- real on-chain transfer per account, however many times the endpoint is
-- called. tx_hash starts NULL and is filled in once the transfer actually
-- broadcasts, so a crash between claiming the row and sending the transfer
-- leaves a safely retryable state rather than a stuck one.
CREATE TABLE onboarding_demo_payments (
    account_id UUID PRIMARY KEY REFERENCES accounts(id),
    payment_id UUID NOT NULL REFERENCES invoices(id),
    tx_hash TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
