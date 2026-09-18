# Deposits and deposit requests

## Deposit request versus deposit

Payday has two primitives. A **deposit request** is the ask; a **deposit** is
the funds that answer it.

A **deposit request** is the document a merchant issues: an issuer and a payer party
(name, optional email, optional free-text details), one amount specified
directly in one currency, optional notes, heading, reference, and metadata,
optional verification add-ons, and at most one PDF attachment. A **deposit** is the on-chain fulfilment of
that deposit request — the virtual account, the transfers that reach it, and its
settlement. The API, the dashboard, and this documentation use the same two words: deposit
request for the document, deposit for the funds.

Payday models no line items, quantities, subtotals, discounts, tax, or fiat.
The amount is authoritative; an attached PDF is stored, presented, and hashed
but never parsed or reconciled against it. An issued deposit request is an immutable
snapshot: changing the amount, parties, verification, or attachment means cancelling
and reissuing. A `customer` is a reusable counterparty record that can supply
defaults; each deposit request still stores its own `payer` snapshot.

## Currency

A deposit request is denominated in exactly one `currency`: `USDC` (the
default) or `USDT`. Each network in `networks` lists the currency's canonical
contract there as `token`; `token.symbol` is what a wallet shows, so a USDT
request on Monad or Arbitrum One shows `USDT0`, Tether's omnichain USDT on
those chains (backed 1:1 by USDT locked on Ethereum). Every supported
stablecoin has six decimals.

The rule behind the two models is that Payday never gives a merchant a rate
worse than 1:1. USDC bridges through CCTP at 1:1, so a USDC request may be
paid on any supported network and withdrawn to any. USDT has no such path, so
a USDT request must pin `chain_id` to a network that serves USDT — Monad or
Arbitrum One; Base carries USDC only — and a USDT withdrawal moves that
network's balance alone. A payer may still pay a USDT request from another
network through Relay; Relay swaps, and the payer carries the spread. Relay
is never offered on a request with the wallet-attestation add-on, because a
relay solver pays from a different wallet.

## One deposit request, one virtual account

Every Payday deposit request receives a unique EVM address once its network is
fixed. The address is *counterfactual*: Payday calculates it before deploying
the deposit contract, so the payer can send the request's stablecoin to it as
soon as it exists.

The network is fixed in one of three ways. When the merchant pins `chain_id`
(or the deployment offers a single network), the create response already
carries the address. Otherwise the payer picks a network on the hosted
checkout's network-selection step (`POST
/v1/payer/deposit-requests/{id}/network`), and the address follows. When the
merchant attaches wallet attestation, the payer also signs an attestation, and
the address exists only after that signature.

The salt that fixes the address is derived from the issued document and a
server-generated issuance nonce and, when wallet attestation is attached, from
the attestation's EIP-712 digest as well: Payday canonicalizes the issued
document (RFC 8785), hashes it, and hashes that with the issuance nonce and,
where present, the attestation's digest to make the salt. Deployment cannot
change those terms. Anyone may execute the
contract, but the caller cannot redirect its funds. Settlement depends on the
stored salt alone; the attribution material exists so the document — and, when
attached, the wallet — can be proven, never so that funds depend on it.

**Recovery always goes to Payday's own dedicated KMS recovery wallet.** Every
deposit address commits to that wallet as its recovery term, in every case and
whatever add-ons the request carries — including wallet attestation. An
overpayment remainder, an expired balance, a late transfer, and a
wrong-network or wrong-token recovery all land in Payday's recovery custody on
chain. Payday then returns the funds to the payer **manually, after review**;
it is never an automatic on-chain return to the payer's wallet, and the
payer's wallet is never the recovery term. Every recovered amount is recorded
against its deposit in the `recovered_funds` ledger and a
`deposit_request.recovered_funds` webhook reports it; `recovery_address` on
the deposit request names Payday's recovery custody, not any payer-chosen
wallet.

Without wallet attestation, deposits from **any** wallet are good. A payer may
pay from an exchange withdrawal, a smart-contract wallet, or any other address
they control; nothing is flagged and every finalized transfer of the request's
token counts. With wallet attestation attached, only transfers from the
attested wallet are attributed to the payer: money from any other wallet still
counts toward the amount and settles, but the deposit request is flagged
`likely_unsolicited_at` and no Proof of Payment is issued.

Treat the address as single-use. Share the `deposit_url` or all returned
deposit instructions, and stop presenting the address after it is no longer
payable. Never recycle it for another order.

## Verification add-ons

A deposit request may carry any combination of three independent, optional
add-ons; the default is none of them, which is fully permissionless:

| Add-on | Merchant supplies | Payer proves |
|---|---|---|
| Email verification | Expected email | Mailbox ownership |
| Merchant auth | Its own user id (`payer_reference`) | Nothing: the merchant's app opens the page with a single-use client secret |
| Wallet attestation | Nothing | Control of the wallet they will pay from, by an EIP-712 signature |

```json
{ "verification": { "email": {"expected_email": "alice@example.com"},
                    "merchant_auth": {"payer_reference": "user_123"},
                    "wallet_attestation": true } }
```

The merchant asserts; Payday confirms. Payday returns whether the check
passed, never the payer's own data. When an email or merchant-auth add-on is
attached, the hosted page withholds the amount, payer, notes, reference, PDF,
address, URI, and QR until the payer's session satisfies it — only the issuer
name and heading show, with a masked hint of the expected mailbox for email
verification. This content gating is driven by the identity add-ons (email and
merchant auth) only; wallet attestation gates nothing.

Merchant auth is the add-on for applications with their own sign-in: a
fund crediting an onboarded investor, an exchange or a prediction market
crediting a logged-in customer. The application creates the request
server-side, receives a client secret, and hands it only to the user it named.
Exchanging that secret is the verification; Payday's added claim is only that
the merchant's server released the secret before the session opened, and the
webhooks carry `payer_reference` back so the application credits the right
ledger. It is created through the API; the dashboard shows these requests but
cannot compose one, because it has no signed-in user to hand the secret to.

Wallet attestation, when attached, is the only case in which the payer signs
anything. The session that satisfied the identity checks — or any session,
when neither identity add-on is attached — is issued a one-time nonce and
signs the EIP-712 attestation from the wallet it will pay from, over the
deposit request's attribution hash and that nonce, and only then does the
address exist. The attestation's statement is that the signer controls that
wallet and intends it to pay this deposit request; it makes no claim about
where any returned funds go. Deposits observed from any wallet other than the
attested one are flagged `likely_unsolicited_at` — they still count toward
the amount and settle, but no Proof of Payment is issued. Relay (paying from
another chain) is unavailable for wallet-attested requests, because a relay
solver pays from a different wallet.

## Public lifecycle

| Status | Meaning |
|---|---|
| `awaiting_deposit` | No finalized, on-time transfer of the request's token has been credited. |
| `partially_deposited` | Some finalized funds are credited, but less than the requested amount. |
| `deposited` | Finalized credits reached the amount; settlement is queued or pending finality. |
| `settled` | Exactly the requested amount reached the payout address at on-time execution; any remainder went to Payday's recovery custody. |
| `expired` | The deadline passed before successful settlement; the balance's recovery into custody is pending. |
| `returned` | The complete balance at post-expiry execution was recovered into Payday's recovery custody. |
| `needs_attention` | Automatic movement stopped; follow `attention.action` or contact support. |

`deposited` is not yet payout finality. Fulfil an order according to your own risk
policy; `settled` is the strongest Payday state for completed routing.

Cancellation is separate lifecycle metadata. It asks Payday clients to stop
presenting the deposit request, but cannot disable an EVM address or alter its immutable
settlement terms. A transfer sent afterward is still detected and routed.

## Finality and freshness

Payday credits only finalized transfers from the request's `token.address`:
the canonical contract of its currency on the chosen chain (Circle's native
USDC, or Tether's USDT0). A wallet may display a submitted, included, or confirmed
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
| Partial total remains short at expiry | The complete balance is recovered into Payday's recovery custody after expiry. |
| More than requested is present at on-time execution | Payout receives exactly the requested amount; the remainder goes to Payday's recovery custody. |
| Execution occurs after the deadline | The complete balance goes to Payday's recovery custody, even if the requested amount arrived earlier. |
| Funds arrive after execution | They are recovered into Payday's recovery custody and do not repeat the payout. |

Execution at the exact expiration timestamp is on time; a later block timestamp
is expired. Leave room for inclusion, finality, and sweeping rather than paying
at the boundary.

The recovery term of every address is Payday's dedicated KMS recovery wallet,
so every return lands in Payday's recovery custody on-chain and Payday then
returns the funds to the payer manually, after review. Funds do not move
automatically back to the payer's wallet, and the payer's wallet is never the
recovery term. Every recovered amount is still recorded against its
deposit in the `recovered_funds` ledger and a `deposit_request.recovered_funds` webhook
reports it. Payday does not hold the intended requested amount, which
moves directly to the payout address. A payer who sent from a wallet other
than the one they attested — on a wallet-attested request — is returned their
funds by the same manual process; tell Payday which wallet or address to
return to when you contact support.

## Proof of Payment

A settled deposit request can be exported as a Proof of Payment
(`payday.proof.v5`): the canonical issuance snapshot (`payday.invoice.v5`,
which names the currency and its decimals, lists every network the request
offered, each with the currency's contract and factory, carries the recovery
address, and records the verification add-ons the request was issued with),
the canonicalization version, the attribution hash, the server-generated
issuance nonce the salt commits to, the payer's wallet attestation when the
request carried the wallet-attestation add-on (the exact EIP-712 document the
wallet signed, its digest, and the signature), the salt, the network the payer
chose with its factory and token, the deposit and recovery addresses, the
credited transfers, the fulfilment transaction that executed the deposit
contract, the attachment's hash, and a Payday-signed attestation of the
verification facts.

Every proof carries a `scope` that states what it claims:

- `wallet_attributed` — the request carried wallet attestation. The claim is
  the old one: the attested wallet signed, and every credited transfer came
  from it (or, for a payment made from another network through Relay, from
  Relay's solver with an origin the signed attestation vouches for).
- `settlement` — the request carried no wallet attestation. The proof ties the
  request document to the address, the credited transfers, and the settlement,
  and makes no claim about who paid: the sender may be any wallet the payer
  chose.

From either proof anyone — merchant, payer, or auditor — can recompute the
hash, verify the attestation where one is present, derive the salt from the
attribution hash, the issuance nonce, and (when present) the attestation's
digest, recompute the CREATE3 deposit address with Payday's recovery wallet
as its recovery term, and confirm that the credited transfers cover the
deposit request amount, with no access to Payday's database and no need to
trust a later PDF export. A PDF receipt proves none of that on its own.

For a `wallet_attributed` proof the sentence it supports is that a session
which satisfied this deposit request's identity checks proved control of
wallet W, every credited transfer came from W to an address that can only
belong to this deposit request and this attestation, and exactly the requested
amount reached the merchant. For a `settlement` proof it is that these
transfers settled this document's address for exactly the requested amount —
who paid stays unstated. What stays Payday's word in both cases is the
identity facts — that the mailbox code was exchanged or the merchant's client
secret was spent, and that a wallet's nonce was issued only after the identity
checks passed — which the attestation lists as `facts` and signs together with
the attribution hash, chain, address, wallet, and nonce, so it belongs to that
deposit request and that payer alone. On a wallet-attested request, funds
credited from any other wallet make the proof unavailable
(`409 deposit_sender_mismatch`): Payday does not issue a proof it cannot
stand behind. The proof is available to the merchant
(`GET /v1/deposit-requests/{id}/proof`) and shared at the merchant's discretion;
`gum_core::verify_proof` checks it offline.

## Safety boundaries

- Only the exact `token.address` on the chosen `chain.id` is credited.
  Bridged wrappers, look-alike tokens, and native gas do not count and may be
  unrecoverable. A transfer of another Payday-served stablecoin (USDC to a
  USDT address, say) is observed but never credited; it stays at the address
  and `recover(address)` on the deployed contract forwards it to Payday's
  recovery custody, from which it is returned to the payer after review. The
  address commits to its chain: on any other supported network the contract
  refuses to settle, and the funds are recovered into Payday's custody and
  returned to the payer by hand after review
  (`runbooks/wrong-network-deposit.md`).
- A deposit link grants read access to the deposit page. Share it with the
  payer. It never exposes merchant data or verification assertions.
- `needs_attention` pauses automatic settlement and recovery, including later
  transfers, until an operator safely resolves and releases the deposit request.
- Every supported stablecoin has six decimals. Use decimal or integer
  arithmetic, never binary floating point; exact base-unit fields are
  provided on the API.

See [Deposit safety](deposit-safety.md) for payer-facing instructions and the
[FAQ](faq.md) for operational decisions.
