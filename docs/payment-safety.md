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
  payment amount is reached. On pre-expiry execution, the complete native-USDC
  balance is sent to the payout address.
- **Partial payment:** multiple payments can accumulate. If the total is still
  short when the payment expires, the complete native-USDC balance is routed
  to the merchant-controlled refund address.
- **Overpayment before expiry:** the complete balance, including the excess, is
  sent to the payout address. Payday does not automatically refund the excess.
- **Late payment:** native USDC is routed to the merchant-controlled refund
  address, not automatically returned to the payer. This also applies to funds
  sent after the payment address has already been swept.

The on-chain boundary is inclusive: execution at the payment's expiration
timestamp can settle a fully funded payment; execution with a later block
timestamp routes its balance to the refund address. Finality, indexer, network, or sweep
delays can therefore affect the eventual route even when a payer initiated a
transfer earlier.

## Merchant responsibility

The refund address is a merchant-controlled exception-handling destination,
**not an automatic payer refund address**. Monitor and reconcile payout-address
and refund-address receipts against orders. Establish your own process to identify
payers and issue any appropriate refunds for partial, excess, duplicate, or
late payments. Before accepting payments, verify that your organization can
access both the payout and refund addresses.
