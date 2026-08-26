# Indexer fatal halt

The indexer intentionally halts when it detects a condition that could cause
incorrect payment processing. This triggers the `payday-indexer-fatal` CloudWatch
alarm via a log metric filter.

## Common causes

| Error in logs | Meaning |
|---------------|---------|
| `FinalityViolation` / `cursor hash mismatch` | A finalized block changed hash (reorg). The cursor no longer matches the canonical chain. |
| `exclusive indexer database lock` failure | Another indexer process is running or the lock is stuck. |
| `indexer fatal` | Generic fatal error from the poll loop. |

## Step 1: Read the error

```bash
aws logs tail /ecs/payday/indexer --since 30m --region "$AWS_REGION" \
  | grep -E "fatal|FinalityViolation|mismatch|halt|panic"
```

## Step 2: Cursor hash mismatch

This means the block at the cursor's `last_block` has a different hash than
when it was originally processed. The indexer refuses to continue because
observations paired with the old hash may be invalid.

1. Check the current canonical hash at the cursor's block:

   ```bash
   # Find the cursor block (requires DB access — see db-access.md)
   # SELECT last_block, encode(last_block_hash, 'hex') FROM indexer_cursor WHERE chain_id = 143;

   # Then check the current hash at that block:
   cast block <CURSOR_BLOCK> --rpc-url "$MONAD_RPC_URL" --json \
     | python3 -c "import sys,json; print(json.load(sys.stdin)['hash'])"
   ```

2. If the hash differs, you need to reset the cursor to a recent finalized
   block. Follow [indexer-cursor-reset.md](indexer-cursor-reset.md).

3. Before resetting, verify no invoices were funded by transfers in the
   reverted blocks. If any were, those payments may need manual review.

## Step 3: Database lock issue

If the error mentions the advisory lock, another indexer may be running:

```bash
aws ecs describe-services --cluster payday --service indexer \
  --region "$AWS_REGION" \
  --query 'services[0].{Running:runningCount,Desired:desiredCount}' --output table
```

If Running > 1, stop the extra task. The lock is designed to allow only one
active indexer; a second process should fail at startup. If the lock is stuck
after a crash, restarting the ECS task will acquire a new session.

## Step 4: Restart after resolving

Once the underlying cause is fixed, restart the indexer:

```bash
aws ecs update-service --cluster payday --service indexer \
  --force-new-deployment --region "$AWS_REGION"
```

See [service-restart.md](service-restart.md) for details.
