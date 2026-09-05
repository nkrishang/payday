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

Endpoint URLs must be credential-free HTTPS URLs. Payday resolves and rejects
every loopback, private, link-local, and reserved destination both when an
endpoint is created and immediately before every request. Redirects are never
followed. `DELETE /v1/webhooks/{id}` disables an endpoint without deleting
delivery history; the same URL can subsequently be registered with a new secret.

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
| `deposit_request.recovered_funds` | Payday's recovery wallet received funds on the deposit request's behalf: an overpayment remainder at settlement, an expired balance, or a late transfer. One event per recovered amount, written in the transaction that records it, so **a deposit request can raise this event more than once** (an overpayment, then a late transfer) and it does not consume the lifecycle uniqueness slot |
| `verification.approved` | Raised by the database when `verification_completed_at` is first set: the deposit request's payer policy was satisfied — a proven mailbox, or a merchant-session client secret exchanged by the hosted checkout |
| `deposit_request.likely_unsolicited` | Raised by the database when `likely_unsolicited_at` is first set: funds were first observed before the payer policy was satisfied, so the deposit request is flagged as likely unsolicited |

`verification.approved` and `deposit_request.likely_unsolicited` are inserted by the
same `invoices` table trigger as the lifecycle events, in the transaction that first
sets `verification_completed_at` or `likely_unsolicited_at`; nothing sets
those columns until the payer-policy features are enabled for an account.

## Payload

Payloads use the public, versioned `2026-08-01` envelope: `id`, `type`,
`occurred_at`, and `data`. Every deposit request event carries `data.deposit_request` with the
public status, `amount`, `received`, `reference`, `metadata`, and four policy
fields: `payer_policy_mode`, `payer_reference`, `verification_completed_at`,
and `likely_unsolicited_at`. `payer_reference` is your own identifier for the
payer on a `merchant_session` deposit (`null` otherwise), so a `deposit_request.deposited`
or `deposit_request.settled` handler can credit that user's ledger directly. The
payload never includes the expected email or the payer's own data.
`deposit_request.needs_attention` adds
`data.deposit_request.attention`. Lifecycle payloads carry no recovery flag by design:
a `deposit_request.settled` for an overpaid deposit request is indistinguishable from one for
an exact deposit, and `deposit_request.recovered_funds` is the recovery signal.

`deposit_request.recovered_funds` adds `data.recovery`:

```json
{
  "id": "…",
  "amount": "250000",
  "reason": "overpayment",
  "transaction_hash": "0x…",
  "block_number": 12345,
  "recovered_at": "2026-09-01T12:00:00Z"
}
```

`amount` is in base units and `reason` is one of `overpayment`, `expired`, or
`late_transfer`. Recovered funds are held by Payday, reviewed manually, and
returned by the operator; use these events to reconcile what Payday holds for
your deposit requests.

Test events are sent only to the requested endpoint, require no deposit request, and
cannot consume a real lifecycle event's uniqueness key.
