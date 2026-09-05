# Payday implementation guide

> **Superseded on 2026-09-05.** Identity verification is no longer part of
> the product. Everything below about `verified_identity`,
> `verified_identity_unattributed`, `expected_identity`, KYC, Didit, the
> `PayerIdentityProvider` boundary, credential reuse, the reconciler, manual
> review, and the identity step of the checkout describes a design that was
> built and then removed. The payer policy has two modes, `permissionless`
> and `verified_email`. Those sections are kept for the record; treat them
> as history, not as the current specification.

This guide implements Slices 1–4 from `features/product-plan.md`. Slices 0, 5, and 6 are prerequisites or future work and are not expanded here.

Line references describe the repository before these changes.

## Cross-slice decisions

1. Add one consolidated migration: `crates/gateway-db/migrations/0011_verified_customer_funds.sql`. Keep editing that migration while Slices 1–4 are under development. Because Payday has no production data, reset development databases as needed and squash `0011` into the baseline before the first production release.
2. Keep `/v1/payments` as the compatibility API path, but use **invoice** for new domain, dashboard, CLI, and documentation terminology.
3. Use S3 presigned PUTs and GuardDuty Malware Protection for S3. Only the managed tag `GuardDutyMalwareScanStatus=NO_THREATS_FOUND` permits finalization.
4. Payer sessions use opaque bearer tokens in the `Payday-Payer-Session` header. Do not use query-string tokens or third-party cookies.
5. The current Didit V3 API accepts `expected_details.first_name` and `expected_details.last_name`; it does not document `legal_name` or `expected_details_mismatch_action`. Therefore the implementation uses:
   ```rust
   pub struct ExpectedIdentity {
       pub first_name: String,
       pub last_name: String,
   }
   ```
   Configure the Didit workflow itself to decline expected-name mismatches. Do not send an unsupported `expected_details_mismatch_action` field.
6. Never log attachment bytes, OTPs, payer-session tokens, identity webhook bodies, Didit decisions, or vendor-extracted identity.

---

# Slice 1 — Contract and consolidated data foundation

**Scope:** L  
**Result:** exact-amount settlement, platform recovery, final schema, recovery ledger, deployment validation.

## 1. Change contract settlement first

Modify `foundry/src/Payment.sol`. Preserve the constructor and `recover()` signatures:

```solidity
constructor(
    address token_,
    uint256 amount,
    address receiver,
    uint64 expirationTimestamp,
    address recovery_
)
```

Replace the live-settlement branch with:

```solidity
uint256 balance = ERC20(token_).balanceOf(address(this));
if (expired) {
    SafeTransferLib.safeTransfer({
        token: token_,
        to: recovery_,
        amount: balance
    });
    emit Recovered(recovery_, balance);
    return;
}

if (balance < amount) {
    revert InsufficientTokenBalance(balance, amount);
}

SafeTransferLib.safeTransfer({
    token: token_,
    to: receiver,
    amount: amount
});
emit Settled(receiver, amount);

uint256 remainder = balance - amount;
if (remainder != 0) {
    SafeTransferLib.safeTransfer({
        token: token_,
        to: recovery_,
        amount: remainder
    });
    emit Recovered(recovery_, remainder);
}
```

Do not change the interfaces in `PaymentFactory.sol` or `BatchSweeper.sol`. `Payment.creationCode` changes, so both contracts still require redeployment.

Update `foundry/README.md` to describe exact settlement and recovery custody.

## 2. Update Solidity tests

Modify `foundry/test/Payment.t.sol`:

- Replace `test_payment_usdc` with:
  - `test_exact_funding_settles_invoice_amount`
  - `test_overpayment_splits_receiver_and_recovery`
  - `test_underfunded_live_deployment_reverts`
  - `test_zero_remainder_emits_no_recovered_event`
  - `test_overpayment_reverts_atomically_when_recovery_transfer_fails`
- Retain:
  - `test_expired_payment_recovers_all_funds`
  - `test_expiration_boundary_still_pays_beneficiary`
  - later `recover()` tests.

For overpayment, assert:

```solidity
vm.expectEmit(true, true, true, true, paymentAddress);
emit Settled(RECEIVER, 10e6);
vm.expectEmit(true, true, true, true, paymentAddress);
emit Recovered(RECOVERY, 2e6);

assertEq(token.balanceOf(RECEIVER), 10e6);
assertEq(token.balanceOf(RECOVERY), 2e6);
assertEq(token.balanceOf(paymentAddress), 0);
```

Modify `foundry/test/BatchSweeper.t.sol`:

- The first receiver must receive `10e6`, not `11e6`.
- Recovery must receive the `1e6` remainder.
- Add `test_overpayment_recovery_failure_does_not_rollback_sibling_sweeps`.
- Retain the full-batch gas-budget test; increase `SWEEP_GAS_PER_ITEM` in both Solidity tests and Rust only if the measured budget no longer has the existing 40% headroom.

Keep `PaymentAddress.t.sol` unchanged as the Rust/Solidity address-parity contract.

## 3. Preserve both settlement events in the indexer

The current receipt parser stores one `SweepOutcome` per address and would discard the overpayment `Recovered` amount after seeing `Settled`.

Modify `crates/gateway-indexer/src/chain.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SweepOutcome {
    Settled {
        amount: U256,
        recovered_amount: U256,
    },
    Recovered {
        amount: U256,
    },
    Collected {
        amount: U256,
    },
    Failed {
        revert_data: Bytes,
    },
}
```

Refactor event parsing so event order does not matter:

- `Settled` initializes or updates `amount`.
- `Recovered` updates `recovered_amount` when the same address also settled.
- A standalone `Recovered` remains `SweepOutcome::Recovered`.
- Reject duplicate/conflicting `Settled` or `Recovered` events as malformed receipts.

Modify `gateway-db/src/sweeps.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryReason {
    Overpayment,
    Expired,
    LateTransfer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredFundsInput {
    pub amount: U256,
    pub reason: RecoveryReason,
}

pub enum InvoiceOutcome {
    Drained {
        status: Option<InvoiceStatus>,
        execute_tx: bool,
        settlement_tx_hash: B256,
        settlement_block: u64,
        settlement_timestamp: u64,
        recovered: Option<RecoveredFundsInput>,
    },
    Retry,
    Blocked { reason: String },
}
```

Update `Indexer::classify`:

- `Settled { recovered_amount, .. }` creates `Overpayment` when nonzero.
- Fresh expired deployment creates `Expired`.
- `Collected` with nonzero amount creates `LateTransfer`.
- Zero recovery does not create a ledger row.

Update `InvoiceRepository::finalize_batch()` to insert recovery ledger rows in the same transaction that resolves the invoice.

## 4. Add the consolidated schema

Create `crates/gateway-db/migrations/0011_verified_customer_funds.sql`.

The final migration must contain these principal definitions:

```sql
CREATE TABLE customers (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 255),
    email TEXT CHECK (email IS NULL OR octet_length(email) BETWEEN 3 AND 254),
    details TEXT CHECK (details IS NULL OR octet_length(details) <= 4000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, id)
);

CREATE INDEX customers_account_created
    ON customers(account_id, created_at DESC, id DESC);

ALTER TABLE invoices
    ADD COLUMN customer_id UUID REFERENCES customers(id),
    ADD COLUMN issuer JSONB,
    ADD COLUMN bill_to JSONB,
    ADD COLUMN notes TEXT,
    ADD COLUMN heading TEXT,
    ADD COLUMN payer_policy_mode TEXT NOT NULL DEFAULT 'permissionless',
    ADD COLUMN expected_email TEXT,
    ADD COLUMN expected_identity JSONB,
    ADD COLUMN verification_completed_at TIMESTAMPTZ,
    ADD COLUMN issuance_snapshot JSONB,
    ADD COLUMN attribution_version SMALLINT,
    ADD COLUMN attribution_nonce BYTEA,
    ADD COLUMN attribution_hash BYTEA,
    ADD COLUMN likely_unsolicited_at TIMESTAMPTZ;

ALTER TABLE invoices
    ADD CONSTRAINT invoices_payer_policy_mode_valid CHECK (
        payer_policy_mode IN (
            'permissionless',
            'verified_email',
            'verified_identity',
            'verified_identity_unattributed'
        )
    ),
    ADD CONSTRAINT invoices_payer_policy_shape CHECK (
        (payer_policy_mode = 'permissionless'
            AND expected_email IS NULL AND expected_identity IS NULL)
        OR
        (payer_policy_mode = 'verified_email'
            AND expected_email IS NOT NULL AND expected_identity IS NULL)
        OR
        (payer_policy_mode = 'verified_identity'
            AND expected_email IS NOT NULL AND expected_identity IS NOT NULL)
        OR
        (payer_policy_mode = 'verified_identity_unattributed'
            AND expected_email IS NOT NULL AND expected_identity IS NULL)
    ),
    ADD CONSTRAINT invoices_expected_email_length CHECK (
        expected_email IS NULL OR octet_length(expected_email) BETWEEN 3 AND 254
    ),
    ADD CONSTRAINT invoices_attribution_complete CHECK (
        (attribution_version IS NULL
            AND attribution_nonce IS NULL
            AND attribution_hash IS NULL
            AND issuance_snapshot IS NULL)
        OR
        (attribution_version = 1
            AND octet_length(attribution_nonce) = 32
            AND octet_length(attribution_hash) = 32
            AND jsonb_typeof(issuance_snapshot) = 'object')
    ),
    ADD CONSTRAINT invoices_notes_length CHECK (
        notes IS NULL OR octet_length(notes) <= 4000
    ),
    ADD CONSTRAINT invoices_heading_length CHECK (
        heading IS NULL OR octet_length(heading) <= 200
    );

CREATE INDEX invoices_customer_created
    ON invoices(account_id, customer_id, created_at DESC)
    WHERE customer_id IS NOT NULL;
CREATE INDEX invoices_verification_pending
    ON invoices(account_id, payer_policy_mode, expiration_timestamp)
    WHERE verification_completed_at IS NULL
      AND payer_policy_mode <> 'permissionless';
CREATE INDEX invoices_likely_unsolicited
    ON invoices(account_id, likely_unsolicited_at DESC)
    WHERE likely_unsolicited_at IS NOT NULL;
```

Use an immutable SQL helper, following the pattern in `0008_section_d_api_foundation.sql`, to validate party objects and expected identity:

```sql
CREATE FUNCTION invoice_party_valid(value JSONB) RETURNS BOOLEAN
LANGUAGE SQL IMMUTABLE PARALLEL SAFE
AS $$
    SELECT value IS NOT NULL
       AND jsonb_typeof(value) = 'object'
       AND jsonb_typeof(value->'name') = 'string'
       AND octet_length(value->>'name') BETWEEN 1 AND 255
       AND (NOT value ? 'email'
            OR value->'email' = 'null'::jsonb
            OR (jsonb_typeof(value->'email') = 'string'
                AND octet_length(value->>'email') BETWEEN 3 AND 254))
       AND (NOT value ? 'details'
            OR value->'details' = 'null'::jsonb
            OR (jsonb_typeof(value->'details') = 'string'
                AND octet_length(value->>'details') <= 4000))
$$;

CREATE FUNCTION expected_identity_valid(value JSONB) RETURNS BOOLEAN
LANGUAGE SQL IMMUTABLE PARALLEL SAFE
AS $$
    SELECT value IS NOT NULL
       AND jsonb_typeof(value) = 'object'
       AND jsonb_typeof(value->'first_name') = 'string'
       AND jsonb_typeof(value->'last_name') = 'string'
       AND octet_length(value->>'first_name') BETWEEN 1 AND 255
       AND octet_length(value->>'last_name') BETWEEN 1 AND 255
$$;

ALTER TABLE invoices
    ADD CONSTRAINT invoices_issuer_valid
        CHECK (issuer IS NULL OR invoice_party_valid(issuer)),
    ADD CONSTRAINT invoices_bill_to_valid
        CHECK (bill_to IS NULL OR invoice_party_valid(bill_to)),
    ADD CONSTRAINT invoices_expected_identity_valid
        CHECK (expected_identity IS NULL OR expected_identity_valid(expected_identity));
```

The invoice fields remain nullable only while Slice 1 and Slice 2 are developed separately. Before release, reset pre-production data and change `issuer`, `bill_to`, `issuance_snapshot`, and all attribution columns to `NOT NULL`.

Add attachment staging and issuance:

```sql
CREATE TABLE invoice_attachments (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    invoice_id UUID UNIQUE REFERENCES invoices(id) ON DELETE CASCADE,
    object_key TEXT NOT NULL UNIQUE,
    original_filename TEXT NOT NULL CHECK (
        octet_length(original_filename) BETWEEN 1 AND 255
    ),
    mime_type TEXT NOT NULL CHECK (mime_type = 'application/pdf'),
    byte_length BIGINT CHECK (
        byte_length IS NULL OR byte_length BETWEEN 1 AND 5242880
    ),
    sha256 BYTEA CHECK (sha256 IS NULL OR octet_length(sha256) = 32),
    status TEXT NOT NULL CHECK (
        status IN ('pending_upload', 'scanning', 'ready', 'rejected', 'attached')
    ),
    scan_result TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finalized_at TIMESTAMPTZ,
    attached_at TIMESTAMPTZ,
    CHECK (
        (status IN ('pending_upload', 'scanning')
            AND byte_length IS NULL AND sha256 IS NULL)
        OR
        (status IN ('ready', 'attached')
            AND byte_length IS NOT NULL
            AND sha256 IS NOT NULL
            AND scan_result = 'NO_THREATS_FOUND')
        OR status = 'rejected'
    )
);

CREATE INDEX invoice_attachments_stale
    ON invoice_attachments(created_at)
    WHERE invoice_id IS NULL;
```

Add payer state:

```sql
CREATE TABLE payer_sessions (
    id UUID PRIMARY KEY,
    token_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    payer_ref BYTEA CHECK (payer_ref IS NULL OR octet_length(payer_ref) = 32),
    email_verified_at TIMESTAMPTZ,
    document_verified_at TIMESTAMPTZ,
    liveness_verified_at TIMESTAMPTZ,
    identity_matched_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    CHECK (expires_at > created_at)
);

CREATE INDEX payer_sessions_invoice ON payer_sessions(invoice_id);
CREATE INDEX payer_sessions_expiry ON payer_sessions(expires_at);

CREATE TABLE payer_verifications (
    id UUID PRIMARY KEY,
    invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    payer_session_id UUID NOT NULL REFERENCES payer_sessions(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('email', 'identity')),
    status TEXT NOT NULL CHECK (
        status IN (
            'pending', 'approved', 'declined', 'in_review',
            'expired', 'abandoned', 'review_required'
        )
    ),
    provider TEXT NOT NULL CHECK (provider IN ('auth0', 'didit', 'manual')),
    provider_reference TEXT,
    provider_event_id TEXT,
    payer_ref BYTEA CHECK (payer_ref IS NULL OR octet_length(payer_ref) = 32),
    expected_identity_hash BYTEA CHECK (
        expected_identity_hash IS NULL OR octet_length(expected_identity_hash) = 32
    ),
    document_status TEXT,
    liveness_status TEXT,
    identity_match_status TEXT,
    risk_codes TEXT[] NOT NULL DEFAULT '{}',
    country_code TEXT CHECK (
        country_code IS NULL OR country_code ~ '^[A-Z]{2,3}$'
    ),
    attempt_number SMALLINT NOT NULL DEFAULT 1 CHECK (attempt_number BETWEEN 1 AND 2),
    verified_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    next_poll_at TIMESTAMPTZ,
    leased_until TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX payer_verifications_provider_reference
    ON payer_verifications(provider, provider_reference)
    WHERE provider_reference IS NOT NULL;
CREATE INDEX payer_verifications_poll
    ON payer_verifications(next_poll_at)
    WHERE status IN ('pending', 'in_review');

CREATE TABLE payer_credentials (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    payer_ref BYTEA NOT NULL CHECK (octet_length(payer_ref) = 32),
    kind TEXT NOT NULL CHECK (kind IN ('document_liveness', 'matched_identity')),
    status TEXT NOT NULL CHECK (status IN ('active', 'revoked', 'expired')),
    expected_identity_hash BYTEA CHECK (
        expected_identity_hash IS NULL OR octet_length(expected_identity_hash) = 32
    ),
    verified_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (kind = 'document_liveness' AND expected_identity_hash IS NULL)
        OR
        (kind = 'matched_identity' AND expected_identity_hash IS NOT NULL)
    )
);

CREATE UNIQUE INDEX payer_generic_credential
    ON payer_credentials(account_id, payer_ref, kind)
    WHERE kind = 'document_liveness' AND status = 'active';
CREATE UNIQUE INDEX payer_matched_credential
    ON payer_credentials(account_id, payer_ref, expected_identity_hash)
    WHERE kind = 'matched_identity' AND status = 'active';
```

Add manual review and webhook deduplication:

```sql
CREATE TABLE verification_reviews (
    id UUID PRIMARY KEY,
    verification_id UUID NOT NULL REFERENCES payer_verifications(id),
    requested_by_account_id UUID REFERENCES accounts(id),
    decision TEXT CHECK (decision IS NULL OR decision IN ('approved', 'declined')),
    reviewer TEXT,
    note TEXT CHECK (note IS NULL OR octet_length(note) <= 2000),
    requested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    decided_at TIMESTAMPTZ,
    CHECK ((decision IS NULL) = (decided_at IS NULL))
);

CREATE TABLE identity_webhook_receipts (
    provider TEXT NOT NULL,
    event_id TEXT NOT NULL,
    provider_reference TEXT NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (provider, event_id)
);
```

Add the recovery ledger:

```sql
CREATE TABLE recovered_funds (
    id UUID PRIMARY KEY,
    invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    chain_id BIGINT NOT NULL,
    token_address BYTEA NOT NULL CHECK (octet_length(token_address) = 20),
    transaction_hash BYTEA NOT NULL CHECK (octet_length(transaction_hash) = 32),
    block_number BIGINT NOT NULL CHECK (block_number >= 0),
    amount TEXT NOT NULL CHECK (amount ~ '^[0-9]+$' AND amount <> '0'),
    reason TEXT NOT NULL CHECK (
        reason IN ('overpayment', 'expired', 'late_transfer')
    ),
    recovered_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(invoice_id, transaction_hash, reason)
);
```

Extend the `webhook_events.event_type` CHECK with:

```text
verification.approved
verification.declined
payment.likely_unsolicited
payment.recovered_funds
```

Update `enqueue_payment_webhook()` from migrations `0009`/`0010` to include only policy mode, verification timestamps, and unsolicited/recovery flags. Never include expected email or expected identity.

Add an immutable-issuance trigger preventing updates to chain parameters, amount, parties, policy, attachment commitment, salt, or attribution fields after insert.

## 5. Remove merchant-controlled recovery

Modify:

- `gatewayd/src/config.rs`: require `PAYDAY_RECOVERY_ADDRESS`.
- `gatewayd/src/state.rs`: add `recovery_address: Address`.
- `gatewayd/src/api/invoices.rs`:
  - remove all `refund_address` parsing;
  - construct `Invoice` with `RecoveryAddress(state.recovery_address)`;
  - compare idempotent requests against configured recovery.
- `gateway-core/src/dto.rs`: remove `CreatePaymentRequest.refund_address`.
- Rename merchant response `refund_address` to `recovery_address`; it remains visible to merchants but not payers.
- `sdk/typescript/src/index.ts`: remove `CreatePayment.refund_address`; rename the response field.
- `gateway-cli/src/cli.rs`: remove `--refund-to`.
- `gateway-cli/src/main.rs`: stop validating or sending recovery.
- Change checkout copy in `checkout-state.ts` from "merchant refund address" to "Payday recovery wallet; contact the merchant and Payday support for return handling."

## 6. Validate deployed contract generations

The indexer already checks `BatchSweeper.factory()` in `main.rs`. Extend this:

- Add `ChainClient::code_hash(address) -> Result<B256, ChainError>`.
- Require:
  - `PAYDAY_FACTORY_CODE_HASH`
  - `PAYDAY_BATCH_SWEEPER_CODE_HASH`
- At startup, compare both deployed runtime bytecode hashes and verify `BatchSweeper.factory()`.

Create `crates/gatewayd/src/deployment.rs` with:

```rust
pub struct ExpectedDeployment {
    pub chain_id: u64,
    pub factory: Address,
    pub factory_code_hash: B256,
    pub batch_sweeper: Address,
    pub batch_sweeper_code_hash: B256,
}

pub async fn verify_deployment(
    rpc_url: &str,
    expected: &ExpectedDeployment,
) -> Result<(), DeploymentError>;
```

Call it in `gatewayd/src/main.rs` before constructing `AppState`.

Update:

- `foundry/script/PaymentFactory.s.sol`
- `foundry/script/Bootstrap.s.sol`
- `scripts/local-runner.sh`
- `scripts/e2e-anvil.sh`
- `.env.example`
- `infra/main.tf`, `infra/variables.tf`, `infra/outputs.tf`
- deployment and stuck-invoice runbooks.

Deploy factory and sweeper together. Never point the new factory at an old sweeper.

## Slice 1 tests

Add or modify:

- `foundry/test/Payment.t.sol`: tests listed above.
- `foundry/test/BatchSweeper.t.sol`: split and failure isolation.
- `gateway-indexer/src/chain.rs`:
  - `receipt_combines_settled_and_recovered_events`
  - `receipt_rejects_conflicting_duplicate_events`
- `gateway-indexer/src/indexer.rs`:
  - `overpayment_records_only_remainder_as_recovered`
  - `expired_sweep_records_full_recovered_balance`
  - `late_collection_appends_recovery_ledger_entry`
- `gateway-db/src/sweeps.rs`:
  - `finalize_batch_writes_recovery_ledger_atomically`
  - `replayed_receipt_does_not_duplicate_recovery`
- `gatewayd/src/api/routes.rs`:
  - `create_rejects_unknown_refund_address`
  - `create_uses_configured_recovery_address`
- SDK tests: assert `refund_address` is absent from create payloads.
- CLI tests: assert `--refund-to` is rejected.
- Startup tests: wrong factory hash, sweeper hash, or bound factory fails startup.

## Slice 1 verification

```bash
forge fmt --check
forge build --sizes
forge test --match-contract PaymentTest -vvv
forge test --match-contract BatchSweeperTest -vvv
cargo fmt --all -- --check
cargo test -p gateway-core -p gateway-db -p gateway-indexer -p gatewayd -p gateway-cli
cargo clippy --workspace --all-targets --no-deps
npm test --workspace @payday/sdk
npm run typecheck --workspace @payday/web
terraform -chdir=infra fmt -check -recursive
terraform -chdir=infra init -backend=false
terraform -chdir=infra validate
just e2e
```

---

# Slice 2 — Invoice document, PDFs, dashboard, and proof

**Scope:** L–XL  
**Depends on:** Slice 1.

## 1. Add invoice and attribution domain types

Create `crates/gateway-core/src/attribution.rs`:

```rust
use alloy_primitives::{Address, B256};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const ATTRIBUTION_VERSION: u16 = 1;
pub const ATTRIBUTION_DOMAIN: &[u8] = b"PAYDAY_ATTRIBUTION_V1";
pub const SALT_DOMAIN: &[u8] = b"PAYDAY_SALT_V1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Party {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExpectedIdentity {
    pub first_name: String,
    pub last_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum PayerPolicy {
    Permissionless,
    VerifiedEmail {
        expected_email: String,
    },
    VerifiedIdentity {
        expected_email: String,
        expected_identity: ExpectedIdentity,
    },
    VerifiedIdentityUnattributed {
        expected_email: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttachmentCommitment {
    pub id: Uuid,
    pub byte_length: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CanonicalIssuanceSnapshot {
    pub schema: String,
    pub canonicalization: String,
    pub issuer: Party,
    pub bill_to: Party,
    pub amount_base_units: String,
    pub notes: Option<String>,
    pub heading: Option<String>,
    pub reference: Option<String>,
    pub expiration_timestamp: String,
    pub payer_policy: PayerPolicy,
    pub attachment: Option<AttachmentCommitment>,
    pub chain_id: String,
    pub token_address: String,
    pub receiver_address: String,
    pub recovery_address: String,
    pub factory_address: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributionMaterial {
    pub canonical_bytes: Vec<u8>,
    pub nonce: B256,
    pub attribution_hash: B256,
    pub salt: crate::Salt,
}

pub fn derive_attribution(
    snapshot: &CanonicalIssuanceSnapshot,
) -> Result<AttributionMaterial, AttributionError>;
```

`derive_attribution()` must:

1. Serialize with RFC 8785 JCS using `serde_jcs::to_vec`.
2. Compute:
   ```rust
   attribution_hash =
       keccak256([ATTRIBUTION_DOMAIN, canonical_bytes.as_slice()].concat());
   ```
3. Generate a 32-byte OS-CSPRNG nonce.
4. Compute:
   ```rust
   salt = Salt(keccak256(
       [SALT_DOMAIN, nonce.as_slice(), attribution_hash.as_slice()].concat()
   ));
   ```

Modify `salt.rs` so only `derive_attribution()` can construct issuance salts. Do not add a public constructor accepting nonce or salt from API input.

Export the module from `lib.rs`.

Modify `Invoice`:

```rust
pub struct Invoice {
    // existing fields
    pub attribution_version: u16,
    pub attribution_nonce: B256,
    pub attribution_hash: B256,
    pub issuance_snapshot: CanonicalIssuanceSnapshot,
}
```

Replace `Invoice::new()` with:

```rust
#[allow(clippy::too_many_arguments)]
pub fn issue(
    factory: FactoryAddress,
    chain_id: ChainId,
    token: TokenAddress,
    beneficiary: BeneficiaryAddress,
    amount: Amount,
    expiration_timestamp: u64,
    recovery: RecoveryAddress,
    snapshot: CanonicalIssuanceSnapshot,
) -> Result<Self, AttributionError>;
```

`address_matches_parameters()` must remain unchanged in principle: it recomputes exclusively from stored payment parameters and stored `salt`.

## 2. Replace the create DTO

Modify `dto.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePaymentRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_address: Option<String>,
    pub payout_address: String,
    pub amount: String,
    pub issuer: Party,
    pub bill_to: Party,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    pub payer_policy: PayerPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}
```

Remove `memo`. Keep `metadata` merchant-only; include it in idempotency equality, but not in the payer DTO.

Add invoice, policy, attachment, verification, and unsolicited fields to `PaymentResponse` and `PaymentSummaryResponse`.

## 3. Add customer and attachment repositories

Create:

- `crates/gateway-db/src/customers.rs`
- `crates/gateway-db/src/attachments.rs`
- `crates/gateway-db/src/proofs.rs`

Core signatures:

```rust
pub struct CreateCustomerInput {
    pub id: Uuid,
    pub account_id: AccountId,
    pub name: String,
    pub email: Option<String>,
    pub details: Option<String>,
}

impl CustomerRepository {
    pub async fn create(
        &self,
        input: &CreateCustomerInput,
    ) -> Result<DbCustomer, sqlx::Error>;

    pub async fn get_for_account(
        &self,
        account: AccountId,
        id: Uuid,
    ) -> Result<Option<DbCustomer>, sqlx::Error>;

    pub async fn list_for_account(
        &self,
        account: AccountId,
        limit: u32,
        starting_after: Option<Uuid>,
    ) -> Result<Vec<DbCustomer>, sqlx::Error>;

    pub async fn update(
        &self,
        account: AccountId,
        id: Uuid,
        name: String,
        email: Option<String>,
        details: Option<String>,
    ) -> Result<Option<DbCustomer>, sqlx::Error>;
}
```

```rust
pub struct CreateAttachmentUpload {
    pub id: Uuid,
    pub account_id: AccountId,
    pub object_key: String,
    pub original_filename: String,
}

impl AttachmentRepository {
    pub async fn create_upload(
        &self,
        input: &CreateAttachmentUpload,
    ) -> Result<DbAttachment, sqlx::Error>;

    pub async fn mark_ready(
        &self,
        account: AccountId,
        id: Uuid,
        byte_length: u64,
        sha256: B256,
        scan_result: &str,
    ) -> Result<Option<DbAttachment>, sqlx::Error>;

    pub async fn attach_to_invoice(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        account: AccountId,
        attachment_id: Uuid,
        invoice_id: Uuid,
    ) -> Result<DbAttachment, AttachInvoiceError>;
}
```

Replace `InvoiceRepository::insert()` with a transaction-aware method:

```rust
pub async fn insert_issued(
    &self,
    input: &CreateInvoiceInput,
    attachment_id: Option<Uuid>,
) -> Result<InsertIssuedInvoice, InsertIssuedInvoiceError>;
```

It must:

- lock the idempotency row and attachment;
- compare all immutable request fields;
- require attachment status `ready`;
- insert invoice and attach PDF atomically;
- return replay only when every immutable field and attachment hash match.

## 4. Implement S3 upload and finalization

Add workspace dependency `aws-sdk-s3` and gateway dependency.

Create `gatewayd/src/attachments.rs`:

```rust
#[derive(Clone)]
pub struct AttachmentStore {
    s3: aws_sdk_s3::Client,
    bucket: String,
    download_ttl: Duration,
}

pub struct PresignedUpload {
    pub attachment_id: Uuid,
    pub upload_url: String,
    pub headers: BTreeMap<String, String>,
    pub expires_at: DateTime<Utc>,
}

impl AttachmentStore {
    pub async fn presign_upload(
        &self,
        account: AccountId,
        attachment_id: Uuid,
    ) -> Result<PresignedUpload, AttachmentError>;

    pub async fn inspect_and_hash(
        &self,
        object_key: &str,
    ) -> Result<InspectedPdf, AttachmentError>;

    pub async fn presign_download(
        &self,
        object_key: &str,
        filename: &str,
    ) -> Result<String, AttachmentError>;
}
```

Create `gatewayd/src/api/attachments.rs` with:

```text
POST /v1/attachments
POST /v1/attachments/{id}/finalize
GET  /v1/payments/{id}/attachment
```

Finalization order:

1. `HeadObject`.
2. Require `Content-Type: application/pdf`.
3. Require `1..=5_242_880` bytes.
4. Read object tags.
5. Return `409 attachment_scan_pending` when the GuardDuty tag is absent.
6. Reject every value except `NO_THREATS_FOUND`.
7. Stream the object, enforce the size again, require `%PDF-` magic bytes, and compute SHA-256.
8. Persist length/hash/status `ready`.

Never trust browser MIME, filename extension, ETag, or a client-supplied digest.

Update `infra/main.tf` with:

- private versioned S3 bucket;
- SSE-KMS;
- lifecycle deletion for unattached uploads;
- GuardDuty Malware Protection plan with managed tagging;
- bucket policy blocking reads unless the scan tag is clean, except GuardDuty and the gateway task role;
- gateway permissions for scoped PUT, HEAD, GET, tags, and delete.

## 5. Implement issuance and idempotency

Refactor `create_payment`:

1. Parse and validate all request fields.
2. Normalize email fields.
3. Validate `customer_id` belongs to the authenticated account.
4. Resolve and lock an optional ready attachment.
5. Perform the existing pre-generation idempotency lookup.
6. Build `CanonicalIssuanceSnapshot` from normalized values and configured chain data.
7. Call `Invoice::issue()`.
8. Call `insert_issued()` with the attachment.
9. On an insert race, fetch and compare every immutable field again.

Extend `RequestedInvoice` and `same_request()` to compare:

- issuer and bill-to snapshots;
- amount;
- notes, heading, reference, metadata;
- customer ID;
- policy mode, expected email, expected identity;
- expiration intent;
- chain parameters and configured recovery;
- attachment ID, byte length, and SHA-256.

## 6. Add deterministic PDF and Proof of Payment

Create `gatewayd/src/invoice_pdf.rs`:

```rust
pub fn render_invoice_pdf(
    invoice: &PaymentResponse,
) -> Result<Vec<u8>, InvoicePdfError>;
```

Use fixed fonts, fixed object ordering, no creation timestamp, no random IDs, and no locale-dependent formatting. The same invoice must produce byte-identical PDF output.

Create `gateway-core/src/proof.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofTransfer {
    pub transaction_hash: String,
    pub sender: String,
    pub recipient: String,
    pub amount_base_units: String,
    pub block_number: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationAttestationPayload {
    pub version: String,
    pub payment_id: String,
    pub payer_policy_mode: String,
    pub result: String,
    pub verified_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedVerificationAttestation {
    pub payload: VerificationAttestationPayload,
    pub signer: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofOfPayment {
    pub version: String,
    pub canonical_issuance_snapshot: CanonicalIssuanceSnapshot,
    pub canonicalization: String,
    pub attribution_nonce: String,
    pub attribution_hash: String,
    pub salt: String,
    pub chain_id: String,
    pub factory_address: String,
    pub payment_address: String,
    pub token_address: String,
    pub settlement_transaction_hash: String,
    pub transfers: Vec<ProofTransfer>,
    pub verification: SignedVerificationAttestation,
}
```

Add:

```text
GET /v1/payments/{id}/invoice.pdf
GET /v1/payments/{id}/proof
```

`/proof` is merchant-authenticated and returns JSON. The attachment descriptor is committed in the snapshot; the PDF can be downloaded separately and supplied to the verifier.

Sign:

```text
keccak256(
  "PAYDAY_VERIFICATION_ATTESTATION_V1" ||
  JCS(VerificationAttestationPayload)
)
```

with a dedicated AWS KMS secp256k1 key. Include the recovered signer address. Do not reuse the sweep signer or recovery-wallet key.

Create CLI verification:

```rust
pub struct VerifyProofArgs {
    pub proof: PathBuf,
    #[arg(long)]
    pub attachment: Option<PathBuf>,
    #[arg(long)]
    pub trusted_attestor: Vec<String>,
    #[arg(long)]
    pub rpc_url: Option<String>,
}
```

`payday proof download <payment> --output proof.json` and `payday proof verify proof.json` must verify:

- JCS hash;
- nonce-derived salt;
- CREATE3 address;
- attachment hash when provided;
- signed attestation;
- included transfer recipient;
- optional live chain receipt when `--rpc-url` is supplied.

## 7. Add the dashboard

Create:

```text
web/app/dashboard/layout.tsx
web/app/dashboard/page.tsx
web/app/dashboard/invoices/page.tsx
web/app/dashboard/invoices/new/page.tsx
web/app/dashboard/invoices/[id]/page.tsx
web/app/dashboard/customers/page.tsx
web/app/dashboard/customers/new/page.tsx
web/app/dashboard/customers/[id]/page.tsx
web/components/dashboard/invoice-form.tsx
web/components/dashboard/invoice-table.tsx
web/components/dashboard/customer-form.tsx
web/components/dashboard/attachment-upload.tsx
web/components/dashboard/verification-status.tsx
web/components/dashboard/recovered-funds.tsx
web/lib/merchant-payday.ts
```

Use the same API routes as SDK users. Do not reproduce payer-policy, attachment, or amount validation in server actions; only provide immediate form feedback.

Add merchant Auth0 browser authentication. Refactor `Auth0Verifier` so dashboard identity tokens map to `AccountId` without issuing or storing an API key in the browser. Keep fresh-authentication and one-time-event enforcement exclusively on API-key issuance/revocation.

## 8. Update SDK and CLI

Update SDK types to exactly mirror Rust definitions. Add:

```ts
export interface Party {
  name: string;
  email?: string;
  details?: string;
}

export interface ExpectedIdentity {
  first_name: string;
  last_name: string;
}

export type PayerPolicy =
  | { mode: "permissionless" }
  | { mode: "verified_email"; expected_email: string }
  | {
      mode: "verified_identity";
      expected_email: string;
      expected_identity: ExpectedIdentity;
    }
  | {
      mode: "verified_identity_unattributed";
      expected_email: string;
    };

export interface AttachmentDescriptor {
  id: string;
  filename: string;
  mime_type: "application/pdf";
  byte_length: string;
  sha256: string;
  download_url?: string;
}
```

Add SDK clients for customers, attachments, invoice PDF, and proof.

Modify CLI `CreateArgs`:

```rust
pub struct CreateArgs {
    #[arg(long, value_name = "FILE", conflicts_with_all = ["amount", "to"])]
    pub from_file: Option<PathBuf>,
    #[arg(long, requires = "to")]
    pub amount: Option<String>,
    #[arg(long, requires = "amount")]
    pub to: Option<String>,
    #[arg(long)]
    pub attachment: Option<PathBuf>,
    #[arg(long)]
    pub idempotency_key: Option<String>,
}
```

`--from-file` deserializes `CreatePaymentRequest` with `deny_unknown_fields`. `--attachment` performs create-upload, PUT, finalize polling, then injects `attachment_id`.

Update the assertion at `cli.rs` to require invoice terminology rather than forbidding it.

## Slice 2 tests

- `gateway-core/src/attribution.rs`:
  - `jcs_is_stable_across_input_field_order`
  - `one_field_change_changes_attribution_hash_and_address`
  - `identical_snapshots_receive_distinct_nonce_salt_and_address`
- `gateway-core/src/invoice.rs`:
  - `address_matches_parameters_uses_stored_salt_only`
- `gateway-db/src/attachments.rs`:
  - `one_attachment_per_invoice`
  - `only_ready_attachment_can_be_issued`
  - `attachment_and_invoice_are_committed_atomically`
- `gatewayd/src/api/routes.rs`:
  - idempotency conflict tests for every document/policy/assertion/attachment field;
  - MIME, size, scan, magic-byte, and account-isolation tests.
- `gatewayd/src/invoice_pdf.rs`:
  - `same_invoice_produces_identical_pdf_bytes`
- `gateway-core/src/proof.rs`:
  - `proof_reconstructs_salt_and_create3_address`
  - `tampered_snapshot_attachment_or_attestation_fails`
- `sdk/typescript/test/client.test.mjs`: customer, upload, proof request shapes.
- `gateway-cli`: `--from-file`, PDF upload, proof verification, and invoice help.
- Web component tests for dashboard forms and attachment progression.
- Playwright: create customer, upload PDF, create invoice, view detail, download proof.

## Slice 2 verification

```bash
cargo fmt --all -- --check
cargo test -p gateway-core -p gateway-db -p gatewayd -p gateway-cli
cargo clippy --workspace --all-targets --no-deps
npm test --workspace @payday/sdk
npm run typecheck --workspace @payday/web
npm run lint --workspace @payday/web
npm test --workspace @payday/web
npm run build --workspace @payday/web
just web-check
just web-e2e
just e2e
```

---

# Slice 3 — Four payer modes, email verification, and sweep gate

**Scope:** XL  
**Depends:** Slices 1–2.

## 1. Add policy validation and public verification DTOs

Implement:

```rust
impl PayerPolicy {
    pub fn mode(&self) -> PayerPolicyMode;
    pub fn expected_email(&self) -> Option<&str>;
    pub fn expected_identity(&self) -> Option<&ExpectedIdentity>;
    pub fn validate(&self) -> Result<(), PayerPolicyError>;
}
```

Add public DTOs:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayerPolicyResponse {
    pub mode: PayerPolicyMode,
    pub expected_email_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationRequirementsResponse {
    pub email: VerificationFactStatus,
    pub document: VerificationFactStatus,
    pub liveness: VerificationFactStatus,
    pub identity_match: VerificationFactStatus,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayerInvoiceDetails {
    pub amount: String,
    pub amount_base_units: String,
    pub bill_to: Party,
    pub notes: Option<String>,
    pub reference: Option<String>,
    pub attachment: Option<AttachmentDescriptor>,
}
```

Update `PayerPaymentResponse`:

- Always return `id`, issuer name, heading, mode, masked email, requirement statuses, lifecycle status, and server time.
- Make amount, chain, token, received, remaining, address, explorer URL, URI, and attachment nullable.
- Populate those fields only for permissionless invoices or a qualifying payer session.

Use one `content_unlocked` boolean in `payer_response()`; do not guard individual fields independently.

## 2. Add payer-session repository

Create `gateway-db/src/verifications.rs`:

```rust
pub struct PayerSessionRepository {
    pool: PgPool,
}

pub struct CreatedPayerSession {
    pub id: Uuid,
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

impl PayerSessionRepository {
    pub async fn create(
        &self,
        invoice_id: Uuid,
        ttl: Duration,
    ) -> Result<CreatedPayerSession, sqlx::Error>;

    pub async fn find_active(
        &self,
        token: &str,
        invoice_id: Uuid,
    ) -> Result<Option<DbPayerSession>, sqlx::Error>;

    pub async fn approve_email(
        &self,
        session_id: Uuid,
        payer_ref: B256,
        verified_at: DateTime<Utc>,
    ) -> Result<VerificationCompletion, sqlx::Error>;
}
```

Generate 32 random bytes, return base64url without padding, and store only `SHA-256(token)`.

Derive payer references:

```rust
pub fn payer_ref(
    master_key: &[u8; 32],
    account_id: Uuid,
    normalized_email: &str,
) -> B256;
```

Derive an account-specific key first:

```text
account_key = HMAC(master_key, "PAYDAY_PAYER_REF_ACCOUNT_V1" || account_id)
payer_ref   = HMAC(account_key, normalized_email)
```

Require `PAYDAY_PAYER_REF_MASTER_KEY` as base64-encoded 32 bytes.

## 3. Add dedicated payer Auth0 flow

Refactor `Auth0Verifier` so merchant and payer verifiers have separate audience/client configuration.

Add configuration:

```text
PAYDAY_PAYER_AUTH0_ISSUER
PAYDAY_PAYER_AUTH0_AUDIENCE
PAYDAY_PAYER_AUTH0_CLIENT_ID
PAYDAY_PAYER_REF_MASTER_KEY
PAYDAY_HOSTED_CHECKOUT_ORIGIN
```

Modify `auth0/actions/payday-email-otp.js` or add `payday-payer-email-otp.js` so the payer audience receives only:

- authentication method;
- client ID;
- authenticated timestamp;
- event ID;
- normalized email.

Update `dev-identity` to accept both merchant and payer clients/audiences.

## 4. Add payer write routes

Create `gatewayd/src/api/payer_verification.rs`:

```text
POST /v1/payer/payments/{id}/verify/email/start
POST /v1/payer/payments/{id}/verify/email/confirm
GET  /v1/payer/payments/{id}/verify
```

Wire types:

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmEmailRequest {
    pub otp: String,
}

#[derive(Serialize)]
pub struct StartEmailResponse {
    pub payer_session: String,
    pub expires_at: String,
}

#[derive(Serialize)]
pub struct VerificationStatusResponse {
    pub requirements: VerificationRequirementsResponse,
    pub identity_start_available: bool,
}
```

Flow:

1. `email/start` loads the invoice and rejects permissionless/expired invoices.
2. Create or reuse a pending email attempt with resend cooldown.
3. Gateway calls Auth0 passwordless start using the server-stored expected email.
4. Return the opaque payer-session token.
5. `email/confirm` reads `Payday-Payer-Session`, exchanges OTP with Auth0, verifies the returned JWT, and compares normalized email.
6. Store payer reference and email fact.
7. For `verified_email`, atomically set `invoices.verification_completed_at`.
8. For identity modes, leave invoice completion null.

Never accept an email from the payer request.

## 5. Split CORS routers

Modify `routes.rs`:

- Keep public GET routes under `allow_origin(Any)`.
- Add `Payday-Payer-Session` to allowed GET headers.
- Place verification POST routes in a separate router:
  - exact `PAYDAY_HOSTED_CHECKOUT_ORIGIN`;
  - methods POST/OPTIONS;
  - allowed headers `Content-Type` and `Payday-Payer-Session`;
  - no `Any` origin.

Add a small JSON body limit, e.g. 8 KiB, to verification writes.

## 6. Make payer reads session-aware

Modify `payer.rs`:

```rust
pub async fn authorized_invoice(
    state: &AppState,
    id: &str,
    session_token: Option<&str>,
) -> Result<PayerInvoiceAccess, ApiError>;
```

`PayerInvoiceAccess` contains the invoice row, optional session, and `content_unlocked`.

For QR:

- require an unlocked session for gated invoices;
- continue returning `410 payment_not_payable` when lifecycle state is closed;
- return `401 verification_required` for a valid gated invoice without qualifying session.

Change the SDK QR API from an unauthenticated URL to:

```ts
qr: (
  id: string,
  payerSession?: string,
  options?: { signal?: AbortSignal },
) => Promise<Blob>;
```

The checkout creates an object URL for the fetched SVG. Never put the payer token in the QR URL.

## 7. Gate the database sweep claim

Modify `claim_sweep_batch()`. The decisive condition must be SQL:

```sql
AND (
    invoice.status IN ('expired', 'fulfilled', 'recovered')
    OR (
        invoice.status IN ('funded', 'deploying')
        AND (
            invoice.payer_policy_mode = 'permissionless'
            OR invoice.verification_completed_at IS NOT NULL
        )
        AND invoice.expiration_timestamp >= COALESCE(
            (
                SELECT last_block_timestamp
                FROM indexer_cursor
                WHERE chain_id = invoice.chain_id
            ),
            0
        )
    )
)
```

Do not rely on a guard in `Indexer::submit_next_batch()`.

On first nonzero funding in `apply_finalized_usdc_range()`, set:

```sql
likely_unsolicited_at =
    CASE
      WHEN payer_policy_mode <> 'permissionless'
       AND verification_completed_at IS NULL
       AND likely_unsolicited_at IS NULL
      THEN to_timestamp(observation_block_timestamp)
      ELSE likely_unsolicited_at
    END
```

Do not change the observation disposition or payment address. There is no quarantine state.

## 8. Update checkout

Modify:

- `web/lib/checkout-state.ts`
- `web/components/checkout/checkout.tsx`
- `web/components/checkout/use-payment.ts`
- `web/app/pay/[id]/page.tsx`

Create:

```text
web/components/checkout/verification-gate.tsx
web/components/checkout/email-verification.tsx
web/components/checkout/invoice-details.tsx
web/components/checkout/attachment-link.tsx
web/lib/payer-session.ts
```

Store the opaque token in `sessionStorage` under a payment-specific key. Never place it in server-rendered HTML, local storage, analytics, or URLs.

Extend checkout phases:

```ts
export type CheckoutPhase =
  | "verification_required"
  | "email_pending"
  | "identity_required"
  // existing phases
  | "awaiting"
  | "partial"
  | "confirming"
  | "closing"
  | "paid"
  | "settled"
  | "expired_empty"
  | "expired_funded"
  | "returned"
  | "attention";
```

Before unlock, render only issuer name, heading, masked email, and verification controls. Do not render hidden data with CSS; absent fields must not enter the React tree.

## Slice 3 tests

- `gateway-core`: all four policy parse/validation cases; unattributed mode rejects identity.
- `gateway-db/src/verifications.rs`:
  - session token hashing and expiry;
  - one session cannot unlock another invoice.
- `gateway-db/src/sweeps.rs`:
  - `permissionless_live_invoice_is_claimable`
  - `verified_live_invoice_without_completion_is_not_claimable`
  - `verified_live_invoice_after_completion_is_claimable`
  - `expired_unverified_invoice_is_claimable_for_recovery`
  - `verification_after_expiry_does_not_create_live_claim`
- `gateway-db/src/invoices.rs`:
  - pre-verification funding sets likely unsolicited once;
  - no quarantine or address replacement occurs.
- `gatewayd/src/api/routes.rs`:
  - masked email only;
  - gated DTO contains no amount, bill-to, reference, attachment, address, or URI;
  - qualifying session unlocks only itself;
  - QR requires that session;
  - write CORS rejects foreign origins.
- Auth0 action and `dev-identity` tests for separate audiences.
- Web unit tests for every phase.
- Playwright tests for all four modes and DOM absence before unlock.
- Anvil E2E:
  - fund before verification, verify before expiry, settle same address;
  - fund before verification, expire, recover full balance.

## Slice 3 verification

```bash
cargo fmt --all -- --check
cargo test -p gateway-core -p gateway-db -p gatewayd -p gateway-indexer
node --test auth0/actions/*.test.js
npm test --workspace @payday/sdk
just web-check
just web-e2e
just e2e
```

---

# Slice 4 — Matched and unattributed identity verification

**Scope:** XL  
**Depends:** Slices 0 and 3.

## 1. Add a thin provider boundary

Create `gatewayd/src/identity/mod.rs`:

```rust
use async_trait::async_trait;
use axum::http::HeaderMap;

#[derive(Debug, Clone)]
pub struct IdentitySessionRequest {
    pub verification_id: Uuid,
    pub payer_ref: B256,
    pub expected_identity: Option<ExpectedIdentity>,
    pub callback_url: String,
}

#[derive(Debug, Clone)]
pub struct IdentitySession {
    pub provider_reference: String,
    pub hosted_url: String,
    pub status: IdentityProviderStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityProviderStatus {
    Pending,
    Approved,
    Declined,
    InReview,
    Expired,
    Abandoned,
}

#[derive(Debug, Clone)]
pub struct IdentityDecision {
    pub status: IdentityProviderStatus,
    pub document: VerificationFactStatus,
    pub liveness: VerificationFactStatus,
    pub face_match: VerificationFactStatus,
    pub expected_identity_match: VerificationFactStatus,
    pub risk_codes: Vec<String>,
    pub country_code: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct VerifiedIdentityCallback {
    pub event_id: String,
    pub provider_reference: String,
}

#[async_trait]
pub trait PayerIdentityProvider: Send + Sync {
    async fn create_session(
        &self,
        request: IdentitySessionRequest,
    ) -> Result<IdentitySession, IdentityProviderError>;

    async fn fetch_decision(
        &self,
        provider_reference: &str,
    ) -> Result<IdentityDecision, IdentityProviderError>;

    fn verify_callback(
        &self,
        headers: &HeaderMap,
        raw_body: &[u8],
    ) -> Result<VerifiedIdentityCallback, IdentityProviderError>;
}
```

Do not add generalized vendor discovery, capability negotiation, or provider-specific types outside `identity/didit.rs`.

## 2. Implement Didit V3

Create `gatewayd/src/identity/didit.rs`.

Use:

```text
POST https://verification.didit.me/v3/session/
GET  https://verification.didit.me/v3/session/{session_id}/decision/
x-api-key: <secret>
```

Wire request types:

```rust
#[derive(Serialize)]
struct DiditCreateSessionRequest {
    workflow_id: String,
    vendor_data: String,
    callback: String,
    callback_method: &'static str,
    metadata: DiditMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_details: Option<DiditExpectedDetails>,
}

#[derive(Serialize)]
struct DiditMetadata {
    verification_id: Uuid,
}

#[derive(Serialize)]
struct DiditExpectedDetails {
    first_name: String,
    last_name: String,
}

#[derive(Deserialize)]
struct DiditCreateSessionResponse {
    session_id: String,
    url: String,
    status: String,
}
```

Set:

- `vendor_data = hex(payer_ref)`; it is merchant-specific because `payer_ref` is.
- `callback_method = "both"`.
- metadata contains only Payday's verification attempt ID.
- omit `contact_details` to avoid giving Didit an email it does not need.
- matched mode supplies `expected_details`;
- unattributed mode omits it.

Configure one pinned Didit workflow requiring document verification, liveness, and face match. Configure the workflow to decline expected-name mismatches.

Deserialize only allowlisted decision fields:

```rust
#[derive(Deserialize)]
struct DiditDecision {
    status: String,
    #[serde(default)]
    id_verifications: Vec<DiditFeature>,
    #[serde(default)]
    liveness_checks: Vec<DiditFeature>,
    #[serde(default)]
    face_matches: Vec<DiditFeature>,
}

#[derive(Deserialize)]
struct DiditFeature {
    status: String,
    #[serde(default)]
    warnings: Vec<DiditWarning>,
}

#[derive(Deserialize)]
struct DiditWarning {
    risk: String,
}
```

Serde ignores extracted names, document numbers, image URLs, DOB, and biometrics because they are absent from these types. Never serialize or log the raw response.

## 3. Verify callbacks safely

Mount:

```text
POST /v1/webhooks/identity
```

The handler must receive raw bytes, then:

1. Require `X-Signature-V2` and `X-Timestamp`.
2. Reject timestamps outside ±300 seconds.
3. Apply Didit's documented canonical JSON algorithm.
4. Verify HMAC-SHA256 in constant time.
5. Extract only event ID and session ID.
6. Insert into `identity_webhook_receipts` with `ON CONFLICT DO NOTHING`.
7. Set `next_poll_at=now()` for the matching attempt.
8. Return `202` without fetching Didit synchronously.

Do not reuse RFC 8785 code for Didit signatures unless provider vectors prove byte equality; Didit's canonicalization has its own float-normalization rules.

## 4. Add identity start and credential reuse

Add:

```text
POST /v1/payer/payments/{id}/verify/identity/start
```

Response:

```rust
#[derive(Serialize)]
pub struct StartIdentityResponse {
    pub outcome: IdentityStartOutcome,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IdentityStartOutcome {
    Reused,
    Redirect { url: String },
    ReviewRequired,
}
```

Processing order:

1. Authenticate active payer session.
2. Require current-session email verification.
3. Reject permissionless/verified-email/expired invoices.
4. Compute expected-identity JCS hash for matched mode.
5. Search active credentials using account + payer reference.
6. Unattributed mode may reuse `document_liveness`.
7. Matched mode may reuse only `matched_identity` with the same assertion hash.
8. If reused, populate session facts and call completion logic.
9. Otherwise create a `payer_verifications` attempt before calling Didit.
10. Lazily create the hosted session and persist only provider reference/status.
11. Return its hosted URL.

On approval:

- both modes set document and liveness facts;
- matched mode also sets identity-match fact;
- create a generic `document_liveness` credential;
- matched mode additionally creates a hash-bound `matched_identity` credential;
- set invoice completion only while the invoice remains live;
- unlock only the qualifying payer session.

## 5. Add reconciliation polling

Create `gatewayd/src/identity/reconciler.rs`:

```rust
pub async fn run(
    repo: VerificationRepository,
    provider: Arc<dyn PayerIdentityProvider>,
    shutdown: watch::Receiver<bool>,
);

pub async fn reconcile_one(
    repo: &VerificationRepository,
    provider: &dyn PayerIdentityProvider,
    verification: ClaimedVerification,
) -> Result<(), ReconciliationError>;
```

Claim pending attempts with `FOR UPDATE SKIP LOCKED` and a lease. Poll:

- actively pending sessions every 15 seconds;
- in-review sessions with increasing backoff;
- stop for approved, declined, expired, or abandoned;
- cap provider retries but leave operational failures pending rather than declining the payer.

Start the worker in `gatewayd/src/main.rs`, alongside existing webhook and notification workers.

## 6. Implement retry and human review

Allow one resubmission:

- first decline may create attempt 2;
- a second decline becomes `review_required`;
- further automatic attempts return `409 review_required`.

Add merchant route:

```text
POST /v1/payments/{id}/verification/review
```

Add admin route:

```text
POST /v1/admin/verifications/{id}/decision
```

Request:

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewDecisionRequest {
    pub decision: ReviewDecision,
    pub note: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approve,
    Decline,
}
```

Change admin middleware to insert a configured reviewer ID rather than `()`. Record reviewer, decision, note, and timestamp. Manual approval must reuse the same expected-identity hash binding as automated approval.

## 7. Complete checkout and dashboard UX

Checkout:

- show explicit consent before redirect;
- link Payday and Didit privacy notices;
- distinguish "identity matched to merchant-provided details" from "document and liveness verified without identity attribution";
- restore the payer token from `sessionStorage` after Didit returns;
- ignore callback query status and call `/verify`;
- state that verification does not prove wallet ownership;
- show the payer appeal contact after decline/review-required.

Dashboard:

- display email, document, liveness, and identity-match facts separately;
- display provider reference and allowlisted risk categories;
- never display extracted identity;
- provide retry and request-review actions;
- display reviewer outcome and timestamp;
- expose likely unsolicited funding independently from verification status.

## Slice 4 tests

- Provider contract tests with recorded, redacted Didit fixtures:
  - approved matched;
  - expected-name mismatch decline;
  - approved unattributed;
  - missing liveness/face match;
  - in-review and expired statuses.
- Callback tests:
  - valid V2 signature;
  - tampered body;
  - stale timestamp;
  - duplicate event;
  - no body or extracted identity in logs.
- Repository tests:
  - same-merchant generic reuse;
  - cross-merchant isolation;
  - matched assertion hash mismatch;
  - expired/revoked credential rejection.
- Reconciler tests:
  - lost webhook followed by polling approval;
  - transient Didit failure does not decline;
  - completion after expiry does not create live eligibility.
- Retry/review tests:
  - one automated resubmission;
  - second decline requires review;
  - reviewer identity and timestamp persisted.
- Web Playwright:
  - Didit return/resume;
  - matched and unattributed copy;
  - credential reuse;
  - decline and appeal path.
- Privacy regression:
  - search serialized fixtures, DB rows, responses, and captured logs for extracted name, document number, image URL, and DOB sentinel values; all must be absent.

## Slice 4 verification

```bash
cargo fmt --all -- --check
cargo test -p gateway-db -p gatewayd -p gateway-indexer
cargo clippy --workspace --all-targets --no-deps
node --test auth0/actions/*.test.js
npm test --workspace @payday/sdk
just web-check
just web-e2e
just e2e
```

Before enabling live identity verification, complete Slice 0's DPA, retention, consent, appeal, human-review, and counsel requirements.
