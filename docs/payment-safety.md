# Payment safety for merchants

Payday payment addresses are single-use. Give the payer the amount, payment
address, network, and deadline shown by the checkout, and ask them not to reuse
the address.

The hosted checkout offers three ways to pay the same one-time address: a
connected wallet, a scanned QR, or a copied address. All three request the
amount still due and stop being offered the moment the payment is no longer
payable.

## What the payer must send

- Send **exactly the displayed amount** of **Circle-issued native USDC** on the
  **displayed chain** to the displayed payment address.
- Verify the network and token contract in the wallet before approving the
  transfer. Token names and symbols are not sufficient; bridged USDC (such as
  `USDC.e`), look-alike tokens, and USDC on another network do not count. Check
  the token contract against [Circle's official USDC contract
  addresses](https://developers.circle.com/stablecoins/usdc-contract-addresses).
- Do not pay at the deadline boundary. The chain's block timestamp determines
  whether the payment has expired, and Payday acts only after required block
  finality and a later sweep transaction. Wallet submission time and the
  checkout countdown do not guarantee that settlement will occur before
  expiry; confirmation and sweeping can take additional time.

Wrong assets or payments on the wrong network may be unrecoverable. Do not
promise recovery unless the relevant wallet or token is demonstrably under
your control.

## How amounts are routed

- **Exact payment before expiry:** finalized payments accumulate until the
  payment amount is reached. On pre-expiry execution, exactly the invoice
  amount of native USDC is sent to the payout address.
- **Partial payment:** multiple payments can accumulate. If the total is still
  short when the payment expires, the complete native-USDC balance is routed
  to the Payday recovery wallet.
- **Overpayment before expiry:** the payout address receives exactly the
  invoice amount; the remainder is routed to the Payday recovery wallet.
- **Late payment:** native USDC is routed to the Payday recovery wallet, not
  automatically returned to the payer. This also applies to funds sent after
  the payment address has already been swept.

The on-chain boundary is inclusive: execution at the payment's expiration
timestamp can settle a fully funded payment; execution with a later block
timestamp routes its balance to the Payday recovery wallet. Finality, indexer,
network, or sweep delays can therefore affect the eventual route even when a
payer initiated a transfer earlier.

## Custody and return of recovered funds

The intended invoice amount never passes through Payday: it moves directly from
the one-time address to the payout address. Payday takes custody of *recovered*
amounts only — overpayment remainders, expired balances, and late transfers —
in the Payday recovery wallet, a platform-controlled address the merchant cannot
change. Every recovered amount is recorded against its payment and reported by
a `payment.recovered_funds` webhook. Recovered funds are reviewed manually and
returned by the operator; there is no automated disbursement, and the wallet is
**not an automatic payer refund address**.

## Merchant responsibility

Reconcile payout-address receipts against orders, and treat a
`payment.recovered_funds` event as a prompt to identify the payer so the return
can be arranged. Tell payers who sent excess, late, or expired funds to contact
you and Payday support for return handling. Refunds of amounts that settled to
your payout address remain your own process. Before accepting payments, verify
that your organization controls the payout address.
