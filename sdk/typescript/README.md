# Payday TypeScript SDK

Zero-runtime-dependency, typed client for the Payday invoicing API. Requires Node.js 18+ (or another runtime with native `fetch`). Wire DTO fields intentionally use the API's canonical snake_case names.

```bash
npm install @payday/sdk
```

```ts
import { PaydayClient } from "@payday/sdk";

const payday = new PaydayClient({ apiKey: process.env.PAYDAY_API_KEY! });
const depositRequest = await payday.depositRequests.create({
  amount: "10.00",
  payout_address: "0x1111111111111111111111111111111111111111",
  issuer: { name: "Acme LLC", email: "billing@acme.example" },
  payer: { name: "Customer Inc", details: "12 Main St, Springfield" },
  heading: "March retainer",
  reference: "INV-1042",
  payer_policy: { mode: "permissionless" },
  expires_in: 3600,
}, crypto.randomUUID()); // caller-supplied idempotency key is mandatory

console.log(await payday.depositRequests.get(depositRequest.id));
console.log(await payday.depositRequests.list({ status: "awaiting_deposit", limit: 20 }));
```

A request may name saved records instead of retyping them: with `issuer_id`,
`issuer` and `payout_address` may be left out (the identity's name, contact
address, details, and first saved payout address are snapshotted), and with
`customer_id`, `payer` may be left out. An inline party still wins:

```ts
await payday.depositRequests.create(
  { amount: "10.00", issuer_id: acme.id, customer_id: globex.id, payer_policy: { mode: "permissionless" } },
  crypto.randomUUID(),
);
```

A deposit request is the document: issuer, payer, one directly specified amount,
optional `notes`, `heading`, `reference`, and `metadata`, a payer policy, and at
most one PDF attachment. The deposit request is its on-chain fulfilment, which is why
the routes stay under `/v1/deposit-requests`. There are no line items; attach a PDF for
an itemized breakdown. An issued deposit request is immutable — change anything and you
cancel and reissue.

Exactly `amount` settles to `payout_address`. The response's `address` is null
at creation: the deposit address exists only once the payer has attested, from
the hosted page, the wallet they will pay from (`payer_wallet`). That wallet is
the address's recovery term (`recovery_address` always equals it), so overpayment
remainders, expired balances, and late transfers return to the payer. Wait for
the `deposit_request.ready` webhook, or poll until `address` is set, before quoting an
address anywhere. A create request carrying `refund_address` is rejected.

## Payer policy

`payer_policy` is one of three presets. The verified mode names the expected
mailbox; the merchant-session mode names the user your own application has
already signed in, by your own identifier:

```ts
{ mode: "permissionless" }
{ mode: "verified_email", expected_email: "alice@example.com" }
{ mode: "merchant_session", payer_reference: "user_123" }
```

Gated deposit requests withhold their content from the payer page until verification
completes; every request, gated or not, withholds its deposit address until the
payer's wallet attestation (`PaydayPayerClient.wallet.challenge` then `attest`).
The full policy is returned only to the merchant.

### Merchant sessions: your app opens the checkout

For `merchant_session` the create response carries a single-use
`client_secret`, valid for fifteen minutes and returned exactly once (never on
a replay or a later read; the API stores only its hash). Your server, having
authenticated the user, sends them to the deposit page with the secret in the
URL fragment; the hosted checkout exchanges it and the page opens unlocked.
No code, no vendor, nothing for the payer to type:

```ts
const deposit = await payday.depositRequests.create(
  { ...request, payer_policy: { mode: "merchant_session", payer_reference: user.id } },
  `deposit-${deposit.id}`,
);
// redirect the signed-in user; the fragment never reaches a server log
response.redirect(checkoutUrl(deposit, deposit.client_secret!));
```

The secret is spent by the first page that opens it: the same link pasted into
another window is refused with `client_secret_used`. When the user comes back
later, mint another with `depositRequests.createClientSecret(deposit.id)` and redirect
again. Every webhook for the deposit request carries `payer_reference`, so the
`deposit_request.deposited` and `deposit_request.settled` handlers can credit the right ledger
without a lookup.

## Attachments

One PDF, at most 5 MiB, scanned before it can be attached. `attachments.upload`
runs the whole exchange — reserve a presigned slot, `PUT` the bytes with the
returned headers verbatim, then poll `finalize` with backoff while the scan is
pending — and resolves to the descriptor whose `id` goes into `attachment_id`:

```ts
const pdf = await payday.attachments.upload(await fs.readFile("request.pdf"), "request.pdf");
await payday.depositRequests.create({ ...requestFields, attachment_id: pdf.id }, crypto.randomUUID());
```

It throws `PaydayError` with `attachment_rejected` for anything that is not a
clean PDF within limits, and `attachment_scan_timeout` if the scan outlasts the
bound (default two minutes; `scanTimeout` overrides it) — call
`attachments.finalize(id)` again later. Pass `signal` to cancel. The pieces are
also exposed separately as `attachments.create({ filename })` and
`attachments.finalize(id)`.

## Documents and proof

- `depositRequests.attachment(id)` — the attached PDF's descriptor with a short-lived `download_url`.
- `depositRequests.requestPdf(id)` — Payday's deterministic deposit request summary as a `Blob`; the same deposit request always renders byte-identical.
- `depositRequests.verification(id)` — the deposit request's verification facts (`email`, `merchant_session`, and `complete`) and every attempt made against it, with its status and times. Never the code, the client secret, or the payer's session.
- `depositRequests.createClientSecret(id)` — a fresh single-use client secret for a `merchant_session` deposit, for a user your app signs in again; see [Merchant sessions](#merchant-sessions-your-app-opens-the-checkout).
- `depositRequests.proof(id)` — the `ProofOfPayment` for a settled deposit request (`409 deposit_request_not_settled` before). It ties the canonical issuance snapshot, nonce, salt, and CREATE3 address to the credited transfers and the fulfilment transaction (`settlement_transaction_hash`, the same hash as the deposit request's `settlement_tx_hash`), carries a Payday attestation bound to that deposit request, and can be verified offline without contacting Payday (the checks live in `gateway_core::verify_proof`).

## Customers

`customers.create/get/list/update` manage reusable counterparty records
(`name`, optional `email` and `details`). `update` is partial: a field left
out keeps its value, and `email: null` or `details: null` clears one. Pass a
customer's `id` as `customer_id` when creating a deposit request; the deposit
request still stores its own immutable `payer` snapshot.

## Issuer identities

`issuers.create/get/list/update/remove` manage the party a deposit request is
issued under, its contact mailbox (proven with `startEmailVerification` and
`confirmEmailVerification`), and the payout wallets it settles to
(`payoutAddresses.create/list/remove`, attached with
`issuers.setPayoutAddresses`). `update` is partial like a customer's; a
changed `contact_email` clears the verification, so a rename alone never
touches a proven mailbox.

## Webhooks

`webhooks.add(url)` registers a credential-free public HTTPS endpoint and
returns its signing `secret` once; `webhooks.list()` answers `{ webhooks }`
without secrets, `webhooks.get(id)` reads one endpoint, `webhooks.remove(id)`
disables it (idempotent; its history stays readable), `webhooks.test(id)`
queues a `webhook.test` event, and `webhooks.deliveries({ endpoint_id, limit,
starting_after })` pages the delivery history newest first as
`{ deliveries, next_cursor }`, each delivery with its attempts. A missing,
malformed, or foreign endpoint id throws `webhook_not_found` (404). See
[Webhooks](../../docs/webhooks.md) for signatures and payloads.

## Dashboard sessions

`new PaydayClient({ accessToken })` takes a dashboard session token — the
Privy identity token a signed-in dashboard holds — in place of an API key,
exactly one of the two. This is how the Payday dashboard talks to the API from
a browser without ever holding a key; the same bearer header carries either
credential.

The client also provides long polling through `depositRequests.get(id, { waitForChange: true })`, `depositRequests.cancel` (returns the deposit request with `cancellation_requested_at` set), `depositRequests.transfers` (`{ transfers }`), and `status`. API failures throw `PaydayError`, exposing `code`, `status`, `requestId`, and a `message` that names the problem — for a body that does not fit a route, the offending field.

Set `baseUrl` in the constructor to target the sandbox or a local gateway. Never expose an API key in browser-delivered code.

## Managing the API key

`account.get()` returns the signed-in account — `email`, `wallet_address` (the account's own EVM wallet, where deposits settle by default), and key metadata: `key_hint`, `generation`, `created_at`, `rotated_at`, `previous_key_expires_at`, `revoked_at` — using the client's own credential. It never returns the raw key.

`account.issueApiKey(expectedGeneration)` and `account.revokeApiKey(expectedGeneration)` mint or revoke a key. They work only from a dashboard session: an API key, however valid for everything else, is refused here (`identity_unauthorized`) on purpose — whoever holds a key must not be able to mint another from it. Pass `expectedGeneration` from the account's current `generation`; a mismatch throws `PaydayError` with code `api_key_generation_conflict`, meaning something else changed the key first. `issueApiKey`'s result carries the raw key exactly once — nothing later, including `account.get()`, can return it again — and, when it replaced an earlier key, that key keeps authenticating for 24 hours.

## Building your own checkout

`PaydayPayerClient` reads the public routes behind a `deposit_url`. It takes no API key and is safe to run in a browser: a deposit link is open by design, because anyone holding it is allowed to fulfil the deposit request.

```ts
import { PaydayPayerClient } from "@payday/sdk";

const payer = new PaydayPayerClient();

const deposit = await payer.depositRequests.get("dr_0198f80c-8d2f-7dc1-a369-90556a64f700");
deposit.issuer_name;          // always shown, with `heading`
deposit.content_unlocked;     // false while a gated deposit request awaits verification
deposit.requirements;         // email status and whether the policy is complete
deposit.remaining_base_units; // exact integer string — the only value to do arithmetic on
deposit.deposit_uri;          // EIP-681 request for the amount still due, or null
deposit.details;              // amount, payer, notes, reference, attachment — or null while locked
deposit.payable;              // false once the address must stop being shown
deposit.server_timestamp;     // render the deadline without trusting the payer's clock

payer.depositRequests.qr(deposit.id, payerSession); // SVG blob for an <img>; 401 while locked, 410 once not payable
payer.depositRequests.attachment(deposit.id, payerSession); // PDF descriptor; 401 verification_required while locked

// Email verification for a gated deposit request: the code goes to the mailbox the
// merchant asserted, and the payer only types it. These writes are answered
// cross-origin for the hosted checkout only.
const { payer_session } = await payer.verification.startEmail(deposit.id);
await payer.verification.confirmEmail(deposit.id, "123456", payer_session);
const { requirements } = await payer.verification.status(deposit.id, { payerSession: payer_session });

// A merchant-session deposit request instead arrives with a client secret in the URL
// fragment (`#cs=…`); exchange it once for the session, then read as above.
const opened = await payer.verification.exchangeClientSecret(deposit.id, clientSecret);
```

For `permissionless` deposit requests everything is unlocked immediately. For
`verified_email`, `chain`, `token`, the amounts, `address`, `deposit_uri`, and
`details` are `null` until the payer's session satisfies the policy; pass the
session token from verification as `payerSession` and it travels in the
`Payday-Payer-Session` header. The response deliberately carries no merchant
data — no payout or recovery address, metadata, customer, or policy
assertions; only a masked `expected_email_hint`. Pass an `AbortSignal` to
cancel a poll. If you build your own checkout, reproduce the guidance in
[Deposit safety](../../docs/deposit-safety.md): payers must send the exact
amount of the exact token on the exact chain, and must not pay at the deadline
boundary.
