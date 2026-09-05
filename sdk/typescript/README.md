# Payday TypeScript SDK

Zero-runtime-dependency, typed client for the Payday invoicing API. Requires Node.js 18+ (or another runtime with native `fetch`). Wire DTO fields intentionally use the API's canonical snake_case names.

```bash
npm install @payday/sdk
```

```ts
import { PaydayClient } from "@payday/sdk";

const payday = new PaydayClient({ apiKey: process.env.PAYDAY_API_KEY! });
const invoice = await payday.payments.create({
  amount: "10.00",
  payout_address: "0x1111111111111111111111111111111111111111",
  issuer: { name: "Acme LLC", email: "billing@acme.example" },
  bill_to: { name: "Customer Inc", details: "12 Main St, Springfield" },
  heading: "March retainer",
  reference: "INV-1042",
  payer_policy: { mode: "permissionless" },
  expires_in: 3600,
}, crypto.randomUUID()); // caller-supplied idempotency key is mandatory

console.log(await payday.payments.get(invoice.id));
console.log(await payday.payments.list({ status: "awaiting_payment", limit: 20 }));
```

An invoice is the document: issuer, bill-to, one directly specified amount,
optional `notes`, `heading`, `reference`, and `metadata`, a payer policy, and at
most one PDF attachment. The payment is its on-chain fulfilment, which is why
the routes stay under `/v1/payments`. There are no line items; attach a PDF for
an itemized breakdown. An issued invoice is immutable — change anything and you
cancel and reissue.

Exactly `amount` settles to `payout_address`. The response's `recovery_address`
is the Payday recovery wallet the payment is committed to: overpayment
remainders, expired balances, and late transfers land there and are returned by
the operator after manual review. It is platform-configured, so a create request
carrying `refund_address` is rejected.

## Payer policy

`payer_policy` is one of two presets. The verified mode names the expected
mailbox:

```ts
{ mode: "permissionless" }
{ mode: "verified_email", expected_email: "alice@example.com" }
```

Gated invoices withhold their content and payment address from the payer page
until verification completes. The full policy is returned only to the merchant.

## Attachments

One PDF, at most 5 MiB, scanned before it can be attached. `attachments.upload`
runs the whole exchange — reserve a presigned slot, `PUT` the bytes with the
returned headers verbatim, then poll `finalize` with backoff while the scan is
pending — and resolves to the descriptor whose `id` goes into `attachment_id`:

```ts
const pdf = await payday.attachments.upload(await fs.readFile("invoice.pdf"), "invoice.pdf");
await payday.payments.create({ ...invoiceFields, attachment_id: pdf.id }, crypto.randomUUID());
```

It throws `PaydayError` with `attachment_rejected` for anything that is not a
clean PDF within limits, and `attachment_scan_timeout` if the scan outlasts the
bound (default two minutes; `scanTimeout` overrides it) — call
`attachments.finalize(id)` again later. Pass `signal` to cancel. The pieces are
also exposed separately as `attachments.create({ filename })` and
`attachments.finalize(id)`.

## Documents and proof

- `payments.attachment(id)` — the attached PDF's descriptor with a short-lived `download_url`.
- `payments.invoicePdf(id)` — Payday's deterministic invoice summary as a `Blob`; the same invoice always renders byte-identical.
- `payments.verification(id)` — the invoice's verification facts (`email` and `complete`) and every attempt made against it, with its status and times. Never the code or the payer's session.
- `payments.proof(id)` — the `ProofOfPayment` for a settled invoice (`409 payment_not_settled` before). It ties the canonical issuance snapshot, nonce, salt, and CREATE3 address to the credited transfers and the fulfilment transaction (`settlement_transaction_hash`, the same hash as the payment's `settlement_tx_hash`), carries a Payday attestation bound to that invoice, and can be verified offline without contacting Payday (the checks live in `gateway_core::verify_proof`).

## Customers

`customers.create/get/list/update` manage reusable counterparty records
(`name`, optional `email` and `details`). Pass a customer's `id` as
`customer_id` when creating an invoice; the invoice still stores its own
immutable `bill_to` snapshot.

## Dashboard sessions

`new PaydayClient({ accessToken })` takes a short-lived Auth0 access token in
place of an API key — exactly one of the two. This is how the Payday dashboard
talks to the API from a browser without ever holding a key; the same bearer
header carries either credential.

The client also provides long polling through `payments.get(id, { waitForChange: true })`, `payments.cancel`, `payments.transfers`, `status`, and `webhooks.add/list/remove/test/deliveries`. API failures throw `PaydayError`, exposing `code`, `status`, and `requestId`.

Set `baseUrl` in the constructor to target the sandbox or a local gateway. Never expose an API key in browser-delivered code.

## Managing the API key

`account.get()` returns key metadata for the signed-in account — `key_hint`, `generation`, `created_at`, `rotated_at`, `previous_key_expires_at`, `revoked_at` — using the client's own credential. It never returns the raw key.

`account.issueApiKey(token, expectedGeneration)` and `account.revokeApiKey(token, expectedGeneration)` mint or revoke a key, but they do not use the client's own credential: `token` must be a *fresh, single-use* email-OTP authentication — completed in roughly the last five minutes and not already spent on another issue or revoke — from the dashboard's own sign-in. A plain session or API key, however valid for everything else, is refused here on purpose: reading an account does not imply permission to mint a live key for it. Pass `expectedGeneration` from the account's current `generation` (omit it only to provision a first key for an identity that has never held one); a mismatch throws `PaydayError` with code `api_key_generation_conflict`, meaning something else changed the key first. `issueApiKey`'s result carries the raw key exactly once — nothing later, including `account.get()`, can return it again — and, when it replaced an earlier key, that key keeps authenticating for 24 hours.

## Building your own checkout

`PaydayPayerClient` reads the public routes behind a `payment_url`. It takes no API key and is safe to run in a browser: a payment link is open by design, because anyone holding it is allowed to fulfil the payment.

```ts
import { PaydayPayerClient } from "@payday/sdk";

const payer = new PaydayPayerClient();

const payment = await payer.payments.get("pay_0198f80c-8d2f-7dc1-a369-90556a64f700");
payment.issuer_name;          // always shown, with `heading`
payment.content_unlocked;     // false while a gated invoice awaits verification
payment.requirements;         // email status and whether the policy is complete
payment.remaining_base_units; // exact integer string — the only value to do arithmetic on
payment.payment_uri;          // EIP-681 request for the amount still due, or null
payment.invoice;              // amount, bill_to, notes, reference, attachment — or null while locked
payment.payable;              // false once the address must stop being shown
payment.server_timestamp;     // render the deadline without trusting the payer's clock

payer.payments.qr(payment.id, payerSession); // SVG blob for an <img>; 401 while locked, 410 once not payable
payer.payments.attachment(payment.id, payerSession); // PDF descriptor; 401 verification_required while locked

// Email verification for a gated invoice: the code goes to the mailbox the
// merchant asserted, and the payer only types it. These writes are answered
// cross-origin for the hosted checkout only.
const { payer_session } = await payer.verification.startEmail(payment.id);
await payer.verification.confirmEmail(payment.id, "123456", payer_session);
const { requirements } = await payer.verification.status(payment.id, { payerSession: payer_session });
```

For `permissionless` invoices everything is unlocked immediately. For
`verified_email`, `chain`, `token`, the amounts, `address`, `payment_uri`, and
`invoice` are `null` until the payer's session satisfies the policy; pass the
session token from verification as `payerSession` and it travels in the
`Payday-Payer-Session` header. The response deliberately carries no merchant
data — no payout or recovery address, metadata, customer, or policy
assertions; only a masked `expected_email_hint`. Pass an `AbortSignal` to
cancel a poll. If you build your own checkout, reproduce the guidance in
[Payment safety](../../docs/payment-safety.md): payers must send the exact
amount of the exact token on the exact chain, and must not pay at the deadline
boundary.
