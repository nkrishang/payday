# CLAUDE.md

Guidance for running and testing the stablecoin payment gateway end-to-end on a
local Anvil node.

> **Disclaimer:** This document describes the project **as it exists today**, mid-build.
> It is a snapshot of current behavior, not a description of the intended end
> state — features, flows, and constraints here are partial and will change.
> Treat anything under "Known gaps" as in-progress, and re-verify these
> instructions against the code before relying on them.

## What the system does

An invoice is created with a counterfactual **payment address** (derived
off-chain via CREATE3 from the factory + salt). A payer sends the native token
to that address. The **indexer** polls the chain and, when a payment address's
balance meets the invoice amount, transitions the invoice `created → funded`.
It then **sweeps** the payment: the backend signing key calls
`PaymentFactory.execute`, which CREATE3-deploys a `Payment` contract at the
payment address whose constructor forwards the funds to the beneficiary,
advancing the invoice `funded → deploying → fulfilled`.

Status lifecycle: `created → funded → deploying → fulfilled`, plus `blocked`
(terminal, needs manual intervention) and `failed` (reserved). The full
`created → … → fulfilled` sweep is implemented today; these instructions
exercise it end to end.

Components (each a crate):
- `gatewayd` — HTTP API (`POST /v1/invoices`, `GET /v1/invoices/:id`, `GET /health`).
- `gateway-indexer` — background worker; funds invoices from balances, then
  sweeps funded invoices to their beneficiaries via `PaymentFactory.execute`.
- `gateway-cli` — HTTP client for the API.
- `gateway-core` / `gateway-db` — domain types and Postgres repository.

### Sweep classification (`funded → fulfilled` / `blocked`)

`PaymentFactory.execute` reverts (`CREATE3.DeploymentFailed`) in two
indistinguishable-by-selector cases, so the indexer classifies by on-chain
reads rather than the revert:

- **Code already at the payment address** → the payment was already executed
  (our own recovered tx, or a permissionless third-party call) → `fulfilled`.
- **Reverts and no code** → the deploy can never succeed because the beneficiary
  rejects the native transfer → `blocked` (`receiver_rejected`), not retried.
- **Transient (transport) failure** → retried with exponential backoff; after a
  bounded number of attempts the invoice is `blocked` (`max_retries_exceeded`).

The `funded → deploying` claim is a compare-and-set, so a crash mid-sweep is
recovered on a later pass (re-`execute` sees the deployed code and fulfills).

## Prerequisites

- Rust toolchain (`cargo`), Foundry (`anvil`, `cast`, `forge`), Postgres (`psql`).
- A running Postgres server and a `gateway` database:
  ```bash
  createdb gateway   # once
  ```
  Migrations run automatically when `gatewayd` / the indexer / tests connect.

## Detection mechanism (why the bootstrap step exists)

The indexer reads balances by calling **`Multicall3.getEthBalance`** at the
canonical address `0xcA11bde05977b3631167028862bE2a173976CA11`, batching many
invoices into one `eth_call` and running batches concurrently. On real chains
Multicall3 is always deployed there; **a fresh Anvil is not**, so it must be
injected before the indexer can read balances. `foundry/script/Bootstrap.s.sol`
does this (and deploys `PaymentFactory` at the `.env` address).

## End-to-end run

Use four terminals (or background the long-running processes). The private key
below is Anvil's well-known **dev account #0** (`0xf39F…2266`) — a local test
key only, never a real secret.

### 1. Start Anvil

```bash
anvil --chain-id 31337
# add `--block-time 1` to auto-mine every second; otherwise Anvil mines on each tx
```

### 2. Bootstrap the chain (PaymentFactory + Multicall3)

Idempotent — safe to re-run.

```bash
forge script foundry/script/Bootstrap.s.sol:BootstrapScript \
  --rpc-url http://127.0.0.1:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --broadcast
```

Deploys `PaymentFactory` at `0x5FbDB2315678afecb367f032d93F642f64180aa3`
(matching `GATEWAY_FACTORY_ADDRESS`) and injects Multicall3 at its canonical
address.

### 3. Start `gatewayd`

The repo `.env` sets `DATABASE_URL`, `GATEWAY_CHAIN_ID=31337`,
`GATEWAY_FACTORY_ADDRESS`, and `GATEWAY_SIGNER_KEY` (the backend key that signs
sweep transactions — Anvil dev account #0). The indexer additionally needs
`GATEWAY_RPC_URL` (not in `.env`) — export it or add it.

```bash
cargo build   # once

set -a; source .env; set +a
./target/debug/gatewayd
```

Check it: `curl -s http://127.0.0.1:3000/health` → `ok`.

### 4. Start the indexer

```bash
set -a; source .env; set +a
GATEWAY_RPC_URL=http://127.0.0.1:8545 \
GATEWAY_INDEXER_POLL_INTERVAL_MS=1000 \
./target/debug/gateway-indexer
```

On startup it asserts the node's chain id matches `GATEWAY_CHAIN_ID`.

### 5. Create an invoice, pay it, watch it fund then fulfill

```bash
# Create (native token sentinel = 0xEeee…EEeE; beneficiary = Anvil account #1)
./target/debug/gateway-cli --json invoice create \
  --chain-id 31337 \
  --token 0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE \
  --beneficiary 0x70997970C51812dc3A010C7d01b50e0d17dc7C01 \
  --amount 1.5
# -> note the `id` and `payment_address`; status is "created"

# Pay the payment address on-chain
cast send <payment_address> --value 1.5ether \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --rpc-url http://127.0.0.1:8545

# The indexer logs "invoice funded", then "invoice fulfilled" once it sweeps.
./target/debug/gateway-cli invoice get <id>          # status -> "fulfilled"

# The beneficiary now holds the funds and the payment address is emptied:
cast balance 0x70997970C51812dc3A010C7d01b50e0d17dc7C01 --rpc-url http://127.0.0.1:8545
cast code <payment_address> --rpc-url http://127.0.0.1:8545   # non-empty: Payment deployed
```

Inspect internal bookkeeping directly if needed:

```bash
psql -d gateway -c \
  "SELECT status, observed_amount, funded_at_block, \
          encode(execute_tx_hash,'hex') AS execute_tx_hash, fulfilled_at_block, \
          sweep_attempts, blocked_reason \
   FROM invoices WHERE id='<id>';"
psql -d gateway -c "SELECT chain_id, last_block FROM indexer_cursor;"
```

## Scenarios

### Covered by automated tests

Indexer reconcile logic (`crates/gateway-indexer/src/indexer.rs`, `#[sqlx::test]`)
runs against a mock chain, so no Anvil is needed — only Postgres:

```bash
DATABASE_URL=postgres://<user>@localhost/gateway cargo test --workspace
```

| Scenario | Expected | Test |
|---|---|---|
| Balance == amount | `funded`, `observed_amount` recorded | `funds_invoice_when_balance_meets_amount` |
| Balance > amount (overpayment) | `funded`, observes full balance | `overpayment_still_funds` |
| Balance < amount (underpayment) | stays `created` | `leaves_invoice_created_when_underfunded` |
| Re-run after funding | no re-transition; funding block stable | `tick_is_idempotent` |
| No new block since last pass | skipped (no reconcile) | `skips_when_no_new_block` |
| Invoice on another chain (funding) | ignored by this indexer | `ignores_invoices_on_other_chains` |
| Corrupt DB row (bad amount/status/length) | typed error, not a panic | `crates/gateway-db/src/invoices.rs` |
| Sweep succeeds | `fulfilled`, tx hash + block recorded | `sweeps_funded_invoice_to_fulfilled` |
| Payment already executed on-chain | `fulfilled`, no tx of ours | `already_deployed_marks_fulfilled_without_tx` |
| Beneficiary rejects transfer | `blocked` (`receiver_rejected`) | `receiver_rejection_marks_blocked` |
| Transient sweep error | stays `deploying`, attempt counted | `transient_failure_increments_attempts_and_stays_deploying` |
| Transient errors exhaust retries | `blocked` (`max_retries_exceeded`) | `transient_failures_block_after_exhausting_retries` |
| Sweep claim is compare-and-set | claimed at most once | `claim_is_idempotent` |
| Backoff gates retry re-selection | not re-picked until backoff elapses | `backoff_gates_deploying_reselection` |
| Re-run after fulfilling | not re-swept; block stable | `fulfilled_invoice_is_not_reswept` |
| Sweep ignores created / other-chain | untouched by the sweep pass | `sweep_ignores_created_invoices`, `sweep_ignores_other_chains` |

API-level idempotency and validation live in `gatewayd`; contract/address
derivation lives in Foundry tests (`forge test`). The `execute` revert behavior
the sweep classification relies on (both failure cases surface the same
`CREATE3.DeploymentFailed`; code presence distinguishes them) is pinned by
`foundry/test/ExecuteRevert.t.sol`.

### Worth exercising manually against Anvil

The happy path plus these edges (the mock-chain tests assert the logic; running
them live validates the real Multicall + RPC path):

- **Overpayment** — send more than the amount; invoice still funds, and
  `observed_amount` reflects the actual (larger) balance.
- **Underpayment then top-up** — send part of the amount (stays `created`), then
  send the remainder; the next poll funds it.
- **Batching / concurrency** — create many invoices (e.g. 200+) and fund a
  subset; confirm only the funded ones transition, in one pass. Verify balances
  are read via `eth_call` (Multicall), not per-address `eth_getBalance`, by
  grepping the Anvil log:
  ```bash
  grep -c eth_call <anvil-log>        # multicall batches
  grep -c eth_getBalance <anvil-log>  # should stay ~0 from the indexer
  ```
- **Idempotent create** — repeat a create with the same `--idempotency-key` and
  identical fields → same invoice returned (HTTP 200); same key with *different*
  fields → `409 idempotency_conflict`.
- **Restart safety** — stop and restart the indexer after funding; it must not
  re-fund (the transition is a compare-and-set) and the cursor resumes. After a
  sweep it must not re-`execute` a `fulfilled` invoice.
- **Sweep to a rejecting beneficiary** — deploy a contract that reverts in
  `receive()` (e.g. `foundry/test/ExecuteRevert.t.sol:RejectingReceiver` via
  `forge create`), create an invoice to it, and fund it. The invoice reaches
  `blocked` (`receiver_rejected`) on the first sweep, and the funds stay at the
  payment address (no code deployed):
  ```bash
  psql -d gateway -c "SELECT status, blocked_reason FROM invoices WHERE id='<id>';"
  cast code <payment_address> --rpc-url http://127.0.0.1:8545   # 0x: deploy never succeeded
  ```

### Milestone constraints (expected rejections)

`gatewayd` currently accepts only chain `31337` and the native token. Creating an
invoice with another `--chain-id` returns `unsupported_chain`; a non-native
`--token` returns `unsupported_token`; a zero/negative `--amount` returns
`invalid_amount`.

## Known gaps (not yet testable)

- No reorg handling — the cursor stores height only, no block hash.
- No confirmation-depth gate — an invoice funds on first sufficient balance
  observation, without waiting N blocks.
- Sweeps are sent sequentially by a single signing key; there is no concurrent
  nonce management, and the `deploying` claim assumes a single indexer process
  (no cross-worker lease on in-flight rows).
- Overpayment sweeps only the invoice `amount` to the beneficiary; any excess
  stays in the deployed `Payment` contract (not refunded).
- The `blocked` reason `receiver_rejected` also covers the (normally
  unreachable) case where the payment address is underfunded at sweep time.
- `failed` is defined but unused; blocked payments use `blocked`.
- ERC-20 tokens are not supported (native only).
