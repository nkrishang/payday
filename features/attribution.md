# Attribution

Status: **proposal / not yet implemented.** This document records both the plan
and the reasoning that produced it.

## Why this document exists

Payday's core product bet beyond "get paid in USDC" is **attribution**: the
structured, verifiable record of *why* money moved. Today an invoice creator
can attach a `reference`/`memo`, which is just text. The question this document
answers is what a first-class attribution offering looks like, and what the
levels between "raw text" and "a fully rendered document set" actually are.

The prompt for the work was products like deel.com and bill.com, where a large
part of the value is the structured information attached to each payment. The
conclusion below deliberately does *not* copy their field set — see
"Jurisdiction-agnostic" and "Payday is AR-shaped".

## Where we are today (audit)

The attribution surface is three columns on `invoices`:

- `reference` — free text, ≤128 chars (migration `0008`)
- `memo` — earlier field (`0007`), now a compatibility alias for the same value
- `metadata` — JSONB, ≤16 keys, ≤512 bytes/value

Four findings from reading the code shaped everything below:

1. **The payer never sees any of it.** `PayerPaymentResponse`
   (`crates/gateway-core/src/dto.rs:111`) deliberately omits merchant data, and
   the SDK documents why: *"The payment page is world-readable, so this is the
   only payment shape safe to render on it."* `/pay/{id}` renders amount,
   countdown, QR, address, and status — the reference appears nowhere.
   Attribution stops at the merchant's own API/CLI/webhook.
2. **Nothing is derivable.** `reference` and `metadata` are opaque strings the
   merchant round-trips to itself. Payday cannot render, validate, total, or
   roll them up. There are no line items and no customer entity.
3. **Attribution is not bound to the money.** The CREATE3 deployment salt
   commits token, amount, receiver, expiry, and recovery — plus 32 random bytes
   (`crates/gateway-core/src/salt.rs:12`). Nothing about *why*. The record is
   editable database text sitting beside an immutable payment.
4. **There is no merchant dashboard.** `web/app/` has exactly two routes: `/`
   and `/pay/[id]`. Nothing in `web/` uses `PaydayClient`, only
   `PaydayPayerClient`.

## Strategic frame

**1. One payment, one address ⇒ attribution is unambiguous by construction.**
ACH addenda records and wire OBI fields are free text that finance teams
hand-match. Raw stablecoin transfers are worse — a treasury team sees four USDC
transfers land on Tuesday with no idea which invoice each belongs to. Payday
already eliminates the matching problem *structurally*. So attribution here is
not "solve matching"; it is "what do we hang off an anchor that is already
unambiguous", and "what can we prove about it".

**2. Payday is AR-shaped; Bill.com and Deel are AP-shaped.** They are
accounts-*payable* products: purchase-order matching, approval chains, spend
coding. Payday's model is the reverse — a merchant requests, someone pays. So
the attribution set is AR-first (what was delivered, to whom, for which
project). Approvals and PO matching belong at L4, if and when Payday adds
payouts. Copying their field list would build the wrong product.

**3. Jurisdiction-agnostic is a hard constraint, not a stage.** Stablecoins are
a cross-jurisdiction rail; that is the point of the product. The moment Payday
models US tax it becomes a US product, and owes every other jurisdiction the
same depth. So: **no tax rates, no tax identity, no jurisdiction-specific forms
(W-9, W-8BEN, 1099, 1042-S), ever.** Merchants and payers own their tax
obligations end to end. This is the same reasoning that keeps Payday on
stablecoins rather than fiat.

This yields a positioning line, and a stronger one than the incumbents can make:

> **Payday gives you a complete, verifiable record of why money moved. What
> your jurisdiction requires you to do with it is yours.**

It also pairs with L3' below: a cryptographic proof is jurisdiction-neutral —
the math is identical everywhere.

## Deriving the invoice from first principles

Rather than importing a standard invoice schema, apply one test to every
candidate field:

> **Does Payday *act* on this field — sum it, verify it, render it, send it,
> aggregate it? If not, it is merchant text and belongs in `metadata`, not in a
> type.**

Running the test over the conventional invoice:

| Field | Verdict |
|---|---|
| line items (description, qty, unit price) | **Keep.** Payday sums and verifies them against `amount`, renders them, can aggregate them. |
| issuer / bill-to **name**, **email** | **Keep.** Rendered; email is actionable (Payday can send the invoice — SES is already wired). |
| notes | **Keep.** Rendered to the payer. Cheap. |
| tax rate, tax IDs, tax forms | **Cut.** Jurisdiction-specific; not Payday's business. |
| `invoice_number` | **Cut — redundant.** `reference` already is the merchant's invoice number. |
| `due_date` | **Cut — redundant.** `expiration_timestamp` already is the due date, and unlike a paper invoice it is *enforced*, not suggested. A second date the system never acts on is just text. |
| `discount` | **Cut.** A discount is a negative line item. Totals still reconcile, one less field. |
| `subtotal`, `total` | **Cut.** Derived from line items; `amount` already exists. |
| structured postal address | **Cut.** Exists to serve tax and legal regimes. |
| `currency` | **Cut.** Hardcoded USDC (`amount.rs:9`, `dto.rs:255`). |
| payment terms, late fees | **Cut.** Not actionable → `notes`. |
| logo | **Defer.** Rendering only, and needs blob storage. |

**The escape hatch.** Some merchants' jurisdictions require printing a VAT
number or a company registration on the document. Rather than model any of it,
give each party one free-text `details` block that Payday renders verbatim and
never parses. Payday stays agnostic; the merchant prints whatever their
jurisdiction demands. This is the seam that keeps the constraint from becoming
a limitation for the merchant.

## The ladder

### L0 — Free text *(where we are)*

`reference` + `metadata`. Merchant-internal, opaque, invisible to the payer.

---

### L1 — The payment becomes a document *(highest leverage, cheapest)*

The whole typed object, after the reduction above:

```
issuer     { name, email?, details? }     # details: free text, rendered verbatim
bill_to    { name, email?, details? }
line_items [ { description, amount, quantity?, unit_price?, category? } ]
notes?
```

plus the two fields that already exist and already do the job: `reference`
(the invoice number) and `expires_at` (the due date).

**Invariants, enforced server-side in `Amount` base units, never floats:**

- `sum(line_items[].amount) == amount`
- where `quantity` and `unit_price` are both present,
  `quantity × unit_price == amount` for that line

A line may carry a negative `amount` (discounts, credits), so long as the sum
reconciles. Totals are **derived, never declared** — the document and the money
can then never disagree. That is the seed of every level above.

`category` is the one speculative field: Payday does not act on it yet, so by
the test it does not strictly earn a type. It is included because it costs
nothing now and unlocks "revenue by category" at L5 — but it is a fair thing to
cut from Slice 1.

**Unlocks:** `/pay/{id}` renders a real invoice instead of a payment box;
Payday generates a PDF *deterministically from structured data* (no upload, no
drift); revenue filterable by client and project; and the pitch changes from
"payment link" to "invoice", the noun customers actually buy.

**Cost:** additive columns + DTO fields + one render surface. No blob storage,
no multi-user, no integrations.

---

### L2 — Counterparties become objects

Attribution today is per-payment and stateless. Promote the L1 party into a
stored `customer` entity: name, email, `details`, payment history.

**Unlocks:** reuse ("bill Acme again"); **AR aging and statements** — the
single most-requested artifact of any invoicing product, and impossible without
this level; per-counterparty payout-address memory with reuse warnings (a real
safety feature on a permissionless rail); recurring invoices; emailing the
invoice to the client (SES is already wired for payout-attention mail —
`crates/gatewayd/src/dispatcher.rs`); and a payer identity, which L3 needs.

---

### L3 — Typed attachments and external links

**Yes to files — but typed, never a generic file store.** The discipline that
keeps this from becoming a dead blob bucket: every attachment declares a *role
in the record*.

```
contract | sow | receipt | timesheet | delivery_proof | supporting_doc
```

All jurisdiction-neutral. Typed attachments do what untyped ones cannot: show
the SOW beside the invoice on the payment page, require a signed contract
before the link goes live, attach delivery proof to a disputed payment.

Mechanics: presigned upload to S3/R2 (none exists today — request bodies are
capped at 64 KB by `RequestBodyLimitLayer` in `api/routes.rs`, so uploads must
bypass the API body path entirely), content-addressed by `sha256`, immutable
once paid, size and MIME allowlists, virus scan.

**Visibility is a security requirement, not a preference.** Possession of the
`pay_<uuid>` link *is* the authorization on the payer route — there is no payer
identity. A `payer_visible` attachment is therefore world-readable to anyone
holding the link. Default everything to `internal`; make `payer_visible`
explicit and serve those through short-lived signed URLs. The existing
merchant/payer DTO split is exactly the right seam.

**On DocuSign and adjacent SaaS: do not build integrations early.** The right
primitive is a generic **typed external link**:

```json
{ "type": "contract", "provider": "docusign",
  "external_id": "env_abc", "url": "…", "content_sha256": "…" }
```

Roughly 80% of the value at 5% of the cost, and it composes across DocuSign,
Linear, GitHub, Notion, Harvest, and Stripe. Build a deep integration only for
a provider customers ask for twice. The integration genuinely worth early
attention is the *inbound* direction — creating Payday payments from where the
obligation already lives (a Harvest timesheet, a Linear issue) — but that is
distribution, not attribution.

---

### L3' — Verifiable attribution *(the differentiator; ship early)*

The salt is opaque `bytes32` on both sides — `sol! { tuple(address, uint256,
address, uint64, address, bytes32) }` in `deterministic_address.rs:19` — and it
is *not* a constructor argument in `foundry/src/Payment.sol`; it only feeds
`PaymentFactory.deploymentSalt`. So replace:

```
salt = random()          →     salt = keccak256(nonce ‖ attribution_root)
```

where `nonce` stays 32 CSPRNG bytes and `attribution_root` is a merkle root
over the canonicalized attribution set (L1 fields, later L3 content hashes).

**The address the payer paid then *is* a commitment to the exact document
set.** Anyone recomputes the root from the disclosed set, recomputes the salt,
recomputes the CREATE3 address, and checks it against the address that received
USDC on a public chain. Nobody — including Payday — can alter or backdate the
record afterward.

Cost: **zero gas, zero on-chain storage, zero contract change.** The commitment
is already being deployed on every payment.

**Three properties that must hold:**

- **Hiding.** Keeping a random `nonce` in the preimage keeps the salt uniformly
  random, so the address leaks nothing about the attribution set and identical
  invoices still get distinct addresses. Without it, addresses become grindable
  from guessable attribution — a privacy and safety regression.
- **Anchoring must never gate settlement.** `address_matches_parameters`
  (`invoice.rs:194`) recomputes the address from stored params and gates
  sweeping. Keep storing `salt` itself and keep that check working from `salt`
  alone; `nonce` and `root` are extra columns used only for proof. Losing them
  must cost provability, never funds.
- **Forward-only.** Existing rows with random salts stay valid and unprovable.

**Honest limits, to be stated in the UI:** it commits, it does not reveal —
verification requires disclosing the preimage (a feature: selective disclosure
to a counterparty or auditor); and it freezes the set at *issuance*, which
conflicts with attaching a receipt later.

**Resolution — two tiers, labelled distinctly:**

- **Committed core** — economic terms + the invoice as issued, bound into the
  address at creation.
- **Appended log** — later additions in an append-only hash chain signed by
  Payday. Optionally anchor its head at settlement: *commitment at issuance in
  the address, commitment at settlement in execution.*

Ship this as a **Proof of Payment** — a shareable, offline-verifiable receipt
tying chain transfer ↔ invoice document ↔ counterparty. This is the marketing
hook, it is jurisdiction-neutral by nature, and no incumbent can match it,
because their rail has no such object.

---

### L4 — Policy and workflow

Attribution stops describing and starts enforcing: approval chains before a
link goes live; per-account required-field policy ("every invoice carries a
project code and a cost center"); account-defined controlled vocabularies;
issuance gated on a required document being attached. This is the enterprise
tier, and it forces multi-user accounts (today: one account, one API key).

---

### L5 — Export and reporting

Attribution that never leaves Payday is a chore, not a feature. The
jurisdiction-agnostic framing makes this level *simpler*, not harder: Payday's
job is to emit a complete, clean, verifiable record — not to interpret it.

- CSV and journal-entry export covering the full attribution set
- Aggregation: revenue by client, project, category; AR aging
- Accounting-tool sync (QuickBooks/Xero/NetSuite) as a later, customer-driven
  addition — mapping a Payday `category` to the customer's own GL account, with
  the customer owning the mapping and the tax treatment

## Cross-cutting principles

1. **Derivable, not declarative.** Anything Payday can compute, it computes.
   Declared-only fields drift and become untrustworthy.
2. **A field earns a type only if Payday acts on it.** Otherwise it is
   `metadata` or free text. This is the test that keeps the schema honest.
3. **Jurisdiction-agnostic by construction.** No tax, no tax identity, no
   jurisdiction-specific documents. Where a merchant's jurisdiction requires
   something printed, it goes in a free-text block Payday renders and never
   parses.
4. **Two audiences, one record.** Every field and file carries a visibility.
   Extend the existing merchant/payer DTO split; never bypass it.
5. **Immutable after payment.** Once money lands the record freezes; changes
   become an append-only amendment log.
6. **Attribution is a first-class API object** — filterable, webhook-carried,
   SDK-typed — not a blob hanging off the side.

## What *not* to build

- Anything tax- or jurisdiction-shaped. See principle 3.
- A generic "attach any file" bucket (L3 without types) — all cost, no product.
- DocuSign/QuickBooks connectors before the generic typed link.
- Multi-currency or FX. `fee_amount = '0'` and `net_amount = amount` are hard
  DB CHECKs; leave settlement math alone.
- Redundant document fields — `invoice_number`, `due_date`, `subtotal`,
  `discount`. Each is already covered by `reference`, `expires_at`, or the line
  items.
- A merchant dashboard, yet. L1's payoff lands on `/pay/{id}` and in the
  generated PDF, both cheap. The dashboard becomes necessary at L2, not before.

## Recommended sequencing

**Slice 1 — L1 + payer-facing invoice render + PDF receipt.** Entirely
additive. No blob storage, no multi-user, no integrations. Changes the pitch
from "payment link" to "invoice". The PDF is the demo.

**Slice 2 — L3' hash-anchoring over L1 data only** (no attachments yet). One
function changes, and it delivers the differentiated story immediately: *every
Payday invoice is cryptographically bound to the address that was paid.*
Attachments later slot into the same merkle root.

**Then:** L2 counterparties → L3 attachments → L4 policy → L5 export.

### Slice 1 — files and gotchas

- `crates/gateway-db/migrations/0011_*.sql` — document storage as validated
  JSONB, mirroring the existing `payment_metadata_values_fit` SQL-function
  pattern from `0008`.
- `crates/gateway-core/src/dto.rs` — `CreatePaymentRequest` (note
  `#[serde(deny_unknown_fields)]`), `PaymentResponse`,
  `PaymentSummaryResponse`, and the payer-visible subset on
  `PayerPaymentResponse`.
- `crates/gatewayd/src/api/invoices.rs` — totals reconciliation in
  `create_payment` (alongside `validate_customer_fields`, ~`:536`); projection
  at ~`:63`. **Extend the idempotency-replay equality check at ~`:527`** — it
  currently compares `memo`/`reference`/`metadata`; without the document, a
  replay carrying a *different* invoice silently returns the original.
- `crates/gateway-db/src/invoices.rs` — `DbInvoice`, `CreateInvoiceInput`
  (`from_invoice` at `:276` hardcodes `reference: None, metadata: {}` and the
  handler overwrites at `invoices.rs:290`; follow that pattern), `insert`
  (`:356`).
- `crates/gatewayd/src/api/openapi.rs` — the duplicated utoipa types (53–140).
- `crates/gateway-cli/src/cli.rs` / `main.rs` — `payday create` gains
  `--from-file invoice.json`; a document does not belong in flags. Today the
  CLI sends `reference: None, metadata: {}` and exposes only `--memo`.
  **Note `cli.rs` has a test asserting `!help.contains("invoice")`** — public
  vocabulary is deliberately "payment". Shipping L1 forces a deliberate call on
  that, because the thing being built genuinely is an invoice.
- `web/app/pay/[id]/page.tsx`, `web/components/checkout/` — render the
  document; PDF generation.
- `sdk/typescript/src/index.ts`, `docs/api-reference.md`,
  `docs/cli-reference.md`, `docs/concepts.md`.
- Webhooks: payloads are built by a **Postgres trigger** (`0009_webhooks.sql`,
  recreated in `0010`) via `jsonb_build_object`. Adding attribution to events
  means editing that trigger, not Rust.

### Slice 2 — files

- `crates/gateway-core/src/salt.rs` — derive rather than randomize; keep the
  "never accepts client-supplied salts" invariant.
- `crates/gateway-core/src/invoice.rs` — thread `attribution_root` through
  `Invoice::new` (`:150`); leave `address_matches_parameters` (`:194`) reading
  `salt`.
- New module — canonicalization + merkle root over the attribution set.
- Migration — `attribution_nonce BYTEA(32)`, `attribution_root BYTEA(32)`, both
  NULL for existing rows.

## Verification

**Slice 1**

- Rust unit tests: reconciliation rejects a document whose line items do not sum
  to `amount`; accepts exact match at six-decimal precision; accepts a negative
  line that still reconciles; rejects `quantity × unit_price ≠ amount`.
- Idempotency test: a replay with the same key but a different document returns
  `409 idempotency_conflict`, not the original.
- `just e2e` — create an invoice with line items; confirm `/pay/{id}` renders
  the document and that internal-only fields never appear in
  `PayerPaymentResponse`.
- A vitest alongside `web/components/checkout/*.test.tsx` asserting
  internal-only fields never reach the DOM.

**Slice 2**

- A **forge test** proving `PaymentFactory.paymentAddress(...)` returns the same
  address for a derived salt as for a random one — i.e. that the salt is
  genuinely opaque to the contract and anchoring changes nothing on-chain.
- Rust test cross-checking `predict_payment_address` against the forge result
  for a derived salt.
- Round-trip: canonicalize → root → salt → address, then re-verify from the
  disclosed set alone, with no DB access.
- Regression: `address_matches_parameters` still holds for rows with random
  salts and NULL `attribution_root`; the sweeper path is untouched.

## Open questions

- Does `category` stay in Slice 1, or wait until L5 gives it a consumer?
- The per-party free-text `details` block is the one place jurisdiction-specific
  content re-enters the document, as unparsed text. The alternative is to omit
  it entirely, which is cleaner but leaves some merchants unable to issue a
  document their jurisdiction accepts.
- Public vocabulary is currently "payment", enforced by a test in
  `crates/gateway-cli/src/cli.rs`. L1 introduces something that genuinely is an
  invoice; that test and the surrounding naming need a deliberate decision.
