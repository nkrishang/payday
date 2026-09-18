# Wrong-network deposit

A payer chose one network at the checkout (say Monad) and then sent the
request's stablecoin to the deposit address on another supported network
(say Base). Nothing
happens on its own: the request stays `awaiting_deposit`, the indexer on
the other chain does not watch that address (it is bound to Monad), and the
funds sit at an address with no code on Base. This runbook recovers them
into Payday's recovery custody; returning them to the payer is then the
manual review process described in the production runbook ("Recovery custody
and manual returns"). It is manual by design; see "Why this is safe".

## What makes it recoverable

- The contract generation is deployed at the **same addresses on every
  chain** (production runbook §2), so Base's `PaymentFactory` reproduces
  the very address Monad's did for the same terms.
- The `Payment` constructor checks `block.chainid` against the chain it was
  committed to. On the wrong chain it stores nothing, touches no token,
  emits `WrongChain(expected, actual)`, and returns. It never pays the
  receiver and never settles.
- `recover(address token)` is permissionless: anyone can forward the whole
  balance of any token at the address to the committed recovery wallet,
  which is Payday's dedicated KMS recovery custody. The receiver cannot be paid from
  the wrong chain and the funds cannot go anywhere but into that custody;
  the return to the payer happens after review, by hand.

The two-Anvil end-to-end suite (`just e2e`) performs exactly the steps
below against chain 31338 for an address bound on 31337, so they are
exercised on every change.

## Prerequisites

- The deposit request id, from the payer or the merchant.
- An API key on the merchant's account, `curl`, `jq`, and `cast`.
- An RPC URL for the chain the funds landed on, and an operator key with a
  little gas there. Both calls are permissionless; do not export or use the
  signers service's KMS keys for this manual operation.

## Step 1: Confirm where the funds are

Read the request. `chain.id` is the network the payer chose; `address` is
the deposit address; `self_settlement` carries the terms the factory needs.

```bash
export PAYDAY_API_URL="https://api.payday.sh"
export PAYDAY_API_KEY="<merchant account api key>"
req=$(curl -fsS "$PAYDAY_API_URL/v1/deposit-requests/<dr_id>" \
  -H "Authorization: Bearer $PAYDAY_API_KEY")
echo "$req" | jq '{status, chain, token, address, payer_wallet, recovery_address, payout_address, amount_base_units, expires_at, self_settlement}'
```

Then check the address's balance on the chain the payer says they used,
with that chain's contract for the request's `currency` (the matching
`tokens` entry of its `PAYDAY_CHAINS` entry; the table in the production
runbook lists them). USDT0 is served on Monad and Arbitrum only, so a USDT
request's funds on Base sit in whatever contract the payer's wallet used
there; `recover` takes that contract's address either way:

```bash
export WRONG_RPC_URL='https://your-rpc-for-that-chain'
export WRONG_TOKEN=0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913   # Base's native USDC, for example
ADDRESS=$(echo "$req" | jq -r .address)
cast call "$WRONG_TOKEN" 'balanceOf(address)(uint256)' "$ADDRESS" --rpc-url "$WRONG_RPC_URL"
cast code "$ADDRESS" --rpc-url "$WRONG_RPC_URL"     # 0x: nothing deployed there yet
```

A nonzero balance on a chain other than `chain.id` is the case this
runbook covers. If the balance is on `chain.id` itself, the request is
simply waiting for finality; see
[stuck-deposit-request.md](stuck-deposit-request.md).

## Step 2: Deploy the Payment on the wrong chain

Call `execute` on that chain's factory with the request's own terms, which
are what `paymentAddress` was derived from. The chain id argument is the
chain the request is bound to (`chain.id`), not the chain you are sending
on; that mismatch is what makes the constructor take the wrong-chain
branch.

```bash
FACTORY=$(echo "$req" | jq -r .self_settlement.factory)
SALT=$(echo "$req" | jq -r .self_settlement.salt)
BOUND_CHAIN=$(echo "$req" | jq -r .chain.id)
BOUND_TOKEN=$(echo "$req" | jq -r .token.address)
AMOUNT=$(echo "$req" | jq -r .amount_base_units)
RECEIVER=$(echo "$req" | jq -r .payout_address)
RECOVERY=$(echo "$req" | jq -r .recovery_address)
EXPIRATION=$(echo "$req" | jq -r '.expires_at | fromdateiso8601')

cast send "$FACTORY" \
  'execute(address,uint256,address,uint64,address,bytes32,uint256)' \
  "$BOUND_TOKEN" "$AMOUNT" "$RECEIVER" "$EXPIRATION" "$RECOVERY" "$SALT" "$BOUND_CHAIN" \
  --private-key <FUNDED_KEY_ON_THAT_CHAIN> --rpc-url "$WRONG_RPC_URL"
```

Verify that the deployment landed at the deposit address, routed nothing,
and did not settle:

```bash
cast call "$FACTORY" \
  'paymentAddress(address,uint256,address,uint64,address,bytes32,uint256)(address)' \
  "$BOUND_TOKEN" "$AMOUNT" "$RECEIVER" "$EXPIRATION" "$RECOVERY" "$SALT" "$BOUND_CHAIN" \
  --rpc-url "$WRONG_RPC_URL"                                           # must equal $ADDRESS
cast call "$ADDRESS" 'settled()(bool)' --rpc-url "$WRONG_RPC_URL"     # false
cast call "$WRONG_TOKEN" 'balanceOf(address)(uint256)' "$ADDRESS" --rpc-url "$WRONG_RPC_URL"   # unchanged
```

The receipt carries a `WrongChain(expected, actual)` event. If
`paymentAddress` does not equal the deposit address, stop: the factory on
that chain is not this generation, and nothing below can move the funds.

## Step 3: Recover the balance into custody

`recover` takes the token to move: the contract the funds sit in on the
wrong chain, not the request's `token.address`.

```bash
cast send "$ADDRESS" 'recover(address)' "$WRONG_TOKEN" \
  --private-key <FUNDED_KEY_ON_THAT_CHAIN> --rpc-url "$WRONG_RPC_URL"

cast call "$WRONG_TOKEN" 'balanceOf(address)(uint256)' "$ADDRESS" --rpc-url "$WRONG_RPC_URL"   # 0
cast call "$WRONG_TOKEN" 'balanceOf(address)(uint256)' "$RECOVERY" --rpc-url "$WRONG_RPC_URL"  # increased
```

The receipt carries `Recovered(recovery, token, amount)` with `recovery`
equal to Payday's recovery custody address (`recovery_address` on the
request). Tell the payer that the funds are safe in custody and that Payday
returns them to a wallet or address they name, after review, and open or
update the support case that records that review — the manual return itself
is the production runbook's "Recovery custody and manual returns" procedure.
The request itself is
untouched: it stays `awaiting_deposit` until paid or expired, and nothing
about this appears in `transfers` or `recovered_funds`, because the
indexer on the bound chain never saw a transfer.

## Wrong asset on the right network

The same `recover` call handles a payer who sent another configured
stablecoin to the address on the chain the request is bound to (USDC to a
USDT request's address, say). The indexer sees transfers of every
configured contract to a watched address but credits only the request's
own `token.address`, so the stray balance stays at the address and the
request does not advance. Once the sweep has deployed the `Payment` on
`chain.id` (at settlement or expiry; do not deploy it by hand there, the
constructor would route the committed token), call `recover(address)` with
the stray contract on that chain: it is permissionless and forwards that
token's whole balance to Payday's recovery custody, from which the manual
review process returns it to the payer. Nothing about it appears in
`transfers` or `recovered_funds`.

## Why this is safe

- Nothing in these steps can pay the receiver: the constructor returns
  before any token call whenever `block.chainid` is not the committed
  chain, and `recover` forwards only to the committed recovery custody.
- Both calls are permissionless, so the key you use has no authority
  beyond paying gas.
- A later transfer to the same address on the wrong chain (the payer tries
  again on the wrong network) is recovered by calling `recover` again; the
  `Payment` is already deployed, so Step 2 is not repeated.

## Why it is not automated

Watching every deposit address on every chain would multiply the
indexer's RPC spend by the number of supported networks for an event that
should be rare (the checkout switches the wallet to the chosen network
before signing, and the wallet signs a domain naming that chain). Payday's
decision (2026-09-10) is to handle these case by case. If they become
common, the two calls above are the whole automation.
