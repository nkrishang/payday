-- Issuer identities and payout addresses: the merchant's own side of an
-- invoice, saved once instead of retyped per issuance.
--
-- An invoice still snapshots the party it was issued under and the address it
-- settles to, exactly as it snapshots the party it was billed to. These rows
-- are where a merchant keeps the values worth reusing, and where the gateway
-- records that the contact address on their invoices is a mailbox they hold —
-- payers are told to write to it, so it is proven with the same emailed code
-- the rest of the product uses, not merely typed.

-- ---------------------------------------------------------------------------
-- Issuer identities. `email_verified_at` is the proof, and clearing the
-- address clears the proof with it (enforced in the repository, which only
-- ever writes them together).
-- ---------------------------------------------------------------------------
CREATE TABLE issuers (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 255),
    contact_email TEXT NOT NULL CHECK (octet_length(contact_email) BETWEEN 3 AND 254),
    details TEXT CHECK (details IS NULL OR octet_length(details) <= 4000),
    email_verified_at TIMESTAMPTZ,
    -- When the last code went out, so a merchant cannot use this route to
    -- post a mailbox a code every second.
    email_code_sent_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, id)
);

CREATE INDEX issuers_account_created ON issuers(account_id, created_at DESC, id DESC);

-- ---------------------------------------------------------------------------
-- Payout addresses. Stored EIP-55 checksummed, which is also what makes the
-- per-account uniqueness meaningful.
-- ---------------------------------------------------------------------------
CREATE TABLE payout_addresses (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    address TEXT NOT NULL CHECK (address ~ '^0x[0-9a-fA-F]{40}$'),
    label TEXT CHECK (label IS NULL OR octet_length(label) BETWEEN 1 AND 120),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, id),
    UNIQUE (account_id, address)
);

CREATE INDEX payout_addresses_account_created
    ON payout_addresses(account_id, created_at DESC, id DESC);

-- ---------------------------------------------------------------------------
-- Which addresses an identity may settle to. Many-to-many: one company can
-- pay into several wallets, and one treasury wallet can serve several
-- identities. The composite foreign keys carry `account_id` into both sides,
-- so an association across two accounts cannot be written at all.
-- ---------------------------------------------------------------------------
CREATE TABLE issuer_payout_addresses (
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    issuer_id UUID NOT NULL,
    payout_address_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (issuer_id, payout_address_id),
    FOREIGN KEY (account_id, issuer_id) REFERENCES issuers(account_id, id) ON DELETE CASCADE,
    FOREIGN KEY (account_id, payout_address_id)
        REFERENCES payout_addresses(account_id, id) ON DELETE CASCADE
);

CREATE INDEX issuer_payout_addresses_by_address
    ON issuer_payout_addresses(payout_address_id);
