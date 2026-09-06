# Webhooks

Set `PAYDAY_WEBHOOK_ENCRYPTION_KEY` to exactly 32 random bytes encoded with
standard base64. Startup without it is an explicit safe boundary: endpoint
creation and delivery are disabled. Signing secrets are returned only by
`POST /v1/webhooks`; the database stores SHA-256 for identification and an
AES-256-GCM ciphertext for delivery. Ciphertexts carry an explicit `v1`
version and key identifier so keys can be rotated safely.

Terraform generates and injects this key through Secrets Manager. Updating a
secret value does not update already-running ECS tasks: deploy or restart all
API tasks after rotation. Existing ciphertext remains tied to its recorded key
version/key ID, so retain old key material until it has been re-encrypted; merely
creating a new Secrets Manager version is not a complete application-level
rotation.

Endpoint URLs must be credential-free HTTPS URLs with a DNS hostname. Payday
resolves and rejects every loopback, private, link-local, and reserved
destination both when an endpoint is created and immediately before every
request. Redirects are never followed. `DELETE /v1/webhooks/{id}` disables an
endpoint (`204`) without deleting delivery history, which
`GET /v1/webhook-deliveries?endpoint_id={id}` keeps paging; the same URL can
subsequently be registered with a new secret. The routes are listed in the
[HTTP API reference](api-reference.md#webhooks).

Deliveries contain `Payday-Event-Id`, `Payday-Event-Type`, and
`Payday-Signature: v1,t=<unix-seconds>,sha256=<hex>`. Verify HMAC-SHA256 with
the signing secret over the exact bytes `v1.<timestamp>.<raw HTTP body>`, and
reject stale timestamps. Event IDs are idempotency keys. Non-2xx responses are
retried exponentially (up to 12 attempts, capped at one hour); redirects are
not followed. A delivery becomes `failed` after its twelfth failed attempt.
The deliveries API returns that terminal state and immutable per-attempt HTTP
status/error, timestamp, and duration history.

## Event types

Lifecycle events map internal transitions as follows and happen at most once
per deposit request: funded → `deposit_request.deposited`, fulfilled → `deposit_request.settled`, expired →
`deposit_request.expired`, recovered → `deposit_request.returned`, blocked →
`deposit_request.needs_attention`. Each is inserted by the same database transaction
that updates the lifecycle row.

| Event | When |
|---|---|
| `deposit_request.deposited`, `deposit_request.settled`, `deposit_request.expired`, `deposit_request.returned`, `deposit_request.needs_attention` | The lifecycle transitions above |
| `deposit_request.recovered_funds` | Funds went back to the payer's attested wallet on the deposit request's behalf: an overpayment remainder at settlement, an expired balance, or a late transfer. One event per returned amount, written in the transaction that records it, so **a deposit request can raise this event more than once** (an overpayment, then a late transfer) and it does not consume the lifecycle uniqueness slot |
| `verification.approved` | Raised by the database when `verification_completed_at` is first set: the deposit request's payer policy was satisfied — a proven mailbox, or a merchant-session client secret exchanged by the hosted checkout |
| `deposit_request.ready` | Raised by the database when `wallet_bound_at` is first set: the payer attested their wallet and the deposit address now exists. This is the moment an integration may quote the address |
| `deposit_request.likely_unsolicited` | Raised by the database when `likely_unsolicited_at` is first set: finalized funds arrived from a wallet other than the attested one. They count toward the amount and settle, but they are not the payer's, and no Proof of Payment is issued |

`verification.approved`, `deposit_request.ready`, and `deposit_request.likely_unsolicited`
are inserted by the same `invoices` table trigger as the lifecycle events, in the
transaction that first sets `verification_completed_at`, `wallet_bound_at`,
or `likely_unsolicited_at`.

## Payload

Payloads use the public, versioned `2026-08-01` envelope: `id`, `type`,
`occurred_at`, and `data`. Every deposit request event carries
`data.deposit_request`, a strict subset of the API's own deposit request
object under the same names, units, and formats — so a handler can hand
`id` straight to `GET /v1/deposit-requests/{id}`, compare `amount` with the
API's `amount`, and parse every timestamp the same way:

```json
{
  "id": "dr_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "status": "settled",
  "amount": "10.500000",
  "amount_base_units": "10500000",
  "received": "10.500000",
  "received_base_units": "10500000",
  "heading": "March retainer",
  "reference": "INV-1042",
  "metadata": {"po": "PO-77"},
  "customer_id": null,
  "issuer_id": "0198f80c-1111-7dc1-a369-90556a64f700",
  "payer_policy_mode": "merchant_session",
  "payer_reference": "user_123",
  "verification_completed_at": "2026-09-01T11:58:00Z",
  "likely_unsolicited_at": null,
  "payer_wallet": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
  "address": "0x2222222222222222222222222222222222222222",
  "wallet_bound_at": "2026-09-01T11:58:30Z",
  "expires_at": "2026-09-02T11:57:00Z",
  "created_at": "2026-09-01T11:57:00Z"
}
```

The `dr_` id, the decimal `amount` and `received` beside their
`_base_units`, EIP-55 addresses, and RFC 3339 UTC timestamps to the second
are exactly what the API returns. `payer_wallet`, `address`, and
`wallet_bound_at` are null before `deposit_request.ready`. `payer_reference`
is your own identifier for the payer on a `merchant_session` deposit (`null`
otherwise), so a `deposit_request.deposited` or `deposit_request.settled`
handler can credit that user's ledger directly. The payload never includes
the expected email or the payer's own data. `deposit_request.needs_attention`
adds `data.deposit_request.attention {code, message, action}`, the same
object the API's `attention` field carries. Lifecycle payloads carry no
recovery flag by design: a `deposit_request.settled` for an overpaid deposit
request is indistinguishable from one for an exact deposit, and
`deposit_request.recovered_funds` is the recovery signal.

`deposit_request.recovered_funds` adds `data.recovery`:

```json
{
  "id": "…",
  "amount": "0.250000",
  "amount_base_units": "250000",
  "reason": "overpayment",
  "transaction_hash": "0x…",
  "block_number": "12345",
  "recovered_at": "2026-09-01T12:00:00Z"
}
```

`reason` is one of `overpayment`, `expired`, or `late_transfer`. Returned
funds went to the payer's attested wallet on-chain in the named transaction;
nothing is held by Payday. Use these events to explain to a payer where the
difference went.

Test events are sent only to the requested endpoint, require no deposit request, and
cannot consume a real lifecycle event's uniqueness key.
