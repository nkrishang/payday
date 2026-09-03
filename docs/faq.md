# Customer FAQ

## What is the difference between an invoice and a payment?

The invoice is the document you issue: issuer and bill-to parties, one amount,
optional notes, heading, reference, and metadata, a payer policy, and at most
one PDF. The payment is its on-chain fulfilment at a one-time address. The API
path stays `/v1/payments`; everything else says invoice. Issued invoices are
immutable — to change one, cancel it and issue another.

## Can I add line items or tax?

No. Specify the amount directly and attach a PDF if you need an itemized
breakdown or jurisdiction-specific content; Payday stores and hashes the file
but never parses it. Payday does not model line items, tax rates, tax IDs, or
fiat, and does not intend to. Bounded free-text `details` on each party and
the PDF are where that information goes.

## What are the payer modes?

`permissionless` lets anyone with the link pay, as before. `verified_email`
requires the payer to prove ownership of the mailbox you name.
`verified_identity` additionally requires a document and liveness check that
matches the first and last name you assert. `verified_identity_unattributed`
requires a successful document and liveness check for any person, without
asserting who. For the verified modes the checkout hides the amount, parties,
PDF, and address until verification completes; funds sent before then are
flagged as likely unsolicited. Payday reports pass or fail, not the verified
person's data.

## How do PDF attachments work?

One PDF per invoice, at most 5 MiB. The SDK's `attachments.upload`, the CLI's
`--attachment`, and the dashboard's upload control reserve a presigned slot,
send the bytes straight to storage, and wait for the malware scan to admit the
file; only then can its `attachment_id` be attached to an invoice. The file's
SHA-256 is committed into the payment address and appears in the Proof of
Payment. Reads return a signed URL valid for a few minutes.

## What is Proof of Payment and how do I verify it?

Once an invoice settles, `payday proof download` (or
`GET /v1/payments/{id}/proof`) exports the canonical invoice, the nonce and
salt, the payment address, the settling transfer, and a Payday-signed
verification attestation. `payday proof verify proof.json` recomputes the hash,
salt, and address and checks the transfer offline, with no Payday access; add
`--attachment` to check the PDF and `--rpc-url` to confirm the receipt on
chain. Share the proof with payers or auditors at your discretion; it is not a
public link.

## Which asset and networks can pay?

Each deployment accepts one exact Circle-issued native USDC contract on one EVM
chain. Use the `chain` and `token` returned with the payment. A matching symbol
is not enough: bridged USDC, USDC on another chain, look-alike tokens, and the
chain's gas currency do not count and may be unrecoverable.

## What is the payment address?

It is a unique counterfactual smart-contract address for one payment. USDC can
arrive before the contract exists; deployment later routes its balance under
the amount, payout, expiry, and Payday recovery terms committed into that
address. Never reuse it for another order.

## What should I give the payer?

Prefer the returned `payment_url`. It shows the invoice, network, token,
remaining amount, address, QR/wallet request, deadline, and finalized live
status — after verification, for a gated invoice. Send it to the payer. If
integrating your own UI, reproduce all safety guidance in
[Payment safety](payment-safety.md).

## Why has a wallet transaction not appeared?

Payday credits only finalized native-USDC transfers. Inclusion or wallet
confirmation can precede Payday's finality boundary and indexing cursor. Check
`as_of`, `indexer_freshness`, transfer provenance, and `/v1/status`; use
`payday get <ID> --watch` or API long polling rather than treating submission as
payment.

## What happens with exact, partial, or excess payment?

- Exact and cumulative partial transfers become `paid` once finalized credits
  reach the requested amount.
- `partially_paid` remains open while the finalized total is short.
- On-time execution sends exactly the invoice amount to payout. Any excess goes
  to the Payday recovery wallet and is returned after manual review; it is
  never forwarded to the merchant.
- If execution happens after expiry, the complete balance goes to the Payday
  recovery wallet—even if enough USDC arrived earlier.

Leave time for inclusion, finality, and settlement before the deadline.

## Where do expired or late funds go?

An underpaid address is recovered after expiry. Native USDC sent after expiry
or after the payment contract executes is routed to the Payday recovery wallet
(`recovery_address` on the payment). Payday holds those funds, records each
amount against the payment, reviews it manually, and returns it; the wallet is
not the payer's and not the merchant's. Payers should contact the merchant and
Payday support for return handling.

## Can I cancel or refund through Payday?

`payday cancel` and `POST /v1/payments/{id}/cancel` are advisory: they stop
Payday clients from presenting the payment but cannot disable an EVM address or
alter its contract. There is no refund endpoint. Funds that settled to your
payout address are yours to refund through your own wallet/process; recovered
funds (overpayments, late or expired transfers) are returned by Payday after
manual review, and `payment.recovered_funds` webhooks tell you when Payday
holds something for one of your payments.

## How do I find and reconcile payments?

Save the `pay_…` ID and your own `reference`. Look up an invoice by complete
ID or payment address. List newest-first with optional status/reference
filters and cursor pagination; summaries carry the heading, bill-to name,
policy mode, customer, and verification state. The transfers endpoint provides
finalized sender, transaction, amount, block, disposition, and collection
state, and the Proof of Payment is the durable reconciliation record.

## Should I poll or use webhooks?

Use signed webhooks for lifecycle automation and API long polling
(`wait_for=change`) for an active screen. Webhooks report paid, settled,
expired, refunded, and attention transitions, plus every amount recovered by
Payday. Verify the HMAC over the exact raw body, reject stale timestamps, and
deduplicate by event ID. See [Webhooks](webhooks.md).

## What does `needs_attention` mean?

Automatic movement stopped because Payday cannot safely continue. Do not send
more funds. Follow the response's `attention.action` and contact support with
the payment and request IDs. Settlement and later-fund recovery stay paused
until an operator resolves and releases the condition; funds are not necessarily
lost or delivered.

## How are amounts and deadlines encoded?

USDC uses six decimals. API human amounts are decimal strings and exact atomic
values are strings ending in `_base_units`; never use binary floating point.
Create requests accept `expires_in` as integer seconds or `expires_at` as RFC
3339, not both. The default is 24 hours and the accepted range is 10 minutes to
366 days.

## How should API keys be stored and rotated?

Keys are bearer secrets with full account authority. Use the CLI's private,
API-bound profile or a secret manager; never source control, logs, URLs, or
shell arguments. The dashboard never holds a key: it signs in with the emailed
code and uses the short-lived Auth0 token as its session. There is one unnamed key generation per account. Rotation
keeps the prior key valid for 24 hours so integrations can move safely;
revocation invalidates current and grace-period keys immediately. Keys are
shown only at issuance. See [Authentication and API keys](authentication.md).

## What is available in sandbox?

`payday --sandbox` selects an isolated Monad testnet service, test USDC, and
`payday_test_…` credentials. It exercises real indexing/finality rather than a
fake “mark paid” endpoint. See [Sandbox](sandbox.md).

## Where can I get help?

Check the public service status first. For payment or authentication support,
email `support@payday.sh` with the payment ID and API `request_id`. Never send
API keys, OTPs, signer keys, webhook secrets, or unnecessary personal data.
Report software or documentation defects through
[GitHub issues](https://github.com/nkrishang/payday/issues).
