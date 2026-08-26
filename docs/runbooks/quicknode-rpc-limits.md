# QuickNode RPC log range limits

The indexer's `eth_getLogs` calls are rejected with HTTP 413 because the
QuickNode plan limits the block range per request.

## Symptoms

Indexer logs show repeated warnings:

```
provider rejected USDC log range; splitting it from_block=... to_block=...
error=HTTP error 413 with body: {"jsonrpc":"2.0","id":...,"error":{"code":-32615,
"message":"eth_getLogs is limited to a N range, upgrade from ... plan ..."}}
```

The indexer handles this gracefully by halving the range and retrying, but
catch-up becomes extremely slow — one request per few blocks instead of
hundreds.

## QuickNode plan limits

| Plan | `eth_getLogs` max range | Approx. cost |
|------|------------------------|--------------|
| Discover (free) | 5 blocks | $0 |
| Build | 1,000 blocks | ~$49/mo |
| Scale | 10,000 blocks | ~$199/mo |
| Enterprise | Custom | Contact sales |

Verify current limits at
[QuickNode billing](https://dashboard.quicknode.com/billing/plan).

## Fix: Upgrade the plan

1. Go to https://dashboard.quicknode.com/billing/plan
2. Upgrade to at least the Build plan (1,000-block ranges)
3. No RPC URL change is needed — the same endpoint works

## Fix: Update `GATEWAY_LOG_RANGE_SIZE` to match the plan

The Terraform variable `log_range_size` controls the indexer's requested block
range per `eth_getLogs` call. Set it to match your plan's limit:

```bash
# In infra/terraform.tfvars:
# log_range_size = 1000   # Build plan
# log_range_size = 10000  # Scale plan
```

Apply the task definition change:

```bash
cd infra
export TF_VAR_rpc_url="$MONAD_RPC_URL"
terraform apply -target=aws_ecs_task_definition.indexer -auto-approve
```

Then update the ECS service to use the new task definition revision and
restart:

```bash
# Find the latest revision
REVISION=$(aws ecs list-task-definitions --family-prefix payday-indexer \
  --region "$AWS_REGION" --sort DESC --max-items 1 --output text \
  | sed 's|.*/||')

aws ecs update-service --cluster payday --service indexer \
  --task-definition "$REVISION" --region "$AWS_REGION"
```

## Verify the fix

```bash
# Check indexer logs — 413 errors should stop
aws logs tail /ecs/payday/indexer --since 5m --region "$AWS_REGION" \
  | grep -E "413|rejected|splitting"

# Test the RPC directly with a 1000-block range
FROM=$(python3 -c "print(hex($(cast block-number --rpc-url "$MONAD_RPC_URL") - 1000))")
TO=$(python3 -c "print(hex($(cast block-number --rpc-url "$MONAD_RPC_URL") - 1))")
curl -s -X POST "$MONAD_RPC_URL" \
  -H "Content-Type: application/json" \
  -d "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getLogs\",\"params\":[{\"fromBlock\":\"$FROM\",\"toBlock\":\"$TO\",\"address\":\"0x754704Bc059F8C67012fEd69BC8A327a5aafb603\",\"topics\":[\"0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef\"]}],\"id\":1}" \
  | python3 -c "import sys,json; d=json.load(sys.stdin); print('error:', d.get('error','none'), 'results:', len(d.get('result',[]))) "
```
