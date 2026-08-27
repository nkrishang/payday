# End-to-end smoke test

Verify the full payment lifecycle: create an invoice, pay it with real USDC,
and confirm the funds reach the beneficiary.

## Prerequisites

- API key and CLI configured (see [README.md](README.md) prerequisites)
- A wallet with USDC on Monad and its private key
- The CLI binary built: `cargo build --release -p gateway-cli`
- The indexer running and caught up (see [daily-monitoring.md](daily-monitoring.md))

## Step 1: Create an invoice

```bash
export GATEWAY_API_URL="https://api.payday.sh"
export GATEWAY_API_KEY="<operator-account-api-key>"

./target/release/gateway-cli --json invoice create \
  --chain-id 143 \
  --token 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  --beneficiary <BENEFICIARY_ADDRESS> \
  --expiration-timestamp "$(($(date +%s) + 3600))" \
  --recovery <RECOVERY_ADDRESS> \
  --amount 0.01
```

Copy `id` and `payment_address` from the JSON output.

## Step 2: Send USDC to the payment address

```bash
cast send 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'transfer(address,uint256)' <PAYMENT_ADDRESS> 10000 \
  --private-key <YOUR_PRIVATE_KEY> \
  --rpc-url "$MONAD_RPC_URL"
```

`10000` is 0.01 USDC in base units (6 decimals).

## Step 3: Monitor the invoice

```bash
# One-time check:
./target/release/gateway-cli invoice get <INVOICE_ID>

# Poll every 5 seconds:
watch -n 5 './target/release/gateway-cli invoice get <INVOICE_ID>'
```

The status should progress: `created → funded → deploying → fulfilled`.

With the indexer caught up, this typically takes under 30 seconds.

## Step 4: Verify on-chain state

```bash
# Payment address should be emptied (balance = 0):
cast call 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'balanceOf(address)(uint256)' <PAYMENT_ADDRESS> \
  --rpc-url "$MONAD_RPC_URL"

# Payment contract should be deployed (non-empty code):
cast code <PAYMENT_ADDRESS> --rpc-url "$MONAD_RPC_URL"

# Beneficiary should have received the USDC:
cast call 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'balanceOf(address)(uint256)' <BENEFICIARY_ADDRESS> \
  --rpc-url "$MONAD_RPC_URL"
```

## Expected results

| Check | Expected |
|-------|----------|
| Invoice status | `fulfilled` |
| Payment address USDC balance | `0` |
| Payment address code | Non-empty (contract deployed) |
| Beneficiary USDC balance | Increased by the invoice amount |

## Understanding the sweep transaction

The sweep creates two contracts on-chain:

1. A tiny **proxy** (15 bytes) deployed via `CREATE2` by the factory
2. The actual **Payment** contract deployed via `CREATE` by the proxy

This is the CREATE3 pattern — it makes the payment address depend only on the
salt and factory address, not the contract bytecode. The proxy is ephemeral
and has no further use after deployment.

## If the invoice is stuck

See [stuck-invoice.md](stuck-invoice.md).
