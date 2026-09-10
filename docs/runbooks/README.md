# Operational runbooks

Scenario-specific guides for monitoring, triaging, and resolving issues
with the payday USDC deposit gateway on AWS, settling on Monad, Base, and
Arbitrum One.

## Prerequisites

All runbooks assume you have set up your shell environment:

```bash
export AWS_PROFILE=payday
export AWS_REGION=eu-north-1
export MONAD_RPC_URL='https://your-quicknode-monad-endpoint'
export BASE_RPC_URL='https://your-quicknode-base-endpoint'
export ARBITRUM_RPC_URL='https://your-quicknode-arbitrum-endpoint'
```

The runbooks show Monad commands (`$MONAD_RPC_URL`, chain id 143). A
deposit request is bound to one chain (`chain.id` on the request); run the
same command against that chain's endpoint, and read `indexer_cursor` rows
by `chain_id`.

Some runbooks also need an account API key, `curl`, and `jq`. Load the key
from the operator's approved secret store; Terraform does not create or retain
user API keys:

```bash
export PAYDAY_API_URL="https://api.payday.sh"
export PAYDAY_API_KEY="<operator-account-api-key>"
```

## Runbooks

| Runbook | When to use |
|---------|-------------|
| [daily-monitoring.md](daily-monitoring.md) | Routine health checks and ongoing operations |
| [stuck-deposit-request.md](stuck-deposit-request.md) | A deposit request is not progressing through its lifecycle |
| [indexer-cursor-reset.md](indexer-cursor-reset.md) | The indexer is far behind or has a stale cursor |
| [indexer-fatal-halt.md](indexer-fatal-halt.md) | The indexer stopped due to a finalized reorg or permanent error |
| [service-restart.md](service-restart.md) | The API or indexer container needs a restart |
| [acm-certificate-failure.md](acm-certificate-failure.md) | ACM certificate validation fails (CAA error) |
| [quicknode-rpc-limits.md](quicknode-rpc-limits.md) | RPC provider rejects log ranges with HTTP 413 |
| [end-to-end-smoke-test.md](end-to-end-smoke-test.md) | Verifying a full deposit cycle works |
| [secrets-rotation.md](secrets-rotation.md) | Rotating an account API key or infrastructure secrets |
| [db-access.md](db-access.md) | Running SQL against the private RDS instance |
| [wrong-network-deposit.md](wrong-network-deposit.md) | A payer sent USDC to a deposit address on a network other than the one they chose |
