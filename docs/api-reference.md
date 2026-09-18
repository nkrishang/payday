# HTTP API reference

Production base URL: `https://api.payday.sh`. Sandbox uses
`https://api.sandbox.payday.sh`. The live OpenAPI 3.1 document is available at
`/openapi.json` and the interactive Scalar reference at `/docs` (also `/api`
and `/api/openapi.json`). The TypeScript client is documented in
[`sdk/typescript`](../sdk/typescript/README.md).

## Conventions

Authenticated customer routes require:

```http
Authorization: Bearer payday_live_...
Content-Type: application/json
```

Sandbox keys use `payday_test_…`. The same header also accepts the dashboard
session a signed-in merchant holds — the Privy identity token — and the API
tells the two apart and maps the token's identity to the account without
issuing a key. The account-key routes take only that session: an API key
cannot mint or revoke a key. A credential grants full read/write access to its
account; keys are not currently scoped.

A **deposit request** is the document — issuer, payer, one directly specified
amount, optional notes, heading, reference, and metadata, a payer policy, and
at most one PDF attachment. A **deposit** is its on-chain fulfilment: the funds that
answer the request. The API resource is the deposit request, and the state of its deposit is
read from it. There are no line items,
tax fields, or fiat amounts. An issued deposit request is an immutable snapshot:
changing anything means cancelling and reissuing.

Every response includes `X-Request-Id`. A printable caller value up to 128 bytes
is echoed; otherwise Payday generates a UUIDv7. JSON errors include the same
value:

```json
{"error":{"code":"invalid_request","message":"…"},"request_id":"…"}
```

A body or query string that does not fit the route — malformed JSON, a
missing or unknown field, a value of the wrong type, a missing
`Content-Type: application/json` — is `400 invalid_request`, and the message
names the problem (`unknown field `rotate`, expected `expected_generation``,
`missing field `amount``). Every request body rejects unknown fields.

Resources follow one set of conventions. Lists are enveloped under the
resource's plural (`deposit_requests`, `customers`, `webhooks`, `deliveries`,
`transfers`) with a
`next_cursor` where they page; `limit` is 1–100 and defaults to 20;
`starting_after` is the id of the last item seen. A missing, malformed, or
another account's id is `404 <resource>_not_found`. Creates answer `201`,
disables and deletes answer `204`, and `PATCH` is partial: a field left out
keeps its value, and only an explicit `null` clears one.

Every id is a UUID behind a prefix that says what it names, so a log line, a
support ticket, or a mistaken field reads for itself:

| Prefix | Resource |
|---|---|
| `dr_` | deposit request |
| `cus_` | customer |
| `att_` | attachment |
| `wh_` | webhook endpoint |
| `whd_` | webhook delivery |
| `evt_` | webhook event (the envelope `id` and `Payday-Event-Id`) |
| `va_` | verification attempt |
| `rec_` | recovery ledger entry (in `deposit_request.recovered_funds`) |
| `acct_` | account |

Only the canonical form the API emits is accepted back: the exact prefix,
then a lowercase hyphenated UUID. A `cus_` id handed to an attachment route
is `404 attachment_not_found`, and a bare UUID in a body field such as
`customer_id` is `400 invalid_request` naming the form wanted. The one place
a raw UUID remains is the Proof of Payment's
`canonical_issuance_snapshot.attachment.id`: that document is hashed into the
deposit address and its schema is frozen, so it carries the UUID the `att_`
id wraps.

Every authenticated merchant request — an API key or a dashboard session
alike — draws on a process-local per-account token bucket: capacity 60,
refill one request per second. Responses include `X-RateLimit-Limit`,
`X-RateLimit-Remaining`, and `X-RateLimit-Reset`. A rejected request returns
`429 rate_limited` and `Retry-After: 1`; send it again after that wait. The
dashboard and its client do so on their own.

Block numbers, log indexes, and exact base-unit amounts are decimal strings. Human
amounts are decimal strings with six-decimal precision: every supported
stablecoin has six decimals. Every timestamp
is RFC 3339 in UTC to the second with a `Z` suffix (`2026-09-06T12:00:00Z`),
in API responses and webhook payloads alike, unless explicitly described as
Unix seconds.

## Deposit requests

### `POST /v1/deposit-requests`

Requires `Idempotency-Key` containing 1–255 bytes.

```json
{
  "amount": "10.50",
  "payout_address": "0x1111111111111111111111111111111111111111",
  "issuer": {"name": "Acme LLC", "email": "billing@acme.example"},
  "payer": {"name": "Customer Inc", "details": "12 Main St, Springfield"},
  "heading": "March retainer",
  "reference": "INV-1042",
  "notes": "Net 30. Thank you.",
  "payer_policy": {"mode": "verified_email", "expected_email": "alice@customer.example"},
  "attachment_id": "att_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "customer_id": "cus_0198f80c-1111-7dc1-a369-90556a64f700",
  "expires_in": 3600,
  "metadata": {"po": "PO-77"}
}
```

| Field | Rules |
|---|---|
| `amount` | Required positive decimal in the request's currency; at most six fractional digits. Used directly; nothing is summed or reconciled |
| `currency` | Optional; `USDC` (default) or `USDT`. USDC bridges 1:1, so a USDC request may be paid on any network and withdrawn to any; USDT has no such path, so a USDT request must pin `chain_id` to a network serving USDT (`400 invalid_request` naming `chain_id` when missing). `422 unsupported_currency` when no network serves it |
| `chain_id` | Optional decimal chain id (`"143"`) pinning the network the payer must pay on; `networks` then holds that one entry and `chain` and `token` name it from issuance. Left out, the payer chooses among every network serving the currency when they sign. `422 unsupported_chain` when the chain does not serve the currency |
| `payout_address` | Required nonzero EVM address; receives exactly `amount` |
| `issuer`, `payer` | The parties: `name` 1–255 bytes, optional `email` 3–254 bytes, optional `details` up to 4,000 bytes of free text rendered verbatim. `issuer` is required and snapshotted onto the deposit request. `payer` is optional when `customer_id` is given: the saved record's name, email, and details are snapshotted in its place, and an inline party always wins. A `payer.email` is also where Payday emails the issued request, except under `merchant_session` (see [Deposit requests](deposit-requests-api.md)) |
| `payer_policy` | Required; one of the three modes below |
| `customer_id` | Optional `cus_` id of a customer the account owns; the deposit request still stores its own `payer` snapshot |
| `issuer_id` | Optional opaque issuer identifier of your own, 1–255 bytes, stored verbatim beside the issued document and returned on reads. Not an internal id: nothing is looked up, and the list route filters on it by exact string equality |
| `notes` | Optional, up to 4,000 bytes |
| `heading` | Optional short description, up to 200 bytes; shown to the payer before verification on gated deposit requests |
| `reference` | Optional merchant reference, at most 128 characters |
| `attachment_id` | Optional `att_` id of a finalized attachment; one PDF per deposit request |
| `expires_in` | Optional lifetime in seconds |
| `expires_at` | Optional RFC 3339 deadline; mutually exclusive with `expires_in` |
| `metadata` | JSON object, at most 16 keys and 512 encoded bytes per value; merchant-only |

Payer policy shapes:

```json
{"mode": "permissionless"}
{"mode": "verified_email", "expected_email": "alice@example.com"}
{"mode": "merchant_session", "payer_reference": "user_123"}
```

`expected_email` is required for `verified_email` and is trimmed and
lowercased; `payer_reference` is required for `merchant_session` and is
trimmed, case preserved: 1–128 bytes of printable text with no whitespace,
your own identifier for the user your application has signed in. Each
assertion is forbidden on the other modes. Assertions are merchant-supplied
and cannot be edited by the payer; the payer route shows only a masked email
hint, and never the payer reference.

For `merchant_session` the `201` response additionally carries
`client_secret` and `client_secret_expires_at`: a single-use secret, valid
for fifteen minutes, that opens the hosted checkout for that payer. It is
returned exactly once — never on an idempotent replay or a later `GET`; the
API stores only its hash — so a server that loses it mints another with
`POST /v1/deposit-requests/{id}/client-secret`. See
[Merchant sessions](#merchant-sessions) for the flow.

Text fields — party names, emails, and details, `heading`, `reference`,
`notes`, and customer fields — reject control characters (a NUL or any other
C0 control) with `400 invalid_request`; multi-line free text keeps its line
breaks and tabs. The check runs before anything is stored, so a bad document
never leaves a half-issued deposit request or a retagged attachment behind.

The smallest valid request names the parties and nothing else:

```json
{"amount": "10.50", "payout_address": "0x1111111111111111111111111111111111111111", "issuer": {"name": "Acme LLC"}, "payer": {"name": "Globex Inc"}, "payer_policy": {"mode": "permissionless"}}
```

Unknown fields are rejected; `memo` and `refund_address` are not fields.
Recovery is not a request field: it is the payer's attested wallet, bound
after issuance. Expiry defaults to 24 hours and must be 10 minutes to 366 days
ahead. A first request returns `201`; an identical retry returns the original
deposit with `200` and `Idempotency-Replayed: true`. Reuse with any changed
immutable field — parties, amount, currency, notes, heading, reference, metadata,
customer, policy mode or assertions, expiry intent, the networks offered
(each chain with its token and factory), or the attachment's ID, length, or
SHA-256 — returns
`409 idempotency_conflict`; the original must then be fetched with `GET`.
Relative-expiry retries retain the original resolved deadline. Deposit request creation
also requires the account to have a verified support email from a recent login.

At issuance Payday canonicalizes the deposit request (RFC 8785 JCS) and hashes it; the
response's `attribution {version, hash}` (version 2) reports the commitment.
The deposit address does not exist yet: `address`, `payer_wallet`,
`recovery_address`, `wallet_bound_at`, and `self_settlement` are `null` until
the payer, on the hosted page and once the policy is satisfied, signs the
request's EIP-712 attestation from the wallet they will pay from. The salt is
then derived from the attribution hash and that signature's digest, the wallet
becomes the address's recovery term, and the address follows. A
`deposit_request.ready` webhook reports the binding; integrations that quote an
address wait for it (or poll until `address` is set). Only transfers from the
attested wallet are the payer's, and excess or late funds return to it.

### `GET /v1/deposit-requests`

Lists newest first. Query parameters:

- `status`: one public status;
- `reference`: exact merchant reference filter;
- `customer_id`: exact `cus_` id filter;
- `issuer_id`: exact match on the opaque identifier a request was created with;
- `verification`: `not_required`, `pending`, `verified`, or
  `likely_unsolicited` — a separate parameter because it is a separate fact:
  a gated request can be funded before its payer has verified;
- `limit`: 1–100, default 20;
- `starting_after`: complete `dr_…` cursor returned as `next_cursor`.

Returns `{ "deposit_requests": [DepositRequestSummary], "next_cursor": null | "dr_…" }`.
Summaries contain `id`, `deposit_url`, `heading`, `payer_name`, `reference`,
`metadata`, `payer_policy_mode`, `customer_id`, `issuer_id`,
`has_attachment`, `verification_completed_at`, `likely_unsolicited_at`,
`created_at`, `updated_at`, `expires_at`, `status`, `amount`, `received`,
`currency`, and `cancellation_requested_at`: what a list needs to render and link each row
without a second read.

### `GET /v1/deposit-requests/{reference}`

`reference` must be a complete `dr_…` ID in the canonical form the API emits,
or the deposit address. Partial IDs are rejected with `400 invalid_request`;
cross-account resources are returned as `404 deposit_request_not_found`.

For long polling, add `wait_for=change&timeout=30`. `wait_for` must be `change`;
timeout is 1–30 seconds and defaults to 30. The request returns when
`updated_at` changes or the window elapses.

### `POST /v1/deposit-requests/{reference}/cancel`

Returns the `DepositRequest` with `cancellation_requested_at` set. Cancellation
is presentation-only: it records the request but cannot disable the address
or change the immutable settlement terms the address commits to.

### `GET /v1/deposit-requests/{reference}/transfers`

Returns `{ "transfers": [Transfer] }`, the finalized transfer provenance. Each
item has `timestamp`, `amount`, `amount_base_units`, `sender`,
`transaction_hash`, optional `explorer_url`, `block`, `disposition`
(`credited`, `late`, or `zero`), and `collected`.

### `GET /v1/deposit-requests/{reference}/attachment`

Returns the deposit request's attachment descriptor with a signed `download_url` valid
for a few minutes (`PAYDAY_ATTACHMENT_DOWNLOAD_TTL_SECS`, default 300).
`404 attachment_not_found` when the deposit request has none.

```json
{
  "id": "att_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "filename": "request.pdf",
  "mime_type": "application/pdf",
  "byte_length": "48211",
  "sha256": "0x9f…",
  "download_url": "https://…"
}
```

### `GET /v1/deposit-requests/{reference}/request.pdf`

Returns Payday's deposit request summary as `application/pdf`. Rendering is
deterministic — fixed fonts and object order, no timestamps or random IDs — so
the same deposit request always produces byte-identical output.

### `GET /v1/deposit-requests/{reference}/verification`

The merchant's verification view of one deposit request: `payer_policy_mode`,
`verification_completed_at`, `likely_unsolicited_at`, `facts` (`email` and
`merchant_session`, each `not_required`, `pending`, or `approved`; `wallet`
as `pending` or `approved`, never `not_required`; plus `complete`, the
identity policy alone), and every `attempts[]` entry (`kind` `email`,
`wallet`, or `merchant_session`, `status` pending, approved, or abandoned,
`verified_at`, `created_at`). A `merchant_session` attempt is the exchange of
a client secret, recorded approved; a `wallet` attempt is the accepted
attestation. The payer's session, the client secret, and the code they typed
are never in this response.

### `POST /v1/deposit-requests/{reference}/client-secret`

Mints a fresh single-use client secret for a `merchant_session` deposit:
`201 {client_secret, expires_at}`, `no-store`. Use it when the payer your
application signed in comes back after the first secret was spent or
expired; earlier unspent secrets stay valid until they expire, so retrying a
redirect never breaks a link already sent. Permissionless deposit requests answer
`409 verification_not_required`, `verified_email` deposits
`409 verification_method_not_applicable`, and a deposit that closed without
completing verification `410 deposit_request_not_payable`. A settled deposit that did
verify still mints, so the app can reopen the receipt for its user.

### `GET /v1/deposit-requests/{reference}/proof`

Returns the Proof of Payment JSON (`payday.proof.v4`) for a settled deposit request
(`payment_id` is the `dr_` id; inside `canonical_issuance_snapshot`, `attachment.id`
is the raw UUID behind the API's `att_` id, since that document is the hashed
commitment and its schema is frozen);
`409 deposit_request_not_settled` before then, and `409 deposit_sender_mismatch` when
any credited transfer came from a wallet other than the attested one and is
not a Relay delivery attributed to it (`transfers[].relay`, vouched for in
`verification.payload.relay_fills`), since no proof can then claim the
attested wallet paid. The proof carries the
canonical issuance snapshot, canonicalization version, attribution hash,
`payer_wallet {address, typed_data, digest, signature, method}` (the exact
EIP-712 document the payer's wallet signed, its signing digest, and the
signature), salt, the chosen chain with its factory and token (one of the
`networks` the snapshot offered, whose entry must match), deposit and
recovery addresses (the recovery address is the attested wallet), every
credited transfer of the request's token
into the deposit address, and `settlement_transaction_hash`: the fulfilment
transaction that executed the `Payment` contract — the same hash the
`DepositRequest` object reports as `settlement_tx_hash`, whether Payday's batch or a
third party submitted it — which is distinct from the transfers that funded
the address. It ends with a Payday-signed verification attestation
(`{payload: {version, payment_id, attribution_hash, chain_id,
payment_address, payer_wallet, wallet_nonce, payer_policy_mode, result,
verified_at, wallet_bound_at, facts[{kind, provider, at}]}, signer,
signature}`). The payload names the deposit request's attribution hash, chain,
deposit address, payer wallet, and the nonce inside the wallet's attestation,
so an attestation is bound to the document and the payer it was issued for
and cannot be transplanted onto a proof for another deposit request or another
wallet. Anyone holding the proof (and, if attached, the PDF) can recompute
hash → attestation → salt → CREATE3 address offline — `gum_core::verify_proof`
is the reference — which also requires every listed transfer to come from the
attested wallet and the transfers to sum to at least the requested amount;
whether the transfers and the settlement transaction really executed is
provable only against the chain (`--rpc-url`). It is merchant-accessible and
shared at the merchant's discretion; it is not a public link.

## Deposit request object

The full deposit request response contains:

- identity and instructions: `id`, `deposit_url`, `address`, optional
  `address_explorer_url`, `networks`, `chain`, `token`, `currency`,
  `payout_address`, `payer_wallet`, `recovery_address`, `wallet_bound_at`,
  and `expires_at`. `currency` is `USDC` or `USDT`. `networks` lists every
  chain the payer may pay on, each as
  `{chain: {id, name}, token: {symbol, address, decimals}}`, in the order the
  checkout offers them; each `token` is the currency's contract on that
  chain and `token.symbol` is what a wallet shows there (`USDT0` for USDT on
  Monad and Arbitrum). The merchant chooses only by pinning `chain_id`, which
  USDT requires. `chain`, `token`,
  `address`, `payer_wallet`, `recovery_address`, and `wallet_bound_at` are
  `null` until the payer's wallet is bound, which also fixes the network:
  `chain` and `token` then name the payer's choice, and `recovery_address`
  always equals `payer_wallet`, the wallet overpayment remainders, expired
  balances, and late transfers return to;
- accounting: `amount`, `received`, `remaining`, `fee_amount`, and `net_amount`,
  each with a corresponding `_base_units` field; current fees are zero;
- state: `status`, `deposited_at`, `deposited_at_block`, `settled_at`, `settled_block`,
  `expired_at`, `cancellation_requested_at`, `settlement_tx_hash`, optional
  explorer URL, and optional `attention {code,message,action}`;
- deposit request document: `issuer`, `payer`, `notes`, `heading`, `reference`,
  `customer_id`, `metadata`, the full `payer_policy` including assertions
  (merchant-only), optional `attachment` descriptor, `created_at`,
  `updated_at`;
- verification: `verification_completed_at`, and `likely_unsolicited_at` when
  finalized funds first arrived from a wallet other than the attested one;
- audit/freshness: `transfers`, optional `as_of {block,at}`,
  `indexer_freshness`, `self_settlement {factory,salt}` (null until bound),
  and `attribution {version,hash}`.

Public status values are `awaiting_deposit`, `partially_deposited`, `deposited`,
`settled`, `expired`, `returned`, and `needs_attention`. Clients must tolerate
new fields and should branch only on documented status values.

## Customers

A customer is a reusable, merchant-owned counterparty record. Bodies carry
`name` (1–255 bytes) with optional `email` and `details`; unknown fields are
rejected.

- `POST /v1/customers` → `201 Customer`;
- `GET /v1/customers?limit=1..100&starting_after={id}` →
  `{ "customers": [Customer], "next_cursor": null | id }`;
- `GET /v1/customers/{id}` → `Customer` plus `stats {request_count,
  totals: [{currency, request_count, collected_base_units,
  pending_base_units}]}` — one entry per currency the customer has been
  asked for, since totals never add across currencies; cross-account IDs are
  `404 customer_not_found`;
- `PATCH /v1/customers/{id}` with any subset of `name`, `email`, and
  `details` → updated `Customer`. A field left out keeps its value; `email`
  or `details` sent as `null` is cleared. The merged record is validated
  whole, so `{"name": "  "}` is `400 invalid_request`.

`Customer` contains `id`, `name`, `email`, `details`, `created_at`, and
`updated_at`.

## The issuer on a request

There are no saved issuer identities. `POST /v1/deposit-requests` takes the
`issuer` party and the `payout_address` inline on every create and snapshots
them, so editing nothing later can change a request already issued. Alongside
them the create takes an optional `issuer_id`: an opaque identifier of the
merchant's own choosing, 1–255 bytes, stored verbatim beside the issued
document and returned on `DepositRequest` and `DepositRequestSummary`. Nothing
is looked up, no mailbox is proven, and the list route filters on it by exact
string equality — it is a handle for the merchant's own records, nothing more.

## Attachments

One PDF per deposit request, at most 5 MiB, uploaded straight to object storage with a
presigned URL so it bypasses the 64 KiB request-body limit, and scanned before
it can be attached.

1. `POST /v1/attachments` with `{ "filename": "request.pdf" }` → `201`
   `{ "id", "upload_url", "headers", "expires_at" }`.
2. `PUT` the bytes to `upload_url` sending exactly the returned `headers`.
   They include `If-None-Match: *`, which makes the object key write-once: a
   second `PUT` to the same URL fails with `412 Precondition Failed`, so the
   bytes Payday hashes cannot be swapped afterwards. Payday also pins the
   object version it finalized, and every later read — the scan tag, the
   download URL, the attached-tag rewrite — refers to that version.
3. `POST /v1/attachments/{id}/finalize` → `200` attachment descriptor once the
   object is a clean PDF: `409 attachment_scan_pending` while the malware scan
   has not reported, `422 attachment_rejected` for anything that is not
   `application/pdf`, outside 1–5,242,880 bytes, missing `%PDF-` magic bytes,
   or flagged. A presigned `PUT` cannot bound the upload's size, so the 5 MiB
   limit is enforced here, and a rejected object is deleted from storage
   rather than left for the lifecycle rule. Payday hashes the stored bytes
   itself; browser MIME, extension, ETag, or a client digest are never
   trusted.
4. Pass the `id` as `attachment_id` when creating the deposit request. A draft that is
   not `ready` fails with `409 attachment_not_ready`; an attachment already on
   a deposit request fails with `409 attachment_already_attached`. Finalize is
   idempotent: a finalized or attached upload answers with its descriptor
   again, a rejected one with `422 attachment_rejected` again. Unattached
   uploads are deleted after a bounded time (seven days); an upload that
   expired before it was used fails issuance with `409 attachment_not_ready`
   and a message asking you to upload the PDF again.

The SDK's `attachments.upload` performs the whole exchange.

## Account and service status

- `GET /v1/account` takes either credential and returns the account ID, the
  mailbox it signs in with (`email`), its own wallet (`wallet_address`, the
  embedded EVM wallet Privy created for it, where deposits settle by default;
  `null` until the first session that carried one), key hint, generation,
  creation/rotation timestamps, previous-key grace expiry, and revocation
  timestamp.
- `GET /v1/status` uses an API key and returns `{chains: [...]}`, one entry
  per supported network with its finalized position, indexer cursor/lag,
  and sweeper state/queue. It may return 503 when status data cannot be
  read.
- `GET /health` is unauthenticated readiness: plain `ok` on 200 or
  `database unavailable` on 503.

## Account-key API

These routes take a dashboard session (the Privy identity token), never a
Payday API key — a key presenting itself here gets `401 identity_unauthorized`.
The key has no read route of its own: `GET /v1/account` is where its
non-secret state (`key_hint`, `generation`, rotation timestamps) lives.

- `POST /v1/account/api-key` with `{ "expected_generation": N }` — first
  issuance (`201`) or safe rotation (`200`), returning the plaintext key once;
  the previous key remains valid for 24 hours. A signed-in account already
  exists at generation 1 before it holds any key, so pass the generation
  `GET /v1/account` reports;
- `DELETE /v1/account/api-key` with `{ "expected_generation": N }` — revoke
  current and grace-period keys (`204`).

Generation checks prevent racing an unexpected rotation.

## Webhooks

- `POST /v1/webhooks` with `{ "url": "https://…" }` → `201 Webhook` with
  `secret`, returned once; `503 webhooks_unavailable` on a deployment with
  no webhook encryption key;
- `GET /v1/webhooks` → `{ "webhooks": [Webhook] }`, the active endpoints
  without secrets;
- `GET /v1/webhooks/{id}` → `Webhook`, disabled or not, without its secret;
- `DELETE /v1/webhooks/{id}` → `204`; disables the endpoint. Idempotent, and
  its delivery history stays readable;
- `POST /v1/webhooks/{id}/test` → `202 { "delivery_id": "…" }`; queues a
  `webhook.test` event for this endpoint only. A disabled endpoint is
  `404 webhook_not_found`;
- `GET /v1/webhook-deliveries?endpoint_id=&limit=1..100&starting_after={id}`
  → `{ "deliveries": [Delivery], "next_cursor": null | id }`, newest first,
  each with its immutable `attempts[]`.

`Webhook` contains `id`, `url`, `created_at`, and `disabled_at`. `Delivery`
contains `id`, `event_id`, `endpoint_id`, `state` (`pending`, `delivered`, or
`failed`), `attempt_count`, `next_attempt_at`, `delivered_at`, `created_at`,
and `attempts[{number, attempted_at, duration_ms, status, error}]`. A
missing, malformed, or another account's endpoint id is
`404 webhook_not_found` everywhere.

URLs must be credential-free HTTPS public destinations named by DNS hostname;
redirects and private, loopback, link-local, or reserved targets are
rejected. See [Webhooks](webhooks.md) for signatures, event types, payloads,
and retry policy.

## Withdrawals

The Payday wallet's whole balance in one currency to one address: every
network's USDC, or the destination network's USDT.
Prepare, sign, submit, poll: the API snapshots the balances into legs, the
merchant signs each leg's EIP-712 document with the wallet's key (Privy's
`useSignTypedData` in the dashboard, or the key exported once from the
dashboard on a server), and gum-indexer relays. A transfer leg (funds
already on the destination network) is one EIP-3009 `transferWithAuthorization`
on the currency's contract there (USDC or USDT0); a bridge leg, USDC only, is
`WithdrawalForwarder.bridge` (a CCTP V2 burn) and, once Circle attests it,
`MessageTransmitterV2.receiveMessage` on the destination. USDT has no 1:1
bridge, so a USDT withdrawal moves the destination network's balance alone;
USDT held on another network is withdrawn separately, to an address there.
Payday pays gas; the signature fixes where each leg's funds may land. One
withdrawal may be open per account. The full guide, the signer's checklist,
and TypeScript/Rust/Go samples are on the docs site under Withdrawals.

### `POST /v1/withdrawals`

`Idempotency-Key` required. Body `{"currency": "USDC", "destination":
{"chain_id": "8453", "address": "0x…"}}`; `currency` defaults to `USDC`, and
the destination must serve it (`400 invalid_request` naming the chain
otherwise). Answers `201` with the withdrawal (`currency`,
`wallet_address`, `destination`, `legs`), every leg `awaiting_signature`,
naming its `source_chain` and `token {symbol, address, decimals}` (the
currency's contract the document is signed under), and carrying
`authorization`:

```json
{
  "primary_type": "ReceiveWithAuthorization",
  "typed_data": { "domain": { "name": "USDC", "version": "2", "chainId": 143, "verifyingContract": "0x7547…" },
                  "primaryType": "ReceiveWithAuthorization", "types": { "…": "…" },
                  "message": { "from": "0x…", "to": "0x…", "value": "1234567", "validAfter": "0", "validBefore": "1800000000", "nonce": "0x…" } },
  "expires_at": "2027-01-15T08:00:00Z",
  "forwarder": "0x…",
  "nonce_preimage": { "destination_domain": 6, "mint_recipient": "0x…", "salt": "0x…" }
}
```

Every `uint256` is a decimal string. A USDT0 leg's domain is `{name: "USDT0"
(Monad) or "USD₮0" (Arbitrum), version: "1"}`. A bridge leg's nonce is
`keccak256(abi.encode(uint32 destination_domain, bytes32(mint_recipient), bytes32 salt))`;
the forwarder recomputes it, so the signature commits to the destination. A
bridge leg is capped by Circle's per-message burn limit: a wallet balance
above 10,000,000 USDC on a source chain cannot be bridged in one withdrawal.
Errors: `409 wallet_not_ready`, `409 withdrawal_in_progress`,
`409 nothing_to_withdraw` (for USDT, the message names balances held on
other networks), `409 idempotency_conflict`, `422 unsupported_currency`,
`422 withdrawal_exceeds_bridge_limit`, `503 withdrawals_unavailable`.

### `POST /v1/withdrawals/{id}/authorizations`

Body `{"authorizations": [{"leg_id": "wdl_…", "signature": "0x…"}]}`, any
subset of the legs; each signature is 65 bytes `r || s || v`, verified against
the wallet before any is stored. `400 signature_invalid` names the leg;
`409 leg_not_awaiting_signature`, `409 authorization_expired` (24 hours),
`409 withdrawal_finished`.

### `GET /v1/withdrawals/{id}`, `GET /v1/withdrawals`

The withdrawal (`status`: `awaiting_signature`, `in_progress`, `completed`,
`failed`, `cancelled`) with every leg's `state` (`awaiting_signature`,
`authorized`, `relaying`, `burned`, `attested`, `minting`, `completed`,
`failed`, `expired`, `cancelled`), `transfer_tx_hash`, `burn_tx_hash`,
`mint_tx_hash`, and `failure_reason`. The list is newest first,
`{ "withdrawals": [...], "next_cursor": "wd_…" | null }`.

### `POST /v1/withdrawals/{id}/cancel`

Cancels while nothing has been relayed; signatures already given are never
used. `409 withdrawal_not_cancellable` once a leg is in flight.

## Payer links and documentation

The `deposit_url` points at the hosted checkout, whose origin is
`PAYDAY_PUBLIC_BASE_URL`.

The checkout reads deposit data from `GET /v1/payer/deposit-requests/{id}`, QR SVG
from `GET /v1/payer/deposit-requests/{id}/qr`, and the PDF descriptor from
`GET /v1/payer/deposit-requests/{id}/attachment`. These routes are unauthenticated,
accept no account API key, and expose no merchant data: no payout or recovery
address, metadata, customer, or policy assertions. JSON responses use
`Cache-Control: no-store`, and QR requests return `410 deposit_request_not_payable` once
the address should no longer be presented. They send
`Access-Control-Allow-Origin: *` for `GET`, so a browser on any origin can build
a checkout against them. The merchant routes (deposit requests, customers,
attachments) answer cross-origin requests from exactly one
origin: the configured web origin (`PAYDAY_PUBLIC_BASE_URL`), where the
dashboard lives, for `GET`, `POST`, `PATCH`, `PUT`, and `DELETE` with the
`Authorization`, `Content-Type`, `Idempotency-Key`, and `Accept` headers. Any other origin receives no
`Access-Control-Allow-Origin` and its preflight fails; server-side
integrations are unaffected, and no other route allows cross-origin reads.

The payer response discloses progressively. It always carries `id`,
`issuer_name`, `heading`, `payer_policy {mode, expected_email_hint}`,
`requirements {email, wallet, merchant_session, complete}` (`email` and
`merchant_session` are each `not_required`, `pending`, or `approved`;
`wallet` is `pending` or `approved`; `complete` is the identity policy
alone), `status`, `payable`, `expires_at`, `server_timestamp`,
`settlement_tx_hash`, `settlement_explorer_url`, `payer_message`, and
`content_unlocked`. For a `permissionless` deposit request `content_unlocked` is true
and the response includes `currency`, `networks`, `amount`, `received`,
`remaining` (each with base units), and
`details {amount, amount_base_units, payer, notes, reference, attachment}`.
`chain`, `token`, `payer_wallet`, `address`, `address_explorer_url`, and
`deposit_uri` are present only once the payer's wallet is bound on a chosen
network: until then the request has no chain and no address to show. For the gated modes every one of those fields — and
`settlement_tx_hash` and `settlement_explorer_url`, since a settlement
transaction would reveal the amount and payout address the gate withholds —
is `null` until the payer's session satisfies the policy; the hint masks the
expected mailbox as `a****@e***.com`, and is `null` for `merchant_session`,
whose payer reference is never shown to the payer.
The attachment and QR routes answer `401 verification_required` while content
is locked, and the QR route `409 wallet_required` while no wallet is bound. A payer session token, obtained by completing verification on the
hosted checkout, travels in the `Payday-Payer-Session` header on every payer
read; the reads' `Access-Control-Allow-Headers` admits it. A session unlocks
exactly the deposit request it was created for. When a session is presented,
`requirements` reports that session's facts; without one it reports what the
deposit request as a whole has completed, which never unlocks a read.

### Email verification

```text
POST /v1/payer/deposit-requests/{id}/verify/email/start
POST /v1/payer/deposit-requests/{id}/verify/email/confirm   {"otp": "123456"}
GET  /v1/payer/deposit-requests/{id}/verify
```

`start` sends a one-time code to the mailbox the merchant asserted at issuance
(the request names no email) and returns `{payer_session, expires_at}`: an
opaque token, valid for 24 hours, of which the API stores only a hash. Sending
it back in `Payday-Payer-Session` on a second `start` resends the code on the
same session. At most one code per deposit request per minute is sent, whoever asks;
sooner answers `429 otp_resend_cooldown` with `Retry-After`. `confirm` takes
the session and the code, exchanges it with Auth0 against the payer audience,
and answers `{requirements}`. If Auth0 accepts and
consumes the OTP but database persistence remains unavailable, it answers
`503 verification_persistence_unavailable` with a five-minute, signed
`continuation`. Retry the same confirm route with `{"continuation":"…"}` and
the same payer session; this proof is audience-, deposit request-, and session-bound,
contains no Auth0 bearer token, and responses are `no-store`. The deposit request's
verification completes and the session unlocks the content.
`GET …/verify` reports the same shape for a session, or for the deposit request as a
whole without one. Permissionless deposit requests answer
`409 verification_not_required`; deposit requests past their deadline or already
settled answer `410 deposit_request_not_payable`, because verification after expiry
cannot revive settlement. The exception is a terminal deposit request that previously
completed verification: `start` and `confirm` explicitly re-prove its expected
mailbox and mint a new 24-hour receipt session. Expired sessions never unlock
terminal content, and receipt re-authentication never makes the deposit request payable.

### Wallet attestation

```text
POST /v1/payer/deposit-requests/{id}/wallet/challenge   {"wallet": "0x…", "chain_id": "143"}
POST /v1/payer/deposit-requests/{id}/wallet/attest      {"wallet": "0x…", "signature": "0x…"}
```

Every request, gated or not, takes this step before it has an address, and
it is where the payer chooses the network: `chain_id` must be one of the
request's `networks` (else `422 unsupported_chain`). `challenge` mints a
one-time nonce on the payer's session (a permissionless request without a
session gets one here, returned as `payer_session`; a gated request needs
the session that satisfied its policy, else `401 payer_session_invalid` or
`401 verification_required`) and answers `{payer_session, expires_at, chain,
typed_data}`, where `typed_data` is the EIP-712 document to hand to
`eth_signTypedData_v4` verbatim: domain `{name: "Payday", version: "1",
chainId, verifyingContract: factory}` for the chosen chain and its factory,
so a wallet on another network refuses to sign it; primary type
`PayerAttestation`, message `{statement, attributionHash, wallet, nonce,
expiresAt}`. The challenge is void after ten minutes. `attest` takes the
wallet and its 65-byte signature; the API rebuilds the document from its own
record, under the chain the challenge was minted for (the client cannot swap
chains between the two calls), requires the signature to recover to `wallet`
(externally owned accounts only for now; `401 wallet_signature_invalid`
otherwise), and binds: the chain, its token and factory, the salt, the
recovery term (the wallet), and the deposit address are written together,
once, and the unlocked payer deposit is returned with `chain`, `token`,
`address`, and `payer_wallet` set. The address commits to the chain: the
`Payment` contract refuses to settle on any other network, so the token sent
to it elsewhere is refused rather than lost and is returned by hand
(`docs/runbooks/wrong-network-deposit.md`). A request already bound
to another wallet answers `409 wallet_already_bound` naming it; attesting
without an outstanding challenge answers `409 wallet_challenge_required`;
a request past `created` or past its deadline answers
`410 deposit_request_not_payable`.

Email routes on a `merchant_session` deposit answer
`409 verification_method_not_applicable`: that mode sends no codes.

### Paying from another network

```text
GET  /v1/payer/deposit-requests/{id}/relay/chains
POST /v1/payer/deposit-requests/{id}/relay/quotes                  {"origin_chain_id": "8453", "origin_token": "0x…"}
POST /v1/payer/deposit-requests/{id}/relay/quotes/{quote_id}/sent  {"transaction_hash": "0x…"}
```

Once the address exists and while the request is payable
(`relay_available` on the payer view), the attested wallet may pay from
another network through Relay. `chains` lists every origin network, each
with `tokens: [{currency, symbol, address, decimals}]`, the stablecoins the
payer may send there (USDC, and USDT where served). `quotes` takes the
origin chain and optionally one of those addresses as `origin_token` (the
network's USDC by default) and answers a `RelayQuote` carrying `origin`,
`origin_token`, `amount_in` in that token, `amount_out` in the request's
currency (exactly the amount still due), and the transactions to send from
the wallet; Relay swaps between currencies and the payer carries the
spread. `sent` reports the origin transaction. `404 relay_unavailable` on a
deployment without Relay, `422 relay_unsupported_origin` for a network or
token not offered, `502 relay_quote_failed` when Relay has no route,
`409 relay_report_conflict` for a conflicting report.

### Merchant sessions

```text
POST /v1/payer/deposit-requests/{id}/session   {"client_secret": "cs_…"}
```

The `merchant_session` mode is for applications that have already signed
their user in. The flow, end to end:

1. Your server creates the deposit request with
   `{"mode": "merchant_session", "payer_reference": "<your user id>"}` and
   receives `client_secret` in the `201`.
2. Your server sends the signed-in user to
   `deposit_url + "#cs=" + client_secret`. The secret rides in the URL
   fragment, which the browser never sends to any server, so it reaches no
   log, Referer header, or analytics beacon. Never put it in the path or
   query.
3. The hosted checkout reads the fragment, removes it from the address bar,
   and exchanges it here: `200 {payer_session, expires_at, requirements}`,
   `no-store`. The exchange is the verification. It mints a 24-hour payer
   session that already satisfies the policy, records an approved
   `merchant_session` attempt, and, while the deposit request is live, sets
   `verification_completed_at`, which raises `verification.approved`. The
   page renders unlocked; the payer types nothing.
4. The payer pays. `deposit_request.deposited` and `deposit_request.settled` follow as usual,
   each carrying `payer_reference`, so your handler credits the right
   ledger without a lookup.

A secret is spent by its first exchange: the same link pasted into another
window answers `409 client_secret_used`, and the checkout says so. An
unknown, malformed, expired, or wrong-deposit secret answers
`401 client_secret_invalid` — one answer for all four, so a guess learns
nothing. A deposit that closed without verifying answers
`410 deposit_request_not_payable`; a settled deposit that did verify still accepts a
fresh secret, minting a receipt session that never makes it payable again.
Exchanging needs no Auth0 audience: merchant sessions never touch the email
provider.

What Payday attests here is narrow and stated plainly: your server released
this secret, and it was exchanged before this session saw the deposit request. Who
the payer is remains your assertion — `payer_reference` — carried in the
policy, the issuance snapshot the address commits to, and every webhook.

The write routes answer cross-origin requests only from the hosted
checkout origin (`PAYDAY_HOSTED_CHECKOUT_ORIGIN`) for `POST` with
`Content-Type` and `Payday-Payer-Session`; they never allow `*`. Bodies are
limited to 8 KiB.

## Stable error codes

| Code | Typical status | Meaning |
|---|---:|---|
| `unauthorized` | 401 | Missing or invalid API key or dashboard session token |
| `payer_session_invalid` | 401 | `Payday-Payer-Session` missing, unknown, expired, or for another deposit request |
| `otp_invalid` | 401 | The verification code was not accepted |
| `verification_required` | 401 | Content or QR requested for a gated deposit request without an unlocked session |
| `verification_not_required` | 409 | Verification started, or a client secret requested, on a permissionless deposit request |
| `verification_method_not_applicable` | 409 | Email codes on a `merchant_session` deposit request, or a client secret on a `verified_email` one |
| `client_secret_invalid` | 401 | The client secret is unknown, malformed, expired, or for another deposit request |
| `client_secret_used` | 409 | The client secret was already exchanged; the link was opened once |
| `verification_not_started` | 409 | Confirm called before a code was sent, or after it was spent |
| `verification_persistence_unavailable` | 503 | Auth0 accepted the OTP but persistence failed; retry confirm with the returned short-lived continuation |
| `otp_resend_cooldown` | 429 | A code was sent for this deposit request within the last minute; see `Retry-After` |
| `identity_provider_unavailable` | 502 | Auth0 did not answer the passwordless exchange |
| `verification_unavailable` | 503 | The deployment has no payer audience configured |
| `identity_unauthorized` | 401 | An account-key route was called without a dashboard session (an API key, or an invalid Privy identity token) |
| `admin_unauthorized` | 401 | An operator route was called without the operator credential |
| `identity_unavailable` | 503 | The identity provider's keys could not be fetched; sessions cannot be verified |
| `account_disabled` | 403 | Account disabled |
| `account_contact_required` | 409 | Login again to attach a verified merchant email |
| `authentication_event_already_used` | 409 | Identity event already mutated key state |
| `api_key_generation_conflict` | 409 | Key generation changed or was omitted incorrectly |
| `missing_idempotency_key` | 400 | Create header absent |
| `idempotency_conflict` | 409 | Key reused with any different immutable deposit request field, including the attachment hash; fetch the original with `GET` |
| `invalid_request` | 400 | Invalid field, query, JSON, or request shape — malformed JSON, a missing or unknown field, a wrong type, a missing JSON content type, control characters in a text field — with the problem named in the message |
| `withdrawal_not_found` | 404 | Missing, malformed, or another account's `wd_` id |
| `withdrawal_leg_not_found` | 404 | A `leg_id` that is not a leg of the withdrawal |
| `wallet_not_ready` | 409 | The account's Payday wallet is not known yet; sign in to the dashboard once |
| `withdrawal_in_progress` | 409 | Another withdrawal is open; finish or cancel it |
| `nothing_to_withdraw` | 409 | The Payday wallet holds none of the currency where this withdrawal could move it: no USDC on any network, or no USDT on the destination (the message names USDT held elsewhere) |
| `signature_invalid` | 400 | A leg's signature is malformed or was not made by the Payday wallet; the message names the leg |
| `leg_not_awaiting_signature` | 409 | The leg already carries another signature or has moved past signing |
| `authorization_expired` | 409 | The leg's 24-hour authorization window passed; create a new withdrawal |
| `withdrawal_not_cancellable` | 409 | A leg has been relayed; the withdrawal runs to completion |
| `withdrawal_finished` | 409 | The withdrawal already completed, failed, or was cancelled |
| `withdrawals_unavailable` | 503 | A balance could not be read, or a chain holding funds cannot bridge on this deployment |
| `invalid_amount` | 400 | Invalid amount syntax, precision, or positivity |
| `unsupported_chain` | 422 | Wallet challenge named a chain the request does not offer, or a `chain_id` pin named a chain that does not serve the request's currency |
| `unsupported_currency` | 422 | No network on this deployment serves the requested `currency`, on a deposit request or a withdrawal |
| `relay_unavailable` | 404 | The deployment has no Relay integration |
| `relay_unsupported_origin` | 422 | A relay quote named a network or token `relay/chains` does not offer |
| `relay_quote_failed` | 502 | Relay has no route for the quote |
| `relay_report_conflict` | 409 | The reported origin transaction conflicts with an earlier report |
| `deposit_request_not_found` | 404 | Missing or cross-account deposit request |
| `customer_not_found` | 404 | Missing or cross-account customer |
| `webhook_not_found` | 404 | Missing, malformed, or cross-account webhook endpoint; also a disabled endpoint asked to send a test event |
| `webhooks_unavailable` | 503 | The deployment has no webhook encryption key, so no endpoint can be registered |
| `wallet_pregeneration_unavailable` | 503 | The deployment has no Privy app secret for wallet pregeneration (dashboard sign-up only) |
| `deposit_request_not_blocked` | 409 | An operator release was asked of a request that needs no attention |
| `attachment_not_found` | 404 | Missing or cross-account attachment, or the deposit request has none |
| `attachment_scan_pending` | 409 | Malware scan has not reported; retry finalize with backoff |
| `attachment_rejected` | 422 | Not a clean PDF within limits; upload a new file |
| `attachment_not_ready` | 409 | `attachment_id` refers to an upload that is not finalized, was rejected, or expired unused before issuance (upload the PDF again); also returned by finalize before anything reached `upload_url` |
| `attachment_already_attached` | 409 | `attachment_id` already belongs to an issued deposit request; one PDF per deposit request |
| `deposit_request_not_settled` | 409 | Proof requested before settlement |
| `deposit_sender_mismatch` | 409 | Proof requested for a deposit request credited from a wallet other than the attested one |
| `wallet_required` | 409 | QR requested before the payer bound a wallet; the address does not exist yet |
| `wallet_already_bound` | 409 | A wallet challenge or attestation for a request already bound to another wallet; the message names it |
| `wallet_challenge_required` | 409 | Attest called without an outstanding, unexpired challenge on the session |
| `wallet_signature_invalid` | 401 | The signature does not recover to the stated wallet (smart-contract wallets are not supported yet) |
| `verification_required` | 401 | Payer content is gated until the session completes verification |
| `invalid_deposit_link` | 401 | Deposit link does not resolve to a deposit request |
| `deposit_request_not_payable` | 410 | QR/deposit request is no longer available |
| `rate_limited` | 429 | Per-account allowance exhausted |
| `database_unavailable` | 503 | Persistent storage unavailable |
| `payload_too_large` | 413 | Body exceeds route limit |
| `not_found`, `method_not_allowed` | 404/405 | Route or method mismatch |
| `internal_error` | 500 | Internal processing failed |

Deposit request, customer, attachment, and webhook API bodies are limited to 64 KiB;
account-key bodies to 16 KiB. PDF bytes go to the presigned URL, never to the
API.
Send `request_id` to `support@payday.sh` when requesting help—never credentials,
OTP codes, webhook secrets, or unnecessary deposit request metadata.
