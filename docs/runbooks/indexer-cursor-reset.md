# Indexer cursor reset

The indexer maintains a cursor in the database tracking the last-processed
block. If the cursor is far behind the chain head (e.g. after a long downtime
or a fresh deploy with an old start block), you can skip ahead to a recent
block to avoid replaying unnecessary history.

**Only do this if no deposit requests were created before the new cursor block.**
Deposit requests created before the cursor block will never be detected because the
indexer only scans forward.

## When to reset

- After a fresh deployment where `usdc_start_block` was set too early
- After the indexer was down for a long time and no deposit requests are pending
- After a cursor hash mismatch (see [indexer-fatal-halt.md](indexer-fatal-halt.md))

## Step 1: Find the target block

Pick a block to reset to. This should be a few blocks before the earliest
unprocessed USDC transfer you care about.

```bash
CURRENT=$(cast block-number --rpc-url "$MONAD_RPC_URL")
echo "Current block: $CURRENT"

# Use a block a few minutes behind head, accounting for finality confirmations
TARGET=$((CURRENT - 10))
echo "Target block: $TARGET"
```

## Step 2: Get the block hash

The cursor requires the canonical block hash for reorg detection.

```bash
BLOCK_HASH=$(cast block "$TARGET" --rpc-url "$MONAD_RPC_URL" --json \
  | python3 -c "import sys,json; print(json.load(sys.stdin)['hash'])")
echo "Block $TARGET hash: $BLOCK_HASH"
```

## Step 3: Reset the cursor in the database

RDS is in a private subnet with no public access. Use a one-off Fargate task
with a Postgres image to run SQL inside the VPC. See [db-access.md](db-access.md)
for the full procedure. The SQL is:

```sql
INSERT INTO indexer_cursor (chain_id, token_address, last_block, last_block_hash)
VALUES (
  143,
  decode('754704bc059f8c67012fed69bc8a327a5aafb603', 'hex'),
  <TARGET_BLOCK>,
  decode('<BLOCK_HASH_WITHOUT_0X>', 'hex')
)
ON CONFLICT (chain_id) DO UPDATE SET
  last_block = <TARGET_BLOCK>,
  last_block_hash = decode('<BLOCK_HASH_WITHOUT_0X>', 'hex'),
  updated_at = now();
```

Replace `<TARGET_BLOCK>` with the numeric block number and
`<BLOCK_HASH_WITHOUT_0X>` with the block hash minus the `0x` prefix.

## Step 4: Verify the indexer picks up from the new cursor

```bash
# Wait a few seconds for the next poll tick, then check logs:
aws logs tail /ecs/payday/indexer --since 2m --region "$AWS_REGION"
```

If the indexer was already running, it will use the new cursor on its next
tick (every 2 seconds by default). No restart is needed.

If the indexer was stopped, restart it — see [service-restart.md](service-restart.md).

## Step 5: Verify deposit requests are progressing

```bash
curl -fsS "$PAYDAY_API_URL/v1/deposit-requests/<DEPOSIT_REQUEST_ID>" \
  -H "Authorization: Bearer $PAYDAY_API_KEY" | jq .status
```

The status should advance from `created` to `funded` within a few seconds.
