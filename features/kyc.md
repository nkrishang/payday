# Payer requirements (KYC)

Status: **proposal / not yet implemented.** This document records both the plan
and the reasoning that produced it.

## Why this document exists

A Payday invoice produces `payday.sh/pay/{id}` — a checkout that is public and
anonymous by explicit design. `crates/gatewayd/src/api/routes.rs` states it
plainly: "A payment link is public by design: anyone holding it may read the
payment and fulfil it."

Merchants want to condition payment on knowing *who is paying*, from "prove you
control this mailbox" up to "present a government ID." The question this
document answers is what that feature is, how far it can actually be enforced
given Payday's architecture, and whether the identity-verification layer should
be bought or built.

The conclusion is: **buy it** (Didit), gate on *disclosure of the payment
address*, and structure the feature so Payday never becomes a repository of
payer identity documents.

## Where we are today (audit)

Four findings from reading the code shaped everything below.

1. **The payer response carries no merchant data at all.**
   `PayerPaymentResponse` (`crates/gateway-core/src/dto.rs:111`) exposes only
   payment mechanics — no `memo`, `reference`, or `metadata`. This is an
   explicit stance repeated in `routes.rs`, `web/README.md`, and
   `docs/api-reference.md`. `payer_message` (set only for blocked payouts,
   `payer.rs:113`) is the sole precedent for merchant-influenced payer-visible
   text.

2. **The `/pay/{id}` flow is read-only.** No POST on any payer route, no CSRF,
   no session, nowhere to put payer input. There is no middleware anywhere under
   `web/`, and `web/lib/payday.ts` says outright: *"It needs no API key: a
   payment link is open by design."*

3. **Auth0 is merchant-only and cannot be reused as-is.**
   `Auth0Verifier::verify` (`crates/gatewayd/src/api/auth.rs`) hard-codes
   merchant semantics: `sub` must start with `email|`, `authentication_method`
   must be `email_otp`, `azp` must equal the Payday client id, the token must be
   **under 5 minutes old**, and its `event_id` is consumed once in
   `account_key_authentication_events`. It is mounted on exactly one route group
   (`/v1/account/api-key`) — a bootstrap credential for minting an API key, not
   a session system.

4. **Nothing gates settlement on off-chain conditions.** Settlement is
   `dispatcher.rs` reacting to finalized transfers. `Payment.sol` is immutable
   and its address encodes the terms.

## The three hard constraints

### 1. Enforcement is only possible by withholding the address

`foundry/src/Payment.sol` sweeps whatever balance is present to the receiver. It
has **no notion of who sent the funds**, and `PaymentFactory` commits every
invoice parameter into a CREATE3 salt, so terms cannot change post-creation.
There is no on-chain hook to gate against, and **this plan changes no Solidity.**

What makes gating viable anyway: the salt is 32 bytes from the OS CSPRNG and is
never client-supplied (`crates/gateway-core/src/salt.rs`), so the counterfactual
address is **unguessable**, and is only ever disclosed by
`GET /v1/payer/payments/{id}`. Withholding `address`, `payment_uri` and the QR
is therefore a real gate, not theater.

Its honest limit — which must go in the docs, mirroring the existing
cancellation language — is that the gate is **per-viewer**. Once a verified
payer sees the address they can hand it to anyone, and funds from an unverified
sender are still detected, credited and settled. **The gate controls disclosure,
never receipt.**

### 2. On-chain attestations are unavailable on Monad

Production is Monad (`chain_id = 143`, `infra/terraform.tfvars`). EAS is not
deployed there, Coinbase Verifications are Base-only, and Chainlink ACE CCID did
not launch there either.

Independently, the crypto-native KYC brands have largely failed: Civic Pass
discontinued (IDV ended 1 Jul 2025), Circle's Verite archived read-only (4 Apr
2025), Fractal ID never recovered from its July 2024 breach — which exposed
uploaded ID images and selfies, a useful reminder of what we are choosing not to
hold. World ID has ~18M verified users (~0.2% of the world) and proves
personhood, not identity.

**No on-chain attestation work.**

### 3. Payer identity is a parallel path to Auth0's merchant path

Per audit finding 3. The clean seam: the Post-Login Action
(`auth0/actions/payday-email-otp.js`) early-returns when the audience is not the
Payday API audience, so a **separate payer audience** does not trip the merchant
deny-gate.

## Core design principle: merchant asserts, Payday confirms

The first framing of this feature was "Payday verifies the payer and tells the
merchant who they are." That is wrong: it makes Payday a PII conduit and an
independent GDPR controller.

Inverted, the problem dissolves. The **merchant asserts the expected identity**
(an email; at the document tier, an expected legal name), and Payday returns only
**whether the payer matched it**. Because the merchant supplied the value, the
pass/fail discloses nothing new. Payday is a confirmation oracle, not a data
pipe — same output the merchant wanted, near-zero disclosure surface.

Stated precisely, and this belongs in the payer-facing docs rather than being
implied away: the binding proves *whoever controlled that mailbox at time T
completed a liveness-checked document check in the same browser session.* It is
a **Payday-asserted binding, not a cryptographic one**. Same-session binding plus
liveness are the only mitigations against a payer handing a half-finished flow to
someone else. Every KYC-at-signup product has this same seam.

## Why a requirement *set*, not a tier ladder

The feature was originally scoped as tiers 0–4 (none → email → document → AML →
KYB). That is the wrong abstraction.

The plausible requirement types are **independent axes, not rungs**. A merchant
paying a known counterparty may want wallet-ownership proof and no document; one
paying a stranger may want a document and not care which mailbox was used. A
linear ladder forces a false ordering — it makes "document" imply "email" — and
needs a schema migration every time a new axis appears.

The market corroborates the orthogonality: Toku collects tax forms with
essentially no KYC program; Bridge and Mural run deep KYC and issue no tax forms;
Trolley sells them as separate SKUs (Pay / Tax / Trust). Stripe's own API is the
cleanest demonstration — `tosType=recipient` requires only `external_account` and
`tos_acceptance`, with no name, DOB or SSN, because "Stripe has no direct service
relationship with the recipient." Remove the regulated entity as the payee's
counterparty and payee KYC disappears. Wise is the control case: pay a contractor
via Wise Business and **Wise never KYCs them**.

So: model requirements as an independent set, each carrying its own verification
mode and satisfied/unsatisfied state. v1 implements `email` and
`identity_document`.

## How we chose a vendor

### Provider landscape

Evaluated against four hard constraints: self-serve signup (no enterprise sales
cycle), a real sandbox, genuine global coverage, and — decisively — support for
a **cold-open hosted link**, because Payday's payer is an anonymous link visitor
with no pre-existing account object.

| Vendor | Public price | Cold link | AML/PEP | Verdict |
|---|---|---|---|---|
| **Didit** | 500 free/mo, then $0.33 | yes | $0.20 add-on | **chosen** |
| Persona | $250/mo + $1.50/service | yes | yes | runner-up |
| Veriff | $0.80–1.89 + add-ons | yes | +$0.64 | no reuse product |
| Sumsub | $1.35–1.85 + $149/mo | applicant must pre-exist | yes | dual-role DPA |
| Stripe Identity | £1.25, 50 free/mo | yes | **none** | tier-2 only |
| Onfido/Entrust | not public | no | yes | sales-led |
| Jumio | not public | unconfirmed | yes | sales-led |
| Plaid IDV | not public | partial | partial | selfie absent in sandbox |
| Trulioo / Alloy | not public | — | yes | right tool later |

Aggregator "estimates" for Jumio and Onfido ($0.75–$8, $25k–$100k minimums) are
analyst guesses, not list prices; do not budget from them.

### Geographic reality

"200+ countries / 14,000+ document types" means document *recognition*, not
verification. Trulioo's own copy separates "195 countries" (database
interrogation) from "14,000+ document types" (OCR), and notes that where physical
documents are unreliable, database interrogation is the only viable approach —
the converse holds wherever no database exists. **No vendor publishes the
document-only vs database-backed split**; demanding that matrix in writing is the
best diligence question available.

The clearest citable failure is Stripe Identity, which lists ~110 countries for
document checks and then adds that it "doesn't support extraction of document
fields written in Arabic, Chinese, Cyrillic, Greek, Hebrew, Korean, Tamil, or
Thai script" — most of Asia, the Middle East and the Russian-speaking world
excluded at the OCR layer while the country stays on the list.

National eID schemes are essentially all closed to us: EU eIDAS 2.0 wallets do
not reach private-sector acceptance until late 2027; Aadhaar authentication is
limited by PMLA s.11A to a gazetted list; Singpass/Myinfo production requires a
Singapore-registered entity. Transliteration also degrades name matching on
non-Latin scripts — that Sumsub maintains dedicated transliteration docs is
itself the point, and it is why our name-match threshold must be explainable.

### Reusable / portable KYC

Vendor reuse networks (Persona Wallet/Connect, Sumsub ID, Jumio selfie.DONE) only
fire when the payer's *original* verification happened at another customer of the
same vendor. Hit rate starts near zero for a new product. Treat as a compounding
conversion optimisation, never a launch feature.

The high-leverage move is that **Payday builds its own reuse layer** — which
works from day one and is vendor-independent. Scoped to one merchant (see
Decisions), it also avoids making Payday the controller of a cross-merchant
identity ledger.

### The decision, and the one it reversed

The first recommendation was **Persona**, on the strength of its privacy
controls: webhooks can be stripped to a status-only payload
(`api-attributes-blocklist: ["*", "!status"]` plus `included-allowlist` and
`relationship-allowlist` set to `include_none`), and API keys are resource-scoped,
so a key with `inquiry.write` but without `verification.read` makes Payday
*structurally incapable* of reading the PII.

That recommendation was challenged and does not survive scrutiny, for three
reasons:

1. **The PII-free webhook is narrower than it looks.** Neither vendor sends
   biometric templates or document images to a webhook. What Persona's blocklist
   excludes is name and DOB — ordinary personal data, not GDPR Art. 9 special
   category. The biometric tail risk (BIPA/CUBI) is identical either way, and
   this codebase already guarantees "Headers and bodies are never logged"
   (`docs/payments-api.md`).

2. **Persona's match verdict is unproven; Didit's is documented.** Persona's
   Government ID *Inquiry comparison check* behaves correctly in principle —
   docs say "If you do not prefill the fields, this check will use details
   extracted from the end user's submission as the baseline," implying
   server-side prefill makes the merchant's assertion the baseline — but its
   machine name, return shape and fuzziness threshold are undocumented (the
   canonical table is behind dashboard auth), and the Workflows "Evaluate Code"
   fallback could not be confirmed to read prefilled and extracted fields in one
   scope. Didit documents the exact primitive as a first-class feature:
   `expected_details{first_name, last_name, date_of_birth, …}`, named risk codes
   (`FULL_NAME_MISMATCH_WITH_PROVIDED`), and
   `expected_details_mismatch_action=Decline`, which collapses the verdict to
   `status`. Choosing the vendor whose core primitive is a maybe over one where
   it is documented is the wrong risk trade.

3. **Cost and time-to-ship are not close.** Didit: 500 full KYC/month free
   forever, then $0.33 ($0.15 ID + $0.10 passive liveness + $0.05 face match),
   self-serve 60-second sandbox, no card, no contract, no pre-production review.
   Persona: $250/mo on an **annual contract**, $1.50/service overage (the $1.00
   figure circulating is the Startup Program *General* cohort; a stablecoin
   product lands in *Fintech*), a sandbox-only non-converting trial, and a
   production compliance review of "a few days to a few weeks."

**Didit's known weaknesses, carried into the design rather than glossed:** no
PII-free webhook (its `decision` payload is the full schema, with no field
allowlist), and only 2 webhook retries. Both are mitigated explicitly in
Implementation §6 and Verification.

### Why not Auth0's IDV marketplace integrations

Auth0 Marketplace carries Persona, Onfido, ID.me, Incognia, Vouched and Incode —
but every one is a **post-login Action** that fires only when a user
authenticates into Auth0. Payday's payers are anonymous link visitors. Routing
them through Auth0 signup to reach a document check would make **every payer a
billable MAU** on top of the per-verification fee.

The split: Auth0 for `email` only, where the plumbing already exists and 25k free
MAU covers any near-term volume; Didit server-to-server from `gatewayd` for
`identity_document`.

## Regulatory framing

*Engineering-grade framing, not legal advice. Counsel should review before the
document requirement ships.*

Because Payday has **no AML/BSA obligation**, it cannot rely on GDPR Art.
6(1)(c) "legal obligation" — which makes expansive collection a
data-minimisation problem rather than a safe default. The available bases are
contract 6(1)(b) (fits well: verification as a genuine condition of the invoice),
legitimate interests 6(1)(f) (fraud prevention is expressly legitimate per
Recital 47, but cannot cover biometrics), and explicit consent 9(2)(a) (required
for the selfie regardless).

Consequences baked into the design:

- Requirements are **opt-in per invoice**, never a default.
- Never proxy document or selfie capture through a Payday domain.
- Store only `{verification_id, type, status, verified_at, expires_at}` and at
  most an ISO country code.
- Write the retention policy **before** the first document collection. BIPA
  §15(a) requires a written retention/destruction schedule *prior to*
  collection, and an Illinois appellate court has held that merely lacking one is
  itself a violation.
- Provide a human-review path for declines — GDPR Art. 22 triggers when an
  automated decision blocks payment.

**Retention: unregulated means keeping less.** The 5-year floors people reach for
(BSA/CIP, EU AMLD) bind *obliged entities*. Payday is not one. There is no legal
floor requiring Payday to retain KYC data at all, and keeping it "to look
compliant" actively worsens the Art. 5(1)(c)/(e) position.

**Biometrics are the real tail risk.** BIPA carries a private right of action at
$1,000 negligent / $5,000 intentional per violation; Texas CUBI is AG-only at up
to $25,000 per violation and produced >$2.7B from Meta and Google combined.
Vendor-holds-the-data helps substantially but not absolutely — BIPA claims
against Microsoft were dismissed as "merely a vendor," while a claim against
Amazon survived where possession was adequately alleged.

**Two obligations that bind regardless of this feature:**

- **OFAC is strict liability with no de minimis, and it reaches Payday.** FinCEN
  exempts "users" of virtual currency; OFAC FAQ 560 explicitly does not, naming
  "administrators, exchangers, and **users**" as parties who should run sanctions
  screening. So: no KYC duty, real OFAC duty. Every platform researched makes IDV
  optional and screening mandatory — Trolley sells IDV as an add-on but stamps
  `complianceStatus` on every recipient; Tipalti screens at registration *and* at
  payment submission.
- **Non-custody is load-bearing.** FinCEN's 2019 guidance states that CVC payment
  processors "are not eligible for the payment processor exemption" — the
  "we're just a processor" defence that works for card rails is unavailable here.
  Only non-custody keeps Payday out of money-transmitter scope, tested as "total
  independent control over the value." Evaluate any future escrow, batching,
  netting or fee-sweeping feature against that sentence.

Payday is very likely **not a VASP** (FATF's Oct 2021 control test; MiCA excludes
non-custodial software providers), and the Travel Rule generally does not reach
unhosted-to-unhosted transfers.

**Unresolved tension to decide deliberately:** if Payday never receives the PII,
Payday cannot adjudicate appeals either — yet Art. 22 makes the human-review duty
Payday's, since Payday is controller and the vendor processor. Whoever holds the
vendor dashboard therefore performs a regulated function on behalf of every
merchant. No vendor researched ships an end-user appeal button; Didit's surface is
a Console "Request Resubmission". Decide who that operator is and reflect it in
the merchant terms, rather than leaving it as an accident of access control.

## Decisions taken

| Decision | Choice |
|---|---|
| v1 requirement types | `email` (Auth0), `identity_document` (Didit) |
| Vendor | Didit, behind a thin trait. No second vendor in v1 |
| Reuse | Within one merchant only — `(account_id, payer_ref, type)` |
| Merchant receives | Match verdict against merchant-asserted values only |
| Enforcement | Withhold `address` / `payment_uri` / QR |
| On-chain / Solidity | No change |
| Proof of address, screening, KYB | Deferred; schema leaves room |
| Tax forms / tax IDs | Out of scope permanently — jurisdiction-agnostic |

Reuse is deliberately merchant-scoped rather than global: cross-merchant reuse is
the better payer experience, but it makes Payday an independent controller of a
cross-merchant identity ledger. Merchant-scoped reuse keeps the merchant as
controller and still removes the friction that matters most (a payer billed
repeatedly by the same merchant).

## Step 0 — vendor diligence, blocking

Didit's capability story is documented, so the open risk is the vendor, not the
API. Resolve before writing integration code — all of it is answerable in about a
day. These are questions to answer, not reasons to expect a problem; but answer
them first, because the honest response to a bad answer is to escalate rather
than work around it.

1. **How is the free tier funded?** 500 full KYC/month free forever needs an
   explanation. Read the DPA for any secondary-use, data-sharing, or
   model-training clause. If the answer is data monetisation in any form, stop
   and raise it — disqualifying for a design premised on minimal disclosure.
2. **DPA role** — processor or dual-role controller? We need
   processor-to-the-merchant. (Sumsub, for contrast, is explicitly dual-role.)
3. **Data residency and certifications** — where documents are processed and
   stored, whether EU-region hosting exists, SOC 2 Type II / ISO 27001. All
   unverified.
4. **Retention controls** — configurable retention plus a deletion API, so the
   retention policy above is enforceable vendor-side.
5. **Sandbox spike** — confirm `expected_details` +
   `expected_details_mismatch_action=Decline` behaves as documented, then probe
   the name-match edge cases the docs do not cover: middle names, diacritics,
   transliteration, given/family order, married vs maiden names. Whatever the
   threshold behaviour is, we must be able to *explain* it (Art. 22). Also
   confirm the session TTL and whether session tokens live in per-tab storage.
6. **Accuracy on non-Latin documents** — unverified, and the weakest-documented
   area across every vendor researched.

Vendor durability is a real but bounded risk (seed-stage: YC W26, ~$7.5M raised,
~$3.3M revenue, reportedly profitable). The thin trait in §6 keeps a future
migration cheap; that is the whole reason it exists.

## Implementation

### 1. Schema — migration `0011_payer_requirements.sql`

> **Collision:** `features/attribution.md` also claims `0011_*.sql`. Whichever
> lands second takes `0012`.

- `ALTER TABLE invoices ADD COLUMN requirements JSONB NOT NULL DEFAULT '[]'`,
  with a CHECK that it is an array of at most 4 objects. Follow the existing
  `payment_metadata_values_fit` SQL-function pattern from `0008` rather than
  inventing a new idiom.
- `payer_verifications` — one row per attempt: `id`, `invoice_id`, `account_id`,
  `requirement_type`, `status` (`pending|approved|declined|expired`), `provider`
  (`auth0|didit`), `provider_reference` (vendor session id), `payer_ref`,
  `verified_at`, `expires_at`, `created_at`. **No name, DOB, document number, or
  images** — at most an ISO country code.
- `payer_credentials` — the merchant-scoped reuse ledger, PK
  `(account_id, payer_ref, requirement_type)`, plus `status`, `verified_at`,
  `expires_at`.
- `payer_sessions` — `token_hash` (SHA-256, mirroring `accounts.api_key_hash`),
  `invoice_id`, `payer_ref`, `satisfied JSONB`, `expires_at`. Stateful and opaque
  rather than a stateless HMAC: revocable, and no new secret to manage.

`payer_ref` is `HMAC(per-account secret, normalised_email)`, and is the value
passed to the vendor as the external reference on a session. This makes merchant
isolation **structural**: the vendor sees only an opaque per-merchant handle, so
records for the same human under two merchants cannot be auto-linked. It also
implements the merchant-scoped reuse decision directly.

Confirm Didit's external-reference field name in Step 0, and that it is not
treated as PII or used for cross-account duplicate detection. If any
cross-merchant reuse or shared-identity-network feature exists, leave it off — it
would defeat this isolation.

### 2. Core types — `crates/gateway-core/`

Add `PayerRequirement` to `invoice.rs` (an enum over `Email { expect }` and
`IdentityDocument { expect_name }`) and DTOs to `dto.rs`. Follow the existing
`InvoiceStatus` idiom: a single `as_str()` that is the source of truth for the
wire form, with `Display`, `FromStr` and the DB CHECK all agreeing with it — the
file explicitly calls that out as the pattern.

### 3. Create path — `crates/gatewayd/src/api/invoices.rs`

- Accept `requirements` on `CreatePaymentRequest`; validate alongside
  `validate_customer_fields()` (`:536`).
- **Add `requirements` to `RequestedInvoice` (`:510`) and to `same_request()`
  (`:522`).** Without this, replaying an idempotency key with *different*
  requirements silently returns the original ungated payment. Security-relevant,
  not a nit.

### 4. The gate — `crates/gatewayd/src/api/payer.rs`

This is the whole enforcement mechanism; the Next.js page is presentation only.

- In `payer_response()` (`:108`), when the invoice has unsatisfied requirements,
  omit `address` and `payment_uri`. `PayerPaymentResponse.address` becomes
  `Option<String>` — consistent with `payment_uri`, already optional for the same
  "not payable" reason. Only gated payments ever omit it, so existing
  merchant-built checkouts are unaffected in practice.
- `qr()` (`:189`) already returns `410 payment_not_payable` when not payable;
  extend the same guard to unsatisfied requirements.
- Add `requirements: Vec<PayerRequirementStatusDto>` to the response —
  `{type, status, expect_hint}`. **Mask the hint** (`a****@a***.com`): the
  merchant asserted the address, but anyone holding the link would otherwise read
  it.

### 5. New payer write routes

The payer router is currently GET/HEAD only with `allow_origin(Any)`.

- `POST …/verify/email/start` and `…/verify/email/confirm` — Auth0 passwordless
  against a **separate payer audience**; issues a `payer_sessions` token.
- `POST …/verify/document/start` — creates the vendor session **lazily, on first
  payer visit**. Verification sessions are short-lived (hours to a day) while
  invoices live up to 366 days, so creating one at invoice time would strand
  every invoice not paid the same day.
- `GET …/verify` — current per-requirement status.
- `POST /v1/webhooks/identity` — signature-verified inbound; flips
  `payer_verifications` and writes `payer_credentials` on approval. Keep the path
  vendor-neutral so a later swap does not change the URL.

**Do not extend `allow_origin(Any)` to these POST routes.** Restrict them to the
`PAYDAY_PUBLIC_BASE_URL` origin. Consequence to document: in v1 the verification
flow works only on Payday's own checkout, so a merchant's self-built cross-origin
checkout can read a gated payment but cannot complete verification.

### 6. Identity provider — new `crates/gatewayd/src/identity/`

A `PayerIdentityProvider` trait with a single `didit` implementation. Keep the
trait **thin** — session-create, status-fetch, callback-verify — so it documents
the vendor boundary and keeps a future migration cheap, without speculatively
generalising for a second vendor that does not exist yet.

- Create the session with `expected_details` populated from the merchant's
  assertion, and set **`expected_details_mismatch_action=Decline`** so the verdict
  collapses to `status` and the handler never needs to read the rest of the
  payload.
- Treat the callback as **status-only by discipline**: destructure `status` and
  the risk codes, never bind the remaining fields. Nothing vendor-side enforces
  this and it is easy to regress silently, so a unit test carries the guarantee.
- **Only 2 webhook retries.** Add a reconciliation poll for sessions still
  `pending` past a grace window, following the adaptive-backoff patterns already
  in `web/lib/poll.ts` and the webhook worker.
- Prefill/assert server-side only, never via query string — a payer-editable
  assertion is not an assertion.

### 7. Checkout — `web/`

- `checkout.tsx` gains a requirements step ahead of the pay step; extend
  `web/lib/checkout-state.ts` rather than branching in components.
- `PayerPayment.address` becomes nullable in `sdk/typescript/src/index.ts`; fix
  the resulting type errors in `wallet-pay.tsx`, `qr-panel.tsx`,
  `address-row.tsx`.
- **Build the resume path on day one.** Hosted-flow session tokens typically live
  in per-tab session storage, so a payer who opens the link in an email in-app
  browser and switches to Safari for camera access hits "Session expired"
  mid-payment. A common path, not an edge case.

### 8. CLI — `crates/gateway-cli/`

`payday create --require email:alice@acme.com --require id:"Alice Smith"`.
Repeatable flag parsed into `Vec<PayerRequirement>`; render
satisfied/unsatisfied state in `presentation.rs`.

### 9. Webhooks

`webhook_events.event_type` is a closed CHECK enum in `0009_webhooks.sql`. Adding
`payment.verification_approved` / `payment.verification_declined` requires
widening it in the same migration.

## Optional, separable: `wallet_ownership_proof`

Outside the two types chosen, but unusually cheap here — cut it freely to keep v1
tight.

A signed message from the paying wallet, verified by address recovery. Zero PII,
zero marginal cost, no vendor. It is simultaneously a Travel Rule originator
attestation, an anti-fraud control, and the only thing that ties a verified
off-chain identity to the on-chain address that actually pays — closing the
"verified payer forwards the address to someone else" gap that no KYC vendor can.

The client stack already exists: `wallet-pay.tsx` connects a wallet via wagmi, so
this is `signMessage` plus a recovery check in `gatewayd`.

## What *not* to build

- **In-house document/liveness verification.** Buying is unambiguously correct;
  the biometric liability alone settles it.
- **Anything on-chain.** Not on Monad, and the credential ecosystem is not
  production-viable in 2026 regardless.
- **Tax forms, tax IDs, jurisdiction-specific documents.** Payday stays
  jurisdiction-agnostic for the same reason it stays off fiat rails. Merchants
  with those obligations discharge them in their own systems; `reference` or
  `metadata` is the seam for correlation.
- **A tier ladder.** See "Why a requirement set".
- **Cross-merchant reuse**, and therefore Persona Wallet / Connect-style
  shared-identity networks, in v1.
- **Storing any payer PII**, including "just the name, for reconciliation."
- **Escrow, or anything else that takes custody** — it forfeits the one argument
  keeping Payday out of money-transmitter scope.

## Verification

**Unit (Rust).** Requirement parse/validate round-trips following the
`InvoiceStatus` test idiom; `same_request()` returns false when requirements
differ (guards the idempotency bug in §3); `payer_response()` omits `address` and
`payment_uri` exactly when requirements are unsatisfied and includes them
otherwise; masked hint never leaks the full asserted email; identity-webhook
signature verification rejects tampered bodies; **the webhook handler persists no
field outside the allowlist** — feed it a fixture containing name, DOB and
document number and assert none reach the database. Didit cannot enforce this
vendor-side, so this test *is* the confirmation-oracle guarantee and must not be
deleted casually.

**DB.** Migration applies cleanly; the `requirements` CHECK rejects >4 entries and
non-array values; `payer_credentials` PK enforces merchant-scoped reuse — assert a
second merchant does *not* get a skip for the same payer.

**Web.** `vitest` for the extended `checkout-state.ts`. Playwright already selects
scenarios by id via `web/e2e/stub-api.mjs` (`/pay/pay_settled`,
`/pay/pay_expired-funded`); add `pay_gated-email` and `pay_gated-verified` and
assert the address and QR are absent in the first and present in the second.

**End-to-end.** `just dev` (or `just e2e`) with `crates/dev-identity` covering the
email path — it already serves `/passwordless/start`, `/oauth/token` and JWKS, so
the payer flow can be exercised with no Auth0 tenant. Point the document path at
the Didit sandbox and confirm: session created lazily on first payer visit; a name
mismatch against `expected_details` returns `Declined` rather than leaking the
extracted name into our decision path; address appears only after approval; a
second invoice from the same merchant skips re-verification while one from a
different merchant does not.

**Check that matters most.** Assert the reconciliation poll recovers a session
whose webhook was dropped entirely — Didit retries only twice, so a brief
`gatewayd` restart during a verification is enough to lose the callback and strand
a payer who has already passed. Simulate by black-holing the webhook and
confirming the poll still unlocks the address.

**Regression.** `Payment.sol` and `PaymentFactory.sol` are untouched;
`address_matches_parameters` and the sweeper path still hold for gated invoices.

## Relationship to `features/attribution.md`

Both documents are proposals and they overlap in two places:

- **Migration number.** Both claim `0011`. Whichever lands second takes `0012`.
- **Counterparties.** Attribution L2 turns counterparties into objects. That is
  the natural home for a payer identity record, and `payer_credentials` here is a
  degenerate one-merchant case of it. If L2 ships first, this feature should
  attach to that object rather than introduce a parallel notion of a payer.

Attribution's cross-cutting principles apply here unchanged — in particular
"jurisdiction-agnostic by construction" and "a field earns a type only if Payday
acts on it," which is exactly why the requirement set stays at two types.

## Open questions

- Who operates the vendor dashboard, given that whoever does performs a regulated
  human-review function for every merchant (see Regulatory framing)? This needs
  an answer in the merchant terms before the document requirement ships.
- Does a gated invoice need a distinct public status, or is
  `awaiting_payment` + `requirements[]` enough? Adding a status touches the
  documented state machine and every SDK consumer; the current plan avoids it.
- Should a declined payer be able to retry, and how many times, before the
  invoice is effectively unpayable by them? Unbounded retries are a fraud vector;
  zero retries will strand legitimate payers with poor lighting.
- `expires_at` on `payer_credentials` has no principled value yet. Vendor
  re-verification cadence and the merchant's own risk appetite both argue for
  making it configurable, but v1 needs a defensible default.
- Cross-origin verification is out for v1 (§5). If merchant-built checkouts turn
  out to matter, the fix is a scoped CORS allowlist per account, which is a
  larger change to a deliberately locked-down surface.
