# Local development and configuration

## System overview

The API creates an invoice with a counterfactual CREATE3 payment address. The
payer transfers USDC to that address. The indexer reads finalized ranges of
`Transfer(address,address,uint256)` logs from the configured USDC contract,
attributes matching recipients in one database query, and advances invoices
from `created` to `funded` when cumulative transfers reach the requested
amount, or to `expired` when the finalized block timestamp passes the deadline.

The sweep worker submits batches to `BatchSweeper`. For an address without
code it calls `PaymentFactory.execute`, which deploys `Payment` at the
counterfactual address; the constructor pays the beneficiary before expiry or
the recovery address after it. For an address that already has code it calls
`Payment.recover`, which forwards anything that arrived later to the recovery
address. The finalized receipt decides the outcome: `Settled` → `fulfilled`,
`Recovered` → `recovered`, `SweepRecovered` → late funds collected, and
`SweepFailed` → retried or `blocked` after reading `paused()`,
`isBlacklisted()`, and `balanceOf()` on the token.

```
created → funded → deploying → fulfilled
   │                    │
   └──► expired ◄───────┘──► recovered        blocked (operator review)
```

Only the exact `PAYDAY_USDC_ADDRESS` is accepted. USDC amounts use six decimal
places. Production should configure Circle's chain-specific native USDC proxy;
the local setup deploys a mintable six-decimal fixture with the same
`paused()`/`isBlacklisted()` views.

## Indexing and finality

The acquisition path uses standard EVM JSON-RPC:

- one `eth_getBlockByNumber("finalized")` per poll to anchor the boundary
  (`PAYDAY_FINALITY_SOURCE=finalized`, minus `PAYDAY_FINALITY_CONFIRMATIONS`
  blocks of margin), or `eth_blockNumber` with `PAYDAY_FINALITY_SOURCE=latest`;
- one `eth_getLogs` per bounded range, filtered to the USDC address and
  `Transfer` topic, draining up to `PAYDAY_INDEXER_MAX_RANGES_PER_TICK` ranges
  per pass so a backlog clears independently of the poll interval;
- header lookups to verify and persist canonical cursor hashes, plus one lookup
  for each distinct transfer-bearing block to classify payments by that
  block's timestamp. These lookups run concurrently within each bounded range.

Observations, invoice projections, and the hash-bearing cursor commit
atomically. Every transfer to a known invoice address is retained: `credited`
transfers count toward the amount, `late` transfers (after settlement, expiry,
or a block) are queued for recovery, and zero-value transfers are `error`.
Each observation records the block and transaction index of the sweep that
collected it, so a lagging cursor can never re-queue funds a finalized sweep
already moved, including when payment and sweep transactions share a block.

Log ranges start at `PAYDAY_LOG_RANGE_SIZE` (100 by default, QuickNode's cap
on Monad), halve when the provider reports a range/result-size error, and grow
after a successful response. A one-block failure is retried rather than
skipped.

## Prerequisites

- Rust, Foundry (`anvil`, `cast`, `forge`), PostgreSQL, and `jq`.
- A running PostgreSQL server and `gateway` database. Migrations run at startup.

## Local Anvil end-to-end run

`scripts/e2e-anvil.sh` runs the complete flow (exact, partial, overpaid, and
batched payments; late transfers; third-party execution; a paused token; a
blacklisted beneficiary and its operator release; an expired partial payment
recovered automatically and completed late) against a fresh Anvil started with
`--slots-in-an-epoch 1 --mixed-mining --block-time 1`, which makes the node's
`finalized` tag advance like a real chain. Use it whenever the contracts or
the worker change:

```bash
cargo build --workspace
DATABASE_URL=postgres:///gateway_e2e scripts/e2e-anvil.sh
```

For a manual session, the key below is Anvil's public development account #0
key. Never use it on a real network.

### 1. Start Anvil

```bash
anvil --chain-id 31337 --slots-in-an-epoch 1 --mixed-mining --block-time 1
```

### 2. Bootstrap PaymentFactory and MockUSDC

Run against a fresh Anvil. The script is safe to repeat on the same node, but
it will not redeploy changed contract code over existing addresses: restart
Anvil after editing a contract.

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

Ensure `.env` contains the local addresses, database URL, RPC URL, finality
settings, start block, and Anvil signer key. Start `gatewayd` so it applies the
schema:

```bash
cargo build --workspace
set -a; source .env; set +a
./target/debug/gatewayd
```

For a manual local run without Auth0, seed one test account in another shell:

```bash
set -a; source .env; set +a
export PAYDAY_API_KEY="$(openssl rand -hex 32)"
key_hash="$(printf %s "$PAYDAY_API_KEY" | shasum -a 256 | awk '{print $1}')"
psql "$DATABASE_URL" -c "INSERT INTO accounts (id, api_key_hash, api_key_hint)
  VALUES ('00000000-0000-0000-0000-000000000001', decode('$key_hash', 'hex'), 'local')"
```

Production users obtain keys through Auth0 email OTP with
`payday account create`; see `docs/authentication.md`.

In another terminal:

```bash
set -a; source .env; set +a
PAYDAY_INDEXER_POLL_INTERVAL_MS=1000 ./target/debug/gateway-indexer
```

### 4. Create a payment

Expirations must be at least ten minutes and at most a year ahead.

```bash
./target/debug/payday --json create \
  --chain-id 31337 \
  --token 0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512 \
  --payout 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 \
  --expires-in 3600 \
  --refund 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC \
  --amount 1.5
```

Copy `id` and `address` from the response, then transfer 1.5 USDC
(`1500000` atomic units):

The `self_settlement` object contains the factory and salt needed for anyone
to settle the payment on-chain if Payday is unavailable.

```bash
cast send 0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512 \
  'transfer(address,uint256)' <payment_address> 1500000 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --rpc-url http://127.0.0.1:8545

./target/debug/payday get <id>
```

The status should reach `settled` with `settlement_tx_hash` set. Verify the
payment address was emptied and the Payment contract was deployed:

```bash
cast call 0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512 \
  'balanceOf(address)(uint256)' <payment_address> --rpc-url http://127.0.0.1:8545
cast call <payment_address> 'settled()(bool)' --rpc-url http://127.0.0.1:8545
```

## Automated tests

```bash
DATABASE_URL=postgresql:///gateway?user="$USER" cargo test --workspace
cargo build -p gateway-core --bin derive-address
forge test
```

Coverage includes exact, partial, and overpayment funding; finality-tag and
confirmation gating; multi-range draining; range replay idempotency; chain
isolation; expiry by block timestamp; settlement, recovery, third-party
execution, and late-fund collection through finalized receipts; failure
classification (paused, blacklisted, underfunded, unknown); same-nonce fee
replacement, abandoned nonces, and the paused sweep worker; API validation;
CREATE3 address parity; and `BatchSweeper` under the production gas budget.

## Configuration

- `DATABASE_URL`
- `PAYDAY_API_KEY` — CLI-only per-account bearer key for invoice requests
- `PAYDAY_AUTH0_ISSUER`, `PAYDAY_AUTH0_AUDIENCE`, `PAYDAY_AUTH0_CLIENT_ID` —
  Auth0 account-management settings (see `docs/authentication.md`)
- `PAYDAY_CHAIN_ID`
- `PAYDAY_FACTORY_ADDRESS`
- `PAYDAY_BATCH_SWEEPER_ADDRESS`
- `PAYDAY_USDC_ADDRESS` — exact Circle native-USDC proxy in production
- `PAYDAY_USDC_START_BLOCK` — required; the block to start indexing from on a
  fresh database (the current block at first deployment)
- `PAYDAY_RPC_URL` — QuickNode HTTPS URL in production, Anvil locally
- `PAYDAY_FINALITY_SOURCE` — `finalized` (default; the node's finalized tag)
  or `latest`
- `PAYDAY_FINALITY_CONFIRMATIONS` — blocks subtracted from the finality
  anchor; defaults to 2, local Anvil uses 0
- `PAYDAY_LOG_RANGE_SIZE` — adaptive `eth_getLogs` range ceiling, default 100
- `PAYDAY_INDEXER_MAX_RANGES_PER_TICK` — ranges drained per pass, default 20
- `PAYDAY_INDEXER_POLL_INTERVAL_MS` — idle interval for both loops, default 2000
- `PAYDAY_SWEEP_PENDING_TIMEOUT_SECS` — seconds without a receipt before a
  helper transaction is replaced on the same nonce, default 60
- `PAYDAY_SWEEP_MAX_SUBMISSIONS` — replacements before the sweep worker
  pauses and alarms, default 5
- `PAYDAY_SWEEP_MAX_ATTEMPTS` — unclassified item failures before an invoice
  is `blocked`, default 8
- `PAYDAY_SIGNER_LOW_BALANCE_WEI` — threshold for the low-balance warning,
  default 0.05 native tokens
- `PAYDAY_SIGNER_KEY` — local/Anvil sweep signer; mutually exclusive with KMS
- `PAYDAY_KMS_KEY_ID` — production AWS KMS secp256k1 key ID or ARN; the worker
  uses its ambient ECS task role for `kms:GetPublicKey` and `kms:Sign`

The AWS + Monad deployment procedure is in `docs/production-runbook.md`; its
Terraform source is under `infra/`.

## Current constraints

- One configured chain and one exact USDC contract per deployment.
- A finalized cursor hash mismatch requires operator intervention; there is no
  automatic finalized-reorg rollback.
- One helper transaction is in flight at a time; replacements share its nonce.
  After `PAYDAY_SWEEP_MAX_SUBMISSIONS` unconfirmed submissions the sweep
  worker pauses and alarms while block indexing continues.
- `blocked` invoices are released by an operator (`docs/runbooks/stuck-invoice.md`);
  the worker never retries them on its own.
- The API has no list endpoint; merchants track invoice IDs themselves.
