# HTTP API reference

Append the paths below to your deployment's HTTPS base URL. JSON request bodies must use `Content-Type: application/json`. Invoice routes have a 64 KiB request-body limit; account routes have a 16 KiB limit. The health route is plain text and unauthenticated.

## Authentication

There are two distinct bearer credential types:

* **Invoice API:** `POST /v1/invoices` and `GET /v1/invoices/{id}` require `Authorization: Bearer <API_KEY>`. Obtain the API key through the account flow. Missing, malformed, revoked, or unknown keys return `401 unauthorized`.
* **Account management:** both `/v1/account/api-key` operations require `Authorization: Bearer <IDENTITY_ACCESS_TOKEN>`, a fresh Auth0 access token produced by the supported email-OTP sign-in flow. It is an identity proof used to issue or inspect API keys; it is not an invoice API key. Invalid identity tokens return `401 identity_unauthorized`, and identity verification being unavailable returns `503 identity_unavailable`.

Bearer scheme matching is case-insensitive, and the header must contain exactly one scheme and one credential. A `401` response includes `WWW-Authenticate: Bearer`.

## Wire conventions

Invoice integer values that may exceed common JSON number precision are decimal **strings**, including `chain_id`, `expiration_timestamp`, `amount_base_units`, `received_base_units`, `salt`, and non-null `resolved_at_block`. `amount` and `received` are human-readable decimal strings. Account `generation` values are JSON integers. Account timestamps (`created_at` and `rotated_at`) are RFC 3339 strings; invoice expiration is Unix seconds encoded as a string.

EVM addresses are returned in checksummed `0x` form. Optional invoice fields and `rotated_at` are present as JSON `null` when unavailable.

## Routes

### `GET /health`

No authentication. This checks service and database reachability.

| Status | Content type/body |
|---|---|
| `200 OK` | plain text `ok` |
| `503 Service Unavailable` | plain text `database unavailable` |

### `POST /v1/invoices`

Requires an API key, JSON, and an `Idempotency-Key` header containing 1–255 bytes.

```json
{
  "chain_id": "8453",
  "token_address": "0x1111111111111111111111111111111111111111",
  "beneficiary_address": "0x2222222222222222222222222222222222222222",
  "amount": "100.5",
  "expiration_timestamp": "1900000000",
  "recovery_address": "0x3333333333333333333333333333333333333333"
}
```

Validation:

* `chain_id` is an unsigned 64-bit decimal string and must equal the deployment's configured chain.
* `token_address` must be a valid EVM address and equal the configured Circle-issued USDC contract.
* `beneficiary_address` and `recovery_address` must be valid, nonzero EVM addresses.
* `amount` is a positive, unsigned, ordinary decimal string (`digits` or `digits.digits`) with at most six fractional digits. Signs, whitespace, exponents, separators, and zero are rejected.
* `expiration_timestamp` is an unsigned 64-bit Unix-seconds string. At request processing time it must be at least 600 seconds and at most 31,622,400 seconds (366 days) in the future, inclusively.

The first successful use of an idempotency key creates an invoice and returns `201 Created`. Repeating the same key under the same account with semantically identical request parameters returns the original invoice with `200 OK`. Reusing it with different parameters returns `409 idempotency_conflict`. Keys and invoices are scoped to an account, so another account may use the same key independently.

Success returns the [invoice object](#invoice-object). Other documented statuses are `400` (`missing_idempotency_key`, `invalid_request`, or `invalid_amount`), `401 unauthorized`, `409 idempotency_conflict`, `422` (`unsupported_chain` or `unsupported_token`), and server errors listed below.

### `GET /v1/invoices/{id}`

Requires an API key. `id` must be a UUID. Success is `200 OK` with the [invoice object](#invoice-object). A malformed UUID returns `400 invalid_request`. A missing invoice returns `404 invoice_not_found`.

Invoice ownership is deliberately concealed: requesting an invoice belonging to another account also returns `404 invoice_not_found`, not that invoice's data.

### Invoice object

```json
{
  "id": "019539a0-7e00-7000-8000-000000000000",
  "chain_id": "8453",
  "factory_address": "0x4444444444444444444444444444444444444444",
  "token": {
    "address": "0x1111111111111111111111111111111111111111",
    "kind": "erc20",
    "decimals": 6
  },
  "beneficiary_address": "0x2222222222222222222222222222222222222222",
  "expiration_timestamp": "1900000000",
  "recovery_address": "0x3333333333333333333333333333333333333333",
  "amount": "100.500000",
  "amount_base_units": "100500000",
  "salt": "123456789",
  "payment_address": "0x5555555555555555555555555555555555555555",
  "status": "created",
  "received": "0.000000",
  "received_base_units": "0",
  "execute_tx_hash": null,
  "resolved_at_block": null,
  "blocked_reason": null
}
```

`payment_address` is single-use and valid only while `status` is `created`; after settlement, funds sent there are forwarded to `recovery_address`, not the beneficiary. `received` is finalized USDC credited so far. `execute_tx_hash` identifies deployment/execution when sent by this service; `resolved_at_block` is populated when the invoice becomes `fulfilled` or `recovered`; `blocked_reason` explains why automatic sweeping stopped when applicable. Status and blocked-reason strings should be treated as values returned by the service rather than inferred client-side.

### `POST /v1/account/api-key`

Requires an Auth0 identity access token and JSON. To create the first key:

```json
{}
```

To replace a known current generation:

```json
{ "expected_generation": 3 }
```

`expected_generation` may be omitted or `null`; if present it must be a positive JSON integer. Initial issuance returns `201 Created`; replacement returns `200 OK`:

```json
{
  "api_key": "payday_live_…",
  "generation": 4,
  "replaced_previous_key": true
}
```

Only one API key is active per account. The plaintext key is shown only in this response and cannot be retrieved later. Replacement immediately invalidates the old key. For safe rotation, first read the current generation, obtain fresh identity authentication, and submit that generation: a stale or omitted generation when a key already exists returns `409 api_key_generation_conflict`. An identity authentication event can issue at most one key; reuse returns `409 authentication_event_already_used`.

### `GET /v1/account/api-key`

Requires an Auth0 identity access token. Returns `200 OK` with non-secret metadata:

```json
{
  "hint": "payday_live_…abcd",
  "generation": 4,
  "created_at": "2026-08-27T12:00:00+00:00",
  "rotated_at": "2026-08-27T12:30:00+00:00"
}
```

`rotated_at` is `null` before the first replacement. An identity with no provisioned account returns `404 account_not_provisioned`.

## Errors

Application errors use JSON:

```json
{
  "error": {
    "code": "invalid_request",
    "message": "human-readable detail"
  }
}
```

Use `error.code` for program logic; messages may include validation detail. Framework-level failures such as an unsupported media type, malformed JSON extraction, oversized body, unmatched route, or unsupported method are not created by this application error type and are not guaranteed to use this envelope.

### Stable error codes

| Code | HTTP status | Meaning |
|---|---:|---|
| `unauthorized` | 401 | A valid bearer API key is required. |
| `identity_unauthorized` | 401 | A valid Auth0 identity access token is required. |
| `identity_unavailable` | 503 | Account authentication is temporarily unavailable. |
| `authentication_event_already_used` | 409 | This identity authentication event already issued a key; authenticate again. |
| `account_not_provisioned` | 404 | The identity has no account/API key. |
| `api_key_generation_conflict` | 409 | The expected API-key generation is absent/stale or the key changed concurrently. |
| `missing_idempotency_key` | 400 | `Idempotency-Key` is missing or not a valid header string. |
| `invalid_request` | 400 | A request field, invoice ID, idempotency-key length, or expected generation is invalid. |
| `invalid_amount` | 400 | The amount syntax, precision, range, or positivity is invalid. |
| `unsupported_chain` | 422 | The configured service does not support the requested chain. |
| `unsupported_token` | 422 | The token is not the configured Circle-issued USDC contract. |
| `idempotency_conflict` | 409 | The account already used this idempotency key with different parameters. |
| `invoice_not_found` | 404 | The invoice does not exist or is owned by another account. |
| `database_unavailable` | 503 | Persistent storage is unavailable. |
| `internal_error` | 500 | Stored data or another internal condition could not be processed. |
