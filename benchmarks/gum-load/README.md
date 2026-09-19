# gum-load

Gum's load-test and benchmark harness: an **entirely independent module** that
talks to Gum only as an external API user — merchant API key, payer routes,
on-chain transfers, and signed webhooks. It never touches the database, the
internal RPC, or any operator credential, in any mode. The full methodology
and metric definitions live in [docs/load-testing-plan.md](../../docs/load-testing-plan.md).

## What it measures

1. **Settlement latency (per chain):** from the moment your payment lands at
   the deposit address until the system sweeps and settles it at the payout
   destination. Headline metric: `payout_block_time − payment_block_time`,
   verified from on-chain receipts, reported as p50/p95/p99 per chain and
   currency.
2. **Capacity60:** the largest cohort of concurrent deposit requests whose
   complete lifecycle (create → pay → publicly settled) finishes inside one
   60-second window.
3. **Soak:** the sustained open-loop arrival rate the system holds for a
   configured span without growing backlog.

## Setup

```bash
cd benchmarks/gum-load
npm install          # also builds the local @gum/sdk dependency
npm run build
npm test
```

## Local mode (test the harness itself)

With the stack running (`just dev`, another shell):

```bash
export GUM_API_KEY=$(../scripts/local-api-key.sh bench@example.test)

npm run gum-load -- preflight --mode local --fund-local   # no money moves; funds payer wallets
npm run gum-load -- run --mode local --experiment latency --count 20
npm run gum-load -- run --mode local --experiment capacity60 --count 50 --repeat 3
npm run gum-load -- run --mode local --experiment soak --rate 1 --duration 5m
```

Defaults: chain `31337` (Anvil A, the Monad-shaped `finalized` path), USDC,
`0.01` per deposit, payouts recycled to the paying wallet so mock stablecoin
circulates. `--chain 31338` exercises the L2-shaped confirmations path.
`--profile attested` runs the wallet-attestation flow; `--profile pinned`
pins the chain at creation.

Webhooks: local Gum rejects loopback endpoint URLs, so to measure webhook
delivery expose the built-in receiver (port 8819) through a tunnel and pass
`--webhook https://<your-tunnel>`. Without it the harness measures via
merchant-API polling and says so in the report.

> Local timings validate the *harness*, not production: `just dev` uses
> accelerated mining and instant finality. Real numbers come from staging.

## Real mode (staging / production)

Staging and production both spend **real money and gas**. The harness starts
from deliberately tiny limits (`0.01` deposits, 30 per run, 2 concurrent)
that no CLI flag can widen — widening them is an explicit edit to the config
file, which is the approval.

```bash
export GUM_API_KEY=gum_test_...            # a dedicated benchmark merchant account
export GUM_RPC_URL_143=https://...         # your own RPC endpoint for the chain
export GUM_LOAD_PAYER_SEED='...'           # a dedicated mnemonic holding only benchmark funds
cp config/real.example.json config/staging.json   # edit chains/limits to the approved values

npm run gum-load -- preflight --config config/staging.json --chain 143
npm run gum-load -- run --config config/staging.json --experiment latency --chain 143 --count 30
```

Preflight refuses to run unless the API is healthy, the chain head advances,
payer wallets hold sufficient balances, the token allowlist matches the
deployment, and (with `--webhook`) a test delivery is received. Stopping with
`q` drains what is outstanding and writes `unresolved-funds.json` if any
funded request is left unaccounted.

## The live view

On a TTY the run renders a full-screen dashboard: phase and elapsed time,
a summary strip, the experiment panel (latency histogram with per-chain
p50/p95/p99 and a sparkline, or the capacity60 countdown and cohort
counters), a health column (chain heads and finality boundary, API calls and
rate-limits, webhook state, guard status, warnings), and a table of the
oldest outstanding lifecycles. `q` stops admission and drains; `p` pauses it.
Use `--display plain` for logs and `--display json` for pipelines.

## Artifacts

Every run writes `runs/<run-id>/`:

| file | contents |
|---|---|
| `events.ndjson` | the authoritative journal (write-ahead; every timing point) |
| `state.json` | snapshot for humans; disposable |
| `summary.json`, `report.md`, `deposits.csv` | regenerate anytime with `gum-load report runs/<id>` |
| `unresolved-funds.json` | funded lifecycles not verifiably settled — empty in clean runs |
| `private/` | payer seed, signed transactions, webhook secret (0600; never exported to reports) |

## Trust boundaries

- The merchant API key travels only to `GUM_API_URL`; payer routes and RPC
  endpoints never see it.
- Real-mode limits are load-bearing: the guard stops admission (never cancels
  funded requests) on any breach, and a stop always drains.
- Anvil development keys are rejected in real mode.
- Capacity numbers state what was tested: one cohort or one soak at one
  retained watched-address population; the report says so.
