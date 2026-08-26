# CLAUDE.md

Guidance for running and testing the USDC payment gateway end to end.

## System overview

The API creates an invoice with a counterfactual CREATE3 payment address. The
payer transfers USDC to that address. The indexer reads finalized ranges of
`Transfer(address,address,uint256)` logs from the configured USDC contract,
attributes matching recipients in one database query, and advances invoices
from `created` to `funded` when cumulative transfers reach the requested amount.

The sweep worker submits batches to `BatchSweeper`, which independently calls
`PaymentFactory.execute` for each invoice. It deploys `Payment` at the
counterfactual address and transfers its complete USDC balance to the
beneficiary before expiration, or to the configured recovery address after
expiration. The expiration timestamp and recovery address are committed into
the counterfactual address. The lifecycle is:

`created → funded → deploying → fulfilled`, with terminal `blocked` and reserved
`failed` states.

Only the exact `GATEWAY_USDC_ADDRESS` is accepted. USDC amounts use six decimal
places. Production should configure Circle's chain-specific native USDC proxy;
the local setup deploys a mintable six-decimal fixture.

## Indexing and QuickNode

The production acquisition path uses standard EVM JSON-RPC and works with a
QuickNode HTTPS endpoint in `GATEWAY_RPC_URL`:

- one `eth_blockNumber` per poll;
- one `eth_getLogs` per bounded catch-up range, filtered to the USDC address and
  `Transfer` topic;
- block lookups to verify and persist canonical cursor hashes.

It does not download full blocks, trace calls, scan open invoices, or poll token
balances. ERC-20 logs cover transfers made by EOAs, `transferFrom`, and internal
contract calls. Observations, invoice projections, and the hash-bearing cursor
commit atomically. Partial transfers accumulate and duplicate logs are ignored.
Every transfer to a known invoice address is retained: accepted payments are
`credited`, while zero-value or late/terminal transfers are marked `error` or
`blocked` for manual review.

`GATEWAY_FINALITY_CONFIRMATIONS` controls the confirmation-depth boundary. Set a
reviewed chain-specific value in production; local Anvil uses `0`. A cursor hash
mismatch stops further range advancement. See
`docs/usdc-indexer-architecture.md` for the product architecture and the tradeoff
with QuickNode Streams.

Log ranges start at a configurable maximum (100 blocks by default), halve when
QuickNode reports an HTTP 413 or range/result-size error, and grow after a
successful response. A one-block failure is retried rather than skipped because
skipping a block could silently lose a payment. Rate limits and temporary RPC
failures retain their own retryable classification; only explicit EVM execution
reverts are treated as contract failures.

## Prerequisites

- Rust, Foundry (`anvil`, `cast`, `forge`), and PostgreSQL.
- A running PostgreSQL server and `gateway` database. Migrations run at startup.

## Local Anvil end-to-end run

The key below is Anvil's public development account #0 key. Never use it on a
real network.

### 1. Start Anvil

```bash
anvil --chain-id 31337
```

### 2. Bootstrap PaymentFactory and MockUSDC

Run against a fresh Anvil. The script is safe to repeat on the same node.

```bash
forge script foundry/script/Bootstrap.s.sol:BootstrapScript \
  --rpc-url http://127.0.0.1:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --broadcast
```

It deploys:

- `PaymentFactory`: `0x5FbDB2315678afecb367f032d93F642f64180aa3`
- `MockUSDC`: `0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512`
- `BatchSweeper`: `0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0`

Anvil accounts #0 and #1 are topped up to 1,000,000 test USDC.

### 3. Build and start the services

Ensure `.env` contains the local addresses, database URL, RPC URL, confirmation
depth, and Anvil signer key. Start `gatewayd` so it applies the schema:

```bash
cargo build --workspace
set -a; source .env; set +a
./target/debug/gatewayd
```

For a manual local run without Auth0, seed one test account in another shell:

```bash
set -a; source .env; set +a
export GATEWAY_API_KEY="$(openssl rand -hex 32)"
key_hash="$(printf %s "$GATEWAY_API_KEY" | sha256sum | awk '{print $1}')"
psql "$DATABASE_URL" -c "INSERT INTO accounts (id, api_key_hash, api_key_hint)
  VALUES ('00000000-0000-0000-0000-000000000001', decode('$key_hash', 'hex'), 'local')"
```

Use `scripts/e2e-anvil.sh` for the automated version. Production users obtain
keys through Auth0 email OTP with `gateway-cli account create`; Auth0 sends OTP
mail through Resend as documented in `docs/authentication.md`.

In another terminal:

```bash
set -a; source .env; set +a
GATEWAY_INDEXER_POLL_INTERVAL_MS=1000 ./target/debug/gateway-indexer
```

### 4. Create and pay an invoice

```bash
./target/debug/gateway-cli --json invoice create \
  --chain-id 31337 \
  --token 0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512 \
  --beneficiary 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 \
  --expiration-timestamp "$(($(date +%s) + 3600))" \
  --recovery 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC \
  --amount 1.5
```

Copy `id` and `payment_address` from the response, then transfer 1.5 USDC
(`1500000` atomic units):

```bash
cast send 0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512 \
  'transfer(address,uint256)' <payment_address> 1500000 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --rpc-url http://127.0.0.1:8545

./target/debug/gateway-cli invoice get <id>
```

The status should reach `fulfilled`. Verify the payment address was emptied and
the Payment contract was deployed:

```bash
cast call 0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512 \
  'balanceOf(address)(uint256)' <payment_address> --rpc-url http://127.0.0.1:8545
cast code <payment_address> --rpc-url http://127.0.0.1:8545
```

## Automated tests

```bash
DATABASE_URL=postgresql:///gateway?user="$USER" cargo test --workspace
cargo build -p gateway-core --bin derive-address
forge test
```

Coverage includes exact, partial, and overpayment funding; confirmation gating;
range replay idempotency; chain isolation; sweep success/recovery/retry; API
validation; CREATE3 address parity; underfunded deployment failure; and complete
overpayment sweeping.

## Configuration

- `DATABASE_URL`
- `GATEWAY_API_KEY` — CLI-only per-account bearer key for invoice requests
- `GATEWAY_AUTH0_ISSUER` — Auth0 tenant issuer for account-management JWTs
- `GATEWAY_AUTH0_AUDIENCE` — Auth0 API audience for `api.payday.sh`
- `GATEWAY_AUTH0_CLIENT_ID` — public Auth0 Native application client ID; required
  by both the API and CLI
- `GATEWAY_CHAIN_ID`
- `GATEWAY_FACTORY_ADDRESS`
- `GATEWAY_BATCH_SWEEPER_ADDRESS`
- `GATEWAY_USDC_ADDRESS` — exact Circle native-USDC proxy in production
- `GATEWAY_USDC_START_BLOCK` — USDC deployment or desired backfill block
- `GATEWAY_RPC_URL` — QuickNode HTTPS URL in production, Anvil locally
- `GATEWAY_FINALITY_CONFIRMATIONS` — defaults to 12; local setup uses 0
- `GATEWAY_LOG_RANGE_SIZE` — adaptive `eth_getLogs` range ceiling, default 100
- `GATEWAY_INDEXER_POLL_INTERVAL_MS` — default 2000
- `GATEWAY_SIGNER_KEY` — local/Anvil sweep signer; mutually exclusive with KMS
- `GATEWAY_KMS_KEY_ID` — production AWS KMS secp256k1 key ID or ARN; the worker
  uses its ambient ECS task role for `kms:GetPublicKey` and `kms:Sign`

The AWS + Monad deployment procedure is in `docs/production-runbook.md`; its
Terraform source is under `infra/`.

## Current constraints

- One configured chain and one exact USDC contract per deployment.
- Finality is a fixed confirmation depth, not the RPC `finalized` tag or
  rollup-specific L1 settlement.
- A finalized cursor hash mismatch requires operator intervention; there is no
  automatic finalized-reorg rollback.
- Sweep batches are submitted sequentially by one signer while block indexing
  runs concurrently. A mined sweep is persisted as `deploying`
  and reaches `fulfilled` only after its inclusion block passes the configured
  confirmation depth and Payment code exists at that exact canonical block.
- Only one batch transaction may remain unresolved. A transaction absent from
  both the receipt and mempool after five minutes returns to the retry queue; a
  still-pending transaction at that age halts for operator fee replacement.
- Sweep nonce ownership is limited to one active indexer by a retained
  PostgreSQL session advisory lock; a second process fails startup.
- Late USDC sent after Payment deployment can be stranded at that address.
- Recovering an expired, underfunded invoice requires a permissionless
  `PaymentFactory.execute` call; the indexer only automatically sweeps invoices
  whose finalized transfers reach the requested amount.
- `failed` is defined but unused; terminal sweep failures use `blocked`.
