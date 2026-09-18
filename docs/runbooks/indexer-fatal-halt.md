# Indexer chain halt

The indexer reports a fault when the hash at its cursor no longer matches the
canonical chain. Gum-server records the fault and publishes chain control;
nothing is scheduled or signed on that chain until an operator repairs the
cursor and explicitly resumes it. This triggers the
`payday-indexer-chain-halted` CloudWatch alarm. See
[`docs/architecture.md`](../architecture.md) for service ownership.

## Common causes

| Error in logs | Meaning |
|---------------|---------|
| `finality violation; chain halted` | The cursor block changed hash. The cursor no longer matches the canonical chain. |
| Provider serves another chain | The configured RPC URL was rotated to the wrong network. |
| Database restored from another environment | The stored cursor belongs to different chain history. |

## Step 1: Read the error

```bash
aws logs tail /ecs/payday/indexer --since 30m --region "$AWS_REGION" \
  | grep -E "finality violation; chain halted|chain_id"
```

Confirm the chain is halted in `GET /v1/status` and note its `chain_id`.

## Step 2: Cursor hash mismatch

Check the canonical hash at the cursor block:

```bash
# Requires DB access — see db-access.md:
# SELECT chain_id, last_block, encode(last_block_hash, 'hex')
# FROM indexer_cursor WHERE chain_id = 143;

cast block <CURSOR_BLOCK> --rpc-url "$MONAD_RPC_URL" --json \
  | python3 -c "import sys,json; print(json.load(sys.stdin)['hash'])"
```

If the hashes differ, verify that no deposit requests were funded by transfers
in the reverted blocks. Those requests require manual review. Then repair the
cursor by following [indexer-cursor-reset.md](indexer-cursor-reset.md). Fix a
wrong RPC URL before changing the cursor.

## Step 3: Resume the chain

Resume only after the underlying problem and cursor are fixed. Run the operator
CLI in a one-off task using the `api` image, overriding its command to:

```bash
gum-server chain resume <CHAIN_ID>
```

Use the same networking, task role, and secrets as the API service. This can be
run with `aws ecs run-task --overrides`; `scripts/run-migrate-task.sh <env>` is
the reference for launching a one-off API task.

## Step 4: Verify recovery

The indexer re-checks the chain every pass, so it does not need a restart.
Confirm `GET /v1/status` reports the chain running and watch all three logs with
the incident's `correlation_id` as new work moves through the system:

```bash
aws logs tail /ecs/payday/indexer --since 5m --region "$AWS_REGION"
aws logs tail /ecs/payday/api --since 5m --region "$AWS_REGION"
aws logs tail /ecs/payday/signers --since 5m --region "$AWS_REGION"
```
