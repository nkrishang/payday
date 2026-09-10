# Deposits and deposit requests

## Deposit request versus deposit

Payday has two primitives. A **deposit request** is the ask; a **deposit** is
the funds that answer it.

A **deposit request** is the document a merchant issues: an issuer and a payer party
(name, optional email, optional free-text details), one amount specified
directly, optional notes, heading, reference, and metadata, a payer policy,
and at most one PDF attachment. A **deposit** is the on-chain fulfilment of
that deposit request — the virtual account, the transfers that reach it, and its
settlement. The API, the dashboard, and this documentation use the same two words: deposit
request for the document, deposit for the funds.

Payday models no line items, quantities, subtotals, discounts, tax, or fiat.
The amount is authoritative; an attached PDF is stored, presented, and hashed
but never parsed or reconciled against it. An issued deposit request is an immutable
snapshot: changing the amount, parties, policy, or attachment means cancelling
and reissuing. A `customer` is a reusable counterparty record that can supply
defaults; each deposit request still stores its own `payer` snapshot.

## One deposit request, one virtual account, one payer wallet

Every Payday deposit request receives a unique EVM address once its payer has attested
the wallet they will pay from. The address is *counterfactual*: Payday
calculates it before deploying the deposit contract, so the payer can send USDC
to it as soon as it exists. Until the payer's wallet is bound, the deposit request has
no address at all (`address` is null), because the address commits to that
wallet.

The wallet attestation is an EIP-712 signature the payer makes on the hosted
page, from the wallet they will pay from, over the deposit request's attribution hash
and a one-time nonce issued to their session once the deposit request's policy is
satisfied. The address then commits to the configured token, amount, payout
address, deadline, the payer's wallet as the recovery term, and a salt derived
from the deposit request and that signature: Payday canonicalizes the issued document
(RFC 8785), hashes it, and hashes that with the attestation's EIP-712 digest
to make the salt. Deployment cannot change those terms. Anyone may execute the
contract, but the caller cannot redirect its funds. Settlement depends on the
stored salt alone; the attribution material exists so the document and the
wallet can be proven, never so that funds depend on it.

Two consequences follow. Only transfers from the attested wallet are the
payer's: money from any other wallet still counts toward the amount and
settles, but the deposit request is flagged `likely_unsolicited_at` and no Proof of
Payment claims the payer paid it. And anything Payday returns — an overpayment
remainder, an expired balance, a late transfer — goes back to the payer's own
wallet on-chain, never to a Payday-held account.

Treat the address as single-use. Share the `deposit_url` or all returned
deposit instructions, and stop presenting the address after it is no longer
payable. Never recycle it for another order.

## Payer policy: three modes

Every deposit request names who may pay and what they must prove first:

| Mode | Merchant supplies | Payer proves |
|---|---|---|
| `permissionless` | Nothing | Nothing |
| `verified_email` | Expected email | Mailbox ownership |
| `merchant_session` | Its own user id (`payer_reference`) | Nothing: the merchant's app opens the page with a single-use client secret |

The merchant asserts; Payday confirms. Payday returns whether the check
passed, never the payer's own data. For the gated modes the hosted page
withholds the amount, payer, notes, reference, PDF, address, URI, and QR
until the payer's session satisfies the policy — only the issuer name and
heading show, with a masked hint of the expected mailbox for
`verified_email`.

`merchant_session` is the mode for applications with their own sign-in: a
fund crediting an onboarded investor, an exchange or a prediction market
crediting a logged-in customer. The application creates the request
server-side, receives a client secret, and hands it only to the user it named.
Exchanging that secret is the verification; Payday's added claim is only that
the merchant's server released the secret before the session opened, and the
webhooks carry `payer_reference` back so the application credits the right
ledger. It is created through the API; the dashboard shows these requests but
cannot compose one, because it has no signed-in user to hand the secret to.

The wallet step follows the policy on every deposit request, whichever mode: the
session that satisfied the policy (any session, for `permissionless`) is
issued a one-time nonce and signs the attestation from the wallet it will pay
from, and only then does the address exist. That is what ties the person who
completed the checks — or the user the merchant's app vouched for — to the
wallet that pays: the identity facts are Payday's word, the wallet signature
and the transfers from that wallet are anyone's to recompute.

## Public lifecycle

| Status | Meaning |
|---|---|
| `awaiting_deposit` | No finalized, on-time USDC has been credited. |
| `partially_deposited` | Some finalized USDC is credited, but less than the requested amount. |
| `deposited` | Finalized credits reached the amount; settlement is queued or pending finality. |
| `settled` | Exactly the requested amount reached the payout address at on-time execution; any remainder went back to the payer's wallet. |
| `expired` | The deadline passed before successful settlement; the return to the payer's wallet is pending. |
| `returned` | The complete balance at post-expiry execution went back to the payer's wallet. |
| `needs_attention` | Automatic movement stopped; follow `attention.action` or contact support. |

`deposited` is not yet payout finality. Fulfil an order according to your own risk
policy; `settled` is the strongest Payday state for completed routing.

Cancellation is separate lifecycle metadata. It asks Payday clients to stop
presenting the deposit request, but cannot disable an EVM address or alter its immutable
settlement terms. A transfer sent afterward is still detected and routed.

## Finality and freshness

Payday credits only finalized transfers from the configured Circle-issued
native USDC contract. A wallet may display a submitted, included, or confirmed
transaction before `received` changes. There is intentionally no privileged
“mark deposited” endpoint.

Deposit request reads expose:

- `as_of`: the block and timestamp through which the deposit request projection is
  committed;
- `indexer_freshness`: indexed and finalized block positions plus cursor update
  time;
- `transfers`: finalized provenance, including sender, transaction, amount,
  block, disposition, and whether funds were collected.

Use these fields when diagnosing a deposit that the payer can already see in a
wallet. The authenticated `/v1/status` endpoint reports broader chain, indexer,
and sweep-queue health.

## Exact, partial, excess, and late funds

Finalized transfers accumulate across transactions and blocks while a deposit request
is open. Chain time—not the payer's device clock or submission time—decides
whether a transfer and eventual execution are on time.

| Situation | Routing |
|---|---|
| Exact amount reaches the address and execution occurs by the deadline | Exactly the requested amount goes to the payout address. |
| Several partial transfers cumulatively reach the amount | They fund one deposit request; the requested amount goes to payout if execution remains on time. |
| Partial total remains short at expiry | The complete balance goes back to the payer's wallet after expiry. |
| More than requested is present at on-time execution | Payout receives exactly the requested amount; the remainder goes back to the payer's wallet. |
| Execution occurs after the deadline | The complete balance goes back to the payer's wallet, even if the requested amount arrived earlier. |
| USDC arrives after execution | It is forwarded to the payer's wallet and does not repeat the payout. |

Execution at the exact expiration timestamp is on time; a later block timestamp
is expired. Leave room for inclusion, finality, and sweeping rather than paying
at the boundary.

The recovery term of every address is the payer's attested wallet, so
returns are automatic and on-chain: nothing is held by Payday, and no
operator action is needed. Every returned amount is still recorded against its
deposit in the `recovered_funds` ledger and a `deposit_request.recovered_funds` webhook
reports it. Payday does not hold the intended requested amount either, which
moves directly to the payout address. A payer who sent from a wallet other
than the one they attested will find excess or late funds returned to the
attested wallet, not the sending one.

## Proof of Payment

A settled deposit request can be exported as a Proof of Payment (`payday.proof.v3`):
the canonical issuance snapshot (which lists every network the request
offered, each with its USDC contract and factory), the canonicalization
version, the attribution hash, the payer's wallet attestation (the exact
EIP-712 document the wallet signed, its digest, and the signature), the salt,
the chain the payer chose with its factory and token, the deposit and
recovery addresses, the credited USDC transfers, the fulfilment
transaction that executed the deposit contract, the attachment's hash, and a
Payday-signed attestation of the verification facts. From it anyone —
merchant, payer, or auditor — can recompute the hash, verify the wallet
signature, derive the salt from the hash and the signature's digest, recompute
the CREATE3 deposit address with the wallet as its recovery term, and confirm
that the address received transfers from that wallet covering the deposit request
amount, with no access to Payday's database and no need to trust a later PDF
export. A PDF receipt proves none of that on its own.

The proof establishes deposit request-to-wallet-to-address-to-transfer integrity: the
sentence it supports is that a session which satisfied this deposit request's policy
proved control of wallet W, every credited transfer came from W to an address
that can only belong to this deposit request and this attestation, and exactly the
requested amount reached the merchant. What stays Payday's word is the
identity facts — that the mailbox code was exchanged, and that the wallet's
nonce was issued only after the policy passed — which the attestation lists
as `facts` and signs together with the attribution hash, chain, address,
wallet, and nonce, so it belongs to that deposit request and that payer alone. Funds
credited from any other wallet make the proof unavailable
(`409 deposit_sender_mismatch`): Payday does not issue a proof it cannot
stand behind. The proof is available to the merchant
(`GET /v1/deposit-requests/{id}/proof`) and shared at the merchant's discretion;
`gateway_core::verify_proof` checks it offline.

## Safety boundaries

- Only the exact `token.address` on the chosen `chain.id` is monitored.
  Bridged USDC, look-alike tokens, and native gas do not count and may be
  unrecoverable. The address commits to its chain: on any other supported
  network the contract refuses to settle, and the funds are returned to the
  payer's wallet by hand (`runbooks/wrong-network-deposit.md`).
- A deposit link grants read access to the deposit page. Share it with the
  payer. It never exposes merchant data or policy assertions.
- `needs_attention` pauses automatic settlement and recovery, including later
  transfers, until an operator safely resolves and releases the deposit request.
- Amounts have six USDC decimals. Use decimal or integer arithmetic, never
  binary floating point; exact base-unit fields are provided on the API.

See [Deposit safety](deposit-safety.md) for payer-facing instructions and the
[FAQ](faq.md) for operational decisions.
