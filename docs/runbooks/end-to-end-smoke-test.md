# End-to-end smoke test

Verify the full deposit request lifecycle: create a deposit request, fund it with real USDC,
and confirm the funds reach the beneficiary.

## Prerequisites

- API key, `curl`, and `jq` configured (see [README.md](README.md) prerequisites)
- A wallet with USDC on Monad and its private key
- The indexer running and caught up (see [daily-monitoring.md](daily-monitoring.md))

## Step 1: Create a deposit request

```bash
export PAYDAY_API_URL="https://api.payday.sh"
export PAYDAY_API_KEY="<operator-account-api-key>"

curl -fsS "$PAYDAY_API_URL/v1/deposit-requests" \
  -H "Authorization: Bearer $PAYDAY_API_KEY" \
  -H 'Content-Type: application/json' \
  -H "Idempotency-Key: smoke-$(date +%s)" \
  -d '{"amount":"0.01","payout_address":"<PAYOUT_ADDRESS>",
       "issuer":{"name":"Payday"},"payer":{"name":"Smoke test"},
       "payer_policy":{"mode":"permissionless"},"expires_in":3600}' | jq
```

Copy `id` and `address` from the JSON output, and confirm `recovery_address`
is the Payday recovery wallet configured as `recovery_address` in Terraform.

## Step 2: Send USDC to the deposit address

```bash
cast send 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'transfer(address,uint256)' <DEPOSIT_ADDRESS> 10000 \
  --private-key <YOUR_PRIVATE_KEY> \
  --rpc-url "$MONAD_RPC_URL"
```

`10000` is 0.01 USDC in base units (6 decimals).

## Step 3: Monitor the deposit request

```bash
# One-time check:
curl -fsS "$PAYDAY_API_URL/v1/deposit-requests/<DEPOSIT_REQUEST_ID>" \
  -H "Authorization: Bearer $PAYDAY_API_KEY" | jq .status

# Poll every 5 seconds:
watch -n 5 "curl -fsS $PAYDAY_API_URL/v1/deposit-requests/<DEPOSIT_REQUEST_ID> \
  -H 'Authorization: Bearer $PAYDAY_API_KEY' | jq .status"
```

The status should progress: `awaiting_deposit → deposited → settled`.

With the indexer caught up, this typically takes under 30 seconds.

## Step 4: Verify on-chain state

```bash
# Deposit address should be emptied (balance = 0):
cast call 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'balanceOf(address)(uint256)' <DEPOSIT_ADDRESS> \
  --rpc-url "$MONAD_RPC_URL"

# Deposit contract should be deployed (non-empty code):
cast code <DEPOSIT_ADDRESS> --rpc-url "$MONAD_RPC_URL"

# Payout address should have received the USDC:
cast call 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'balanceOf(address)(uint256)' <PAYOUT_ADDRESS> \
  --rpc-url "$MONAD_RPC_URL"
```

## Step 5: Verify late funds reach the Payday recovery wallet

Send a second, small transfer to the same deposit address. It must reach the
Payday recovery wallet within a minute while the status stays `settled`, and
the `recovered_funds` ledger must record it:

```bash
cast send 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'transfer(address,uint256)' <DEPOSIT_ADDRESS> 1000 \
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
WHERE invoice_id = '<DEPOSIT_REQUEST_UUID>';
```

Expect one row with reason `late_transfer` and amount `1000`, and a
`deposit_request.recovered_funds` delivery on any registered webhook. Return the
recovered amount by hand afterwards; nothing automates it.

## Expected results

| Check | Expected |
|-------|----------|
| Deposit status | `settled` |
| Deposit address USDC balance | `0` |
| Deposit address code | Non-empty (contract deployed) |
| Payout address USDC balance | Increased by exactly the deposit request amount |
| Recovery wallet USDC balance | Increased by the late transfer, with a matching `recovered_funds` row |

## Understanding the sweep transaction

The sweep creates two contracts on-chain:

1. A tiny **proxy** (15 bytes) deployed via `CREATE2` by the factory
2. The actual **Deposit** contract deployed via `CREATE` by the proxy

This is the CREATE3 pattern — it makes the deposit address depend only on the
salt and factory address, not the contract bytecode. The proxy is ephemeral
and has no further use after deployment.

## If the deposit request is stuck

See [stuck-deposit-request.md](stuck-deposit-request.md).
