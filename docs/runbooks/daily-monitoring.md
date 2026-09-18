# Daily monitoring

Routine health checks for the deposit request gateway. Run these proactively or
when investigating a reported issue.

## 1. Check alarm states

Every CloudWatch alarm should be in `OK` state. Any `ALARM` needs attention —
see the relevant runbook.

```bash
aws cloudwatch describe-alarms --region "$AWS_REGION" \
  --query 'MetricAlarms[*].{Alarm:AlarmName,State:StateValue,Reason:StateReason}' \
  --output table
```

| Alarm | Meaning | Runbook |
|-------|---------|---------|
| `payday-unhealthy-targets` | API readiness is failing | [service-restart.md](service-restart.md) |
| `payday-api-task-count` | API container is not running | [service-restart.md](service-restart.md) |
| `payday-indexer-task-count` | Indexer container is not running | [service-restart.md](service-restart.md) |
| `payday-signers-task-count` | Signers container is not running | [service-restart.md](service-restart.md) |
| `payday-indexer-chain-halted` | Indexer reported a finality violation and the chain requires an explicit resume | [indexer-fatal-halt.md](indexer-fatal-halt.md) |
| `payday-indexer-pass-failing` | An indexer pass repeatedly failed | check provider status and indexer logs |
| `payday-indexer-signal-down` | Transfer signal is disconnected | [quicknode-rpc-limits.md](quicknode-rpc-limits.md) |
| `payday-signers-stalled` | A transaction reached its replacement limit | [stuck-deposit-request.md](stuck-deposit-request.md) |
| `payday-signers-low-balance` | A signer is below its chain's low-balance level | step 7 below |
| `payday-signers-failing` | Signing or reconciliation repeatedly failed | inspect signers logs and `execution.executor_status` |
| `payday-signers-dead-letter` | Signers permanently rejected a bus delivery | inspect with `gum-server bus dead` |
| `payday-server-dead-letter` | API permanently rejected a bus delivery | inspect with `gum-server bus dead` |
| `payday-server-sweep-stuck` | An open sweep job is stale | [stuck-deposit-request.md](stuck-deposit-request.md) |
| `payday-server-attention` | A deposit request needs operator attention | [stuck-deposit-request.md](stuck-deposit-request.md) step 5 |
| `payday-notification-delivery-failures` | Merchant email or webhook delivery repeatedly failed | inspect gum-server logs and pending rows in `notification_outbox`; delivery retries automatically |
| `payday-notification-missing-contact` | A deposit request needing attention has neither an email nor webhook snapshot | recover the account contact, notify the merchant manually, and inspect `notification_outbox` |
| `payday-db-high-cpu` | RDS CPU > 80% for 15 min | scale the instance |
| `payday-db-low-storage` | RDS has < 5 GB free storage | raise `db_max_allocated_storage` |

Alarms derived from worker logs return to `OK` on their own once the worker
stops reporting the condition.

## 2. Check service health

```bash
aws ecs describe-services --cluster payday --services api indexer signers \
  --region "$AWS_REGION" \
  --query 'services[*].{Service:serviceName,Running:runningCount,Desired:desiredCount,Status:status}' \
  --output table
```

Running and Desired should match. If Running is 0, see
[service-restart.md](service-restart.md).

## 3. Check indexer logs for problems

```bash
# Last 30 minutes, errors and warnings only:
aws logs tail /ecs/payday/indexer --since 30m --region "$AWS_REGION" \
  | grep -E "WARN|ERROR|finality violation|signal|lagging|stale"
```

The indexer is read-only and has no database connection or keys. Execution
health is reported by `execution.executor_status` and
`execution.signer_status`, and by `GET /v1/status`.

## 4. Check API logs

```bash
aws logs tail /ecs/payday/api --since 30m --region "$AWS_REGION"
```

Also check signers logs for alarm-keyed failures:

```bash
aws logs tail /ecs/payday/signers --since 30m --region "$AWS_REGION" \
  | grep -E "signer balance is low|job deferred after a transient failure|reconciling transaction failed|broadcast failed|transaction unconfirmed|dead-lettered"
```

## 5. Verify the API endpoint is reachable

```bash
curl -sf "$GUM_API_URL/health/live" && echo " live OK" || echo " live FAIL"
curl -sf "$GUM_API_URL/health/ready" && echo " ready OK" || echo " ready FAIL"
```

## 6. Check indexer cursor lag

Compare each chain's indexer cursor against its finality boundary. The
`chains` array of `GET /v1/status` reports both per network; by hand, for
Monad (Base and Arbitrum use `latest` minus their `finality_confirmations`
instead of `finalized`):

```bash
# Finalized Monad block
cast block finalized --rpc-url "$MONAD_RPC_URL" --field number

# Indexer cursor (requires DB access — see db-access.md)
# After connecting to the DB:
#   SELECT chain_id, last_block FROM indexer_cursor;
```

A small lag (a few blocks) is normal. A chain with nothing to watch
fast-forwards its cursor once per `GUM_INDEXER_IDLE_INTERVAL_MS` (five
minutes) without scanning, so an idle chain's cursor legitimately trails by
up to that long; the `lag_blocks` in `/v1/status` is measured at the last
pass. The worker drains up to
`GUM_INDEXER_MAX_RANGES_PER_TICK` ranges per pass, so a backlog after an
outage clears on its own; a lag that keeps growing means the provider is
rejecting requests — see [quicknode-rpc-limits.md](quicknode-rpc-limits.md).

## 7. Check every KMS signer's gas balance on every chain

The signers service is the only holder of the KMS keys. Each signer address
needs gas on each chain: MON on Monad, ETH on Base, ETH on Arbitrum One. The
service warns per signer and chain below the configured low-balance level.
Monad bills
the gas *limit* of every helper transaction (`100k + 400k × items`), so a
full batch reserves about 8.1M gas worth of MON from the signer that sends
it. If one signer runs out, its submissions fail and the other signers carry
the queue more slowly; the `payday-signers-low-balance` alarm fires
first.

```bash
# Read signer addresses and balances from the status table (see db-access.md).
SIGNERS=$(psql "$DATABASE_URL" -Atc \
  "SELECT DISTINCT '0x' || encode(signer, 'hex') FROM execution.signer_status")

for addr in $SIGNERS; do
  echo "$addr"
  cast balance "$addr" --rpc-url "$MONAD_RPC_URL"
  cast balance "$addr" --rpc-url "$BASE_RPC_URL"
  cast balance "$addr" --rpc-url "$ARBITRUM_RPC_URL"
done
```

Fund whichever signer and chain is low with that chain's gas token.
