# Payment safety for merchants

Payday payment addresses are single-use and belong to one payer wallet. The
address exists only once the payer has signed the request's attestation from
the wallet they will pay from; it commits to that wallet, and only transfers
from that wallet are the payer's. Give the payer the amount, payment address,
network, and deadline shown by the checkout, and ask them to pay from the
wallet they signed with and not to reuse the address.

The hosted checkout offers three ways to pay the same one-time address: the
connected wallet (which refuses to send from any wallet but the attested
one), a scanned QR, or a copied address. All three request the amount still
due and stop being offered the moment the payment is no longer payable.

## What the payer must send

- Send **exactly the displayed amount** of **Circle-issued native USDC** on the
  **displayed chain** to the displayed payment address, **from the wallet that
  signed the attestation**. Money from any other wallet still counts toward the
  amount and settles, but the payment is flagged `likely_unsolicited`, no Proof
  of Payment is issued for it, and anything returned goes to the attested
  wallet, not the sending one.
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
  short when the payment expires, the complete native-USDC balance is returned
  to the payer's attested wallet.
- **Overpayment before expiry:** the payout address receives exactly the
  invoice amount; the remainder is returned to the payer's attested wallet.
- **Late payment:** native USDC is returned to the payer's attested wallet.
  This also applies to funds sent after the payment address has already been
  swept.

The on-chain boundary is inclusive: execution at the payment's expiration
timestamp can settle a fully funded payment; execution with a later block
timestamp returns its balance to the payer's wallet. Finality, indexer,
network, or sweep delays can therefore affect the eventual route even when a
payer initiated a transfer earlier.

## No custody

The intended invoice amount never passes through Payday: it moves directly from
the one-time address to the payout address. Neither do returns: the payer's
attested wallet is the address's recovery term, so overpayment remainders,
expired balances, and late transfers go back to the payer on-chain, without
anyone holding them. Every returned amount is recorded against its payment and
reported by a `payment.recovered_funds` webhook. The one case a payer should
know about: funds sent from a wallet other than the attested one are returned
to the attested wallet, never to the sender.

## Merchant responsibility

Reconcile payout-address receipts against orders, and treat a
`payment.likely_unsolicited` event as a prompt to check who actually paid: the
attested wallet did not. Refunds of amounts that settled to your payout address
remain your own process. Before accepting payments, verify that your
organization controls the payout address.
