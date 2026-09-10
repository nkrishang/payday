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

Monad's `log_range_size` (per entry of `chains` in Terraform, and of
`PAYDAY_CHAINS`) is 100 for that reason; Base and Arbitrum One allow 10,000. Raising it on QuickNode only produces rejections; it is useful
when switching to a provider with a larger window.

## Fix: request budget

The indexer runs one worker per chain in `PAYDAY_CHAINS`, and each worker
scans only while it has something to watch (an open bound request,
uncollected funds, or a request settled within `PAYDAY_INDEXER_LATE_WATCH_DAYS`).
An idle chain costs two calls (boundary header and cursor check) every
`PAYDAY_INDEXER_IDLE_INTERVAL_MS` (five minutes), fast-forwards its cursor
without `eth_getLogs`, and holds no WebSocket. Per chain, per day:

| State | Calls | Notes |
|--------|-------|-------|
| Idle (empty watch list) | ~900 | 288 passes × 2, plus the signer balance check |
| Active Monad (`scan = full`, `finalized`) | ~14k | 1,440 passes × 2 + 2,880 ranges at the 100-block cap × 3 + 2,880 keepalives |
| Active Base or Arbitrum (`scan = watched`, `latest` − N) | ~7–9k | 1,440 passes × 3 + per range two headers + ⌈watched ÷ 500⌉ filtered `eth_getLogs`, plus keepalives |
| Transfer signal notifications | ≈ 0 | one per commit state per payment to us |

Three idle chains are about 2.6k calls a day. An active chain returns to
idle as soon as its last watched request leaves the late-watch window.
Detection latency does not come from the cadence: the WebSocket transfer
signal wakes a pass the moment a payment finalizes (Monad) or lands
(`latest` chains, which then wait `finality_confirmations` blocks). A block
that carries USDC transfers costs nothing extra when the log carries
`blockTimestamp`; a node that omits it costs one header per distinct block.

If credits climb well above that, check in this order:

1. `transfer signal disconnected` / `connection failed` warnings (the
   `payday-indexer-transfer-signal-down` alarm; every log line names its
   `chain_id`). While the socket is down on an active chain the reconciler
   runs every `indexer_poll_interval_ms` (5 s), which is the old cost
   profile: about 3.9M credits a day on Monad. Confirm that chain's `wss://`
   URL works (`PAYDAY_RPC_WS_URL_<chain_id>` overrides the derivation from
   `PAYDAY_RPC_URL_<chain_id>`). An idle chain holds no socket and never
   raises this.
2. `indexer cursor lagging`: a backlog is draining at
   `range × PAYDAY_INDEXER_MAX_RANGES_PER_TICK` blocks per pass, three calls
   per range. This is bounded work that ends when the cursor catches up.
3. `429` / `-32007` errors counted by `payday-indexer-retryable-failures`:
   the pacing below should make these rare at this call volume; a sustained
   run means something else shares the endpoint's requests-per-second budget.

Since the 2026-09 livelock (below), the indexer defends itself in two ways:

- `PAYDAY_INDEXER_RPC_MAX_RPS` (default 40) paces every outgoing RPC call so
  no burst can exceed the plan's requests-per-second budget; 0 disables pacing
  (the local runner sets it for Anvil, which has no budget to trip).
- Retryable failures (429, timeouts) inside a tick back off exponentially and
  retry the same read — up to 10 attempts per range — instead of aborting the
  tick and discarding its uncommitted ranges.

### The catch-up livelock this replaces

Before that fix, a large backlog (fresh database, new `usdc_start_block`)
made each 5 s tick attempt its whole remaining catch-up at once. The burst
tripped QuickNode's 50 requests/second budget, the resulting 429 aborted the
tick, and the next tick re-fetched the same ranges — 4.3M requests in one day
with the cursor never advancing. Symptoms: `indexer poll failed; retrying next
tick` every poll interval since startup, no `indexer cursor lagging` warnings
(a tick that never completes never reaches that check), and provider usage
climbing while invoice statuses never change. The remedy then was to stop the
indexer service and deploy the paced build; the structural remedy is the
pacing and in-range retry above.

## Fix: switch provider window

To use a provider with a larger window, rotate the RPC secret
([secrets-rotation.md](secrets-rotation.md)), set `log_range_size` to its cap,
and apply the task definition:

```bash
cd infra
export TF_VAR_rpc_urls='{"143":"'"$MONAD_RPC_URL"'","8453":"'"$BASE_RPC_URL"'","42161":"'"$ARBITRUM_RPC_URL"'"}'
terraform apply -target=aws_ecs_task_definition.indexer -auto-approve
```

`log_range_size` is per entry of `chains`; change the chain that was
rejected.

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
