# End-to-end smoke test

Verify the full payment lifecycle: create a payment, fund it with real USDC,
and confirm the funds reach the beneficiary.

## Prerequisites

- API key, `curl`, and `jq` configured (see [README.md](README.md) prerequisites)
- A wallet with USDC on Monad and its private key
- The indexer running and caught up (see [daily-monitoring.md](daily-monitoring.md))

## Step 1: Create a payment

```bash
export PAYDAY_API_URL="https://api.payday.sh"
export PAYDAY_API_KEY="<operator-account-api-key>"

curl -fsS "$PAYDAY_API_URL/v1/payments" \
  -H "Authorization: Bearer $PAYDAY_API_KEY" \
  -H 'Content-Type: application/json' \
  -H "Idempotency-Key: smoke-$(date +%s)" \
  -d '{"amount":"0.01","payout_address":"<PAYOUT_ADDRESS>",
       "issuer":{"name":"Payday"},"bill_to":{"name":"Smoke test"},
       "payer_policy":{"mode":"permissionless"},"expires_in":3600}' | jq
```

Copy `id` and `payment_url` from the JSON output; `address` is null until the
payer's wallet is bound. Open `payment_url` in a browser, connect the wallet
you will pay from, and sign the attestation it offers. Then re-read the
payment and confirm `address` is set, `payer_wallet` is your wallet, and
`recovery_address` equals it:

```bash
curl -s "$PAYDAY_API_URL/v1/payments/<ID>" -H "Authorization: Bearer $PAYDAY_API_KEY" | jq
```

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
curl -fsS "$PAYDAY_API_URL/v1/payments/<PAYMENT_ID>" \
  -H "Authorization: Bearer $PAYDAY_API_KEY" | jq .status

# Poll every 5 seconds:
watch -n 5 "curl -fsS $PAYDAY_API_URL/v1/payments/<PAYMENT_ID> \
  -H 'Authorization: Bearer $PAYDAY_API_KEY' | jq .status"
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

## Step 5: Verify late funds come back to the payer's wallet

Send a second, small transfer to the same payment address from the wallet you
signed with. It must come back to that wallet within a minute while the
status stays `settled`, and the `recovered_funds` ledger must record it:

```bash
cast send 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'transfer(address,uint256)' <PAYMENT_ADDRESS> 1000 \
  --private-key <YOUR_PRIVATE_KEY> \
  --rpc-url "$MONAD_RPC_URL"

cast call 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'balanceOf(address)(uint256)' <YOUR_WALLET> \
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
