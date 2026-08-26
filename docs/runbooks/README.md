# Operational runbooks

Scenario-specific guides for monitoring, triaging, and resolving issues
with the payday USDC payment gateway on AWS + Monad.

## Prerequisites

All runbooks assume you have set up your shell environment:

```bash
export AWS_PROFILE=payday
export AWS_REGION=eu-north-1
export MONAD_RPC_URL='https://your-quicknode-endpoint'
```

Some runbooks also need the API key and CLI:

```bash
export GATEWAY_API_URL="https://api.payday.sh"
export GATEWAY_API_KEY="$(aws secretsmanager get-secret-value \
  --secret-id "$(terraform -chdir=infra output -raw api_key_secret_arn)" \
  --query SecretString --output text --region "$AWS_REGION")"
```

## Runbooks

| Runbook | When to use |
|---------|-------------|
| [daily-monitoring.md](daily-monitoring.md) | Routine health checks and ongoing operations |
| [stuck-invoice.md](stuck-invoice.md) | An invoice is not progressing through its lifecycle |
| [indexer-cursor-reset.md](indexer-cursor-reset.md) | The indexer is far behind or has a stale cursor |
| [indexer-fatal-halt.md](indexer-fatal-halt.md) | The indexer stopped due to a finalized reorg or permanent error |
| [service-restart.md](service-restart.md) | The API or indexer container needs a restart |
| [acm-certificate-failure.md](acm-certificate-failure.md) | ACM certificate validation fails (CAA error) |
| [quicknode-rpc-limits.md](quicknode-rpc-limits.md) | RPC provider rejects log ranges with HTTP 413 |
| [end-to-end-smoke-test.md](end-to-end-smoke-test.md) | Verifying a full payment cycle works |
| [secrets-rotation.md](secrets-rotation.md) | Rotating the API key or other secrets |
| [db-access.md](db-access.md) | Running SQL against the private RDS instance |
