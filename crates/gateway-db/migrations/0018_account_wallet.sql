-- The merchant's own wallet: the embedded EVM wallet Privy creates for every
-- account at sign-up, read from the identity token on each session and kept
-- here so the API can answer "where do this merchant's deposits settle by
-- default" without a round trip to Privy. Checksummed like payout addresses.
ALTER TABLE accounts ADD COLUMN wallet_address TEXT;
ALTER TABLE accounts ADD CONSTRAINT accounts_wallet_address_shape
    CHECK (wallet_address IS NULL OR wallet_address ~ '^0x[0-9a-fA-F]{40}$');
