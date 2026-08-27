# QuickNode RPC log range limits

The indexer's `eth_getLogs` calls are rejected because the provider limits the
block range per request.

## Symptoms

Indexer logs show repeated warnings:

```
provider rejected USDC log range; splitting it from_block=... to_block=...
error=HTTP error 413 with body: {"jsonrpc":"2.0","id":...,"error":{"code":-32615,
"message":"eth_getLogs is limited to a N range, upgrade from ... plan ..."}}
```

The indexer handles this gracefully by halving the range and retrying, then
growing it again after successes. Catch-up throughput is
`range × PAYDAY_INDEXER_MAX_RANGES_PER_TICK` blocks per pass, so a smaller
range mostly costs extra requests, not lag.

## Provider limits on Monad

Monad's RPC documentation lists the `eth_getLogs` block range each provider
allows, independent of plan:

| Provider | Block range per call |
|----------|----------------------|
| QuickNode | 100 |
| Monad Foundation | 100 |
| Ankr | 1,000 |
| Alchemy | 1,000 blocks / 10,000 logs |

`PAYDAY_LOG_RANGE_SIZE` (`log_range_size` in Terraform) defaults to 100 for
that reason. Raising it on QuickNode only produces rejections; it is useful
when switching to a provider with a larger window.

## Fix: request budget

With the 100-block cap and Monad's ~0.4 s blocks, steady-state indexing needs
about 90 `eth_getLogs` calls per hour, finality and cursor-header calls, and one
header lookup for each distinct block containing a USDC transfer. Transfer-block
lookups run concurrently within a range but are still billed RPC requests. If
the endpoint's request-rate limit is being hit (`429` / `-32007` errors,
counted by the `payday-indexer-retryable-failures` alarm), distinguish idle-poll
traffic from catch-up and transfer-header traffic before changing
`indexer_poll_interval_ms`: the poll interval affects detection latency and idle
calls, not the request volume required to process a backlog.

## Fix: switch provider window

To use a provider with a larger window, rotate the RPC secret
([secrets-rotation.md](secrets-rotation.md)), set `log_range_size` to its cap,
and apply the task definition:

```bash
cd infra
export TF_VAR_rpc_url="$MONAD_RPC_URL"
terraform apply -target=aws_ecs_task_definition.indexer -auto-approve
```

Then update the ECS service to use the new task definition revision and
restart — see [service-restart.md](service-restart.md).

## Verify the fix

```bash
# Check indexer logs — rejections should stop
aws logs tail /ecs/payday/indexer --since 5m --region "$AWS_REGION" \
  | grep -E "413|rejected|splitting|lagging"

# Test the RPC directly with a 100-block range
FROM=$(python3 -c "print(hex($(cast block-number --rpc-url "$MONAD_RPC_URL") - 100))")
TO=$(python3 -c "print(hex($(cast block-number --rpc-url "$MONAD_RPC_URL") - 1))")
curl -s -X POST "$MONAD_RPC_URL" \
  -H "Content-Type: application/json" \
  -d "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getLogs\",\"params\":[{\"fromBlock\":\"$FROM\",\"toBlock\":\"$TO\",\"address\":\"0x754704Bc059F8C67012fEd69BC8A327a5aafb603\",\"topics\":[\"0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef\"]}],\"id\":1}" \
  | python3 -c "import sys,json; d=json.load(sys.stdin); print('error:', d.get('error','none'), 'results:', len(d.get('result',[]))) "
```
