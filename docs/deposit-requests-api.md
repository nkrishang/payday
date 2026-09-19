# Deposit requests API

TypeScript applications can use the zero-runtime-dependency client in
[`sdk/typescript`](../sdk/typescript/README.md).

`/v1/deposit-requests` is the canonical customer API. A deposit request is the document; a
deposit is its on-chain fulfilment, and the deposit request's state is read from the
request. All deposit request routes require bearer authentication — an API key,
or the Privy identity token a signed-in dashboard holds — and creates
additionally require `Idempotency-Key`. A replay returns
`Idempotency-Replayed: true`.

Create requests contain `amount`, `payout_address`, an `issuer` and a `payer`
party (`name`, optional `email` and `details`), and an optional `verification`
object of up to three independent add-ons: `email` (names the expected email,
verified by the same OTP flow as before), `merchant_auth` (names the user your
own application has signed in, by your `payer_reference`, and returns a
single-use `client_secret` your server hands that user — see
[Merchant sessions](api-reference.md#merchant-sessions)), and
`wallet_attestation` (`true`, which requires the payer to sign the EIP-712
attestation from the wallet they will pay from). Omitted or `{}` means no
add-ons: the request is fully permissionless. `payer_policy` is rejected as an
unknown field. Optional
fields are `currency`, `chain_id`, `notes`, `heading`, `reference`, a small JSON-object `metadata`, a
`customer_id`, an `issuer_id`, and one finalized `attachment_id` for a scanned
PDF. A request that names a saved customer may leave `payer` out, and one
that names a saved issuer identity may leave `issuer` and `payout_address`
out: the saved record is snapshotted in their place, and the smallest valid
request is `amount`, `issuer_id`, and `customer_id`. Ids are
prefixed (`dr_`, `cus_`, `iss_`, `att_`; see the reference's Conventions) and
are passed back exactly as received. The amount
is used directly; there are no line items. Exactly `amount` settles to
`payout_address`. The deposit address exists as soon as the request's network
is fixed: pin `chain_id` (or issue where the deployment offers one network)
and the create response already carries `address`, with no payer signature
involved at all and no restriction on which wallet the payer pays from — an
exchange withdrawal, for example, is fine. Without a pinned `chain_id`, the
payer picks a network on the hosted checkout (`POST
/v1/payer/deposit-requests/{id}/network`, no wallet signature needed) and the
address follows. With the `wallet_attestation` add-on, the old wallet step
happens instead: the payer signs the attestation from the chosen network and
the address exists only after it (`payer_wallet`), raising a
`deposit_request.ready` webhook. Recovery is never a request field, so a
request carrying
`refund_address` is rejected: every deposit address's recovery term is
Gum's own dedicated KMS recovery wallet (`recovery_address`), where
overpayment remainders, expired balances, and late transfers land on-chain
before Gum returns them to the payer manually, after review. Choose either `expires_in` (seconds) or RFC3339 `expires_at`, or omit
both for a 24-hour lifetime. `currency` is `USDC` (the default) or `USDT`,
and every response carries it. There is no token field: a USDC request
offers every supported network (`networks`) and the payer chooses one on
the hosted checkout when the network is not pinned; `chain` and
`token` are `null` until the network is fixed. USDC bridges 1:1, so a USDC
request may be paid on any network and the merchant withdraws to any. USDT
has no such path, so a USDT request must pin `chain_id` to a network
serving USDT (Monad or Arbitrum One; `400 invalid_request` naming
`chain_id` otherwise, `422 unsupported_chain` for a chain that does not
serve it), and its USDT is withdrawn on that network. The payer may still
pay either from another network through Relay, carrying the spread — never
on a wallet-attested request, where a relay solver would pay from a
different wallet.

Every immutable field, including the attachment's hash, takes part in
idempotency: reusing a key with a different document returns
`409 idempotency_conflict`. The issued deposit request is canonicalized and,
together with the issuance nonce and any wallet attestation, committed into
the deposit address through the salt, which is what makes the Proof of Payment
(`GET /v1/deposit-requests/{id}/proof`, after settlement) verifiable offline: the
document, the address, the transfers, and — when the wallet-attestation
add-on is attached — the wallet that paid.

Deposit requests expose the public states `awaiting_deposit`, `partially_deposited`, `deposited`,
`settled`, `expired`, `returned`, and `needs_attention`; amounts are trimmed decimal strings in the
request's `currency` (every supported stablecoin has six decimals) and are
also returned in integer base units. The current fee is explicitly zero, so
`net_amount` equals `amount`. Every read includes the indexer's committed
`as_of` block and timestamp once the indexer has committed its first range. Cancellation only changes API presentation: it
cannot disable the deposit address or prevent detection and collection of
on-chain transfers.

Every deposit request includes a `deposit_url` that can be shared directly with
the payer. Its checkout page shows the remaining amount, a copyable one-time
address, an EIP-681 wallet request and QR code, a server-clock countdown, and
live finalized status, and can send the transfer from a connected wallet.
Address, settlement, and transfer explorer URLs are included when the configured
chain has an explorer.

When the `payer` party carries an `email`, Gum also emails that address as
the request is issued: a message from `contact@gum.money`, in Gum's design,
that names the issuer, the amount, the heading and reference, and the expiry,
with a button to the same `deposit_url` and `contact@gum.money` for questions.
The email is queued in the issuing transaction and sent by a background
worker, so it never delays the create response and is never lost to a
provider outage; an idempotent replay sends nothing again, and a request that
is cancelled, funded, or expired before the worker reaches it is not sent.
Merchant-auth requests are never emailed, since their link opens only
from your own application. The email is a courtesy, not a verification step:
the email add-on still checks `expected_email`, which may differ.

The link is unauthenticated by design — anyone holding it may read the deposit request
and pay it. `GET /v1/payer/deposit-requests/{id}`, its `/qr`, and its `/attachment`
accept no API key, return no merchant data, and send
`Access-Control-Allow-Origin: *`, so a merchant can build a checkout of their
own against them. For requests with an email or merchant-auth add-on the page
shows only the issuer name and heading until the payer's session satisfies it; the amount,
payer, notes, reference, PDF, address, and QR are withheld
(`content_unlocked: false`). A merchant-auth page unlocks the moment your
application opens it with the client secret in the URL fragment
(`deposit_url#cs=…`); the bare link tells the payer to open it from your app.
Without the wallet-attestation add-on, no signature is asked for: the payer
picks a network if none is pinned and the address appears. With it, the
request takes the wallet step (`/wallet/challenge` and
`/wallet/attest`, see the API reference) before the address, QR, and wallet
button appear. Reproduce the guidance in [Deposit safety](deposit-safety.md)
if you build your own.

`GET /v1/deposit-requests` accepts `status`, `reference`, `customer_id`,
`issuer_id`, `verification`, `starting_after`, and `limit`.
`POST /v1/deposit-requests/{id}/cancel` returns the deposit request with
`cancellation_requested_at` set.
`GET /v1/deposit-requests/{id}/transfers` returns `{ "transfers": [...] }`, the
finalized transfer provenance.
`GET /v1/deposit-requests/{id}/attachment` returns the PDF descriptor with a signed
download URL, `GET /v1/deposit-requests/{id}/request.pdf` renders Gum's
deterministic deposit request summary, and `GET /v1/deposit-requests/{id}/proof` returns the
Proof of Payment once settled. `GET /v1/deposit-requests/{id}?wait_for=change&timeout=30`
waits until the deposit request changes or the timeout elapses, avoiding a polling
loop. `/v1/customers` manages reusable counterparty records and
`/v1/attachments` runs the presigned PDF upload and finalization; see the
[HTTP API reference](api-reference.md). `GET /v1/account` returns the
authenticated account ID and `GET /v1/status` reports chain, indexer,
finalized-head, and sweep-queue state.

## HTTP contract and operational policy

The OpenAPI 3.1 document is served at `/openapi.json` and an interactive Scalar
reference at `/docs`. Aliases `/api` and `/api/openapi.json` allow the same
gateway routes to be published directly at `https://docs.gum.money/api`.

Every response has `X-Request-Id`. A printable, non-whitespace caller value up
to 128 bytes is echoed; otherwise the gateway generates a UUIDv7. Every JSON
error includes the same value as top-level `request_id`, including
authentication, malformed JSON, route/extractor, and body-size failures.
For support, email `support@gum.money` with that request ID; never send an API
key, webhook secret, or full deposit metadata.

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
logged, which keeps bearer keys, webhook secrets, and deposit metadata out.

## Curl quickstart

```bash
export API=https://api.sandbox.gum.money
export GUM_API_KEY=gum_test_...
REQUEST=$(curl -fsS "$API/v1/deposit-requests" \
  -H "Authorization: Bearer $GUM_API_KEY" -H 'Content-Type: application/json' \
  -H "Idempotency-Key: quickstart-$(date +%s)" \
  -d '{"amount":"1.00","payout_address":"0x1111111111111111111111111111111111111111",
       "issuer":{"name":"Acme LLC"},"payer":{"name":"Customer Inc"},
       "verification":{"wallet_attestation":true},"expires_in":3600}')
DEPOSIT_REQUEST_ID=$(printf '%s' "$REQUEST" | jq -r .id)
DEPOSIT_URL=$(printf '%s' "$REQUEST" | jq -r .deposit_url)
# Open $DEPOSIT_URL as the payer and sign the wallet attestation: with the
# wallet-attestation add-on the one-time address exists only once the payer's
# wallet is bound (deposit_request.ready), so `.address` is null on the create
# response. Without the add-on (and with chain_id pinned, or after the payer
# picks a network) `.address` is set immediately. Then send test USDC to it —
# from the attested wallet when the add-on is attached, from any wallet when
# it is not; there is intentionally no privileged "mark deposited" endpoint
# because indexer finality is tested.
curl -fsS "$API/v1/deposit-requests/$DEPOSIT_REQUEST_ID?wait_for=change&timeout=30" \
  -H "Authorization: Bearer $GUM_API_KEY" | jq '{status, address, received}'
```
