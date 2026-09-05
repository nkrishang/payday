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

Sandbox keys use `payday_test_…`. The same header also accepts the short-lived
Auth0 access token a signed-in dashboard holds; the API tells the two apart and
maps the token's identity to the account without issuing a key. Account-key
issuance routes instead require a fresh Auth0 email-OTP access token. A
credential grants full read/write access to its account; keys are not
currently scoped.

An **invoice** is the document — issuer, bill-to, one directly specified
amount, optional notes, heading, reference, and metadata, a payer policy, and
at most one PDF attachment. A **payment** is its on-chain fulfilment; the
routes stay under `/v1/payments` for compatibility. There are no line items,
tax fields, or fiat amounts. An issued invoice is an immutable snapshot:
changing anything means cancelling and reissuing.

Every response includes `X-Request-Id`. A printable caller value up to 128 bytes
is echoed; otherwise Payday generates a UUIDv7. JSON errors include the same
value:

```json
{"error":{"code":"invalid_request","message":"…"},"request_id":"…"}
```

API-key traffic has a process-local per-account token bucket: capacity 60,
refill one request per second. Responses include `X-RateLimit-Limit`,
`X-RateLimit-Remaining`, and `X-RateLimit-Reset`. A rejected request returns
`429 rate_limited` and `Retry-After: 1`.

Invoice-sized integers and exact base-unit amounts are decimal strings. Human
USDC values are decimal strings with six-decimal precision. Timestamps are RFC
3339 unless explicitly described as Unix seconds.

## Payments

### `POST /v1/payments`

Requires `Idempotency-Key` containing 1–255 bytes.

```json
{
  "amount": "10.50",
  "payout_address": "0x1111111111111111111111111111111111111111",
  "issuer": {"name": "Acme LLC", "email": "billing@acme.example"},
  "bill_to": {"name": "Customer Inc", "details": "12 Main St, Springfield"},
  "heading": "March retainer",
  "reference": "INV-1042",
  "notes": "Net 30. Thank you.",
  "payer_policy": {"mode": "verified_email", "expected_email": "alice@customer.example"},
  "attachment_id": "0198f80c-8d2f-7dc1-a369-90556a64f700",
  "customer_id": "0198f80c-1111-7dc1-a369-90556a64f700",
  "expires_in": 3600,
  "metadata": {"po": "PO-77"}
}
```

| Field | Rules |
|---|---|
| `amount` | Required positive USDC decimal; at most six fractional digits. Used directly; nothing is summed or reconciled |
| `payout_address` | Required nonzero EVM address; receives exactly `amount` |
| `issuer`, `bill_to` | Required parties: `name` 1–255 bytes, optional `email` 3–254 bytes, optional `details` up to 4,000 bytes of free text rendered verbatim |
| `payer_policy` | Required; one of the two modes below |
| `customer_id` | Optional customer UUID owned by the account; the invoice still stores its own `bill_to` snapshot |
| `notes` | Optional, up to 4,000 bytes |
| `heading` | Optional short description, up to 200 bytes; shown to the payer before verification on gated invoices |
| `reference` | Optional merchant reference, at most 128 characters |
| `attachment_id` | Optional finalized attachment UUID; one PDF per invoice |
| `expires_in` | Optional lifetime in seconds |
| `expires_at` | Optional RFC 3339 deadline; mutually exclusive with `expires_in` |
| `chain_id`, `token_address` | Optional deployment overrides; otherwise configured chain/native USDC |
| `metadata` | JSON object, at most 16 keys and 512 encoded bytes per value; merchant-only |

Payer policy shapes:

```json
{"mode": "permissionless"}
{"mode": "verified_email", "expected_email": "alice@example.com"}
```

`expected_email` is required for `verified_email`, forbidden for
`permissionless`, and is trimmed and lowercased. The assertion is
merchant-supplied and cannot be edited by the payer; the payer route shows
only a masked hint.

Text fields — party names, emails, and details, `heading`, `reference`,
`notes`, and customer fields — reject control characters (a NUL or any other
C0 control) with `400 invalid_request`; multi-line free text keeps its line
breaks and tabs. The check runs before anything is stored, so a bad document
never leaves a half-issued invoice or a retagged attachment behind.

Unknown fields are rejected; `memo` and `refund_address` are not fields.
Recovery is not a request field: Payday stamps its own recovery wallet on every
payment. Expiry defaults to 24 hours and must be 10 minutes to 366 days ahead.
A first request returns `201`; an identical retry returns the original payment
with `200` and `Idempotency-Replayed: true`. Reuse with any changed immutable
field — parties, amount, notes, heading, reference, metadata, customer,
policy mode or assertions, expiry intent, chain parameters (chain, token, and
factory), the attachment's ID, length, or SHA-256, or the Payday recovery
wallet — returns
`409 idempotency_conflict`; the original must then be fetched with `GET`.
Relative-expiry retries retain the original resolved deadline. Payment creation
also requires the account to have a verified support email from a recent login.

At issuance Payday canonicalizes the invoice (RFC 8785 JCS), hashes it with a
random nonce into the salt, and derives the payment address from that salt, so
the address commits to the exact document. The response's
`attribution {version, hash}` reports the commitment.

### `GET /v1/payments`

Lists newest first. Query parameters:

- `status`: one public status;
- `reference`: exact merchant reference filter;
- `limit`: 1–100, default 20;
- `starting_after`: complete `pay_…` cursor returned as `next_cursor`.

Returns `{ "payments": [PaymentSummary], "next_cursor": null | "pay_…" }`.
Summaries contain `id`, `heading`, `bill_to_name`, `reference`, `metadata`,
`payer_policy_mode`, `customer_id`, `has_attachment`,
`verification_completed_at`, `likely_unsolicited_at`, `created_at`, `status`,
`amount`, `received`, and `cancellation_requested_at`.

### `GET /v1/payments/{reference}`

`reference` must be a complete `pay_…` ID in the canonical form the API emits,
or the payment address. Partial IDs are rejected with `400 invalid_request`;
cross-account resources are returned as `404 payment_not_found`.

For long polling, add `wait_for=change&timeout=30`. `wait_for` must be `change`;
timeout is 1–30 seconds and defaults to 30. The request returns when
`updated_at` changes or the window elapses.

### `POST /v1/payments/{reference}/cancel`

Returns `{ "payment": Payment, "advisory": "…" }`. Cancellation is
presentation-only: it records `cancellation_requested_at` but cannot disable the
address or change immutable settlement terms.

### `GET /v1/payments/{reference}/transfers`

Returns finalized transfer provenance as an array. Each item has `timestamp`,
`amount`, `amount_base_units`, `sender`, `transaction_hash`, optional
`explorer_url`, `block`, `disposition`, and `collected`.

### `GET /v1/payments/{reference}/attachment`

Returns the invoice's attachment descriptor with a signed `download_url` valid
for a few minutes (`PAYDAY_ATTACHMENT_DOWNLOAD_TTL_SECS`, default 300).
`404 attachment_not_found` when the invoice has none.

```json
{
  "id": "0198f80c-8d2f-7dc1-a369-90556a64f700",
  "filename": "invoice.pdf",
  "mime_type": "application/pdf",
  "byte_length": "48211",
  "sha256": "0x9f…",
  "download_url": "https://…"
}
```

### `GET /v1/payments/{reference}/invoice.pdf`

Returns Payday's invoice summary as `application/pdf`. Rendering is
deterministic — fixed fonts and object order, no timestamps or random IDs — so
the same invoice always produces byte-identical output.

### `GET /v1/payments/{reference}/verification`

The merchant's verification view of one invoice: `payer_policy_mode`,
`verification_completed_at`, `likely_unsolicited_at`, `facts` (`email` as
`not_required`, `pending`, or `approved`, plus `complete`), and every
`attempts[]` entry (`kind` email, `status` pending, approved, or abandoned,
`verified_at`, `created_at`). The payer's session and the code they typed
are never in this response.

### `GET /v1/payments/{reference}/proof`

Returns the Proof of Payment JSON for a settled invoice; `409
payment_not_settled` before then. The proof carries the canonical issuance
snapshot, canonicalization version, attribution nonce and hash, salt, chain,
factory, token and payment addresses, every credited USDC transfer into the
payment address, and `settlement_transaction_hash`: the fulfilment
transaction that executed the `Payment` contract — the same hash the
`Payment` object reports as `settlement_tx_hash`, whether Payday's batch or a
third party submitted it — which is distinct from the transfers that funded
the address. It ends with a Payday-signed verification attestation
(`{payload: {version, payment_id, attribution_hash, chain_id,
payment_address, payer_policy_mode, result, verified_at}, signer,
signature}`). The payload names the invoice's attribution hash, chain, and
payment address, so an attestation is bound to the document it was issued for
and cannot be transplanted onto a proof for another invoice. Anyone holding
the proof (and, if attached, the PDF) can recompute hash → salt → CREATE3
address offline with `payday proof verify`, which also requires the listed
transfers to sum to at least the invoice amount; whether the settlement
transaction really executed is provable only against the chain
(`--rpc-url`). It is merchant-accessible and shared at the merchant's
discretion; it is not a public link.

## Payment object

The full payment response contains:

- identity and instructions: `id`, `payment_url`, `address`, optional
  `address_explorer_url`, `chain`, `token`, `currency`, `payout_address`,
  `recovery_address`, and `expires_at`. `recovery_address` is the Payday
  recovery wallet the payment is committed to; overpayment remainders, expired
  balances, and late transfers land there and are returned by the operator
  after manual review;
- accounting: `amount`, `received`, `remaining`, `fee_amount`, and `net_amount`,
  each with a corresponding `_base_units` field; current fees are zero;
- state: `status`, `paid_at`, `paid_at_block`, `settled_at`, `settled_block`,
  `expired_at`, `cancellation_requested_at`, `settlement_tx_hash`, optional
  explorer URL, and optional `attention {code,message,action}`;
- invoice document: `issuer`, `bill_to`, `notes`, `heading`, `reference`,
  `customer_id`, `metadata`, the full `payer_policy` including assertions
  (merchant-only), optional `attachment` descriptor, `created_at`,
  `updated_at`;
- verification: `verification_completed_at`, and `likely_unsolicited_at` when
  finalized funds arrived before a gated invoice's verification completed;
- audit/freshness: `transfers`, optional `as_of {block,at}`,
  `indexer_freshness`, `self_settlement {factory,salt}`, and
  `attribution {version,hash}`.

Public status values are `awaiting_payment`, `partially_paid`, `paid`,
`settled`, `expired`, `returned`, and `needs_attention`. Clients must tolerate
new fields and should branch only on documented status values.

## Customers

A customer is a reusable, merchant-owned counterparty record. Bodies carry
`name` (1–255 bytes) with optional `email` and `details`; unknown fields are
rejected.

- `POST /v1/customers` → `201 Customer`;
- `GET /v1/customers?limit=1..100&starting_after={id}` →
  `{ "customers": [Customer], "next_cursor": null | id }`;
- `GET /v1/customers/{id}` → `Customer`; cross-account IDs are
  `404 customer_not_found`;
- `PATCH /v1/customers/{id}` with the same body as create → updated `Customer`.

`Customer` contains `id`, `name`, `email`, `details`, `created_at`, and
`updated_at`.

## Issuer identities

An issuer identity is the merchant's own side of an invoice, saved once instead
of retyped: the party it is issued under, the address payers write to, and the
wallets it settles to. Issuance is unchanged — `POST /v1/payments` still takes
`issuer` and `payout_address` inline and snapshots them — so editing an identity
never reaches an invoice already issued.

Bodies carry `name` (1–255 bytes) and `contact_email` (3–254 bytes, lowercased
on write) with optional `details`; unknown fields are rejected. A name is unique
among your identities — case and surrounding space are not a difference — so
create and update answer `409 issuer_name_taken` for one already used. Contact
addresses may repeat: two identities can share a support mailbox.

- `POST /v1/issuers` → `201 Issuer`, unverified;
- `GET /v1/issuers?limit=1..100&starting_after={id}` →
  `{ "issuers": [Issuer], "next_cursor": null | id }`;
- `GET /v1/issuers/{id}` → `Issuer`; cross-account IDs are
  `404 issuer_not_found`;
- `PATCH /v1/issuers/{id}` with the same body as create → updated `Issuer`. A
  different `contact_email` clears the verification;
- `DELETE /v1/issuers/{id}` → `204`, with its payout-address associations.

The contact address is proven before an invoice carries it, because payers are
told to write to it:

- `POST /v1/issuers/{id}/verify/email/start` → `202 { contact_email,
  resend_available_at }`. The code goes to the *stored* address; the request
  never names a mailbox. One code per identity per minute
  (`429 otp_resend_cooldown`), `409 issuer_email_already_verified` once proven,
  and `503 verification_unavailable` where no identity provider is configured;
- `POST /v1/issuers/{id}/verify/email/confirm` with `{ "otp": "123456" }` →
  `Issuer` with `email_verified`. A wrong, spent, or expired code is
  `401 otp_invalid`, as is a code confirmed after the address moved.

Payout addresses are account-owned and shared between identities:

- `POST /v1/payout-addresses` with `{ address, label? }` → `201 PayoutAddress`.
  The address is stored EIP-55 checksummed and once per account: saving one you
  already hold returns the row you have. `label` is at most 20 characters,
  starts on a letter or digit, and uses letters, digits, spaces, and
  `. _ ' & ( ) -`;
- `GET /v1/payout-addresses` → `{ "payout_addresses": [PayoutAddress] }`;
- `DELETE /v1/payout-addresses/{id}` → `204`, with every association to it;
- `PUT /v1/issuers/{id}/payout-addresses` with `{ payout_address_ids: [id] }`
  replaces the whole set (at most 25). An id that is not yours is
  `404 payout_address_not_found`.

`GET /v1/payments` filters on `customer_id`, `issuer_id`, and `verification`
(`not_required`, `pending`, `verified`, or `likely_unsolicited`) alongside
`status` and `reference`. Verification is a separate parameter because it is a
separate fact: a gated request can be funded before its payer has verified.

An identity's `id` is the durable handle. `POST /v1/payments` takes it as
`issuer_id`, stores it immutably beside the issued document, and returns it on
`Payment` and `PaymentSummary` — so the requests issued under an identity stay
identifiable after it is renamed, moved to another mailbox, or pointed at
different wallets. The `issuer` party on the invoice remains the snapshot taken
at issuance, and is what the attribution hash commits to. Deleting an identity
that requests were issued under is `409 issuer_in_use`.

`Issuer` contains `id`, `name`, `contact_email`, `details`, `email_verified`,
`email_verified_at`, `payout_addresses` (in association order), `created_at`,
and `updated_at`. `PayoutAddress` contains `id`, `address`, `label`, and
`created_at`.

## Attachments

One PDF per invoice, at most 5 MiB, uploaded straight to object storage with a
presigned URL so it bypasses the 64 KiB request-body limit, and scanned before
it can be attached.

1. `POST /v1/attachments` with `{ "filename": "invoice.pdf" }` → `201`
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
4. Pass the `id` as `attachment_id` when creating the invoice. A draft that is
   not `ready` fails with `409 attachment_not_ready`; an attachment already on
   an invoice fails with `409 attachment_already_attached`. Finalize is
   idempotent: a finalized or attached upload answers with its descriptor
   again, a rejected one with `422 attachment_rejected` again. Unattached
   uploads are deleted after a bounded time (seven days); an upload that
   expired before it was used fails issuance with `409 attachment_not_ready`
   and a message asking you to upload the PDF again.

The SDK's `attachments.upload` performs the whole exchange.

## Account and service status

- `GET /v1/account` uses an API key and returns account ID, key hint,
  generation, creation/rotation timestamps, previous-key grace expiry, and
  revocation timestamp.
- `GET /v1/status` uses an API key and returns chain finalized position,
  indexer cursor/lag, and sweeper state/queue. It may return 503 when status
  data cannot be read.
- `GET /health` is unauthenticated readiness: plain `ok` on 200 or
  `database unavailable` on 503.

## Account-key API

These routes use a fresh Auth0 identity token, not a Payday API key:

- `GET /v1/account/api-key` — non-secret metadata;
- `POST /v1/account/api-key` with `{ "expected_generation": null | N }` —
  first issuance (`201`) or safe rotation (`200`), returning the plaintext key
  once; the previous key remains valid for 24 hours;
- `DELETE /v1/account/api-key` with `{ "expected_generation": N }` — revoke
  current and grace-period keys (`204`).

Generation checks prevent racing an unexpected rotation. Each signed
authentication event can mutate key state once.

## Webhooks

- `POST /v1/webhooks` with `{ "url": "https://…" }` → `201`; secret returned once;
- `GET /v1/webhooks` → active endpoints without secrets;
- `DELETE /v1/webhooks/{uuid}` → disable endpoint;
- `POST /v1/webhooks/{uuid}/test` → `202 {"delivery_id":"…"}`;
- `GET /v1/webhook-deliveries` → delivery state and immutable attempt history.

URLs must be credential-free HTTPS public destinations; redirects and private,
loopback, link-local, or reserved targets are rejected. See
[Webhooks](webhooks.md) for signatures, event types, and retry policy.

## Payer links and documentation

The `payment_url` points at the hosted checkout, whose origin is
`PAYDAY_PUBLIC_BASE_URL`. This service answers its own `GET /pay/{id}` with a
`301` to that origin so links shared earlier keep working.

The checkout reads payment data from `GET /v1/payer/payments/{id}`, QR SVG
from `GET /v1/payer/payments/{id}/qr`, and the PDF descriptor from
`GET /v1/payer/payments/{id}/attachment`. These routes are unauthenticated,
accept no account API key, and expose no merchant data: no payout or recovery
address, metadata, customer, or policy assertions. JSON responses use
`Cache-Control: no-store`, and QR requests return `410 payment_not_payable` once
the address should no longer be presented. They send
`Access-Control-Allow-Origin: *` for `GET`, so a browser on any origin can build
a checkout against them. The merchant routes (payments, customers, issuers,
payout addresses, attachments) answer cross-origin requests from exactly one
origin: the configured web origin (`PAYDAY_PUBLIC_BASE_URL`), where the
dashboard lives, for `GET`, `POST`, `PATCH`, `PUT`, and `DELETE` with the
`Authorization`, `Content-Type`, `Idempotency-Key`, and `Accept` headers. Any other origin receives no
`Access-Control-Allow-Origin` and its preflight fails; server-side
integrations are unaffected, and no other route allows cross-origin reads.

The payer response discloses progressively. It always carries `id`,
`issuer_name`, `heading`, `payer_policy {mode, expected_email_hint}`,
`requirements {email, complete}` (`email` is `not_required`, `pending`, or
`approved`), `status`, `payable`,
`expires_at`, `server_timestamp`, `settlement_tx_hash`,
`settlement_explorer_url`, `payer_message`, and `content_unlocked`. For a
`permissionless` invoice `content_unlocked` is true and the response includes
`chain`, `token`, `amount`, `received`, `remaining` (each with base units),
`address`, `address_explorer_url`, `payment_uri`, and
`invoice {amount, amount_base_units, bill_to, notes, reference, attachment}`.
For the verified modes those fields — and `settlement_tx_hash` and
`settlement_explorer_url`, since a settlement transaction would reveal the
amount and payout address the gate withholds — are `null` until the payer's
session satisfies the policy; the hint masks the expected mailbox as
`a****@e***.com`.
The attachment and QR routes answer `401 verification_required` while content
is locked. A payer session token, obtained by completing verification on the
hosted checkout, travels in the `Payday-Payer-Session` header on every payer
read; the reads' `Access-Control-Allow-Headers` admits it. A session unlocks
exactly the invoice it was created for. When a session is presented,
`requirements` reports that session's facts; without one it reports what the
invoice as a whole has completed, which never unlocks a read.

### Email verification

```text
POST /v1/payer/payments/{id}/verify/email/start
POST /v1/payer/payments/{id}/verify/email/confirm   {"otp": "123456"}
GET  /v1/payer/payments/{id}/verify
```

`start` sends a one-time code to the mailbox the merchant asserted at issuance
(the request names no email) and returns `{payer_session, expires_at}`: an
opaque token, valid for 24 hours, of which the API stores only a hash. Sending
it back in `Payday-Payer-Session` on a second `start` resends the code on the
same session. At most one code per invoice per minute is sent, whoever asks;
sooner answers `429 otp_resend_cooldown` with `Retry-After`. `confirm` takes
the session and the code, exchanges it with Auth0 against the payer audience,
and answers `{requirements}`. If Auth0 accepts and
consumes the OTP but database persistence remains unavailable, it answers
`503 verification_persistence_unavailable` with a five-minute, signed
`continuation`. Retry the same confirm route with `{"continuation":"…"}` and
the same payer session; this proof is audience-, invoice-, and session-bound,
contains no Auth0 bearer token, and responses are `no-store`. The invoice's
verification completes and the session unlocks the content.
`GET …/verify` reports the same shape for a session, or for the invoice as a
whole without one. Permissionless invoices answer
`409 verification_not_required`; invoices past their deadline or already
settled answer `410 payment_not_payable`, because verification after expiry
cannot revive settlement. The exception is a terminal invoice that previously
completed verification: `start` and `confirm` explicitly re-prove its expected
mailbox and mint a new 24-hour receipt session. Expired sessions never unlock
terminal content, and receipt re-authentication never makes the invoice payable.

The write routes answer cross-origin requests only from the hosted
checkout origin (`PAYDAY_HOSTED_CHECKOUT_ORIGIN`) for `POST` with
`Content-Type` and `Payday-Payer-Session`; they never allow `*`. Bodies are
limited to 8 KiB.

## Stable error codes

| Code | Typical status | Meaning |
|---|---:|---|
| `unauthorized` | 401 | Missing or invalid API key or dashboard access token |
| `payer_session_invalid` | 401 | `Payday-Payer-Session` missing, unknown, expired, or for another invoice |
| `otp_invalid` | 401 | The verification code was not accepted |
| `verification_required` | 401 | Content or QR requested for a gated invoice without an unlocked session |
| `verification_not_required` | 409 | Verification started on a permissionless invoice |
| `verification_not_started` | 409 | Confirm called before a code was sent, or after it was spent |
| `verification_persistence_unavailable` | 503 | Auth0 accepted the OTP but persistence failed; retry confirm with the returned short-lived continuation |
| `otp_resend_cooldown` | 429 | A code was sent for this invoice within the last minute; see `Retry-After` |
| `identity_provider_unavailable` | 502 | Auth0 did not answer the passwordless exchange |
| `verification_unavailable` | 503 | The deployment has no payer audience configured |
| `identity_unauthorized` | 401 | Invalid Auth0 identity token |
| `identity_unavailable` | 503 | Identity verification unavailable |
| `account_disabled` | 403 | Account disabled |
| `account_not_provisioned` | 404 | Identity has no Payday account |
| `account_contact_required` | 409 | Login again to attach a verified merchant email |
| `authentication_event_already_used` | 409 | Identity event already mutated key state |
| `api_key_generation_conflict` | 409 | Key generation changed or was omitted incorrectly |
| `missing_idempotency_key` | 400 | Create header absent |
| `idempotency_conflict` | 409 | Key reused with any different immutable invoice field, including the attachment hash and the Payday recovery wallet; fetch the original with `GET` |
| `invalid_request` | 400 | Invalid field, query, JSON, or request shape, including control characters in a text field |
| `invalid_amount` | 400 | Invalid amount syntax, precision, or positivity |
| `unsupported_chain`, `unsupported_token` | 422 | Deployment does not support requested asset context |
| `payment_not_found` | 404 | Missing or cross-account payment |
| `customer_not_found` | 404 | Missing or cross-account customer |
| `issuer_not_found` | 404 | Missing or cross-account issuer identity |
| `payout_address_not_found` | 404 | Missing or cross-account payout address |
| `issuer_email_already_verified` | 409 | The contact address is already proven |
| `issuer_in_use` | 409 | Requests were issued under this identity |
| `issuer_name_taken` | 409 | Another of your identities already uses this name |
| `attachment_not_found` | 404 | Missing or cross-account attachment, or the invoice has none |
| `attachment_scan_pending` | 409 | Malware scan has not reported; retry finalize with backoff |
| `attachment_rejected` | 422 | Not a clean PDF within limits; upload a new file |
| `attachment_not_ready` | 409 | `attachment_id` refers to an upload that is not finalized, was rejected, or expired unused before issuance (upload the PDF again); also returned by finalize before anything reached `upload_url` |
| `attachment_already_attached` | 409 | `attachment_id` already belongs to an issued invoice; one PDF per invoice |
| `payment_not_settled` | 409 | Proof requested before settlement |
| `verification_required` | 401 | Payer content is gated until the session completes verification |
| `invalid_payment_link` | 401 | Payment link does not resolve to a payment |
| `payment_not_payable` | 410 | QR/payment request is no longer available |
| `rate_limited` | 429 | Per-account allowance exhausted |
| `database_unavailable` | 503 | Persistent storage unavailable |
| `payload_too_large` | 413 | Body exceeds route limit |
| `not_found`, `method_not_allowed` | 404/405 | Route or method mismatch |
| `internal_error` | 500 | Internal processing failed |

Payment, customer, attachment, and webhook API bodies are limited to 64 KiB;
account-key bodies to 16 KiB. PDF bytes go to the presigned URL, never to the
API.
Send `request_id` to `support@payday.sh` when requesting help—never credentials,
OTP codes, webhook secrets, or unnecessary payment metadata.
