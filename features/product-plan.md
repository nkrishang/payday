# Payday: Verified Customer Funds — Consolidated Product Plan

> **Superseded on 2026-09-05.** Identity verification is no longer part of
> the product. Everything below about `verified_identity`,
> `verified_identity_unattributed`, `expected_identity`, KYC, Didit, the
> `PayerIdentityProvider` boundary, credential reuse, the reconciler, manual
> review, and the identity step of the checkout describes a design that was
> built and then removed. The payer policy has two modes, `permissionless`
> and `verified_email`. Those sections are kept for the record; treat them
> as history, not as the current specification.

**Status:** Consolidated implementation plan, updated with owner decisions on all open questions.

**Positioning:** **Payday is the complete solution for receiving attributable, verified customer funds over stablecoin rails.**

**Guiding principle:** The product should be the *leanest comprehensive* version — every necessary capability, nothing superfluous, nothing missing.

---

## 1. Product direction

Payday already creates a unique counterfactual payment address for each invoice, detects USDC transfers, and deploys a payment contract to settle funds.

The product adds four connected capabilities:

1. **Structured attribution** — a bounded record explaining why money moved.
2. **Payer assurance** — one of four merchant-selectable verification modes.
3. **Supporting PDFs** — bounded PDF attachments for itemization or supporting documents.
4. **Verifiable payment records** — a simple cryptographic commitment tying the issued invoice to the address that received payment.

These form one product. The invoice records the obligation, the payer policy controls disclosure and settlement, and the resulting Proof of Payment connects the invoice to the public chain transfer.

### Product surfaces

1. **API** — canonical integration and policy surface.
2. **Merchant dashboard** — primary operating interface.
3. **Hosted payment page** — invoice presentation and payer verification.

Use **invoice** for the issued business document and **payment** for its on-chain fulfillment.

---

## 2. Product promise and honest boundaries

For a `verified_identity` invoice, Payday can assert:

> The merchant specified an expected email and identity. A visitor controlling that mailbox completed a liveness-backed document check matching those assertions before Payday disclosed and swept the payment address.

For a `verified_identity_unattributed` invoice, Payday can assert:

> The merchant specified an expected email. A visitor controlling that mailbox completed a liveness-backed document check successfully before Payday disclosed and swept the payment address. The merchant did not assert, and Payday does not disclose, that person's legal identity.

Payday cannot assert that the verified person controlled the wallet that ultimately sent funds. Once disclosed, an address can be forwarded, and the payment contract cannot inspect sender identity.

The gate therefore controls:

- Disclosure of the address, payment URI, and QR code.
- Whether the indexer settles a live invoice.

It does not prevent an informed third party from transferring funds to the deterministic address.

A future wallet-ownership signature could strengthen this binding, but it is not required for the initial product.

---

## 3. Product principles

### 3.1 Merchant asserts; Payday confirms

For attributed identity verification, the merchant supplies the expected email and legal identity. Payday returns a match result, not vendor-extracted identity data.

For unattributed identity verification, the merchant supplies only the expected email. Payday confirms that document and liveness verification passed, without returning the verified person's identity.

Payday is a confirmation service, not an identity-data marketplace.

### 3.2 Four presets externally, independent facts internally

The public product exposes exactly two payer modes:

| Mode | Merchant supplies | Payer proves |
|---|---|---|
| `permissionless` | Nothing | Nothing |
| `verified_email` | Expected email | Mailbox ownership |

Internally, each fact a policy needs keeps its own status. This supports correct implementation without exposing a merchant-configurable policy builder.

*Historical: the two identity modes that used to sit below these were removed on 2026-09-05; see the note at the top of this document.*

AML screening, wallet ownership, proof of address, and KYB are not additional payer modes in v1.

### 3.3 Disclosure and settlement control, not on-chain access control

For gated invoices Payday withholds:

- Payment address
- Payment URI
- QR code

The indexer also refuses to execute a live, funded payment until the invoice's configured verification requirements have been completed.

This is stronger than disclosure-only gating, but it remains an off-chain policy. The contract itself has no payer identity concept.

### 3.4 One amount, specified directly

The merchant specifies the invoice amount directly.

Payday does **not** model:

- Line items
- Quantity
- Unit price
- Subtotals
- Discounts
- Line-item categories
- `sum(line_items) == amount` reconciliation

If the merchant needs an itemized breakdown, they attach a PDF. Payday stores, presents, and hashes the document but does not parse or reconcile its contents.

### 3.5 Jurisdiction-agnostic by construction

Payday will not model:

- Tax rates or calculations
- Tax identities
- W-9, W-8BEN, 1099, 1042-S, VAT, or equivalent forms
- Jurisdiction-specific invoice schemas
- Fiat settlement or FX

Merchants own their jurisdiction-specific obligations. Bounded free-text `details` fields and PDF attachments let them provide required information without requiring Payday to interpret it.

> **Payday gives you a complete, verifiable record of why money moved. What your jurisdiction requires you to do with it is yours.**

### 3.6 Minimize identity data

Payday stores merchant-supplied assertions because they are necessary to enforce payer policy.

It must not persist vendor-extracted:

- Names
- Dates of birth
- Document numbers
- Document images
- Selfies
- Biometric templates

Stored KYC output is limited to provider reference, verification type, status, timestamps, expiry, necessary mismatch/risk categories, and potentially an ISO country code.

### 3.7 Normal settlement remains direct

The exact invoice amount moves directly from the deterministic payment address to the merchant's receiver address. Payday does not intermediate the intended invoice amount.

Overpayments, stray funds, expired balances, and later transfers move to Payday's recovery wallet. This introduces custody for recovered funds and means Payday can no longer make an unqualified claim that it never controls customer value.

The recovery path must remain a narrow exception. No escrow, discretionary holding of the intended payment amount, batching of customer balances, netting, or fee custody should be added without legal review.

---

## 4. Unified feature model

### 4.1 Invoice

An issued invoice contains:

```text
issuer {
  name
  email?
  details?       # bounded free text, rendered verbatim, never parsed
}

bill_to {
  name
  email?
  details?
}

amount
notes?
reference?       # merchant invoice number or external reference
heading?         # short description shown before verification on gated invoices
expires_at       # enforced payment deadline
payer_policy
attachments[]?  # PDF only, max 1
```

Existing chain mechanics remain part of the payment record:

- Chain
- Token
- Receiver
- Payment address
- Payday recovery address
- Factory
- Salt
- Lifecycle status

The invoice amount is authoritative. Attached PDFs do not alter or derive it.

An issued invoice is an immutable snapshot. Changing the amount, parties, payer policy, or issuance attachments requires cancellation and reissuance.

### 4.2 PDF attachments

PDF attachments are an early feature, not a later typed-attachment platform.

Initial limits (lean — one document, bounded size):

- PDF only
- Maximum 1 file per invoice
- Maximum 5 MiB per file
- Validate MIME type and PDF magic bytes
- Virus scan before issuance
- SHA-256 content hash
- Immutable once the invoice is issued
- Served through short-lived signed URLs

Uploads use presigned object-store URLs so they bypass the API's existing 64 KB request-body limit.

The initial model does not assign roles such as `contract`, `sow`, or `timesheet`. The attachment is simply a PDF associated with the invoice. Typed attachments, multiple files, and external links should be reconsidered only after demonstrated demand.

### 4.3 Invoice content visibility

For gated invoices (`verified_email`, `verified_identity`, `verified_identity_unattributed`), the hosted payment page gates invoice content progressively:

- **Before any verification:** The page shows only that an invoice exists, the issuer's identity (name), and a heading description. The amount, `bill_to`, `notes`, `reference`, PDF attachment, payment address, URI, and QR are all hidden.
- **After email verification:** For `verified_email` invoices, the full invoice content (amount, `bill_to`, notes, reference, PDF) and payment mechanics (address, URI, QR) are revealed.
- **After email + identity verification:** For `verified_identity` and `verified_identity_unattributed` invoices, the full invoice content and payment mechanics are revealed only after both email and identity checks pass.

For `permissionless` invoices, all content is visible immediately, as today.

This means the public payer route (`GET /v1/payer/payments/{id}`) must omit invoice detail fields as well as payment mechanics for gated invoices, not just the address. The existing merchant/payer DTO split is extended to support this progressive disclosure.

### 4.4 Customers and payer assertions

A `customer` is a merchant-owned counterparty record used for:

- Reuse when issuing invoices
- Invoice history
- AR aging and statements
- Dashboard filtering
- Recurring invoices
- Invoice delivery

The billed entity and verifying person may differ. For example, an invoice may be billed to a company while an employee completes verification.

Therefore:

- `bill_to` describes the invoiced counterparty.
- `payer_policy` describes the expected human payer.
- Customer data may provide defaults.
- Every invoice stores its own immutable snapshot.

### 4.5 Payer policy

Recommended API shapes:

```json
{
  "mode": "permissionless"
}
```

```json
{
  "mode": "verified_email",
  "expected_email": "alice@example.com"
}
```

```json
{
  "mode": "verified_identity",
  "expected_email": "alice@example.com",
  "expected_identity": {
    "legal_name": "Alice Smith"
  }
}
```

```json
{
  "mode": "verified_identity_unattributed",
  "expected_email": "alice@example.com"
}
```

Rules:

- `expected_email` is required for all verified modes.
- `expected_identity` is required only for `verified_identity`.
- `expected_identity` is forbidden for `verified_identity_unattributed`.
- Assertions are supplied server-side by the merchant and cannot be edited by the payer.
- The public route returns only a masked email hint such as `a****@a***.com`.
- Policy changes require cancellation and reissuance.
- "Unattributed" means no asserted legal identity; the payment remains associated with the verified email supplied by the merchant.

### 4.6 Verification sessions and reuse

Payer access is session-scoped:

1. A visitor opens the hosted payment link.
2. The public response includes invoice information and requirement status but omits payment mechanics for gated invoices.
3. The visitor verifies the expected email through a dedicated Auth0 payer audience.
4. Identity modes either reuse an eligible merchant-scoped credential or start a Didit hosted session lazily.
5. When all requirements pass, Payday:
   - Marks the invoice's verification requirements complete.
   - Reveals the address, URI, and QR to that payer session.
   - Allows the indexer to sweep a live funded invoice.

Identity reuse is:

- Scoped to one merchant.
- Bound to `payer_ref = HMAC(account-specific secret, normalized email)`.
- Never shared across merchants.
- Conditional on fresh email verification for the current invoice.

A generic successful document/liveness credential may satisfy `verified_identity_unattributed`.

A `verified_identity` credential may be reused only when its stored assertion hash matches the current expected-identity assertion. A previous match against one asserted identity must not be treated as a match against a different identity.

### 4.7 Settlement and recovery

#### Recovery ownership

The recovery address is Payday's custodial recovery wallet, not an expected receiver or payer-controlled refund address.

Consequences:

- Remove merchant control of `refund_address` from the create API, SDK, and dashboard.
- Configure recovery centrally per chain/environment.
- Continue storing the selected recovery address on each invoice because it is committed into the counterfactual address.
- The recovery wallet key is held in AWS KMS (infrastructure already in use), not in a hot wallet or manual key file.
- Recovered funds are reviewed manually and returned to the appropriate party by the operator. Payday has no real users at this point, so this is a manual process — no automated disbursement logic is needed yet.
- Maintain an invoice-level ledger for every amount recovered so manual review has a clear record.

#### Payment contract behavior

`Payment.sol` changes from transferring the complete live balance to the receiver to transferring exactly the invoice amount.

For a live deployment:

```text
balance < amount:
    revert InsufficientTokenBalance

balance >= amount:
    transfer amount to receiver
    transfer balance - amount to Payday recovery
```

For an expired deployment:

```text
transfer the entire balance to Payday recovery
```

`recover()` remains unchanged and forwards the contract's complete current token balance to Payday recovery.

Event behavior:

- `Settled(receiver, amount)` always reports the exact invoice amount.
- `Recovered(recovery, remainder)` is emitted for a nonzero overpayment.
- Expired deployment and later recovery continue to emit `Recovered`.

This allows an address containing accidental or unsolicited funds to remain usable for its intended invoice.

#### Factory deployment

`PaymentFactory` continues to commit:

- Token
- Amount
- Receiver
- Expiration
- Recovery
- Opaque salt

Its external interface and address-prediction formula do not need to change.

However, `PaymentFactory` embeds `Payment.creationCode`. The modified contract therefore requires deploying a new factory build. `BatchSweeper` has an immutable factory reference and must be redeployed or otherwise repointed as part of the same pre-production rollout.

Because Payday is not in production, all environments should move together rather than support mixed contract generations.

### 4.8 Indexer verification gate

The indexer records all finalized transfers to known invoice addresses, regardless of verification state.

A live funded invoice is eligible for factory execution only when:

```text
payer_policy.mode == permissionless
OR verification_completed_at IS NOT NULL
```

An expired invoice remains eligible for recovery regardless of verification state.

Therefore:

- Payment before verification is recorded and may mark the invoice funded.
- It does not trigger live settlement.
- Completing verification before expiry permits the same address to settle normally.
- If verification never completes, expiry permits the complete balance to move to Payday recovery.
- Verification after expiry cannot revive the invoice.
- Partial or stray funds on expiry also move to recovery.
- No address is quarantined or discarded.

The decisive guard belongs in the database claim path used by `claim_sweep_batch`, not only in the web application or indexer process. This prevents another worker from accidentally bypassing policy.

### 4.9 Unsolicited payments and accepted griefing

Unsolicited transfers are accepted as operational noise, not a settlement-safety problem.

Payday should:

- Record every transfer.
- Mark whether verification had completed when the first funding was observed.
- Surface likely unsolicited payments to the merchant.
- Avoid representing them as proof that the verified person controlled the sending wallet.
- Continue using the same address.
- Route any excess over the invoice amount to Payday recovery.

For verified invoices, unsolicited pre-verification funding waits until verification or expiry.

For permissionless invoices, funding can trigger settlement immediately. An attacker could pay invoices and cause sweeps, but this requires transferring real USDC to known invoice addresses. The issuer receives the intended amount, and the source and amount are visible on-chain. This is accepted as economically prohibitive griefing.

"No quarantine" is a firm product decision.

---

## 5. Verifiable attribution and Proof of Payment

### 5.1 Decision: use a simple canonical hash, not a Merkle root

A Merkle root is not justified for the current product.

Its primary benefit would be selective disclosure: proving one field belongs to an invoice without revealing the rest. Payday has no demonstrated workflow requiring field-level proofs. Merchants, payers, and auditors normally need the complete invoice record, and expected-identity assertions already require access control regardless of proof format.

A Merkle implementation would introduce:

- Leaf definitions and ordering rules
- Inclusion-proof generation
- More canonicalization surfaces
- More proof versions
- More complicated customer explanations

It would not materially improve the initial invoice, audit, or receipt workflow.

Payday will instead hash the complete canonical issuance snapshot. If customers later demonstrate a real selective-disclosure requirement, a versioned Merkle commitment can be introduced without changing the Solidity interface because the contract treats the salt as opaque.

### 5.2 Commitment construction

At issuance:

```text
canonical_bytes = JCS(canonical_issuance_snapshot)
attribution_hash = keccak256("PAYDAY_ATTRIBUTION_V1" || canonical_bytes)
salt = keccak256("PAYDAY_SALT_V1" || random_nonce || attribution_hash)
```

Where:

- JCS is RFC 8785 JSON Canonicalization Scheme.
- Numeric token amounts are canonical base-unit strings, never floating point.
- `random_nonce` is 32 bytes from the OS CSPRNG.
- Domain separators and canonicalization version are fixed and documented.
- The API never accepts a client-supplied nonce or salt.

The canonical issuance snapshot includes:

- Schema and canonicalization version
- Issuer snapshot
- `bill_to` snapshot
- Amount
- Notes
- Heading
- Reference
- Expiration
- Payer-policy mode
- Merchant-supplied assertions
- Attachment ID, byte length, and SHA-256
- Chain, token, receiver, recovery, and factory identifiers

The random nonce preserves privacy and uniqueness. Identical invoices still produce different addresses, and guessable invoice contents cannot be used to enumerate addresses.

### 5.3 Settlement safety

Continue storing the final `salt` directly.

`Invoice::address_matches_parameters()` must continue recomputing the payment address from stored payment parameters and `salt` alone. Attribution metadata is proof material, not a settlement dependency.

Losing the nonce or canonical snapshot may make a proof unavailable, but must never strand funds.

### 5.4 Why Proof of Payment is useful

A conventional PDF receipt is only a document. It does not independently prove:

- Which public-chain transfer paid it
- That the invoice was associated with that payment address
- That the invoice content was not substituted after payment
- That an attached PDF is the same file presented at issuance

Payday's Proof of Payment lets a merchant, payer, or auditor independently:

1. Hash the canonical invoice and attached PDFs.
2. Recompute the salt.
3. Recompute the CREATE3 payment address.
4. Verify that address received the referenced USDC transfer.
5. Confirm that the invoice record and payment belong together.

This is useful for reconciliation, audit evidence, disputes, and counterparty records. The verifier does not need access to Payday's database or need to trust that a later PDF export was left unchanged.

The proof establishes **invoice-to-address-to-transfer integrity**. It does not prove that the KYC-verified person owned the sending wallet.

### 5.5 Proof package

A settled Proof of Payment package contains:

- Canonical issuance snapshot
- Canonicalization version
- Random nonce
- Attribution hash
- Salt
- Factory and chain parameters
- Payment address
- Transaction hash and transfer details
- Attachment hash, with the attachment file optionally included
- Payday-attested verification mode, result, and timestamp

The issuance commitment is independently verifiable from the address.

Verification outcomes happen after issuance and therefore cannot be included in the address commitment. They are explicitly labelled **Payday-attested**, not address-committed. A simple signed attestation is sufficient; an append-only hash-chain product is deferred.

Proof of Payment is accessible to the merchant. The merchant may share it at their discretion with payers, auditors, or counterparties. It is not a public bearer-link download.

---

## 6. Identity-provider architecture

### 6.1 Auth0 for email ownership

Use Auth0 only for email verification, with a dedicated payer audience separate from the merchant API-key bootstrap path.

Do not route document verification through Auth0 marketplace login integrations. Payday's payers are anonymous link visitors, and doing so would turn every payer into an additional billable Auth0 user.

### 6.2 Didit for document and liveness checks

Use Didit behind a thin `PayerIdentityProvider` trait supporting:

- Session creation
- Status fetch/reconciliation
- Callback verification

For `verified_identity`:

- Supply merchant assertions through Didit's `expected_details`.
- Set `expected_details_mismatch_action=Decline`.
- Treat approval as document/liveness success plus assertion match.
- Ask for the **minimal data that satisfies the merchant's demand** — start with legal name only. Add fields (DOB, country) only if merchants need stronger matching and counsel approves. Do not collect more than necessary.

For `verified_identity_unattributed`:

- Do not supply an expected legal identity.
- Require the configured document, face match, and liveness checks.
- Treat approval as successful KYC for a real person, regardless of identity.
- Do not return extracted identity to the merchant.

Name representation follows Didit's capabilities and best practices. Payday does not impose its own transliteration, ordering, or normalization rules — it passes the merchant's assertion to Didit and relies on Didit's documented matching behaviour for non-Latin scripts, diacritics, middle names, and name ordering. Whatever threshold Didit applies, Payday must be able to explain it plainly (GDPR Art. 22).

For both:

- Create sessions lazily; vendor sessions are much shorter-lived than invoices.
- Never put merchant assertions in payer-editable query parameters.
- Never proxy document or selfie capture through Payday.
- Verify webhook signatures before processing.
- Bind only status, provider reference, and allowed risk categories.
- Do not log webhook bodies.
- Reconcile pending sessions by polling because Didit retries webhooks only twice.

The provider boundary should remain deliberately thin. Do not design a speculative multi-vendor abstraction.

### 6.3 Human review

An automated decline that prevents payment may create a GDPR Art. 22 review obligation. In the initial stage, the founder operates the Didit console and handles payer appeals manually. This is a deliberate "do things that don't scale" choice — the volume is low, the stakes are high, and building an automated review system before having users would be premature.

The manual process must still be defined:

- The payer appeal route (how a declined payer contacts Payday).
- What evidence the reviewer inspects (vendor dashboard status, risk codes — never extracted identity stored locally).
- What is recorded (decision, timestamp, reviewer).
- How quickly a review is completed (target: within 1 business day).
- How the merchant is informed.

As volume grows, this transitions to a dedicated operator or team, and the process is documented in merchant terms.

### 6.4 Optional wallet ownership proof

Wallet ownership proof remains optional and separable.

A payer signs a challenge with the connected wallet, and Payday verifies address recovery. This is the only proposed feature that directly links a verified browser session to a wallet.

It is not required for the four-mode launch and must not delay it.

---

## 7. API, dashboard, and checkout

### 7.1 API

The API is the source of truth for:

- Invoice validation
- Payer-policy validation
- Verification state
- Settlement eligibility
- Attachment finalization
- Proof generation

Compatibility and contract changes:

- Preserve current permissionless behavior where possible.
- Remove merchant-supplied `refund_address`; recovery is platform-controlled.
- Add the four-mode `payer_policy`.
- Make payer `address` and `payment_uri` nullable.
- Gate invoice detail fields (amount, `bill_to`, notes, reference, attachment) on the public payer route for gated invoices, not just payment mechanics — progressive disclosure per §4.3.
- Include invoice data and attachment descriptors in the payer response only when the session's verification level permits.
- Include the complete invoice, policy, and attachment hashes in idempotency equality.
- Reject a reused idempotency key when any immutable issuance field differs.

Payer write routes:

- `POST .../verify/email/start`
- `POST .../verify/email/confirm`
- `POST .../verify/identity/start`
- `GET .../verify`
- `POST /v1/webhooks/identity`

Do not extend the payer router's unrestricted CORS policy to write routes. In v1, verification is completed on Payday's hosted checkout.

### 7.2 Dashboard

Minimum merchant workflow:

- Authenticate
- Create and inspect customers
- Create invoices
- Upload and finalize PDFs
- Select one of four payer modes
- View payment and verification status separately
- Identify likely unsolicited transfers
- Initiate review or retry where permitted
- Download invoice PDF and Proof of Payment
- List and filter invoices and customers
- Review recovered balances associated with invoices

The dashboard uses the same API as external integrations and must not duplicate policy logic.

### 7.3 Hosted checkout

The checkout:

- Shows issuer name and a heading description before any verification for gated invoices.
- Hides the amount, `bill_to`, notes, reference, PDF attachment, address, URI, and QR until the active session satisfies policy.
- Reveals full invoice content and payment mechanics only after the required verification level is met (see §4.3).
- Shows verification requirements before payment controls.
- Supports resuming after returning from a hosted KYC flow.
- Clearly distinguishes email verification, matched identity verification, and unattributed human verification.
- States that verification does not prove ownership of the sending wallet.

### 7.4 CLI

Removed. Payday is API-first with the dashboard for management; automation
uses the API or the TypeScript SDK directly. Offline proof verification lives
in `gateway_core::verify_proof`.

The existing test forbidding the word "invoice" should be deliberately updated. "Invoice" is now the correct product term.

---

## 8. Consolidated schema plan

Payday is not in production. Do not create one migration per roadmap slice.

Implement the entire product schema as one consolidated migration for reviewability. Before the first production release, squash it into the baseline migrations if that leaves a cleaner fresh-install schema.

The consolidated schema includes:

### `invoices`

Add:

- Structured invoice document (issuer, bill_to, notes, heading, reference)
- Four-mode `payer_policy`
- `verification_completed_at`
- `attribution_version`
- `attribution_nonce`
- `attribution_hash`
- Likely-unsolicited funding timestamp or equivalent flag
- Existing recovery address retained but populated from platform configuration

Do not add line-item columns or reconciliation functions.

### `customers`

Merchant-owned customer records:

- Account
- Name
- Email
- Free-text details
- Timestamps

Issued invoices store snapshots rather than depending on mutable customer fields.

### `invoice_attachments`

- Invoice ID
- Object key
- Original filename
- MIME type
- Byte length (max 5 MiB)
- SHA-256
- Scan/finalization status
- Created timestamp

One row per invoice (enforced by a unique constraint on invoice_id). No attachment-type enum in the initial model.

### `payer_verifications`

One row per verification attempt:

- Invoice and account
- Verification kind
- Status
- Provider
- Provider reference
- Payer reference
- Expected-identity assertion hash where applicable
- Verified and expiry timestamps
- Created timestamp
- Optional ISO country code

No extracted identity or biometric data.

### `payer_credentials`

Merchant-scoped reuse ledger:

- Account
- Payer reference
- Credential kind
- Status
- Expected-identity assertion hash where applicable
- Verified and expiry timestamps

### `payer_sessions`

- Hashed opaque token
- Invoice
- Payer reference
- Satisfied verification facts
- Expiry
- Created timestamp

### Payment observations and webhooks

- Persist whether policy had been completed when funding was first observed.
- Add verification approved/declined events.
- Add recovered-funds and likely-unsolicited indicators as needed.
- Update the existing Postgres webhook trigger in the same consolidated schema change.

No quarantine status or quarantine table is needed.

---

## 9. Implementation roadmap

### Slice 0 — Vendor and compliance readiness

**Effort:** M · **Blocking:** production identity verification

- Complete Didit DPA and sandbox diligence.
- Confirm processor/controller role, residency, retention/deletion controls, non-Latin support, name matching, session expiry, resume behavior, and external-reference isolation.
- Write biometric retention and destruction policy.
- Define the payer appeal route and manual review process (founder-operated initially).
- Obtain counsel guidance on GDPR consent and automated decisions.
- Recovery-wallet: provision AWS KMS key; recovered funds handled by manual review (no real users yet, no automated disbursement needed).

Attribution, contract, and non-KYC invoice work can proceed in parallel.

### Slice 1 — Contract and consolidated data foundation

**Effort:** L

- Change `Payment.sol` to send exactly `amount` to the receiver and any remainder to Payday recovery.
- Keep expired deployment and `recover()` full-balance recovery behavior.
- Update contract events and documentation.
- Deploy a new `PaymentFactory` build.
- Redeploy or repoint `BatchSweeper`.
- Replace merchant-supplied recovery with centrally configured recovery.
- Apply the single consolidated product schema.
- Update configuration, runbooks, SDKs, and API documentation.

**Acceptance:**

- Exact funding sends exactly the invoice amount to the receiver.
- Overpayment sends the invoice amount to the receiver and only the remainder to recovery.
- Underfunded live deployment reverts.
- Expired deployment sends the complete balance to recovery.
- `recover()` sends the complete later balance to recovery.
- Predicted addresses agree between Solidity and Rust.
- Gateway and indexer reject a mismatched factory/BatchSweeper deployment.
- Merchants cannot choose the recovery address.

### Slice 2 — Invoice document, PDFs, dashboard, and proof

**Effort:** L–XL · **Depends on:** Slice 1

- Add issuer, `bill_to`, amount, notes, heading, reference, and payer-policy DTOs.
- Do not add line items.
- Add presigned PDF upload (1 file, max 5 MiB), validation, scanning, finalization, and signed download.
- Render invoices and attachments on `/pay/{id}` — with progressive content visibility per §4.3 for gated invoices.
- Generate a deterministic Payday invoice-summary PDF.
- Add dashboard create/list/detail flows.
- Implement canonicalization, simple attribution hash, and derived salt.
- Add Proof of Payment export (merchant-accessible, shareable at merchant's discretion) and offline verifier.
- Extend idempotency comparison to every issuance field and attachment hash.

**Acceptance:**

- Merchant-specified amount is used directly without line-item validation.
- Non-PDF, oversized (>5 MiB), or unscanned files cannot be attached to an issued invoice.
- More than 1 attachment per invoice is rejected.
- Attachment bytes match their stored SHA-256.
- Different immutable invoice content under one idempotency key returns `409 idempotency_conflict`.
- Canonical invoice → hash → salt → CREATE3 address verifies offline.
- Identical invoices receive different salts and addresses.
- Address verification and settlement continue to depend on stored salt alone.
- Internal merchant data not intended for the payer never reaches the payer DTO or DOM.

### Slice 3 — Four payer modes, email verification, and sweep gate

**Effort:** XL · **Depends on:** Slices 1–2

- Implement all four payer-policy variants and validation.
- Add dedicated Auth0 payer audience.
- Add email start, confirm, and status routes.
- Add opaque payer sessions.
- Make address, URI, and QR session-dependent.
- Add masked expected-email hint.
- Persist invoice-level verification completion.
- Gate `claim_sweep_batch` for live invoices.
- Preserve unconditional expiry recovery eligibility.
- Record and surface likely unsolicited transfers.
- Add checkout verification state and resume behavior.
- Restrict payer POST CORS to Payday's hosted origin.

**Acceptance:**

- Permissionless invoices settle unchanged.
- Verified invoices never expose or sweep a live payment before requirements pass.
- Correct email verification unlocks only the relevant payer session.
- A pre-verification payment remains on the same address and is not quarantined.
- Verification before expiry allows that address to settle.
- Expiry without verification routes its complete balance to recovery.
- Verification after expiry cannot revive settlement.
- Direct database tests prove the sweep claim cannot bypass verification.
- Playwright tests cover all four modes and address visibility.

### Slice 4 — Matched and unattributed identity verification

**Effort:** XL · **Depends on:** Slices 0 and 3

- Add thin Didit provider implementation.
- Create hosted sessions lazily.
- Implement expected-detail matching for `verified_identity`.
- Implement document/liveness-only approval for `verified_identity_unattributed`.
- Persist status-only results.
- Add merchant-scoped credential reuse.
- Bind matched-identity reuse to the expected-identity assertion hash.
- Add dropped-webhook reconciliation polling.
- Add retry and human-review workflows.
- Add consent, privacy, retention, and limitation copy.

**Acceptance:**

- Identity mismatch declines without extracted identity entering persistence.
- Unattributed mode approves any payer who passes the configured document and liveness checks.
- Both identity modes also require current email verification.
- A generic document credential may satisfy unattributed mode for the same merchant and payer reference.
- A matched credential cannot satisfy a different expected identity.
- Another merchant cannot reuse the credential.
- Polling unlocks an approved session even when every webhook is lost.
- Logs and database fixtures contain no vendor-extracted identity data.

### Slice 5 — Reporting and operating product

**Effort:** XL

- CSV and journal-entry exports.
- AR aging and statements.
- Revenue reporting by customer and reference.
- Recurring invoices.
- Invoice email delivery using existing SES infrastructure.
- Recovery-wallet reconciliation and operational reports.
- Merchant-facing unsolicited-payment flags.
- Customer-requested accounting integrations.

Do not introduce line-item or tax modeling to support reporting.

### Slice 6 — Enterprise policy

**Effort:** XL

- Account-defined required metadata fields.
- Controlled metadata vocabularies.
- Approval workflows before issuance.
- Required-PDF gates where customers demonstrate demand.
- Role-based multi-user accounts.

Do not turn the four payer modes into an arbitrary verification-policy builder without demonstrated demand.

---

## 10. Verification strategy

### Solidity

Forge tests must cover:

- Exact funding
- Overpayment
- Underpayment
- Expired full recovery
- Later `recover()`
- Zero remainder
- Recovery-transfer failure
- Event amounts
- Batch isolation when one sweep fails
- Factory address prediction after recompilation

### Rust and database

Tests must cover:

- Four-mode policy parse and validation.
- Unattributed mode rejecting `expected_identity`.
- Idempotency conflict when document, policy, assertion, or attachment differs.
- Canonicalization stability across field order.
- Salt derivation and Rust/Solidity address parity.
- `address_matches_parameters()` using stored salt alone.
- Live sweep eligibility requiring verification.
- Expired sweep eligibility ignoring verification.
- No quarantine transition.
- Correct unsolicited-funding classification.
- Merchant-scoped credential isolation.
- Assertion-hash binding for matched-identity reuse.
- Webhook persistence allowlist.

### Web

Tests must cover:

- All four checkout modes.
- For gated invoices: only issuer name and heading visible before verification; full content revealed after appropriate verification level.
- For permissionless invoices: all content visible immediately.
- Address and QR hidden before verification.
- Address and QR revealed only to the qualifying session.
- PDF access and rendering (only after content is unlocked for gated invoices).
- KYC return/resume flow.
- No full expected email exposure.
- Clear distinction between matched and unattributed identity verification.
- Expired funded invoices remaining unpayable.

### End-to-end

Verify:

- Invoice issuance with PDF attachment.
- Offline Proof of Payment reconstruction.
- Pre-verification funding followed by successful verification and settlement.
- Pre-verification funding followed by expiry and recovery.
- Overpayment split between receiver and recovery.
- Dropped Didit webhook recovered by polling.
- Credential reuse for the same merchant and isolation across merchants.
- Likely unsolicited payment displayed without quarantining the address.

---

## 11. Regulatory framing

*Engineering-grade framing, not legal advice. Counsel must review identity verification and custodial recovery before launch.*

The current design assumption is that Payday has no general AML/BSA KYC obligation. It therefore cannot rely on GDPR Art. 6(1)(c) "legal obligation" to justify expansive collection.

Potential bases include:

- Contract under Art. 6(1)(b)
- Legitimate interests under Art. 6(1)(f), subject to balancing
- Explicit consent under Art. 9(2)(a) for biometric processing

Consequences:

- Identity requirements are opt-in per invoice.
- Payday does not proxy document or selfie capture.
- Vendor-extracted identity is not stored.
- A retention/destruction policy exists before collection.
- Declined payers have a defined human-review route.

Unregulated does not mean "retain for five years." BSA/CIP and EU AMLD retention floors bind obliged entities. Retaining unnecessary KYC records would worsen Payday's data-minimization position.

Biometrics remain the largest liability tail. Vendor-held data reduces but does not eliminate that risk.

### OFAC and sanctions

OFAC obligations may apply even where AML/KYC obligations do not. Sanctions screening is not part of the initial product launch, but will be incorporated in the future — either through Didit's AML/PEP add-on capability or a separate screening step. This is a deferred-but-planned capability, not a permanent exclusion. It should not become an optional merchant-selectable payer mode; when implemented, it should be a platform-wide control.

### Custodial recovery

Moving overpayments, expired balances, and stray funds to Payday's wallet creates actual custody over those amounts.

For the initial product:

- The recovery wallet key is held in AWS KMS (infrastructure already in use).
- Recovered funds are reviewed manually by the operator and returned to the appropriate party. No automated disbursement.
- Payday has no real users yet, so this is a manual, non-scaling process — appropriate for the current stage.
- Maintain an invoice-level ledger for every recovered amount so manual review has a clear record.

Before production launch with real users, Payday needs:

- Counsel's classification of this recovery role.
- Terms defining ownership of recovered funds.
- A return/disbursement procedure (manual for now, formalized later).
- Segregation and key-control policy for the KMS-held recovery key.

The intended invoice amount still moves directly to the merchant, but the prior blanket "Payday never takes custody" statement is no longer accurate — Payday takes custody of *recovered* amounts only.

---

## 12. Explicit non-goals

- Line items, quantities, unit prices, subtotals, discounts, or total reconciliation.
- Tax schemas or jurisdiction-specific forms.
- In-house document, face, or liveness verification.
- Arbitrary merchant-configured verification combinations.
- On-chain identity attestations.
- Merkle proofs or field-level selective disclosure.
- Generic file storage or non-PDF attachments.
- Typed attachment roles in the initial product.
- Cross-merchant KYC reuse.
- Deep SaaS integrations before customer demand.
- Multi-currency or FX.
- Escrow or discretionary custody of the intended invoice amount.
- Quarantining addresses that receive pre-verification funds.
- A claim that KYC cryptographically identifies the sending wallet.

---

## 13. Decisions taken

| Decision | Direction |
|---|---|
| Payer modes | Four presets: permissionless, verified email, matched identity, and unattributed identity |
| Invoice amount | Merchant specifies it directly |
| Line items | Not a Payday primitive |
| Itemization | Merchant may attach a PDF |
| Attachments | PDF only, 1 file, max 5 MiB, immutable at issuance |
| Invoice content visibility | Full content gated behind verification for gated invoices; issuer name and heading visible before verification |
| Recovery destination | Payday-controlled custodial wallet, AWS KMS key |
| Live contract settlement | Exact invoice amount to receiver; remainder to recovery |
| Expired and later funds | Complete balance to recovery |
| Recovered funds handling | Manual review by operator; no automated disbursement (no real users yet) |
| Verification enforcement | Withhold payment mechanics and invoice content; gate live indexer sweeps |
| Pre-verification funding | Record and wait; no quarantine |
| Griefing | Accepted as economically prohibitive and operationally identifiable |
| Attribution commitment | Simple canonical hash plus random nonce |
| Merkle root | Rejected until selective disclosure is a demonstrated product need |
| Proof of Payment | Retained as an offline-verifiable invoice-to-address-to-transfer record |
| Proof of Payment access | Merchant-accessible; shared at merchant's discretion |
| KYC vendor | Didit behind a thin provider trait, subject to diligence |
| KYC reuse | Merchant-scoped only |
| Credential lifetime | Minimum reasonable, up to vendor capability |
| Identity storage | Status-only; no vendor-extracted identity |
| Expected identity fields | Minimal data to satisfy merchant demand — legal name only to start |
| Name representation | Follows Didit's capabilities and best practices; Payday imposes no custom normalization |
| Human review | Founder-operated manually; formalized as volume grows |
| Decline retries | One automated resubmission, then human review or cancellation |
| Sanctions screening | Deferred; will use Didit AML/PEP capability or separate screening in the future |
| Schema rollout | One consolidated pre-production migration, optionally squashed into baseline |
| Vocabulary | Invoice for the document; payment for fulfillment |
| Dashboard MVP | Create/list/detail invoices and customers, payer-policy state, verification review, recovered-funds visibility, and proof downloads |

---

## 14. Open items for future resolution

These are not blocking the initial build but should be addressed before production launch with real users:

1. **Sanctions screening implementation:** Incorporate via Didit AML/PEP add-on or a separate screening step. Platform-wide, not a merchant-selectable mode.
2. **Recovered-funds legal framework:** Counsel must classify the recovery role, define ownership terms, and formalize the return/disbursement procedure before real users are onboarded.
3. **Credential lifetime tuning:** Start with the minimum the vendor supports; adjust based on merchant risk appetite and counsel guidance once usage data exists.
4. **Expected identity field expansion:** Add DOB or country only if merchants demonstrably need stronger matching and counsel approves.
5. **Human review transition:** Move from founder-operated manual review to a dedicated operator or team as volume grows; document the process in merchant terms.
6. **Counsel review of KYC and biometric handling:** Required before the document verification feature ships to production.
