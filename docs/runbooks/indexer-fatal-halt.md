# Indexer fatal halt

The block indexer intentionally halts when it detects a condition that could
cause incorrect deposit processing. This triggers the `payday-indexer-fatal`
CloudWatch alarm via a log metric filter and ECS restarts the task, which
halts again until the cause is fixed.

A stuck *sweep* is not fatal: the sweep worker pauses on its own and raises
`payday-indexer-sweep-paused` while block indexing continues. See
[stuck-deposit-request.md](stuck-deposit-request.md#sweep-worker-paused) for that case.

## Common causes

| Error in logs | Meaning |
|---------------|---------|
| `FinalityViolation` / `cursor hash mismatch` | A finalized block changed hash (reorg). The cursor no longer matches the canonical chain. |
| `RPC error (…)` marked permanent (HTTP 400/401/403/404/413) | The provider rejected the request outright; usually a rotated or exhausted endpoint. |
| `sweep transaction … targeted …, not configured BatchSweeper` | A recorded helper transaction hash points at a foreign transaction; the database was edited. |
| Deployment verification failure at startup (factory or BatchSweeper code hash differs from `PAYDAY_FACTORY_CODE_HASH` / `PAYDAY_BATCH_SWEEPER_CODE_HASH`, or `BatchSweeper.factory()` is not `PAYDAY_FACTORY_ADDRESS`) | The chain does not carry the contract generation this build was configured for; see step 2a. `gatewayd` refuses to start on the same check. |
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
observations paired with the old hash may be invalid. With
`PAYDAY_FINALITY_SOURCE=finalized` this should never happen on Monad without a
hard fork; a provider serving a different chain or a database restored from a
different environment is the likelier explanation.

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

3. Before resetting, verify no deposit requests were funded by transfers in the
   reverted blocks. If any were, those deposit requests may need manual review.

## Step 2a: Deployment verification refused

Both services compare the runtime bytecode at `PAYDAY_FACTORY_ADDRESS` and
`PAYDAY_BATCH_SWEEPER_ADDRESS` with the configured code hashes and check that
the sweeper is bound to the configured factory before doing anything else. A
refusal means the configuration and the chain disagree: a wrong address or
RPC (another network, a provider serving a different chain), a hash recorded
from the wrong build, or a factory pointed at an old sweeper. Recompute from
the chain the services are meant to use and compare with the Terraform
variables:

```bash
cast keccak "$(cast code <FACTORY_ADDRESS> --rpc-url "$MONAD_RPC_URL")"
cast keccak "$(cast code <BATCH_SWEEPER_ADDRESS> --rpc-url "$MONAD_RPC_URL")"
cast call <BATCH_SWEEPER_ADDRESS> 'factory()(address)' --rpc-url "$MONAD_RPC_URL"
```

Fix the configuration, never the check. If the contracts really changed, that
is a new contract generation and needs the fresh-database procedure in
[production-runbook.md](../production-runbook.md); do not repoint a running
database at different contracts.

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

See [service-restart.md](service-restart.md) for details. On restart the
worker reconciles any in-flight helper transaction from its receipt before
submitting a new one.
