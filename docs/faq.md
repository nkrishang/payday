# Customer FAQ

## What asset and networks can I use?

Payday accepts only Circle-issued native USDC on the one EVM chain configured
for a deployment. The chain ID and token address in a create request must
exactly match that configuration. USDC is identified by chain ID and proxy
address, not by its name or symbol; bridged tokens such as USDC.e and other
tokens are not accepted. Ask the operator of your Payday deployment for its
configured values. See [Local development and configuration](local-development.md)
for the configuration model.

## What is a virtual account or payment address?

It is a unique, counterfactual smart-contract deposit address generated for one
invoice. It is not a bank account or a reusable customer wallet. The contract
does not need to be deployed before USDC is sent to the address. When the
invoice is swept, the contract is deployed there and forwards its balance to
the beneficiary before expiry or to the recovery address after expiry.

Treat every `payment_address` as single-use and show it only while the invoice
status is `created`. Do not reuse an old address for a new order or customer.
Transfers after settlement are not credited to that invoice and are forwarded
to its recovery address.

## Why is a transfer not shown immediately?

Payday credits only finalized on-chain USDC `Transfer` events. The delay
therefore includes chain inclusion, the deployment's finality policy and
indexer polling. The default policy uses the RPC `finalized` block minus two
additional blocks, but operators can configure a different source and margin.
Do not treat a wallet's submitted or confirmed transaction as Payday
settlement; poll the invoice until its status changes. Finality policy is
chain-specific and is described in the
[USDC indexer architecture](usdc-indexer-architecture.md).

## What happens with exact, partial, or excess payments?

- **Exact payment:** finalized credits reaching the requested amount move the
  invoice toward settlement; `fulfilled` means the beneficiary was paid.
- **Partial payments:** finalized, on-time transfers accumulate across
  transactions and blocks. They do not settle until their sum reaches the
  requested amount.
- **Overpayment:** once finalized credits meet or exceed the amount, the full
  USDC balance present when the contract executes goes to the beneficiary if
  execution is before expiry. `received` can therefore exceed `amount`.

## What happens to late or expired payments?

Expiry uses the timestamp of the transfer's block, not the payer's local clock.
A transfer is on time only when that block timestamp is no later than the
invoice deadline. If an open invoice has not been fully funded when chain time
passes the deadline, it becomes `expired`; any balance, including a partial
payment, is swept to the recovery address and the status becomes `recovered`.

Transfers observed after expiry or settlement are marked late, do not increase
the credited invoice amount, and are queued for the recovery address. Recovery
and sweeping are asynchronous, so do not promise an immediate return. A
`blocked` invoice remains paused—including recovery of later transfers—until an
operator reviews and releases it. Never ask a payer to complete an expired or
blocked invoice; create a new one instead.

## What is the recovery address?

It is the nonzero EVM address chosen when the invoice is created to receive
expired balances and transfers sent after settlement. The recovery address and
deadline are committed into the deterministic payment address and cannot be
changed for that invoice. Use an address your organization controls and can
operate on the configured chain; it need not be the payer's address. Recovery
is routing to that address, not a refund service.

## How do I look up invoices?

Save the invoice UUID returned by creation. The only lookup is
`GET /v1/invoices/{id}` or `gateway-cli invoice get <id>`, authenticated with
the owning account's API key. An invoice owned by another account is returned
as not found. There is no search or list endpoint, including lookup by payment
address, transaction hash, customer, or idempotency key.

The API also has no customer webhooks. Store invoice IDs in your own order
records and poll by ID. Payday does not expose a refund endpoint; handle any
refund separately after confirming where funds settled.

## How should I store and rotate API keys?

Treat an API key as a secret bearer credential. The plaintext is displayed
once; Payday stores only its SHA-256 digest and a final-six-character hint.
Keep it in a secret manager or password manager, not in source control, logs,
URLs, command arguments, or shell history. The CLI can read it from the
`GATEWAY_API_KEY` environment variable and requires HTTPS except for local
loopback development.

Each identity has one active key. Running `gateway-cli account create` again
and confirming replacement creates a new generation and invalidates the old
key immediately. Update consumers promptly; there is no overlap period or way
to retrieve the old key. `gateway-cli account get` shows only key metadata.
See [Email authentication and API keys](authentication.md).

## What timestamp and amount formats does the API accept?

Create-request values are strings on the HTTP wire:

- `chain_id` is a non-negative integer string.
- `expiration_timestamp` is an unsigned Unix timestamp in **seconds**, not
  milliseconds. It must be at least 10 minutes and at most 366 days in the
  future when the request is processed.
- `amount` is a positive, unsigned, ordinary decimal USDC string such as `1`
  or `1.50`, with at most six fractional digits. Signs, whitespace, exponent
  notation and separators are rejected.

Responses provide six-decimal `amount` and `received` strings as well as exact
integer `amount_base_units` and `received_base_units` strings. One USDC is
`1000000` base units. Parse monetary values as decimals or integers, never
binary floating point.

## What do invoice statuses mean?

`created` is awaiting enough finalized, on-time credit; `funded` has enough;
`deploying` is being swept; `fulfilled` paid the beneficiary; `expired` missed
the deadline and awaits recovery; `recovered` paid the recovery address; and
`blocked` means automatic sweeping stopped after a permanent or repeatedly
unclassified failure. `execute_tx_hash`, `resolved_at_block`, and
`blocked_reason` may be absent until applicable.

If an invoice is `blocked`, stop presenting its payment address, do not resend
or top up, preserve the invoice ID and transaction hashes, and inspect
`blocked_reason`. Funds have not necessarily been lost or delivered. Resolution
requires operator review; there is no customer retry control. If you operate
the deployment, follow the
[stuck-invoice runbook](runbooks/stuck-invoice.md). Otherwise contact your
deployment operator.

## Where can I get support?

For defects or documentation problems in Payday, search or open a
[GitHub issue](https://github.com/nkrishang/payday/issues). Do not include API
keys, OTPs, signer keys, personal data, or other secrets. For a particular
hosted deployment, its network configuration, or a blocked payment, contact
that deployment's operator; this repository does not publish a separate
customer support channel.
