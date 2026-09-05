# Invoice and payment concepts

## Invoice versus payment

An **invoice** is the document a merchant issues: an issuer and a bill-to party
(name, optional email, optional free-text details), one amount specified
directly, optional notes, heading, reference, and metadata, a payer policy,
and at most one PDF attachment. A **payment** is the on-chain fulfilment of
that invoice — the virtual account, the transfers that reach it, and its
settlement. The API keeps `/v1/payments` as the path; the dashboard, CLI, and
this documentation say invoice for the document and payment for the funds.

Payday models no line items, quantities, subtotals, discounts, tax, or fiat.
The amount is authoritative; an attached PDF is stored, presented, and hashed
but never parsed or reconciled against it. An issued invoice is an immutable
snapshot: changing the amount, parties, policy, or attachment means cancelling
and reissuing. A `customer` is a reusable counterparty record that can supply
defaults; each invoice still stores its own `bill_to` snapshot.

## One invoice, one virtual account

Every Payday invoice receives a unique EVM address. It is *counterfactual*:
Payday calculates it before deploying the payment contract, so a payer can send
USDC to it immediately.

The address commits to the configured token, amount, payout address, deadline,
Payday's recovery wallet, and a salt that is itself derived from the invoice:
Payday canonicalizes the issued document (RFC 8785), hashes it with a random
nonce, and uses the result as the salt. Deployment cannot change those terms.
Anyone may execute the contract, but the caller cannot redirect its funds.
Settlement depends on the stored salt alone; the attribution material exists so
the document can be proven, never so that funds depend on it.

Treat the address as single-use. Share the `payment_url` or all returned
payment instructions, and stop presenting the address after it is no longer
payable. Never recycle it for another order.

## Payer policy: two modes

Every invoice names who may pay and what they must prove first:

| Mode | Merchant supplies | Payer proves |
|---|---|---|
| `permissionless` | Nothing | Nothing |
| `verified_email` | Expected email | Mailbox ownership |

The merchant asserts; Payday confirms. Payday returns whether the check
passed, never the payer's own data. For `verified_email` the hosted page
withholds the amount,
bill-to, notes, reference, PDF, address, URI, and QR until the payer's session
satisfies the policy — only the issuer name and heading show, with a masked
hint of the expected mailbox. Funds that arrive before verification completes
are recorded and flagged as likely unsolicited rather than credited as a
verified payment. Verification proves who completed the checks; it does not
prove who controls the sending wallet.

## Public lifecycle

| Status | Meaning |
|---|---|
| `awaiting_payment` | No finalized, on-time USDC has been credited. |
| `partially_paid` | Some finalized USDC is credited, but less than the requested amount. |
| `paid` | Finalized credits reached the amount; settlement is queued or pending finality. |
| `settled` | Exactly the invoice amount reached the payout address at on-time execution; any remainder went to the Payday recovery wallet. |
| `expired` | The deadline passed before successful settlement; recovery is pending. |
| `returned` | The complete balance at post-expiry execution reached the Payday recovery wallet. |
| `needs_attention` | Automatic movement stopped; follow `attention.action` or contact support. |

`paid` is not yet payout finality. Fulfil an order according to your own risk
policy; `settled` is the strongest Payday state for completed routing.

Cancellation is separate lifecycle metadata. It asks Payday clients to stop
presenting the payment, but cannot disable an EVM address or alter its immutable
settlement terms. A transfer sent afterward is still detected and routed.

## Finality and freshness

Payday credits only finalized transfers from the configured Circle-issued
native USDC contract. A wallet may display a submitted, included, or confirmed
transaction before `received` changes. There is intentionally no privileged
“mark paid” endpoint.

Payment reads expose:

- `as_of`: the block and timestamp through which the payment projection is
  committed;
- `indexer_freshness`: indexed and finalized block positions plus cursor update
  time;
- `transfers`: finalized provenance, including sender, transaction, amount,
  block, disposition, and whether funds were collected.

Use these fields when diagnosing a payment that the payer can already see in a
wallet. The authenticated `/v1/status` endpoint reports broader chain, indexer,
and sweep-queue health.

## Exact, partial, excess, and late funds

Finalized transfers accumulate across transactions and blocks while a payment
is open. Chain time—not the payer's device clock or submission time—decides
whether a transfer and eventual execution are on time.

| Situation | Routing |
|---|---|
| Exact amount reaches the address and execution occurs by the deadline | Exactly the invoice amount goes to the payout address. |
| Several partial transfers cumulatively reach the amount | They fund one payment; the invoice amount goes to payout if execution remains on time. |
| Partial total remains short at expiry | The complete balance goes to the Payday recovery wallet after expiry. |
| More than requested is present at on-time execution | Payout receives exactly the invoice amount; the remainder goes to the Payday recovery wallet. |
| Execution occurs after the deadline | The complete balance goes to the Payday recovery wallet, even if the requested amount arrived earlier. |
| USDC arrives after execution | It is collected to the Payday recovery wallet and does not repeat the payout. |

Execution at the exact expiration timestamp is on time; a later block timestamp
is expired. Leave room for inclusion, finality, and sweeping rather than paying
at the boundary.

The Payday recovery wallet is Payday's custodial exception handling, not the
payer and not a merchant wallet. Every recovered amount is recorded against its
payment, reviewed manually, and returned by the operator; a
`payment.recovered_funds` webhook reports each one. Payday does not hold the
intended invoice amount, which moves directly to the payout address. Payers who
sent excess, late, or expired funds should contact the merchant and Payday
support for return handling.

## Proof of Payment

A settled invoice can be exported as a Proof of Payment: the canonical
issuance snapshot, the canonicalization version, the random nonce, the
attribution hash, the salt, the chain, factory, token, and payment addresses,
the credited USDC transfers, the fulfilment transaction that executed the
payment contract, the attachment's hash, and a Payday-signed attestation of
the verification mode and result. From it anyone — merchant, payer, or
auditor — can recompute the hash, derive the salt, recompute the CREATE3
payment address, and confirm that the address received transfers covering the
invoice amount, with no access to Payday's database and no need to trust a
later PDF export. A PDF receipt proves none of that on its own.

The proof establishes invoice-to-address-to-transfer integrity. Verification
outcomes happen after issuance, so they are Payday-attested rather than
address-committed; the attestation names the invoice's attribution hash and
payment address, so it belongs to that invoice alone. The proof does not claim
that the verified person owned the sending wallet. It is available to the merchant
(`payday proof download`, `GET /v1/payments/{id}/proof`) and shared at the
merchant's discretion; `payday proof verify` checks it offline.

## Safety boundaries

- Only the exact `token.address` on the returned `chain.id` is monitored.
  Bridged USDC, look-alike tokens, another network's USDC, and native gas do not
  count and may be unrecoverable.
- A payment link grants read access to the payment page. Share it with the
  payer. It never exposes merchant data or policy assertions.
- `needs_attention` pauses automatic settlement and recovery, including later
  transfers, until an operator safely resolves and releases the payment.
- Amounts have six USDC decimals. Use decimal or integer arithmetic, never
  binary floating point; exact base-unit fields are provided on the API.

See [Payment safety](payment-safety.md) for payer-facing instructions and the
[FAQ](faq.md) for operational decisions.
