# Stuck invoice triage

An invoice is not progressing through its lifecycle. The normal flow is:

```
created → funded → deploying → fulfilled
```

Terminal states: `blocked`, `failed` (unused).

## Step 1: Check the invoice status

```bash
export GATEWAY_API_URL="https://api.payday.sh"
export GATEWAY_API_KEY="<API-key-for-the-account-that-created-the-invoice>"

./target/release/gateway-cli invoice get <INVOICE_ID>
```

Or with curl:

```bash
curl -s -H "Authorization: Bearer $GATEWAY_API_KEY" \
  "$GATEWAY_API_URL/v1/invoices/<INVOICE_ID>" | python3 -m json.tool
```

Note the `status` and `payment_address` from the response.

## Step 2: Verify the USDC transfer landed on-chain

```bash
cast call 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'balanceOf(address)(uint256)' <PAYMENT_ADDRESS> \
  --rpc-url "$MONAD_RPC_URL"
```

If the balance is 0, the payer has not sent USDC yet (or sent to the wrong
address). The invoice will remain in `created` until a transfer is detected
by the indexer.

## Step 3: Stuck in `created` (USDC was sent)

The indexer hasn't processed the block containing the transfer yet.

1. Check if the indexer is running — see [daily-monitoring.md](daily-monitoring.md).
2. Check indexer logs for errors:

   ```bash
   aws logs tail /ecs/payday/indexer --since 30m --region "$AWS_REGION" \
     | grep -E "WARN|ERROR|fatal"
   ```

3. Check if the indexer is far behind — if the cursor is much earlier than the
   transfer block, see [indexer-cursor-reset.md](indexer-cursor-reset.md).

4. If the indexer is running, caught up, and still not detecting the transfer,
   verify the transfer actually exists by searching for USDC Transfer logs to
   the payment address:

   ```bash
   RECIPIENT_TOPIC=$(python3 -c "print('0x' + '<PAYMENT_ADDRESS>'.lower()[2:].zfill(64))")
   FROM=$((CURRENT_BLOCK - 1000))
   curl -s -X POST "$MONAD_RPC_URL" \
     -H "Content-Type: application/json" \
     -d "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getLogs\",\"params\":[{\"fromBlock\":\"$(python3 -c "print(hex($FROM))")\",\"toBlock\":\"latest\",\"address\":\"0x754704Bc059F8C67012fEd69BC8A327a5aafb603\",\"topics\":[\"0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef\",null,null,\"$RECIPIENT_TOPIC\"]}],\"id\":1}"
   ```

## Step 4: Stuck in `funded`

The indexer detected the payment but the sweep worker hasn't submitted a batch
transaction. Check sweep worker logs:

```bash
aws logs tail /ecs/payday/indexer --since 30m --region "$AWS_REGION" \
  | grep -E "sweep|batch|funded|deploying|blocked"
```

Possible causes:

- **KMS signer out of MON**: See [daily-monitoring.md](daily-monitoring.md) step 7.
- **Sweep transaction failed**: Look for `reverted` or `blocked` in logs. The
  invoice may need manual intervention.
- **Indexer crashed after funding**: Restart it — see
  [service-restart.md](service-restart.md). The indexer will pick up funded
  invoices on restart.

## Step 5: Stuck in `deploying`

The sweep transaction was submitted but not yet finalized. This is normal for
a few minutes while waiting for block confirmations.

```bash
aws logs tail /ecs/payday/indexer --since 60m --region "$AWS_REGION" \
  | grep -E "deploying|fulfilled|batch.*finalized|receipt|orphan|revert"
```

If stuck for more than 10 minutes:

- The transaction may be stuck in the mempool. Check the indexer logs for
  `dropped batch transaction` or `pending` warnings.
- The sweep transaction may have been orphaned. The indexer will retry after
  5 minutes and return it to the queue.

## Step 6: Status is `blocked`

A blocked invoice means a sweep execution failed terminally. This requires
manual investigation. Check the indexer logs for the specific invoice ID:

```bash
aws logs tail /ecs/payday/indexer --since 24h --region "$AWS_REGION" \
  | grep "<INVOICE_ID>"
```

The Payment contract can still be executed permissionlessly by anyone calling
`PaymentFactory.execute` with the invoice parameters, since the factory has no
owner gate.
