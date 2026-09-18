# End-to-end smoke test

Verify the full deposit request lifecycle: create a deposit request, fund it with real USDC,
and confirm the funds reach the beneficiary. `scripts/live-smoke.sh` runs the
same flow unattended; `GUM_CURRENCY=USDT` makes it a USDT request pinned
to `CHAIN_ID` (Monad or Arbitrum, where USDT0 is served) and
`GUM_TOKEN_ADDRESS` overrides the contract it pays from.

## Prerequisites

- API key, `curl`, and `jq` configured (see [README.md](README.md) prerequisites)
- A wallet with USDC on the network you will test and its private key (the
  commands below use Monad; substitute the chain's endpoint and USDC address
  for Base or Arbitrum One, and choose that network on the checkout). For a
  USDT smoke test add `"currency":"USDT"` and `"chain_id":"143"` to the
  request body and pay with Monad's USDT0
  (`0xe7cd86e13AC4309349F30B3435a9d337750fC82D`) wherever the USDC contract
  appears below.
- All three services running, with the indexer caught up and signers healthy
  (see [daily-monitoring.md](daily-monitoring.md))

## Step 1: Create a deposit request

```bash
export GUM_API_URL="https://api.gum.money"
export GUM_API_KEY="<operator-account-api-key>"

curl -fsS "$GUM_API_URL/v1/deposit-requests" \
  -H "Authorization: Bearer $GUM_API_KEY" \
  -H 'Content-Type: application/json' \
  -H "Idempotency-Key: smoke-$(date +%s)" \
  -d '{"amount":"0.01","payout_address":"<PAYOUT_ADDRESS>",
       "issuer":{"name":"Gum"},"payer":{"name":"Smoke test"},
       "payer_policy":{"mode":"permissionless"},"expires_in":3600}' | jq
```

Copy `id` and `deposit_url` from the JSON output; `currency` is `USDC`
(the default) and `address` is null until the payer's wallet is bound. Open `deposit_url` in a browser, connect the wallet
you will pay from, and sign the attestation it offers. Then re-read the
deposit and confirm `address` is set, `payer_wallet` is your wallet, and
`recovery_address` equals it:

```bash
curl -s "$GUM_API_URL/v1/deposit-requests/<ID>" -H "Authorization: Bearer $GUM_API_KEY" | jq
```

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
curl -fsS "$GUM_API_URL/v1/deposit-requests/<DEPOSIT_REQUEST_ID>" \
  -H "Authorization: Bearer $GUM_API_KEY" | jq .status

# Poll every 5 seconds:
watch -n 5 "curl -fsS $GUM_API_URL/v1/deposit-requests/<DEPOSIT_REQUEST_ID> \
  -H 'Authorization: Bearer $GUM_API_KEY' | jq .status"
```

The status should progress: `awaiting_deposit → deposited → settled`.

With the indexer caught up, this typically takes under 30 seconds.

For an operator trace, read the request's `sweep_jobs.correlation_id`, then:

```sql
SELECT * FROM bus.messages
WHERE correlation_id = '<CORRELATION_ID>' ORDER BY published_at;
```

The API, indexer, and signers logs carry the same `correlation_id`.

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

## Step 5: Verify late funds come back to the payer's wallet

Send a second, small transfer to the same deposit address from the wallet you
signed with. It must come back to that wallet within a minute while the
status stays `settled`, and the `recovered_funds` ledger must record it:

```bash
cast send 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'transfer(address,uint256)' <DEPOSIT_ADDRESS> 1000 \
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
| Payout address USDC balance | Increased by exactly the requested amount |
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
