-- The product is Gum. The wallet-attestation provider's value was the old
-- name, `payday`; this renames the recorded rows and admits only the new
-- value from now on. The provider is a display fact in the proof, not part
-- of any commitment hash, so rewriting it does not disturb verification.
--
-- The constraint's name is the one PostgreSQL derives from the inline CHECK
-- in 0001_application.sql; drop and recreate it rather than widening the
-- original migration, which existing databases have already applied.

UPDATE payer_verifications SET provider = 'gum' WHERE provider = 'payday';

ALTER TABLE payer_verifications
    DROP CONSTRAINT payer_verifications_provider_check;
ALTER TABLE payer_verifications
    ADD CONSTRAINT payer_verifications_provider_check
    CHECK (provider IN ('auth0', 'gum', 'merchant'));
