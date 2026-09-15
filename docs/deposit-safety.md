# Deposit safety for merchants

Payday deposit addresses are single-use and belong to one payer wallet. The
address exists only once the payer has signed the request's attestation from
the wallet they will pay from; it commits to that wallet, and only transfers
from that wallet are the payer's. Give the payer the amount, deposit address,
network, and deadline shown by the checkout, and ask them to pay from the
wallet they signed with and not to reuse the address.

The hosted checkout offers three ways to pay the same one-time address: the
connected wallet (which refuses to send from any wallet but the attested
one), a scanned QR, or a copied address. All three request the amount still
due and stop being offered the moment the deposit request is no longer payable.

## What the payer must send

- Send **exactly the displayed amount** of the **request's stablecoin** (the
  displayed `token`: USDC, or USDT — shown as `USDT0` on Monad and Arbitrum
  One) on the **network they chose** to the displayed deposit address, **from
  the wallet that signed the attestation**. The choice is made before signing and is
  final: the address exists on that chain only. Money from any other wallet still counts toward the
  amount and settles, but the deposit request is flagged `likely_unsolicited`, no Proof
  of Deposit is issued for it, and anything returned goes to the attested
  wallet, not the sending one. The one way to pay from another network is
  the hosted checkout's "Pay from another network": Payday quotes the route
  through Relay for the attested wallet, and the delivery Relay's solver
  makes is attributed to that wallet once Relay confirms it sent the deposit.
- Verify the network and token contract in the wallet before approving the
  transfer. Token names and symbols are not sufficient; bridged wrappers (such
  as `USDC.e`), look-alike tokens, the other stablecoin, and the same token on
  another network do not count. Payday credits only these contracts:

  | Network | USDC | USDT |
  |---|---|---|
  | Monad (143) | `0x754704Bc059F8C67012fEd69BC8A327a5aafb603` | `0xe7cd86e13AC4309349F30B3435a9d337750fC82D` (USDT0) |
  | Base (8453) | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | not served |
  | Arbitrum One (42161) | `0xaf88d065e77c8cC2239327C5EDb3A432268e5831` | `0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9` (USDT0) |

  Reconfirm them against [Circle's official USDC contract
  addresses](https://developers.circle.com/stablecoins/usdc-contract-addresses)
  and [Tether's USDT0 deployments](https://docs.usdt0.to/technical-documentation/deployments).
  Base's USDT is a bridge wrapper without EIP-3009, so it is not a deposit
  currency; a payer holding it pays a request through Relay instead.
- Do not pay at the deadline boundary. The chain's block timestamp determines
  whether the deposit request has expired, and Payday acts only after required block
  finality and a later sweep transaction. Wallet submission time and the
  checkout countdown do not guarantee that settlement will occur before
  expiry; confirmation and sweeping can take additional time.

Wrong assets may be unrecoverable. Two cases are recoverable. A transfer of
the *other* Payday-served stablecoin to the address (USDC to a USDT address,
or the reverse) is observed but never credited; it stays at the address, and
`recover(address)` on the deployed deposit contract — callable by anyone —
returns it to the payer's attested wallet. A deposit of the right token to
the address on another *supported* network is refused by the contract rather
than settled, and Payday's operator can return it to the payer's attested
wallet by hand (`runbooks/wrong-network-deposit.md`); treat that as a support
case, not a feature. Do not promise recovery of anything else unless the
relevant wallet or token is demonstrably under your control.

## How amounts are routed

- **Exact deposit before expiry:** finalized transfers accumulate until the
  requested amount is reached. On pre-expiry execution, exactly the deposit request
  amount of its token is sent to the payout address.
- **Partial deposit:** multiple transfers can accumulate. If the total is still
  short when the deposit request expires, the complete balance is returned
  to the payer's attested wallet.
- **Overpayment before expiry:** the payout address receives exactly the
  requested amount; the remainder is returned to the payer's attested wallet.
- **Late deposit:** the funds are returned to the payer's attested wallet.
  This also applies to funds sent after the deposit address has already been
  swept.

The on-chain boundary is inclusive: execution at the deposit request's expiration
timestamp can settle a fully funded deposit request; execution with a later block
timestamp returns its balance to the payer's wallet. Finality, indexer,
network, or sweep delays can therefore affect the eventual route even when a
payer initiated a transfer earlier.

## No custody

The intended requested amount never passes through Payday: it moves directly from
the one-time address to the payout address. Neither do returns: the payer's
attested wallet is the address's recovery term, so overpayment remainders,
expired balances, and late transfers go back to the payer on-chain, without
anyone holding them. Every returned amount is recorded against its deposit and
reported by a `deposit_request.recovered_funds` webhook. The one case a payer should
know about: funds sent from a wallet other than the attested one are returned
to the attested wallet, never to the sender.

## Merchant responsibility

Reconcile payout-address receipts against orders, and treat a
`deposit_request.likely_unsolicited` event as a prompt to check who actually paid: the
attested wallet did not. Refunds of amounts that settled to your payout address
remain your own process. Before accepting deposits, verify that your
organization controls the payout address.
