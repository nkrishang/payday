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

Sandbox keys use `payday_test_…`. Account-key issuance routes instead require a
fresh Auth0 email-OTP access token. A credential grants full read/write access
to its account; keys are not currently scoped.

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
  "expires_in": 3600,
  "reference": "order-42",
  "metadata": {"customer":"cus_123"}
}
```

| Field | Rules |
|---|---|
| `amount` | Required positive USDC decimal; at most six fractional digits |
| `payout_address` | Required nonzero EVM address; receives exactly `amount` |
| `expires_in` | Optional lifetime in seconds |
| `expires_at` | Optional RFC 3339 deadline; mutually exclusive with `expires_in` |
| `chain_id`, `token_address` | Optional deployment overrides; otherwise configured chain/native USDC |
| `reference` | Optional merchant reference, at most 128 characters |
| `memo` | Compatibility alias for reference, at most 128 characters and 2,000 encoded bytes; if both are present they must match |
| `metadata` | JSON object, at most 16 keys and 512 encoded bytes per value |

Unknown fields are rejected. Recovery is not a request field: Payday stamps its
own recovery wallet on every payment, so a body carrying `refund_address` is
rejected like any other unknown field. Expiry defaults to 24 hours and must be
10 minutes to 366 days ahead. A first request returns `201`; an identical retry returns the
original payment with `200` and `Idempotency-Replayed: true`. Reuse with changed
parameters returns `409 idempotency_conflict`. The Payday recovery wallet is
one of the committed parameters even though it is not a request field, because
it is part of the payment address: a replay after Payday rotates the recovery
wallet also returns `409`, and the original payment must be fetched with
`GET /v1/payments/{reference}` rather than replayed. Relative-expiry retries retain
the original resolved deadline. Payment creation also requires the account to
have a verified support email from a recent login.

### `GET /v1/payments`

Lists newest first. Query parameters:

- `status`: one public status;
- `reference`: exact merchant reference filter;
- `limit`: 1–100, default 20;
- `starting_after`: complete `pay_…` cursor returned as `next_cursor`.

Returns `{ "payments": [PaymentSummary], "next_cursor": null | "pay_…" }`.
Summaries contain `id`, `memo`, `reference`, `metadata`, `created_at`, `status`,
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
- merchant data: `memo`, `reference`, `metadata`, `created_at`, `updated_at`;
- audit/freshness: `transfers`, optional `as_of {block,at}`,
  `indexer_freshness`, and `self_settlement {factory,salt}`.

Public status values are `awaiting_payment`, `partially_paid`, `paid`,
`settled`, `expired`, `returned`, and `needs_attention`. Clients must tolerate
new fields and should branch only on documented status values.

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

The checkout reads payment data from `GET /v1/payer/payments/{id}` and QR SVG
from `GET /v1/payer/payments/{id}/qr`. These routes are unauthenticated, accept
no account API key, and expose no merchant data. JSON responses use
`Cache-Control: no-store`, and QR requests return `410 payment_not_payable` once
the address should no longer be presented. Both send
`Access-Control-Allow-Origin: *` for `GET`, so a browser on any origin can build
a checkout against them; no other route allows cross-origin reads.

## Stable error codes

| Code | Typical status | Meaning |
|---|---:|---|
| `unauthorized` | 401 | Missing or invalid API key |
| `identity_unauthorized` | 401 | Invalid Auth0 identity token |
| `identity_unavailable` | 503 | Identity verification unavailable |
| `account_disabled` | 403 | Account disabled |
| `account_not_provisioned` | 404 | Identity has no Payday account |
| `account_contact_required` | 409 | Login again to attach a verified merchant email |
| `authentication_event_already_used` | 409 | Identity event already mutated key state |
| `api_key_generation_conflict` | 409 | Key generation changed or was omitted incorrectly |
| `missing_idempotency_key` | 400 | Create header absent |
| `idempotency_conflict` | 409 | Key reused with different payment parameters; the Payday recovery wallet counts, so a replay after it was rotated conflicts and the original must be fetched with `GET` |
| `invalid_request` | 400 | Invalid field, query, JSON, or request shape |
| `invalid_amount` | 400 | Invalid amount syntax, precision, or positivity |
| `unsupported_chain`, `unsupported_token` | 422 | Deployment does not support requested asset context |
| `payment_not_found` | 404 | Missing or cross-account payment |
| `invalid_payment_link` | 401 | Payment link does not resolve to a payment |
| `payment_not_payable` | 410 | QR/payment request is no longer available |
| `rate_limited` | 429 | Per-account allowance exhausted |
| `database_unavailable` | 503 | Persistent storage unavailable |
| `payload_too_large` | 413 | Body exceeds route limit |
| `not_found`, `method_not_allowed` | 404/405 | Route or method mismatch |
| `internal_error` | 500 | Internal processing failed |

Payment/webhook API bodies are limited to 64 KiB; account-key bodies to 16 KiB.
Send `request_id` to `support@payday.sh` when requesting help—never credentials,
OTP codes, webhook secrets, or unnecessary payment metadata.
