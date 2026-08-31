# Payday quickstart

Create a one-time USDC payment, share its hosted checkout, and watch finalized
funds settle to your wallet.

## 1. Install and sign in

On Linux x86-64 or macOS, install the latest checksum-verified release:

```sh
curl --fail --proto '=https' --tlsv1.2 \
  https://raw.githubusercontent.com/nkrishang/payday/main/scripts/install.sh | sh
```

See [Install the Payday CLI](install.md) for Cargo, Homebrew, Windows, pinned
versions, and upgrades. Then authenticate with the emailed one-time code:

```sh
payday login
```

The CLI saves the API key in a private, API-bound credentials profile. Use
`payday login --show` only when copying the key into an approved secret manager.
For CI, set `PAYDAY_API_KEY` instead of running an interactive login.

## 2. Create a payment

```sh
payday create \
  --amount 25.00 \
  --to 0x1111111111111111111111111111111111111111 \
  --refund-to 0x2222222222222222222222222222222222222222 \
  --expires-in 1h \
  --memo 'Order 1042'
```

- `--to` is the wallet that receives an on-time successful payment.
- `--refund-to` receives expired, late, or post-settlement USDC. It defaults to
  `--to` and is not an automatic refund to the payer.
- Expiry defaults to 24 hours. Use a duration such as `30m`, `24h`, or `7d`, or
  an RFC 3339 `--expires-at` value. The allowed window is 10 minutes to 366 days.
- The deployment chooses the chain and exact Circle-issued native USDC contract.
  Hidden chain/token overrides are intended for controlled deployments only.

The result includes a `pay_…` ID, one-time address, amount, deadline, current
status, and a `payment_url`. Add `--json` for the full API object.

For scripted retries, supply a stable key and reuse it only with the same
parameters:

```sh
payday --json create \
  --amount 25 --to 0x1111111111111111111111111111111111111111 \
  --idempotency-key order-1042
```

Without `--idempotency-key`, each invocation generates a new UUIDv7 and can
create a new payment.

## 3. Share and track

Send the returned `payment_url` to the payer. The hosted checkout displays the
remaining amount, network, exact token, one-time address, QR code, wallet
request, deadline, and live finalized status. Treat the full URL as sensitive:
its signed token grants read access to that payment until 30 days after expiry.

Track settlement in another terminal:

```sh
payday get <PAYMENT-ID> --watch
```

A normal payment moves through `awaiting_payment` → `paid` → `settled`.
`partially_paid` appears when finalized transfers are still short. Payday does
not credit wallet-submitted or merely included transactions; reads report the
indexer's `as_of` block and freshness.

List recent payments or continue a page:

```sh
payday list --status partially_paid --limit 20
payday list --limit 20 --starting-after <NEXT-CURSOR>
```

Read [Payment concepts](concepts.md) before production integration, then use
the [CLI reference](cli-reference.md), [HTTP API reference](api-reference.md),
and [webhook guide](webhooks.md).

## Try the sandbox

`--sandbox` selects `https://api.sandbox.payday.sh` and sandbox credentials:

```sh
payday --sandbox login
payday --sandbox create \
  --amount 1 --to 0x1111111111111111111111111111111111111111
```

Sandbox is an isolated Monad testnet deployment using test USDC; it never marks
a payment paid without a finalized on-chain transfer. See [Sandbox](sandbox.md).
