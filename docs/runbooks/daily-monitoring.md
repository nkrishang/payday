# Daily monitoring

Routine health checks for the payment gateway. Run these proactively or
when investigating a reported issue.

## 1. Check alarm states

All six CloudWatch alarms should be in `OK` state. Any `ALARM` needs immediate
attention — see the relevant runbook.

```bash
aws cloudwatch describe-alarms --region "$AWS_REGION" \
  --query 'MetricAlarms[*].{Alarm:AlarmName,State:StateValue,Reason:StateReason}' \
  --output table
```

| Alarm | Meaning |
|-------|---------|
| `payday-unhealthy-targets` | API `/health` is failing |
| `payday-api-task-count` | API container is not running |
| `payday-indexer-task-count` | Indexer container is not running |
| `payday-indexer-fatal` | Indexer hit a permanent halt (cursor mismatch, etc.) |
| `payday-db-high-cpu` | RDS CPU > 80% for 15 min |
| `payday-db-low-storage` | RDS has < 5 GB free storage |

## 2. Check service health

```bash
aws ecs describe-services --cluster payday --services api indexer \
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
  | grep -E "WARN|ERROR|fatal|blocked|mismatch|failed"
```

Silence (no output) is normal during steady-state operation — the indexer only
logs warnings, errors, and lifecycle events. No output means no problems.

## 4. Check API logs

```bash
aws logs tail /ecs/payday/api --since 30m --region "$AWS_REGION"
```

## 5. Verify the API endpoint is reachable

```bash
curl -sf "$GATEWAY_API_URL/health" && echo " OK" || echo " FAIL"
```

## 6. Check indexer cursor lag

Compare the indexer's last-processed block against the current chain head:

```bash
# Current Monad block
cast block-number --rpc-url "$MONAD_RPC_URL"

# Indexer cursor (requires DB access — see db-access.md)
# After connecting to the DB:
#   SELECT last_block FROM indexer_cursor WHERE chain_id = 143;
```

A small lag (a few blocks) is normal. A large gap means the indexer is behind
or stuck — see [indexer-cursor-reset.md](indexer-cursor-reset.md).

## 7. Check KMS signer MON balance

The sweep signer needs MON for gas. If it runs out, sweep transactions will
fail and invoices will stall in `funded`.

```bash
# Get the signer address from indexer logs or Terraform output
SIGNER_ADDR=$(aws logs tail /ecs/payday/indexer --since 24h --region "$AWS_REGION" \
  | grep "configured sweep signer" | grep -oE '0x[0-9a-fA-F]{40}' | head -1)

cast balance "$SIGNER_ADDR" --rpc-url "$MONAD_RPC_URL"
```

Fund with more MON if the balance is low (under ~0.1 MON for a few dozen sweeps).
