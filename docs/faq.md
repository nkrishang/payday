# Customer FAQ

## Which asset and networks can pay?

Each deployment accepts one exact Circle-issued native USDC contract on one EVM
chain. Use the `chain` and `token` returned with the payment. A matching symbol
is not enough: bridged USDC, USDC on another chain, look-alike tokens, and the
chain's gas currency do not count and may be unrecoverable.

## What is the payment address?

It is a unique counterfactual smart-contract address for one payment. USDC can
arrive before the contract exists; deployment later routes its balance under
the amount, payout, expiry, and refund terms committed into that address.
Never reuse it for another order.

## What should I give the payer?

Prefer the returned `payment_url`. It shows network, token, remaining
amount, address, QR/wallet request, deadline, and finalized live status. Send
it to the payer. If integrating your own UI, reproduce all safety
guidance in [Payment safety](payment-safety.md).

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
- On-time execution sends the complete balance to payout, including any excess.
  Payday does not automatically refund overpayment.
- If execution happens after expiry, the complete balance goes to the
  merchant-controlled refund address—even if enough USDC arrived earlier.

Leave time for inclusion, finality, and settlement before the deadline.

## Where do expired or late funds go?

An underpaid address is recovered after expiry. Native USDC sent after expiry
or after the payment contract executes is routed to `refund_address`. That
address is chosen by the merchant and is not automatically the payer. Merchants
must reconcile it and decide whether/how to refund the payer.

## Can I cancel or refund through Payday?

`payday cancel` and `POST /v1/payments/{id}/cancel` are advisory: they stop
Payday clients from presenting the payment but cannot disable an EVM address or
alter its contract. Payday has no payer-refund endpoint. Issue refunds through
your own wallet/process after confirming where the funds settled.

## How do I find and reconcile payments?

Save the `pay_…` ID and your own `reference`. Look up a payment by full ID,
unambiguous ID prefix, or payment address. List newest-first with optional
status/reference filters and cursor pagination. The transfers endpoint provides
finalized sender, transaction, amount, block, disposition, and collection state.

## Should I poll or use webhooks?

Use signed webhooks for lifecycle automation and API long polling
(`wait_for=change`) for an active screen. Webhooks report paid, settled,
expired, refunded, and attention transitions. Verify the HMAC over the exact raw
body, reject stale timestamps, and deduplicate by event ID. See
[Webhooks](webhooks.md).

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
shell arguments. There is one unnamed key generation per account. Rotation
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
