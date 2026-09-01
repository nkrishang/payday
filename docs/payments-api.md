# Payments API

TypeScript applications can use the zero-runtime-dependency client in
[`sdk/typescript`](../sdk/typescript/README.md).

`/v1/payments` is the canonical customer API. All payment routes require bearer authentication and
creates additionally require `Idempotency-Key`. A replay returns
`Idempotency-Replayed: true`.

Create requests contain `amount` and `payout_address`. Exactly `amount` settles
to `payout_address`; the response's `recovery_address` is the Payday recovery
wallet the payment is committed to, where overpayment remainders, expired
balances, and late transfers land before the operator returns them after manual
review. It is platform-configured, so a request carrying `refund_address` is
rejected. Choose either `expires_in` (seconds) or RFC3339 `expires_at`, or omit
both for a 24-hour lifetime. `chain_id` and
`token_address` default to the configured chain and USDC contract. `reference`
and a small JSON-object `metadata` are optional.

Payments expose the public states `awaiting_payment`, `partially_paid`, `paid`,
`settled`, `expired`, `returned`, and `needs_attention`; amounts are trimmed USDC strings and are
also returned in integer base units. The current fee is explicitly zero, so
`net_amount` equals `amount`. Every read includes the indexer's committed
`as_of` block and timestamp once the indexer has committed its first range. Cancellation only changes API presentation: it
cannot disable the deposit address or prevent detection and collection of
on-chain transfers.

Every payment includes a `payment_url` that can be shared directly with
the payer. Its checkout page shows the remaining amount, a copyable one-time
address, an EIP-681 wallet request and QR code, a server-clock countdown, and
live finalized status, and can send the transfer from a connected wallet.
Address, settlement, and transfer explorer URLs are included when the configured
chain has an explorer.

The link is unauthenticated by design — anyone holding it may read the payment
and pay it. `GET /v1/payer/payments/{id}` and its `/qr` accept no API key,
return no merchant data, and send `Access-Control-Allow-Origin: *`, so a
merchant can build a checkout of their own against them. Reproduce the guidance
in [Payment safety](payment-safety.md) if you do.

`GET /v1/payments` accepts `status`, `reference`, `starting_after`, and `limit`.
`GET /v1/payments/{id}/transfers` returns finalized transfer provenance.
`GET /v1/payments/{id}?wait_for=change&timeout=30` waits until the payment
changes or the timeout elapses, avoiding a polling loop.
`GET /v1/account` returns the authenticated account ID and `GET /v1/status`
reports chain, indexer, finalized-head, and sweep-queue state.

## HTTP contract and operational policy

The OpenAPI 3.1 document is served at `/openapi.json` and an interactive Scalar
reference at `/docs`. Aliases `/api` and `/api/openapi.json` allow the same
gateway routes to be published directly at `https://docs.payday.sh/api`.

Every response has `X-Request-Id`. A printable, non-whitespace caller value up
to 128 bytes is echoed; otherwise the gateway generates a UUIDv7. Every JSON
error includes the same value as top-level `request_id`, including
authentication, malformed JSON, route/extractor, and body-size failures.
For support, email `support@payday.sh` with that request ID; never send an API
key, webhook secret, or full payment metadata.

Authenticated API-key traffic uses an in-memory token bucket independently per
account: capacity 60, refill 1 token/second. Responses expose
`X-RateLimit-Limit`, `X-RateLimit-Remaining`, and `X-RateLimit-Reset` (Unix
epoch seconds when the bucket is expected to be full). Rejections are 429 with
`Retry-After: 1`. Buckets are
process-local, so a multi-task deployment permits this allowance per task; the
WAF remains the deployment-wide abuse boundary.

`/health` is unauthenticated load-balancer readiness: it returns 200 only after
a database round trip and 503 otherwise. `/v1/status` is authenticated,
rate-limited operational state and may return 503 when database state cannot be
read. Structured request logs contain method, path, status, latency in
milliseconds, and authenticated account ID. Headers and bodies are never
logged, which keeps bearer keys, webhook secrets, and payment metadata out.

## Curl quickstart

```bash
export API=https://api.sandbox.payday.sh
export PAYDAY_API_KEY=payday_test_...
PAYMENT=$(curl -fsS "$API/v1/payments" \
  -H "Authorization: Bearer $PAYDAY_API_KEY" -H 'Content-Type: application/json' \
  -H "Idempotency-Key: quickstart-$(date +%s)" \
  -d '{"amount":"1.00","payout_address":"0x1111111111111111111111111111111111111111","expires_in":3600}')
PAYMENT_ID=$(printf '%s' "$PAYMENT" | jq -r .id)
PAYMENT_ADDRESS=$(printf '%s' "$PAYMENT" | jq -r .address)
# Send test USDC to $PAYMENT_ADDRESS using the sandbox faucet/wallet; there is
# intentionally no privileged "mark paid" endpoint because indexer finality is tested.
curl -fsS "$API/v1/payments/$PAYMENT_ID" -H "Authorization: Bearer $PAYDAY_API_KEY" | jq
```
