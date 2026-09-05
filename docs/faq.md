# Customer FAQ

## What is the difference between a deposit request and a deposit?

The deposit request is the document you issue: issuer and payer parties, one amount,
optional notes, heading, reference, and metadata, a payer policy, and at most
one PDF. The deposit request is its on-chain fulfilment at a one-time address. The API
resource is the deposit request, and the deposit request's state is read from it.
Issued deposit requests are immutable — to change one, cancel it and issue another.

## Can I add line items or tax?

No. Specify the amount directly and attach a PDF if you need an itemized
breakdown or jurisdiction-specific content; Payday stores and hashes the file
but never parses it. Payday does not model line items, tax rates, tax IDs, or
fiat, and does not intend to. Bounded free-text `details` on each party and
the PDF are where that information goes.

## What are the payer modes?

`permissionless` lets anyone with the link pay, as before. `verified_email`
requires the payer to prove ownership of the mailbox you name. For
`verified_email` the checkout hides the amount, parties, PDF, and address
until verification completes; funds sent before then are flagged as likely
unsolicited. Payday reports pass or fail, not the payer's own data.

## How do PDF attachments work?

One PDF per deposit request, at most 5 MiB. The SDK's `attachments.upload` and the
dashboard's upload control reserve a presigned slot,
send the bytes straight to storage, and wait for the malware scan to admit the
file; only then can its `attachment_id` be attached to a deposit request. The file's
SHA-256 is committed into the deposit address and appears in the Proof of
Payment. Reads return a signed URL valid for a few minutes.

## What is Proof of Payment and how do I verify it?

Once a deposit request settles, `GET /v1/deposit-requests/{id}/proof` exports the canonical
deposit request, the nonce and salt, the deposit address, the settling transfer, and a
Payday-signed verification attestation. Anyone holding it can recompute the
hash, salt, and address and check the transfer offline, with no Payday access;
the checks are published as `gateway_core::verify_proof`, and the attached
PDF's hash can be compared with the one the proof commits to. Share the proof
with payers or auditors at your discretion; it is not a public link.

## Which asset and networks can pay?

Each deployment accepts one exact Circle-issued native USDC contract on one EVM
chain. Use the `chain` and `token` returned with the deposit request. A matching symbol
is not enough: bridged USDC, USDC on another chain, look-alike tokens, and the
chain's gas currency do not count and may be unrecoverable.

## What is the deposit address?

It is a unique counterfactual smart-contract address for one deposit request and one
payer wallet. It exists once the payer has signed the request's attestation
from the wallet they will pay from; USDC can then arrive before the contract
exists, and deployment later routes its balance under the amount, payout,
expiry, and recovery terms committed into that address, the recovery term
being the payer's own wallet. Never reuse it for another order.

## What should I give the payer?

Prefer the returned `deposit_url`. It shows the deposit request, network, token,
remaining amount, address, QR/wallet request, deadline, and finalized live
status — after verification, for a gated deposit request. Send it to the payer. If
integrating your own UI, reproduce all safety guidance in
[Deposit safety](deposit-safety.md).

## Why has a wallet transaction not appeared?

Payday credits only finalized native-USDC transfers. Inclusion or wallet
confirmation can precede Payday's finality boundary and indexing cursor. Check
`as_of`, `indexer_freshness`, transfer provenance, and `/v1/status`; poll
`GET /v1/deposit-requests/{id}` or register a webhook rather than treating submission
as a deposit request.

## What happens with exact, partial, or excess deposits?

- Exact and cumulative partial transfers become `deposited` once finalized credits
  reach the requested amount.
- `partially_deposited` remains open while the finalized total is short.
- On-time execution sends exactly the requested amount to payout. Any excess goes
  back to the payer's attested wallet in the same transaction; it is never
  forwarded to the merchant.
- If execution happens after expiry, the complete balance goes back to the
  payer's attested wallet—even if enough USDC arrived earlier.

Leave time for inclusion, finality, and settlement before the deadline.

## Where do expired or late funds go?

An underpaid address is returned after expiry. Native USDC sent after expiry
or after the deposit contract executes is forwarded to the payer's attested
wallet (`recovery_address` on the deposit request, always equal to `payer_wallet`).
Payday holds nothing: every return is on-chain and recorded against the
deposit. A payer who sent from a wallet other than the one they signed with
will find the return in the attested wallet.

## Can I cancel or refund through Payday?

`POST /v1/deposit-requests/{id}/cancel` is advisory: it stops
Payday clients from presenting the deposit request but cannot disable an EVM address or
alter its contract. There is no refund endpoint. Funds that settled to your
payout address are yours to refund through your own wallet/process; recovered
funds (overpayments, late or expired transfers) are returned by Payday after
manual review, and `deposit_request.recovered_funds` webhooks tell you when Payday
holds something for one of your deposit requests.

## How do I find and reconcile deposits?

Save the `dr_…` ID and your own `reference`. Look up a deposit request by complete
ID or deposit address. List newest-first with optional status/reference
filters and cursor pagination; summaries carry the heading, payer name,
policy mode, customer, and verification state. The transfers endpoint provides
finalized sender, transaction, amount, block, disposition, and collection
state, and the Proof of Payment is the durable reconciliation record.

## Should I poll or use webhooks?

Use signed webhooks for lifecycle automation and API long polling
(`wait_for=change`) for an active screen. Webhooks report deposited, settled,
expired, returned, and attention transitions, plus every amount recovered by
Payday. Verify the HMAC over the exact raw body, reject stale timestamps, and
deduplicate by event ID. See [Webhooks](webhooks.md).

## What does `needs_attention` mean?

Automatic movement stopped because Payday cannot safely continue. Do not send
more funds. Follow the response's `attention.action` and contact support with
the deposit request ID and the API request ID. Settlement and later-fund recovery stay paused
until an operator resolves and releases the condition; funds are not necessarily
lost or delivered.

## How are amounts and deadlines encoded?

USDC uses six decimals. API human amounts are decimal strings and exact atomic
values are strings ending in `_base_units`; never use binary floating point.
Create requests accept `expires_in` as integer seconds or `expires_at` as RFC
3339, not both. The default is 24 hours and the accepted range is 10 minutes to
366 days.

## How should API keys be stored and rotated?

Keys are bearer secrets with full account authority. Keep them in a secret
manager; never source control, logs, URLs, or shell arguments. The dashboard never holds a key: it signs in with the emailed
code and uses the short-lived Auth0 token as its session. There is one unnamed key generation per account. Rotation
keeps the prior key valid for 24 hours so integrations can move safely;
revocation invalidates current and grace-period keys immediately. Keys are
shown only at issuance. See [Authentication and API keys](authentication.md).

## What is available in sandbox?

`https://api.sandbox.payday.sh` is an isolated Monad testnet service with test
USDC and `payday_test_…` credentials. It exercises real indexing/finality rather than a
fake “mark deposited” endpoint. See [Sandbox](sandbox.md).

## Where can I get help?

Check the public service status first. For deposit or authentication support,
email `support@payday.sh` with the deposit request ID and API `request_id`. Never send
API keys, OTPs, signer keys, webhook secrets, or unnecessary personal data.
Report software or documentation defects through
[GitHub issues](https://github.com/nkrishang/payday/issues).
