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

Events map internal transitions as follows: funded → `payment.paid`, fulfilled
→ `payment.settled`, expired → `payment.expired`, recovered →
`payment.refunded`, blocked → `payment.needs_attention`. The event is inserted
by the same database transaction that updates the lifecycle row. Payloads use
the public, versioned `2026-08-01` envelope and include the stable event ID,
`occurred_at`, public payment status, reference, and metadata. Test events are
sent only to the requested endpoint, require no invoice, and cannot consume a
real lifecycle event's uniqueness key.
