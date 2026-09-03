# End-to-end smoke test

Verify the full payment lifecycle: create a payment, fund it with real USDC,
and confirm the funds reach the beneficiary.

## Prerequisites

- API key and CLI configured (see [README.md](README.md) prerequisites)
- A wallet with USDC on Monad and its private key
- The CLI binary built: `cargo build --release -p gateway-cli --bin payday`
- The indexer running and caught up (see [daily-monitoring.md](daily-monitoring.md))

## Step 1: Create a payment

```bash
export PAYDAY_API_URL="https://api.payday.sh"
export PAYDAY_API_KEY="<operator-account-api-key>"

./target/release/payday --json create \
  --chain-id 143 \
  --token 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  --payout <PAYOUT_ADDRESS> \
  --expires-in 3600 \
  --amount 0.01
```

Copy `id` and `address` from the JSON output, and confirm `recovery_address`
is the Payday recovery wallet configured as `recovery_address` in Terraform.

## Step 2: Send USDC to the payment address

```bash
cast send 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'transfer(address,uint256)' <PAYMENT_ADDRESS> 10000 \
  --private-key <YOUR_PRIVATE_KEY> \
  --rpc-url "$MONAD_RPC_URL"
```

`10000` is 0.01 USDC in base units (6 decimals).

## Step 3: Monitor the payment

```bash
# One-time check:
./target/release/payday get <PAYMENT_ID>

# Poll every 5 seconds:
watch -n 5 './target/release/payday get <PAYMENT_ID>'
```

The status should progress: `awaiting_payment → paid → settled`.

With the indexer caught up, this typically takes under 30 seconds.

## Step 4: Verify on-chain state

```bash
# Payment address should be emptied (balance = 0):
cast call 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'balanceOf(address)(uint256)' <PAYMENT_ADDRESS> \
  --rpc-url "$MONAD_RPC_URL"

# Payment contract should be deployed (non-empty code):
cast code <PAYMENT_ADDRESS> --rpc-url "$MONAD_RPC_URL"

# Payout address should have received the USDC:
cast call 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'balanceOf(address)(uint256)' <PAYOUT_ADDRESS> \
  --rpc-url "$MONAD_RPC_URL"
```

## Step 5: Verify late funds reach the Payday recovery wallet

Send a second, small transfer to the same payment address. It must reach the
Payday recovery wallet within a minute while the status stays `settled`, and
the `recovered_funds` ledger must record it:

```bash
cast send 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'transfer(address,uint256)' <PAYMENT_ADDRESS> 1000 \
  --private-key <YOUR_PRIVATE_KEY> \
  --rpc-url "$MONAD_RPC_URL"

cast call 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'balanceOf(address)(uint256)' <RECOVERY_ADDRESS> \
  --rpc-url "$MONAD_RPC_URL"
```

```sql
-- See db-access.md
SELECT reason, amount, encode(transaction_hash, 'hex') AS tx, recovered_at
FROM recovered_funds
WHERE invoice_id = '<PAYMENT_UUID>';
```

Expect one row with reason `late_transfer` and amount `1000`, and a
`payment.recovered_funds` delivery on any registered webhook. Return the
recovered amount by hand afterwards; nothing automates it.

## Expected results

| Check | Expected |
|-------|----------|
| Payment status | `settled` |
| Payment address USDC balance | `0` |
| Payment address code | Non-empty (contract deployed) |
| Payout address USDC balance | Increased by exactly the payment amount |
| Recovery wallet USDC balance | Increased by the late transfer, with a matching `recovered_funds` row |

## Understanding the sweep transaction

The sweep creates two contracts on-chain:

1. A tiny **proxy** (15 bytes) deployed via `CREATE2` by the factory
2. The actual **Payment** contract deployed via `CREATE` by the proxy

This is the CREATE3 pattern — it makes the payment address depend only on the
salt and factory address, not the contract bytecode. The proxy is ephemeral
and has no further use after deployment.

## If the invoice is stuck

See [stuck-invoice.md](stuck-invoice.md).
