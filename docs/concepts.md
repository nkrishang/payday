# Payment concepts

## One payment, one virtual account

Every Payday payment receives a unique EVM address. It is *counterfactual*:
Payday calculates it before deploying the payment contract, so a payer can send
USDC to it immediately.

The address commits to the configured token, amount, payout address, deadline,
refund address, and a unique salt. Deployment cannot change those terms.
Anyone may execute the contract, but the caller cannot redirect its funds.

Treat the address as single-use. Share the `payment_url` or all returned
payment instructions, and stop presenting the address after it is no longer
payable. Never recycle it for another order.

## Public lifecycle

| Status | Meaning |
|---|---|
| `awaiting_payment` | No finalized, on-time USDC has been credited. |
| `partially_paid` | Some finalized USDC is credited, but less than the requested amount. |
| `paid` | Finalized credits reached the amount; settlement is queued or pending finality. |
| `settled` | The complete balance present at on-time execution reached the payout address. |
| `expired` | The deadline passed before successful settlement; recovery is pending. |
| `returned` | The complete balance at post-expiry execution reached the refund address. |
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
| Exact amount reaches the address and execution occurs by the deadline | The complete balance goes to the payout address. |
| Several partial transfers cumulatively reach the amount | They fund one payment; the complete balance goes to payout if execution remains on time. |
| Partial total remains short at expiry | The complete balance goes to the refund address after expiry. |
| More than requested is present at on-time execution | Payout receives the requested amount and all excess; Payday does not automatically refund it. |
| Execution occurs after the deadline | The complete balance goes to the refund address, even if the requested amount arrived earlier. |
| USDC arrives after execution | It is collected to the refund address and does not repeat the payout. |

Execution at the exact expiration timestamp is on time; a later block timestamp
is expired. Leave room for inclusion, finality, and sweeping rather than paying
at the boundary.

The refund address is merchant-controlled exception handling, not necessarily
the payer. Reconcile it and implement your own payer-refund process for partial,
duplicate, excess, or late payments.

## Safety boundaries

- Only the exact `token.address` on the returned `chain.id` is monitored.
  Bridged USDC, look-alike tokens, another network's USDC, and native gas do not
  count and may be unrecoverable.
- A payment link grants read access to the payment page. Share it with the payer.
- `needs_attention` pauses automatic settlement and recovery, including later
  transfers, until an operator safely resolves and releases the payment.
- Amounts have six USDC decimals. Use decimal or integer arithmetic, never
  binary floating point; exact base-unit fields are provided on the API.

See [Payment safety](payment-safety.md) for payer-facing instructions and the
[FAQ](faq.md) for operational decisions.
