# Load-testing and benchmarking plan

> Scope: how to measure Gum's two headline performance planes — per-chain
> settlement latency and deposit-request capacity — with a harness that is an
> entirely independent external API user, run first in local mode and then
> against the real stacks.

## 1. Recommended approach

Build a **standalone TypeScript benchmark package** that uses:

- `GumClient` for merchant operations.
- `GumPayerClient` for network selection and wallet attestation.
- `viem` for payer signing, ERC-20 transfers, receipts, and chain observation.
- A harness-owned HTTPS webhook receiver.
- A persistent local run journal, with offline reporting.

The harness must have **no Gum database connection, internal RPC access,
operator credentials, or service-control capability**. Starting environments,
provisioning merchant accounts, and configuring infrastructure are separate
operator tasks.

Use the same lifecycle implementation in both modes:

| Mode | Target | Purpose |
|---|---|---|
| `local` | Existing `just dev` stack and its two Anvils | Validate the complete harness, accounting, timing, recovery, and load-generation behavior |
| `prod --target staging` | `https://api.staging.gum.money` | Real-chain latency, capacity ramps, and sustained-load qualification |
| `prod --target production` | `https://api.gum.money` | Small canaries and explicitly approved, bounded load — not unbounded saturation testing |

**Treat `prod` as "real-money mode," including staging.** Require an explicit
target and API origin; never fall back to the SDK's production default.

Publish three distinct results:

1. **Settlement latency per chain:** payment inclusion → payout execution, plus
   finality and merchant-observation latency.
2. **One-minute capacity:** the largest tested cohort whose complete lifecycle
   finishes within a common 60-second window.
3. **Sustainable throughput:** the highest tested open-loop arrival rate
   satisfying latency, correctness, queue stability, and projected
   provider-budget constraints.

A finite soak cannot prove "indefinitely." Report the tested duration,
retained-address population, and long-term budget assumptions.

## 2. Contracts that affect the benchmark

These are implementation constraints for the harness, not proposed changes to
Gum.

### Supported matrix

| Environment | Chain | Currency | Finality interpretation |
|---|---|---|---|
| Real | Monad, `143` | USDC, USDT/USDT0 | `finalized` |
| Real | Base, `8453` | USDC | `latest − N`; checked-in configuration uses 10 confirmations |
| Real | Arbitrum One, `42161` | USDC, USDT/USDT0 | `latest − N`; checked-in configuration uses 40 confirmations |
| Local | `31337` | Mock USDC, mock USDT | Monad-shaped `finalized` path |
| Local | `31338` | Mock USDC | L2-shaped confirmation path |

Confirm actual deployment settings before each campaign. Checked-in Terraform
examples are not evidence of the currently deployed configuration. The
distinction between local and real environments is documented in
[staging.md](staging.md).

### Important measurement constraints

- **`deposited_at` is the payment block timestamp that crossed the funding
  threshold, not detection time.** See its writer
  (`gum-ledger/src/invoices.rs`) and public serialization
  (`gum-server/src/api/deposit_requests.rs`).
- **`settled_at` is the settlement evidence's block timestamp, not when the
  server changed status.** See the settlement writer
  (`gum-ledger/src/sweeps.rs`).
- Lifecycle webhook `occurred_at` uses those lifecycle timestamps. It is **not
  webhook creation, dispatch, or receipt time**.
- `Gum-Event-Id` identifies an event. The current envelope has **no
  per-deposit sequence number**; do not sort event IDs to infer ordering. The
  webhook trigger (`gum-schema/migrations/0001_application.sql`) establishes
  these semantics.
- `/v1/status` is an available **external** diagnostic surface. Its queue
  counts are chain-wide, not restricted to the benchmark merchant.
  `sweeper.in_flight` counts deposit requests assigned to sweep jobs, **not
  occupied nonce lanes or transactions** (`gum-server/src/api/status.rs`,
  `gum-ledger/src/sweeps.rs`).
- Merchant API traffic shares a per-account allowance: documented default
  burst 60, refill 1 request/second. Multiple keys for one account do not
  create independent allowances. Terraform also defines an IP-based WAF limit,
  default **2,000 requests per five minutes** (`docs/api-reference.md`,
  `infra/variables.tf`).

The benchmark must distinguish these admission limits from downstream
settlement capacity.

## 3. Measurement specification

### 3.1 Capture three clocks without conflating them

For every observation, retain:

1. **Harness monotonic time:** elapsed durations between observations on the
   same process.
2. **Harness UTC wall time:** correlation with server timestamps and chain
   data.
3. **Chain block number, hash, and timestamp:** authoritative execution
   position.

Run the driver and webhook receiver in the same process initially, exposed
through a public HTTPS ingress or tunnel. This gives HTTP calls, transfer
submission, chain observations, and incoming webhooks a common monotonic
clock.

Synchronize the host clock and record its synchronization status. If receiver
and driver are later separated, record their clock-offset uncertainty; never
subtract their monotonic clocks.

### 3.2 Measurement points

| Point | Exact definition |
|---|---|
| `arrival_scheduled` | Intended lifecycle start from the experiment's load schedule |
| `create_start` | Immediately before the first merchant create attempt |
| `create_complete` | Successful create response fully received and parsed |
| `address_ready` | Public create/network/attest response has supplied the bound payment address |
| `payment_submit_start` | Immediately before broadcasting the signed ERC-20 transfer |
| `payment_submit_ack` | RPC returns its transaction hash |
| `payment_inclusion` | Successful receipt containing the expected token `Transfer` into the payment address; record block/hash/timestamp/log index |
| `payment_first_seen` | First local observation of that inclusion through a receipt or subscription |
| `payment_finality_seen` | First observed configured finality boundary covering the payment block, with canonical block identity checked |
| `deposited_webhook_received` | First valid, authenticated `deposit_request.deposited` event received |
| `credited_api_seen` | First merchant response showing `received_base_units ≥ amount_base_units`, even if status has already advanced to `settled` |
| `payout_inclusion` | Successful settlement receipt containing the exact token transfer from the payment address to its requested `payout_address` |
| `payout_finality_seen` | First observed configured boundary covering that settlement block |
| `settled_webhook_received` | First valid `deposit_request.settled` webhook received |
| `settled_api_seen` | First merchant GET/list response reporting `settled` |
| `settled_detail_verified` | Full merchant GET agrees with the on-chain settlement evidence and transaction hash |

For partial-payment tests, `payment_inclusion` means the transfer that brings
the cumulative total to the requested amount. Also report time from the first
partial payment, but do not mix it into exact-payment latency.

### 3.3 Required latency metrics

| Metric | Formula | Interpretation |
|---|---|---|
| **Payment landed → payout landed** | `payout_block_timestamp − payment_block_timestamp` | Primary money-movement latency; receipt-verified and independent of client polling |
| **Observed payment → observed finalized payout** | `payout_finality_seen − payment_first_seen` | High-resolution external observation of the accepted-final settlement path |
| Payment landed → merchant sees settled | `UTC(settled_api_seen) − payment_block_timestamp` | Customer-visible estimate, including finality and API observation delay |
| Submit → settled webhook | `settled_webhook_received − payment_submit_start` | Fully external notification experience, including payer transaction inclusion |
| Submit → settled API | `settled_api_seen − payment_submit_start` | Fully external API experience |
| Create → settled API | `settled_api_seen − create_start` | Full-lifecycle latency |
| Scheduled arrival → settled API | `settled_api_seen − arrival_scheduled` | Includes harness/admission queueing; essential for capacity testing |
| Payment observed → credit visible | `credited_api_seen − payment_first_seen` | Finality + detection + server application + API observation |
| Payment observed → deposited webhook | `deposited_webhook_received − payment_first_seen` | Same general path, plus webhook delivery |
| Payout inclusion → settled notification | `UTC(settled_webhook_received) − payout_block_timestamp` | Sweep finality + server application + webhook queue/delivery |

**Report the first three together.** A fast payout execution does not imply
that Gum has accepted finality or notified the merchant.

Precision rules:

- Block timestamps and existing public lifecycle timestamps have second-level
  precision. Do not present their differences as millisecond-accurate physical
  elapsed time.
- `payment_first_seen` happens after actual inclusion. Its observer delay
  affects observation-based metrics.
- Polling produces intervals, not exact transition times. Retain the previous
  unsuccessful GET's request start and the successful GET's response
  completion as conservative observation bounds.
- For Base and Arbitrum, label finality **"Gum confirmation policy," not L1
  economic finality**.
- Do not compute a "pure sweep duration" by subtracting deposited-webhook
  receipt time from settled-webhook receipt time. Delivery queues and retries
  can distort or reverse that difference.
- Do not add stage p95s to obtain an end-to-end p95. Compute complete
  per-deposit durations first.

### 3.4 Webhook handling

The receiver must:

- Preserve raw request bytes for signature verification.
- Verify `Gum-Signature` using HMAC-SHA256 over `v1.<timestamp>.<raw body>`.
- Check timestamp freshness and agreement between header event ID/type and the
  envelope.
- Persist receipt timestamps and authenticated content before promptly
  returning 2xx.
- Deduplicate **logical events** by event ID, while retaining every delivery
  attempt.
- Track receipt per endpoint separately when testing fan-out.
- Accept out-of-order delivery; reconstruct the lifecycle from event type and
  final API state, not arrival order.

Use delivery history after the timed workload to obtain `created_at`, attempt
timestamps, durations, errors, and retry counts. These provide coarse
enqueue-to-attempt diagnostics, not exact detection/commit times.

## 4. Standalone harness design

### 4.1 Package and components

Suggested location: `benchmarks/gum-load/`, with its own package manifest,
lockfile, TypeScript configuration, README, and tests.

Use Node.js 22+, TypeScript, the public SDK package, and the repository's
currently used `viem` version, pinned in the harness lockfile. A local SDK
package dependency is acceptable; imports from backend code, web internals, or
database models are not.

| Component | Responsibility |
|---|---|
| CLI/configuration | Validate target, experiment, approved limits, chain configuration, and secret references |
| SDK transport wrapper | Capture timings, request IDs, response headers, status codes, retry attempts, and response sizes |
| Lifecycle driver | Create → network selection/attestation → transfer → observe → verify |
| Load scheduler | Synchronized cohorts and independent open-loop arrivals |
| Wallet pool | Persistent nonce ownership, payer signing, balance reservations, bounded pending transactions |
| Chain observer | Shared per-chain subscriptions/head tracking, bounded receipt reads, cached block headers |
| Webhook receiver | Signature verification, durable capture, duplicate/ordering accounting |
| Guard controller | Enforce admission, money, concurrency, RPC, duration, and health limits |
| Run journal | Harness-owned SQLite database or equivalent durable transactional journal |
| Reporter | Deterministic exports and summaries from recorded evidence |

The journal is **the harness's own state**, not access to Gum's database.

Use the SDK's injectable `fetch` to add `X-Request-Id`, capture `Retry-After`
and rate-limit headers, and set the configured checkout `Origin` on payer
writes. Never send the merchant bearer key to payer routes or blockchain RPC
endpoints.

### 4.2 Workload profiles

Keep separate profiles rather than silently changing the request flow during a
ramp:

1. **Standard checkout**
   - USDC: create unpinned, select the intended network through the payer
     route.
   - USDT: pin the supported chain at creation.
   - No verification add-ons.

2. **Wallet-attested checkout**
   - Create with `wallet_attestation: true`.
   - Call challenge, validate its chain/factory/wallet commitments, sign its
     typed data, and attest with the returned payer session.
   - Pay from that attested wallet.

3. **Pinned permissionless**
   - Address returned at creation.
   - Useful for merchant-pinned integrations, but report separately from
     checkout flows.

Baseline: exact transfers, no attachment, no email address, no Relay, no
withdrawals, and one healthy webhook endpoint. This isolates the requested
deposit gateway behavior without introducing SES, Auth0 OTP, S3, or bridge
traffic.

Use `scripts/live-smoke.sh` as the lifecycle reference, not as a subprocess
for every sample.

### 4.3 One lifecycle

1. Reserve money/gas exposure and an experiment slot.
2. Persist a unique logical operation and immutable create body.
3. Create using an idempotency key such as `bench:<run>:<sequence>`.
4. Complete the selected payer flow.
5. Validate chain, token, amount, address, payout, and sufficient remaining
   expiry.
6. Register observation correlation **before payment**. Never pre-fund a
   derivable address before its binding response.
7. Reserve the wallet nonce, sign, persist the transaction identity, then
   broadcast.
8. Observe payment, credit, payout, webhooks, and public settlement.
9. Verify successful receipts and exact payout; mark the lifecycle complete.
10. Release reservations according to observed outcomes.

Use integer base units throughout; a `0.01` transfer is `10,000` units for
these six-decimal tokens.

### 4.4 Retry and restart rules

- Retry create with the **same idempotency key and body**.
- Respect `Retry-After`; record rejected attempts and their time in
  user-visible latency.
- After ambiguous transaction submission, reconcile the known hash/nonce or
  rebroadcast the same signed transaction. **Never send another payment with a
  new nonce merely because an RPC timed out.**
- Keep signed transaction material and payer/session secrets out of report
  exports.
- On restart, resume outstanding operations; do not start a fresh load phase
  automatically.
- A timeout is not a settled payment and not a refund. Keep tracking the
  liability.
- SIGINT stops admission and new funding, then drains observations.

Do not call `PaymentFactory.execute` or a recovery function during measured
runs. That would bypass the system being benchmarked.

### 4.5 Avoid making the harness the bottleneck

- Allocate wallet nonces through one owner per `(chain, wallet)`.
- Use enough funded payer wallets to avoid serializing the workload on one
  payer nonce stream.
- Release a payer's transaction slot after payment inclusion; **do not wait
  for Gum settlement before scheduling the next lifecycle**.
- Reserve enough stablecoin working capital so recycling payouts does not turn
  an open-loop test into a closed-loop test.
- Share finality/head observation per chain rather than polling it per
  deposit.
- Observe outbound token transfers to the controlled payout-wallet pool to
  discover sweep receipts before API settlement, if fine-grained external
  finality timing is required.
- Deduplicate settlement-receipt fetches: several requests can share one sweep
  transaction.
- Record event-loop lag, scheduled-versus-actual dispatch delay, receiver
  response time, local persistence latency, and driver CPU/memory.

If scheduled arrivals cannot be dispatched, mark that load level
**generator-limited**, not a Gum capacity result.

## 5. Environment preparation and funding

### 5.1 Local mode

Operator preparation:

1. Start the existing full stack with `just dev`.
2. Provision a dedicated account using `just seed`; hand its API key to the
   harness.
3. Enable webhook delivery through the documented local environment
   configuration if not already enabled.
4. Supply a public HTTPS webhook ingress and verify it with a test event.

The harness must not invoke the account-seeding script or inherit
`DATABASE_URL`. The seed operation is an environment bootstrap exception
outside the harness.

For payer funds:

- Use a funded development payer, or derive a separate pool and transfer
  native currency and mock tokens to it from the bootstrap-funded accounts.
- Do not use the sweep-signing accounts as active benchmark payers.
- Perform funding before the measured phase.
- Check all chains and token balances through RPC.

**Local webhook delivery still requires public HTTPS.** Gum rejects private
and loopback webhook destinations. Use a tunnel/public receiver rather than
adding a security bypass. A run without actual delivered webhooks is not a
full harness validation.

`just dev` uses accelerated settings and mixed mining; local results validate
the harness, not production performance. The local runner
(`scripts/local-runner.sh`) makes this distinction important.

### 5.2 Real-money mode

Provision dedicated benchmark merchant accounts through ordinary dashboard
authentication, then supply their API keys.

Fund dedicated payer wallets on each intended mainnet with:

- Canonical USDC or USDT0 on that chain.
- MON on Monad.
- ETH on Base and Arbitrum.

Use an approved treasury transfer, exchange withdrawal, or onramp/bridge
completed before testing. **Do not assume a mainnet stablecoin faucet
exists.** Testnet faucet assets cannot benchmark the named mainnet
deployment.

Recommended default: `payout_address` equals the paying wallet, as in the
live smoke script. Stablecoin then circulates back; both payer gas and Gum
sweep gas are still spent.

Working-capital calculation:

```text
stablecoin required per chain
  ≥ amount × maximum simultaneously funded-but-not-returned deposits
    + explicit failure reserve
```

For example, 100 simultaneous `0.01` deposits require at least 1 token of
working capital before reserve. For a soak, size the reserve against the
admitted backlog limit, not average latency alone.

Estimate native funding from:

```text
planned payer transactions × conservative transaction fee
+ wallet-distribution/rebalancing fees
+ replacement reserve
+ final cleanup reserve
```

Use current chain fee estimates; do not prescribe a fixed ETH or MON amount as
permanently sufficient.

A separate controlled beneficiary should be used for a small correctness
cohort. Verify payouts by receipt logs — not just wallet balance deltas,
which become ambiguous under concurrency.

## 6. Experiment campaign

### 6.1 Acceptance invariants

Before performance numbers count:

- Each logical create maps to one deposit request.
- Each intended exact payment is sent once.
- Payment and payout use the expected chain and canonical token.
- Exactly the requested amount reaches the requested payout address.
- The settlement hash in the API matches on-chain evidence.
- The payment address is drained for an exact-payment scenario.
- No baseline request ends `returned`, `needs_attention`, or unexpectedly
  expired.
- Duplicate webhook delivery does not create duplicate logical completion.
- Every funded request remains accounted for, including timed-out and
  interrupted runs.

Receipt verification may finish after the timing window. It validates an
already captured completion timestamp; it must not retroactively replace that
timestamp with an earlier inferred API observation.

### 6.2 Harness qualification on local

Exercise both local finality paths and all three local chain/currency
combinations.

Required cases:

- Exact payment with each supported payer flow.
- Several simultaneous transfers and shared sweep transactions.
- Duplicate webhook deliveries and out-of-order capture handling.
- Merchant throttling and delayed API responses.
- Ambiguous payer submission and harness restart after broadcast.
- Spend/concurrency cap reached before an operation is funded.
- Missing webhook followed by API reconciliation.
- Load scheduler dispatching arrivals independently of completion.

Use small partial/overpayment cases to validate accounting logic, outside the
main exact-payment distributions. Broader recovery and failure tests remain
separate correctness scenarios.

### 6.3 Low-load settlement latency

Per supported real chain/currency pair:

1. Run 5–10 pilot deposits.
2. Run 20 warm-up deposits, explicitly excluded from steady-state
   percentiles.
3. Collect 100 measured samples for initial characterization.
4. For a reported p99, target **at least 1,000 measured samples per reported
   cohort**; 10,000 is preferable if budget permits.
5. Spread measurements across at least three time windows, with randomized
   spacing.

At 100 samples, a p99 is a tail order statistic with little evidence. Label it
exploratory; do not present it as a reliable service guarantee. At 1,000,
include uncertainty rather than implying precision.

Separate:

- Permissionless versus wallet-attested profiles.
- USDC versus USDT0.
- Chains.
- Low-load versus loaded measurements.
- Warm steady state versus fresh-address activation.

Do not add an artificial post-binding delay to the default lifecycle. A newly
bound address's subscription activation is part of the real experience. A
delayed-payment control cohort can help isolate that effect.

A genuinely idle-chain test requires an empty watch list. Waiting a few
minutes does not create that condition, because settled addresses remain
watched. Use a fresh isolated environment for that case.

For each distribution report p50/p95/p99, maximum, sample count, timeout
count, success rate, and a CDF. Use time-window/block-aware resampling for
confidence intervals because deposits in one sweep or chain interval are
correlated.

### 6.4 Maximum full lifecycles in one minute

Define:

> `N60` is the largest tested cohort size N for which all N lifecycles,
> scheduled at a common `T0`, are publicly observed settled by `T0 + 60
> seconds`, with exact finalized payouts subsequently verified.

Preparation may fund wallets, register webhooks, initialize clients, and warm
connections. **Do not pre-create or pre-bind the measured deposit requests.**

Procedure:

1. Start at `N = 1, 2, 4, 8, …`, subject to the approved maximum.
2. Schedule all N lifecycles at `T0`; record actual dispatch delay.
3. At 60 seconds, freeze the completion counts.
4. Continue observing and draining late completions; never abandon funded
   requests.
5. Stop exponential growth at the first failing level or guardrail.
6. Refine between the last pass and first fail.
7. Repeat candidate boundary levels at least three times.
8. Report the largest level passing all three repetitions and the first
   failing level.

Use merchant GETs or paginated merchant list responses to observe status. List
responses can amortize the observation cost; record the completion time of the
page containing each request. Fetch full details and hashes afterward.

Report separately:

- Public API settled by 60 seconds.
- Settled webhook received by 60 seconds.
- Payout executed on-chain by 60 seconds.
- Total eventual settlements and drain time.
- Peak outstanding lifecycles and peak funded-but-unsettled deposits.

Keep observation strategy constant across comparable runs.

Also distinguish:

1. **Single-merchant capacity under current public limits.**
2. **Aggregate deployment capacity using an approved pool of independent
   benchmark merchant accounts.**

For M independent accounts, burst B, refill r, and average `a` authenticated
calls per lifecycle:

```text
authenticated calls available in 60 s ≲ M × (B + 60r)
sustained lifecycle rate             ≤ (M × r − monitoring calls/s) / a
```

This is an admission bound, not a settlement-throughput prediction.

Do not evade production WAF limits through IP rotation. Higher backend-load
qualification belongs in staging with an explicitly recorded admission
configuration.

A separate "pre-bound addresses, simultaneous payments" burst is useful for
settlement-engine diagnosis, but must not be labeled full-lifecycle `N60`.

### 6.5 Sustained-rate ramp and soak

Use **open-loop arrivals**, initially deterministic with jitter and then a
seeded Poisson schedule. Completion must not control the arrival rate.

Per chain:

1. Begin well below observed limits.
2. Increase rate by roughly 1.5× between levels.
3. Hold each level for at least 15 minutes and long enough to cover several
   hundred completions where affordable.
4. After finding instability, drain, refine the last stable interval, and
   repeat.
5. Soak at 70–80% of the highest stable tested rate for at least 2 hours.
6. Run a 24-hour qualification only with separately approved spending and
   retained-state budgets.

A concurrency ceiling is an abort condition, not a silent throttle. If it is
reached, stop admission and report the level as unable to sustain the offered
load.

Track:

```text
B_lifecycle = admitted lifecycles − publicly settled lifecycles
B_payment   = included payments − verified settled payouts
B_webhook   = expected unique lifecycle deliveries − received unique deliveries
```

Do not hide failed/expired requests by subtracting them as successful
departures.

A stable level requires:

- Completion rate converges to admitted arrival rate.
- No material upward trend in outstanding count or oldest outstanding age.
- Stable indexer lag and sweep queue ages.
- No accumulating webhook-delivery backlog.
- No rate-limit saturation, signing failures, attention states, or correctness
  errors.
- Latency meets a **predeclared** per-chain SLO.

The sustained SLO is a product choice, not implied by the one-minute cohort
metric. A reasonable initial proposal is p95 ≤ 60 seconds and p99 ≤ 120
seconds for payment-to-merchant-visible settlement, subject to owner approval.

Estimate queue trends over five-minute windows, report confidence bounds and
detection sensitivity, and require recovery to baseline during drain.

Finally run mixed-chain traffic, both an equal split and the intended business
mix. Shared Postgres, CPU, KMS quotas, RPC plans, and webhook delivery mean
per-chain maxima are not additive.

## 7. Retained state and bottleneck experiments

### 7.1 Watched-address population is a first-class axis

Measure performance at approximately:

- 1–100 watched addresses.
- 500 and 501.
- 1,000 and 1,001.
- A larger population representative of the deployment's planning horizon.

Populate these only through supported external create/bind/pay flows. Unpaid
bound addresses can cheaply exercise filtering, but they do not reproduce
settled-history behavior and may persist as a different watch-list population.
Keep the cases separate.

**Draining payments does not reset watched-address count.** Record cumulative
bound requests across the campaign and any operator-supplied
deployment-wide count. Do not compare successive ramp levels as though each
began with the same retained state.

The watch-list contract (`docs/indexer-architecture.md`) retains recently
changed settled addresses for a year by default.

For long-term planning:

```text
W ≈ recently settled/bound addresses inside retention
    + older addresses still watched because they remain open/uncollected
```

At one newly settled deposit per minute, one year contains about **525,600
addresses**. On an always-active Monad chain, the idealized 100-block-window
floor is approximately:

```text
2,880 ranges/day × ceil(525,600 / 500)
≈ 3.03 million eth_getLogs calls/day
```

That excludes headers, wake-driven smaller ranges, retries, signer traffic,
and notifications. It is a planning calculation, not measured provider usage.

A fresh-database two-hour soak therefore cannot justify a year-long
sustainable-rate claim. Report:

- Capacity at the measured retained population.
- Projected capacity/cost at the intended retention horizon.
- Any retention-policy assumption needed for sustainability.

### 7.2 Bottleneck model and evidence

| Potential limit | External evidence | Experiment |
|---|---|---|
| API/WAF admission | Rate-limit headers, 429s, edge blocks, create latency | Single-account versus approved multi-account runs |
| Indexer scan budget | Independent chain head versus API cursor, credit delay, watched-population sensitivity | 500-address boundaries, sparse versus bursty payments |
| Scheduler | Credit-to-payout delay; global queued/in-flight request counts | Bursts spanning multiple batch sizes |
| Signer lanes/finality | Sweep receipt frequency, batch occupancy, signer status | Small versus full batches, per-chain comparison |
| KMS | Signer failure/detail fields, reduced submission throughput | Staging operator correlates KMS throttles separately |
| Chain gas throughput | Block inclusion delay, batch gas, competing block demand | Larger bursts with bounded payer submission rate |
| Postgres | Correlated API latency and queue growth across chains | Mixed-chain run; operator correlates DB contention separately |
| Webhooks | API settlement remains fast while delivery age rises | One versus several healthy endpoints in staging |

The scheduler schedules at most **16 batches per chain per pass**, with
default **20 requests per batch**: up to 320 requests scheduled in one pass.
This is not "320 settlements per second"; wakes, pass duration, lane
occupancy, and finality govern throughput (`gum-server/src/sweep_scheduler.rs`,
`gum-ledger/src/sweeps.rs`).

Use a planning estimate:

```text
signer-limited requests/s
  ≈ lane_count × observed_batch_occupancy / observed_lane_cycle_seconds
```

Sweep gas provisioning is currently `100,000 + 400,000 × items` — 8.1 million
gas for 20 items; cost modeling must use chain-appropriate charged fees, not
assume every chain bills identically (`gum-chain/src/lib.rs`).

For indexer requests:

```text
daily calls ≈ passes × pass_overhead
            + Σ ranges(header_reads + ceil(W_range / 500))
            + timestamp fallback reads
            + WS upkeep/notifications
            + retries
```

Include the current implementation's continuity/header checks, not only the
older two-header budget examples. Use `gum-indexer/src/indexer.rs` and
[the QuickNode runbook](runbooks/quicknode-rpc-limits.md) as planning inputs.

Backend RPC totals, KMS throttling, and database contention cannot be measured
authoritatively by this black-box harness. Operators may attach monitoring
exports afterward; the harness itself must not gain internal access.

## 8. Real-money and shared-stack guardrails

### Mandatory preflight

Refuse to fund requests unless all checks pass:

- API origin, environment, account IDs, chains, token contracts, and payout
  addresses match explicit allowlists.
- Local mode rejects mainnet chain IDs and real endpoints.
- Real-money mode rejects known Anvil development keys.
- Dedicated merchant accounts contain only approved benchmark webhook
  endpoints.
- Webhook test delivery succeeds.
- Wallet balances and reserved working capital are sufficient.
- `/v1/status` has no chain halt, signer low balance, or failing/stale
  executor.
- Independent RPC heads are progressing; compare them with API cursor
  freshness, since two stale server positions can misleadingly show zero lag.
- Maximum amount, gross transfer volume, outstanding principal, native fees,
  request count, duration, and concurrency are explicitly configured.
- The run has sufficient remaining request expiry and a drain reserve.

### Safe starting production profile

For the first approved production campaign, a conservative starting point is:

- `0.01` token per deposit.
- 30 deposits total per run.
- Two funded lifecycles outstanding globally, at most one per chain.
- At most one lifecycle admission every ten seconds.
- One healthy webhook endpoint per benchmark account.
- No automatic ramp.

These are proposed initial limits, not service capabilities. Larger campaigns
require a revised approval manifest. Production canaries cannot produce a
trustworthy p99 at this sample size.

### Cost controls

Track separately:

1. Gross stablecoin transferred.
2. Current unsettled principal exposure.
3. Payer gas.
4. Gum sweep gas, deduplicated by settlement transaction hash.
5. Wallet funding/rebalancing gas.
6. Harness RPC calls.
7. Estimated/operationally monitored backend RPC and KMS usage.

Reserve conservatively before funding, assuming a singleton sweep rather than
guaranteed batch efficiency. Maintain headroom for in-flight work and retries.

**An external harness cannot guarantee a hard cap on Gum's future sweep gas
after it has funded a request.** Gum controls fee replacement and retry
execution. The harness can enforce its own transfers and stop new admissions;
all-in guarantees require bounded server execution policies or a separately
funded isolated environment.

If that distinction is unacceptable, perform the experiment only on staging
with an operator-approved execution budget.

Use a separate payer/observer RPC allocation where possible. A second URL
sharing the same QuickNode plan does not necessarily provide independent
quota. Account for combined indexing, signing, harness reads, and WS
notifications.

### Blast-radius controls

- Dedicated merchant accounts are mandatory.
- Mark every request with `reference` and metadata such as `benchmark_run`,
  `scenario`, and `sequence`.
- Exclude benchmark account IDs explicitly from business analytics.
  **Metadata does not automatically create a test-payment exemption.**
- Omit payer/issuer email addresses from benchmark documents.
- Use only benchmark-controlled webhook destinations.
- Never send deliberate webhook failures or slow responses on production.
- Do not change finality, worker availability, rate limits, or database state
  during a production run.
- Freeze staging deployments during measured campaigns; a deployment/schema
  reset invalidates the run and may orphan the API records of funded deposits.
- Budget for ongoing watched-address RPC cost after a campaign, not only its
  active duration.

### Stop conditions and drain

Immediately stop new admissions on:

- Any payout mismatch, unexpected recovery, duplicate funding, or chain halt.
- Budget/reservation threshold reached.
- Missing health observations or a stale/failing signer.
- Unplanned provider throttling.
- Growing oldest-unsettled age beyond the approved bound.
- Production alarm or operator stop signal.

For ordinary error-rate triggers, use predeclared windows rather than reacting
to one transient network failure — for example, sustained API errors or
latency materially above the pre-run baseline.

Stopping does **not** cancel already broadcast payments. Continue collecting
outcomes, keep the receiver available, and generate an unresolved-funds
manifest.

Do not use request cancellation as a fund-control mechanism: it is
presentation-only. Expired/recovered money reaches Gum recovery custody, not
automatically the payer.

## 9. Optional system instrumentation for stage latency

**No backend change is required for the two headline benchmark planes.**
Existing API, webhook, and chain data support externally observable latency
and capacity.

However, they cannot accurately separate:

- Indexer observation from ledger credit.
- Credit application from scheduling.
- Scheduled command from signer preparation/KMS work.
- Signing from first broadcast.
- Sweep finality recognition from server status application.

If those stages are required, add a **merchant-authenticated read-only
timeline**, preferably as an additive endpoint such as:

```text
GET /v1/deposit-requests/{id}/timeline
```

The endpoint is proposed, not currently available.

Expose sanitized milestones:

| Milestone | Recording owner |
|---|---|
| Finalized payment observed | Indexer, carried with observation evidence |
| Credit applied | Server's finalized-range transaction |
| Sweep scheduled/command enqueued | Scheduler transaction |
| Execution accepted/preparation started | Signers |
| Signing started/completed | Signers |
| Broadcast attempted/acknowledged | Signers, per transaction attempt |
| Sweep finality recognized | Signers |
| Settlement applied | Server event-handler transaction |
| Webhook enqueued | Existing lifecycle-event transaction |

Requirements:

- Millisecond-or-better UTC precision, clock/source semantics documented.
- Stable deposit identity; execution attempts distinguishable across retries.
- Monotonic event sequence allocated for persisted timeline entries, if
  ordering is exposed.
- Transaction timestamps explicitly described as mutation/enqueue times — not
  falsely advertised as exact database commit timestamps.
- Service-local spans measure precise local durations; cross-service durations
  include clock uncertainty.
- No raw signed transactions, keys, RPC credentials, or unrelated merchant
  data.
- Ordinary account authorization; no benchmark-specific internal access.
- Read after completion so timeline retrieval does not perturb the timed path.
- Preserve existing timestamp meanings.
- Prefer this read-only surface over adding a webhook for every stage, which
  would itself change fan-out load.

Do not make telemetry disappear into a fallback estimate: reports must label
each stage **measured externally**, **instrumented internally and exposed
publicly**, or **unavailable**.

## 10. Data products and implementation sequence

### Required run artifacts

```text
manifest.json
requests.ndjson
deposits.csv
webhooks.ndjson
chain-observations.ndjson
status-samples.ndjson
costs.json
summary.json
report.md
unresolved-funds.json
```

The manifest should include:

- Run ID, harness/SDK versions, deployment image identifier supplied by
  operators.
- Environment and API origin.
- Chain/currency/profile mix.
- Actual finality settings and known infrastructure sizing.
- Signer count, API/WAF policy, relevant configured cadences.
- Starting retained-address population, or explicitly `unknown`.
- Arrival schedule, random seed, observation strategy, and sample policy.
- Approval limits and stop thresholds.
- Generator region, clock status, receiver placement.
- Warm-up, measurement, and drain boundaries.

Per-deposit records should include all timing points, logical ID/idempotency
key, deposit ID, addresses, exact amounts, transaction hashes, block evidence,
first event IDs, retries, terminal outcome, and verification result.

Never export credentials, payer sessions, webhook secrets, private keys,
signed raw transactions, or credential-bearing RPC URLs.

### Report layout

For each chain/currency/profile:

- Sample count, offered/admitted/completed rates, success and timeout counts.
- Primary latency metrics with p50/p95/p99 and uncertainty.
- One-minute capacity bracket and repeatability.
- Highest stable soak rate and tested duration.
- Queue depth/age and latency-over-time charts.
- Watched-address population and projected RPC budget.
- Batch occupancy, unique sweep transactions, gas and RPC costs per
  settlement.
- API-versus-webhook completion gap.
- Explicit limiting factor: admission-limited, generator-limited,
  budget-limited, or observed settlement instability.
- Outstanding funds and required operator actions.

Explorer links are drill-down conveniences; receipts and block evidence are
the measurement source.

Do not discard timeouts when computing percentiles. Report completion
distributions with right-censoring; if unresolved samples prevent identifying
p99, report a lower bound or "not established."

### Smallest complete implementation path

| Phase | Deliverable | Rough scope |
|---|---|---|
| 1 | Standalone package, explicit modes/config, SDK transport, durable journal, guard controller | L: 1–2 days |
| 2 | Payer wallet pool, public payer flows, chain evidence, signed webhook receiver, exact-payout verification | L: 1–2 days |
| 3 | Local qualification, restart/ambiguity tests, burst and open-loop schedulers | L: 1–2 days |
| 4 | Offline reporting, staging pilots, budget calibration, first measured campaign | L: 1–2 days plus runtime |
| Optional | Merchant timeline and cross-service milestone recording | XL: roughly 3–5 days |

**Completion criterion:** a single implementation runs unchanged against local
and staging, produces reproducible per-chain latency and capacity reports,
accounts for every funded request, and stops safely without requiring any
access to Gum internals. Production then uses that validated implementation
with deliberately smaller, approved limits.
