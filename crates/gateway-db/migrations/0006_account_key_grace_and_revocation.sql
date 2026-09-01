-- Allow one previous key during rotation and explicit immediate revocation.
ALTER TABLE accounts
    ALTER COLUMN api_key_hash DROP NOT NULL,
    ALTER COLUMN api_key_hint DROP NOT NULL,
    ADD COLUMN previous_api_key_hash BYTEA UNIQUE,
    ADD COLUMN previous_api_key_expires_at TIMESTAMPTZ,
    ADD COLUMN key_revoked_at TIMESTAMPTZ,
    ADD CONSTRAINT accounts_previous_api_key_hash_length
        CHECK (previous_api_key_hash IS NULL OR octet_length(previous_api_key_hash) = 32),
    ADD CONSTRAINT accounts_previous_key_complete
        CHECK ((previous_api_key_hash IS NULL) = (previous_api_key_expires_at IS NULL)),
    ADD CONSTRAINT accounts_current_key_complete
        CHECK ((api_key_hash IS NULL) = (api_key_hint IS NULL)),
    ADD CONSTRAINT accounts_revoked_key_state
        CHECK ((api_key_hash IS NULL) = (key_revoked_at IS NOT NULL));
