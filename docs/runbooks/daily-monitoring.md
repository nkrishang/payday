# Daily monitoring

Routine health checks for the payment gateway. Run these proactively or
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
| `payday-unhealthy-targets` | API `/health` is failing (the check includes a database round-trip) | [service-restart.md](service-restart.md) |
| `payday-api-task-count` | API container is not running | [service-restart.md](service-restart.md) |
| `payday-status-unhealthy-targets` / `payday-status-task-count` | The independently deployed public status page is unavailable | [service-restart.md](service-restart.md) |
| `payday-indexer-task-count` | Indexer container is not running | [service-restart.md](service-restart.md) |
| `payday-indexer-fatal` | Block indexer hit a permanent halt (cursor mismatch, etc.) | [indexer-fatal-halt.md](indexer-fatal-halt.md) |
| `payday-indexer-sweep-paused` | Sweep worker cannot resolve its in-flight helper transaction; indexing continues | [stuck-invoice.md](stuck-invoice.md#sweep-worker-paused) |
| `payday-indexer-signer-low-balance` | KMS sweep signer below `PAYDAY_SIGNER_LOW_BALANCE_WEI` | step 7 below |
| `payday-indexer-cursor-lagging` | Cursor trails finality by more than 1,000 blocks | [stuck-invoice.md](stuck-invoice.md) step 3 |
| `payday-indexer-sweep-backlog-stale` | Collectable funds have waited more than 15 minutes | [stuck-invoice.md](stuck-invoice.md) step 4 |
| `payday-indexer-retryable-failures` | More than ten retryable RPC/database failures in five minutes | check the provider status page and indexer logs |
| `payday-notification-delivery-failures` | Merchant email or webhook delivery repeatedly failed | inspect gatewayd logs and pending rows in `notification_outbox`; delivery retries automatically |
| `payday-notification-missing-contact` | A blocked legacy invoice has neither an email nor webhook snapshot | recover the account contact, notify the merchant manually, and inspect `notification_outbox` |
| `payday-db-high-cpu` | RDS CPU > 80% for 15 min | scale the instance |
| `payday-db-low-storage` | RDS has < 5 GB free storage | raise `db_max_allocated_storage` |

Alarms derived from worker logs return to `OK` on their own once the worker
stops reporting the condition.

## 2. Check service health

```bash
aws ecs describe-services --cluster payday --services api status indexer \
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
  | grep -E "WARN|ERROR|fatal|paused|blocked|lagging|stale"
```

The worker logs a `sweep worker health` line every thirty passes with the
queue depth, in-flight count, age of the oldest uncollected transfer, and the
signer balance; otherwise it only logs warnings, errors, and lifecycle events.

## 4. Check API logs

```bash
aws logs tail /ecs/payday/api --since 30m --region "$AWS_REGION"
```

## 5. Verify the API endpoint is reachable

```bash
curl -sf "$PAYDAY_API_URL/health" && echo " OK" || echo " FAIL"
```

## 6. Check indexer cursor lag

Compare the indexer's last-processed block against the chain's finalized head:

```bash
# Finalized Monad block
cast block finalized --rpc-url "$MONAD_RPC_URL" --field number

# Indexer cursor (requires DB access — see db-access.md)
# After connecting to the DB:
#   SELECT last_block FROM indexer_cursor WHERE chain_id = 143;
```

A small lag (a few blocks) is normal. The worker drains up to
`PAYDAY_INDEXER_MAX_RANGES_PER_TICK` ranges per pass, so a backlog after an
outage clears on its own; a lag that keeps growing means the provider is
rejecting requests — see [quicknode-rpc-limits.md](quicknode-rpc-limits.md).

## 7. Check KMS signer MON balance

The sweep signer needs MON for gas. Monad bills the gas *limit* of every
helper transaction (`100k + 400k × items`), so a full batch reserves about
8.1M gas worth of MON. If the balance runs out, submissions fail and invoices
wait in the queue; the `payday-indexer-signer-low-balance` alarm fires first.

```bash
# Get the signer address from indexer logs or Terraform output
SIGNER_ADDR=$(aws logs tail /ecs/payday/indexer --since 24h --region "$AWS_REGION" \
  | grep "configured sweep signer" | grep -oE '0x[0-9a-fA-F]{40}' | head -1)

cast balance "$SIGNER_ADDR" --rpc-url "$MONAD_RPC_URL"
```

Fund it with more MON if the balance is low.
