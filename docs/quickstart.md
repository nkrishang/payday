# Payday quickstart

Issue a USDC invoice, share its hosted checkout, and watch finalized funds
settle to your wallet. The invoice is the document; the payment is its on-chain
fulfilment. Everything below is also available in the dashboard at
`payday.sh/dashboard`, which signs in with the same emailed code and needs no
API key.

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

## 2. Issue an invoice

```sh
payday create \
  --amount 25.00 \
  --to 0x1111111111111111111111111111111111111111 \
  --issuer 'Acme LLC' \
  --bill-to 'Customer Inc' \
  --heading 'March retainer' \
  --reference 'INV-1042' \
  --expires-in 1h
```

- `--amount` is the invoice amount, used directly. There are no line items;
  attach a PDF with `--attachment invoice.pdf` when you need an itemized
  breakdown (one PDF, at most 5 MiB; the CLI uploads it and waits for the
  malware scan before issuing).
- `--to` is the wallet that receives exactly the invoice amount from an on-time
  successful payment.
- The quick path issues a `permissionless` invoice. To require the payer to
  verify an email address first, or to add party details, notes,
  a customer, or metadata, write the API body to a file and use
  `payday create --from-file invoice.json`; see the
  [CLI reference](cli-reference.md).
- Overpayments, late transfers, and expired balances go to the Payday recovery
  wallet. They are reviewed manually and returned by Payday; they are not sent
  back to the payer automatically, and you cannot choose that wallet.
- Expiry defaults to 24 hours. Use a duration such as `30m`, `24h`, or `7d`, or
  an RFC 3339 `--expires-at` value. The allowed window is 10 minutes to 366 days.
- The deployment chooses the chain and exact Circle-issued native USDC contract.
  Hidden chain/token overrides are intended for controlled deployments only.

The result includes a `pay_…` ID, one-time address, amount, deadline, current
status, and a `payment_url`. Add `--json` for the full API object.

For scripted retries, supply a stable key and reuse it only with the same
document:

```sh
payday --json create \
  --amount 25 --to 0x1111111111111111111111111111111111111111 \
  --issuer 'Acme LLC' --bill-to 'Customer Inc' \
  --idempotency-key order-1042
```

Without `--idempotency-key`, each invocation generates a new UUIDv7 and can
create a new payment.

## 3. Share and track

Send the returned `payment_url` to the payer. The hosted checkout displays the
invoice (issuer, bill-to, heading, reference, attached PDF), the remaining
amount, network, exact token, one-time address, QR code, deadline, and live
finalized status, and lets the payer pay from a connected wallet in the page.
For a verified payer mode it shows only the issuer name and heading until the
payer completes verification. The link is deliberately open: anyone holding it
can read the invoice and fulfil it, which is what makes it shareable. It
carries no merchant data — no payout address, recovery address, metadata, or
policy assertions.

Track settlement in another terminal:

```sh
payday get <PAYMENT-ID> --watch
```

A normal payment moves through `awaiting_payment` → `paid` → `settled`.
`partially_paid` appears when finalized transfers are still short. Payday does
not credit wallet-submitted or merely included transactions; reads report the
indexer's `as_of` block and freshness.

List recent invoices or continue a page:

```sh
payday list --status partially_paid --limit 20
payday list --limit 20 --starting-after <NEXT-CURSOR>
```

Once settled, download the Proof of Payment and verify it offline — it ties
the exact invoice to its payment address and the transfers that paid it,
without trusting Payday's database:

```sh
payday proof download <PAYMENT-ID> --output proof.json
payday proof verify proof.json --attachment invoice.pdf \
  --trusted-attestor <PAYDAY-ATTESTOR-ADDRESS>
```

Add `--rpc-url <JSON-RPC-URL>` to also confirm each transfer's receipt and the
settlement transaction that forwarded the invoice amount to the receiver. Save the Payday-rendered invoice PDF at any time with
`payday get <PAYMENT-ID> --pdf invoice-summary.pdf`.

Read [Payment concepts](concepts.md) before production integration, then use
the [CLI reference](cli-reference.md), [HTTP API reference](api-reference.md),
and [webhook guide](webhooks.md).

## Try the sandbox

`--sandbox` selects `https://api.sandbox.payday.sh` and sandbox credentials:

```sh
payday --sandbox login
payday --sandbox create \
  --amount 1 --to 0x1111111111111111111111111111111111111111 \
  --issuer 'Acme LLC' --bill-to 'Customer Inc'
```

Sandbox is an isolated Monad testnet deployment using test USDC; it never marks
a payment paid without a finalized on-chain transfer. See [Sandbox](sandbox.md).
