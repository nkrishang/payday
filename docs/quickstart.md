# Payday quickstart

Issue a USDC deposit request, share its hosted checkout, and watch finalized funds
settle to your wallet. The deposit request is the document; the deposit request is its on-chain
fulfilment. Everything below is also available in the dashboard at
`payday.sh/dashboard`, which signs in with an emailed code and needs no API
key.

## 1. Sign in and mint an API key

Open `payday.sh`, choose **Start Building**, and sign in with the emailed
one-time code. The account is created the first time the code is accepted.
In the dashboard's **API key** section, mint a key; it asks for a fresh code
first and shows the key exactly once. Put it in your server's secret store as
`PAYDAY_API_KEY`. See
[Authentication and API keys](authentication.md) for rotation, revocation,
and what the key may do.

The examples below use `curl` and `jq`. TypeScript applications can use the
zero-dependency client in [`sdk/typescript`](../sdk/typescript/README.md),
which wraps the same calls.

```sh
export API=https://api.payday.sh
export PAYDAY_API_KEY=payday_live_...
```

## 2. Issue a deposit request

```sh
curl -fsS "$API/v1/deposit-requests" \
  -H "Authorization: Bearer $PAYDAY_API_KEY" \
  -H 'Content-Type: application/json' \
  -H "Idempotency-Key: order-1042" \
  -d '{
    "amount": "25.00",
    "payout_address": "0x1111111111111111111111111111111111111111",
    "issuer": { "name": "Acme LLC" },
    "payer": { "name": "Customer Inc" },
    "heading": "March retainer",
    "reference": "INV-1042",
    "payer_policy": { "mode": "permissionless" },
    "expires_in": 3600
  }' | jq
```

- `amount` is the requested amount, used directly. There are no line items;
  attach a PDF when you need an itemized breakdown (one PDF, at most 5 MiB,
  reserved through `POST /v1/attachments`, uploaded to the presigned URL, and
  finalized once the malware scan admits it; the SDK's `attachments.upload`
  runs the whole exchange).
- `payout_address` is the wallet that receives exactly the requested amount from
  an on-time successful deposit.
- `payer_policy` here is `permissionless`. To require the payer to verify an
  email address first, to have your own application open the checkout for a
  user it has signed in (`merchant_session`), or to add party details, notes,
  a customer, or metadata, see the [API reference](api-reference.md).
- The response's `address` is null until the payer, on the hosted page, signs
  the request's attestation from the wallet they will pay from; a
  `deposit_request.ready` webhook reports it. Overpayments, late transfers, and
  expired balances go back to that wallet on-chain, automatically; you cannot
  choose it, and Payday never holds them.
- Expiry defaults to 24 hours. Use `expires_in` in seconds or an RFC 3339
  `expires_at`. The allowed window is 10 minutes to 366 days.
- You do not choose a network. The request offers every supported one
  (`networks`, each with its exact Circle-issued native USDC contract) and
  the payer picks where to pay when they sign; `chain` and `token` are set
  from then on.

The response includes a `dr_…` ID, one-time address, amount, deadline, current
status, and a `deposit_url`.

`Idempotency-Key` is required. Reuse a key only with the same document: a
replay returns the original deposit request with `Idempotency-Replayed: true`, and a
different body under the same key is `409 idempotency_conflict`.

## 3. Share and track

Send the returned `deposit_url` to the payer. The hosted checkout displays the
deposit request (issuer, payer, heading, reference, attached PDF), the remaining
amount, the network step, then the exact token, one-time address, QR code,
deadline, and live finalized status, and lets the payer pay from a connected wallet in the page.
For a verified payer mode it shows only the issuer name and heading until the
payer completes verification. The link is deliberately open: anyone holding it
can read the deposit request and fulfil it, which is what makes it shareable. It
carries no merchant data — no payout address, recovery address, metadata, or
policy assertions.

Track settlement by polling, or register a webhook
([Webhooks](webhooks.md)) and let Payday tell you:

```sh
curl -fsS "$API/v1/deposit-requests/<DEPOSIT_REQUEST_ID>" \
  -H "Authorization: Bearer $PAYDAY_API_KEY" | jq '{status, received_base_units, settlement_tx_hash}'
```

A normal deposit moves through `awaiting_deposit` → `deposited` → `settled`.
`partially_deposited` appears when finalized transfers are still short. Payday does
not credit wallet-submitted or merely included transactions; reads report the
indexer's `as_of` block and freshness.

List recent deposit requests or continue a page:

```sh
curl -fsS "$API/v1/deposit-requests?status=partially_deposited&limit=20" \
  -H "Authorization: Bearer $PAYDAY_API_KEY" | jq
curl -fsS "$API/v1/deposit-requests?limit=20&starting_after=<NEXT-CURSOR>" \
  -H "Authorization: Bearer $PAYDAY_API_KEY" | jq
```

Once settled, download the Proof of Payment. It ties the exact deposit request to its
deposit address and the transfers that paid it, and verifies offline without
trusting Payday's database; the checks are the ones in
`gateway_core::verify_proof`, and the [API reference](api-reference.md)
describes what each field commits to:

```sh
curl -fsS "$API/v1/deposit-requests/<DEPOSIT_REQUEST_ID>/proof" \
  -H "Authorization: Bearer $PAYDAY_API_KEY" > proof.json
```

Save the Payday-rendered deposit request PDF at any time from
`GET /v1/deposit-requests/<DEPOSIT_REQUEST_ID>/request.pdf`.
