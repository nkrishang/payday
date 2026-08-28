# Payday concepts

## Virtual accounts and counterfactual addresses

Each invoice is a virtual account represented by a unique EVM payment address.
The address is *counterfactual*: Payday can calculate and return it before a
contract exists there. The configured USDC can therefore be transferred to the
address like any other EVM address.

The address commits to the token, requested amount, beneficiary, expiration,
recovery address, and a unique salt. When the payment contract is deployed at
that address, those terms determine where its USDC goes. Execution is not
custodial discretion: anyone may trigger it, but cannot change its destinations.

## Finality and received amounts

Payday credits only transfers that pass the deployment's chain-specific
finality policy. The current implementation waits for a configured confirmation
depth and has no provisional or “payment detected” state. A wallet may show a
transaction before `received` changes.

`received` is the cumulative finalized USDC credited while the invoice is open.
Partial finalized transfers accumulate across transactions and blocks. A
transfer is eligible only if its block timestamp is no later than the invoice's
expiration timestamp. Settlement and recovery transactions are also considered
complete only after finalization.

## Invoice statuses

- `created` — open and waiting for enough finalized USDC. Finalized partial
  payments accumulate in `received`.
- `funded` — finalized credited transfers have reached or exceeded the requested
  amount; settlement is ready.
- `deploying` — the payment contract execution has been submitted and its final
  result is pending.
- `fulfilled` — execution occurred by the expiration timestamp and sent the
  address's complete pre-execution USDC balance to the beneficiary.
- `expired` — chain time has passed the deadline while the invoice remained
  open. Any balance is awaiting recovery.
- `recovered` — execution occurred after expiration and sent the address's
  complete balance to the recovery address.
- `blocked` — automatic settlement or recovery stopped because it cannot safely
  complete. `blocked_reason` may provide a reason; contact support rather than
  reusing the address.

`execute_tx_hash` identifies the deployment transaction when Payday submitted
it. `resolved_at_block` is set when an invoice becomes `fulfilled` or
`recovered`.

## Payment outcomes

The contract uses chain time at execution. The expiration condition is strictly
after the timestamp: execution at the expiration timestamp is still on time.

| Situation | Result |
| --- | --- |
| Exact payment finalized and execution occurs by expiry | The complete balance goes to the beneficiary. |
| Several partial payments finalize while open and cumulatively reach the amount | They accumulate; the invoice funds and the complete balance goes to the beneficiary if execution occurs by expiry. |
| Partial total never reaches the amount before expiry | No underfunded settlement occurs. After expiry, the complete balance goes to recovery. |
| Overpayment is present when execution occurs by expiry | The requested amount **and all excess USDC** go to the beneficiary. |
| Execution occurs after chain-time expiry, whether under-, exactly, or overfunded | The complete balance goes to recovery. |
| Transfer arrives after expiry | It is not credited toward the invoice and is recovered. |
| Transfer arrives after the payment contract has executed | It goes to recovery when collected; it never goes to the beneficiary or changes a fulfilled/recovered outcome. |

There can be a delay between a transfer, its finalization, and execution. Pay
early enough for all three to occur before expiry; a payment that was credited
before the deadline can still go to recovery if execution occurs after it.

## Beneficiary and recovery

The **beneficiary** is the successful-payment destination. It receives every
unit of configured USDC present when an adequately funded invoice executes on
time—not merely the requested amount.

The **recovery address** is the safety destination for expired balances and
transfers made after execution. Choose an address you control and can reconcile;
it is part of the invoice's immutable address derivation.

## One-time-address safety

Treat `payment_address` as single-use and show it only while the invoice is
`created`. Never recycle it for another order, even if the amount and customer
are the same. Each new invoice receives a distinct address.

Before paying, verify the chain, the exact native USDC contract, address, amount,
and deadline from the invoice response. Unsupported tokens are not monitored or
routed by the USDC payment contract. Sending again after settlement does not
repeat the payment to the beneficiary; the later transfer is routed to recovery.
