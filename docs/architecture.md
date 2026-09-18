# Backend architecture

Gum's backend is three Rust processes sharing one Postgres database, with
the responsibilities split by *what must never be confused*: who may change
a deposit request, who may hold a key, and who may only look at a chain.

```diagram
                 merchants / payers / dashboard
                              │ HTTPS
                              ▼
┌───────────────────────────────────────────────────────────────────────┐
│ gum-server                                                            │
│  public API · auth · deposit-request state machine · sweep scheduler  │
│  withdrawal orchestrator · execution-event handler · operator CLI     │
│  internal RPC listener (indexer) · /health                            │
└──────┬─────────────────────▲───────────────────────────┬──────────────┘
       │ owns                │ HTTP RPC (typed JSON,     │ publishes commands,
       │ domain tables       │ bearer token)             │ consumes events
       ▼                     │                           ▼
┌──────────────┐    ┌────────┴─────────┐        ┌─────────────────────┐
│  Postgres    │    │ gum-indexer      │        │ bus.* (in Postgres) │
│  public.*    │    │ reads chains,    │        │ execution.commands  │
│  (domain)    │    │ reports facts;   │        │ execution.events    │
│              │    │ no DB, no keys   │        │ chain.control       │
└──────────────┘    └──────────────────┘        └──────────┬──────────┘
                                                           │ consumes commands,
                                                           │ publishes events
                                                ┌──────────▼──────────┐
                                                │ gum-signers         │
                                                │ KMS/local keys,     │
                                                │ nonce lanes, submit,│
                                                │ replace, reconcile  │
                                                │ owns execution.*    │
                                                └─────────────────────┘
```

| Process | Holds | Writes | Talks to |
|---|---|---|---|
| `gum-server` | DB credentials, admin secrets | `public.*` (domain), `bus.*` (as producer/consumer) | Postgres, Relay, Iris, Privy, Resend, S3 |
| `gum-indexer` | internal bearer token | nothing durable | chain RPC (read), `gum-server` internal HTTP |
| `gum-signers` | DB credentials, **the only signing keys** | `execution.*`, `bus.*` (as producer/consumer) | Postgres, chain RPC (read + `eth_sendRawTransaction`), KMS |

The rest of this document is the reasoning: the deposit-request lifecycle,
why the boundaries sit where they do, how the services talk, what happens
at every crash boundary, how a deposit is followed through the logs, and
where it all runs.

## 1. The deposit request lifecycle

A deposit request (`invoices` internally) is the object the whole system
transforms. Its authoritative state is one row, and only `gum-server`
changes it. Everything the other two services do is *evidence* the server
turns into transitions.

```diagram
                   payer attests wallet
                   (chain, salt, address derived once)
  POST /v1/deposit-requests            finalized Transfer ≥ amount
 ┌─────────┐   bound   ┌─────────┐  reported by indexer   ┌──────────┐
 │ created │──────────▶│ created │───────────────────────▶│  funded  │
 │(unbound)│           │ (bound) │                        └────┬─────┘
 └────┬────┘           └────┬────┘                             │ scheduler opens
      │ finalized head      │ finalized head                   │ sweep job, publishes
      │ passes expiry       │ passes expiry                    │ SweepBatch command
      ▼                     ▼                                  ▼
 ┌─────────┐           ┌─────────┐                     (in job; status
 │ expired │           │ expired │                      unchanged)
 └─────────┘           └────┬────┘                             │ SweepFinalized
                            │ late transfer arrives            │ evidence per item
                            │ (address swept, funds returned)  ▼
                            ▼                     ┌───────────────────────┐
                       ┌───────────┐              │ fulfilled | recovered │
                       │ recovered │              └───────────────────────┘
                       └───────────┘
```

Internal statuses: `created`, `funded`, `fulfilled`, `recovered`,
`expired` (`invoices_status` CHECK). Two facts are deliberately **not**
statuses, because they are orthogonal to the lifecycle:

* `sweep_job_id` — the request is inside an open sweep job. A `funded`
  request in a job is still `funded`; a request in a job is never scheduled
  twice.
* `attention_reason` — automation stopped and an operator must act
  (`retries_exhausted`, `settlement_unknown`, `payment_address_blacklisted`,
  …). The lifecycle status is untouched; the release endpoint clears it.

The public vocabulary (`awaiting_deposit`, `partially_deposited`,
`deposited`, `settled`, `expired`, `returned`, `needs_attention`) is a pure
function of `(status, confirmed_received, attention_reason)` computed by
`webhook_public_status`, never stored.

Funds after the terminal statuses are normal, not exceptional: a late
transfer to a settled or expired address is observed by the indexer, bumps
`uncollected_count`, and the same scheduler sweeps it again (the helper
contract routes it to the payer's recovery wallet). `SWEEPABLE_STATUSES`
is therefore `funded | expired | fulfilled | recovered`; only `created` is
excluded, because a partial balance cannot be settled before expiry.

### Who transitions what

| Transition | Trigger | Applied by |
|---|---|---|
| issue → `created` | API request | server (`POST /v1/deposit-requests`) |
| bind wallet (chain, salt, payment address) | payer attestation | server |
| `created` → `funded`, `confirmed_received` += | `RecordFinalizedRange` from indexer | server, in the range's transaction |
| `created` → `expired` | `FinalizedHeadReport` past `expiration_timestamp` | server, in the report's transaction |
| open sweep job (`sweep_job_id`) | scheduler pass | server, same transaction as the command publish |
| `funded` → `fulfilled`/`recovered`, `expired` → `recovered`, clear job, set `drained_at_block` | `SweepFinalized` event | server, same transaction as the bus ack |
| return items to queue, count attempt, set `retries_exhausted` | `SweepAbandoned`, `Failed` item outcomes | server |
| set/clear `attention_reason` | evidence or operator | server |

`gum-ledger` is a **library**, not a service. The first sketch had it as a
process every other service RPCs into; that would have been a database
proxy with a network hop, and every "domain operation" it exposed would
have been called from exactly one place. Instead the ledger crate owns the
state machine (`gum_ledger::sweeps::apply_*`, `withdrawals`, `cursor`,
`chain_faults`) and is linked only into `gum-server`. The ownership rule
"only the server changes domain state" is enforced by credentials and
topology (indexer has no DB URL; signers' role owns `execution.*` and the
bus), not by a service in the middle.

## 2. Service boundaries and state ownership

The boundaries are drawn along three lines that must never blur:

1. **Who may mutate a deposit request.** Exactly one process, so every
   transition is a locked row update in one transaction with whatever
   justified it (a range, an event ack, an API request).
2. **Who may sign.** Exactly one process, holding KMS (or local dev keys),
   with no public listener and no domain tables in reach. A compromised
   API cannot sign; a compromised signer cannot rewrite balances.
3. **Who may only observe.** The indexer has no durable state, no database
   and no keys. It is a stateless function of (ledger cursor, chain).

| State | Owner | Where | Others |
|---|---|---|---|
| deposit requests, sweep jobs, withdrawals/legs, accounts, webhooks, proofs | `gum-server` | `public.*` | signers/indexer: never |
| per-chain indexer cursor, finalized head (chain clock), chain faults | `gum-server` | `indexer_cursor`, `chain_faults` | indexer reads via RPC; CAS on write |
| execution jobs, transaction lanes, attempts, chain halts, executor/signer health | `gum-signers` | `execution.*` | server reads `executor_status`/`signer_status` for `/v1/status` only |
| bus messages and deliveries | `gum-bus` (library) | `bus.*` | each process is a named consumer |
| signing keys | `gum-signers` | KMS / `GUM_SIGNER_KEYS` | nobody |

Rejected alternatives:

* **`gum-ledger` as a service.** See above. Also fails the "kill any one
  service" test badly: the API and the scheduler would both depend on a
  fourth process for every read.
* **Indexer writes to the DB directly.** Cheaper per range, but then two
  processes own the deposit-request row and the funded transition would
  need to be duplicated or shared through a library that both link,
  creating the distributed monolith we are avoiding.
* **Signers as a library inside the server.** Puts the keys in the
  process with the public listener. Not acceptable.
* **A separate `gum-queue` service.** A queue is infrastructure, not a
  domain. See §4.

## 3. Synchronous vs asynchronous interactions

| Interaction | Style | Why |
|---|---|---|
| indexer → server: cursor, watch list, finalized head, finalized range, fault | **sync HTTP RPC** | The indexer cannot continue its pass without the answer (which block to scan from, whether the CAS applied). Fire-and-forget would require it to keep state it must not have. |
| server → signers: `SweepBatch`, `WithdrawalStep` | **async command** | Work that takes minutes, must survive either process dying, and must run exactly once per job. |
| server → signers: `ChainControl::{Halted,Resumed}` | **async command** | A safety notice; delivery may lag but must not be lost. |
| signers → server: `SweepSubmitted`, `SweepFinalized`, `SweepAbandoned`, `WithdrawalStepFinalized`, `ExecutionStalled`, `ExecutionRejected` | **async event** | Facts. The server is the consumer today; the dashboard/alerting could subscribe later without touching the signers. |
| server → Relay / Iris / Privy / Resend / S3 | sync, external | Unchanged. |

**Commands vs events.** A command names one job (`job_id`) and has one
consumer; its deduplication key is the job id, so a job has one command,
forever. An event names something that happened; its key is
`{job_id}:{kind}[:{replacement}]`, so a redundant publish is a no-op and a
divergent one (same key, different payload) is an error the producer sees.
Both are Rust enums in `gum-contracts` (`ExecutionCommand`,
`ExecutionEvent`, `ChainControl`), serialized as tagged JSON with the
variant name stored as `kind` next to the payload so operators can query
the bus without decoding it.

**RPC shape.** Typed axum handlers on the server, a typed `reqwest` client
in the indexer, request/response structs in `gum_contracts::rpc`, errors as
`RpcError { code, message }` with `is_retryable()` decided by the code.
gRPC/protobuf was considered and rejected for five endpoints between two
Rust binaries built from one workspace: the compile-time type sharing it
would buy we already have, and JSON is greppable in logs and curl-able in
an incident. If a non-Rust consumer ever appears, `gum-contracts` is the
one crate to add a protobuf mapping to.

## 4. Messaging: the bus is Postgres

`gum-bus` is a library over three tables (`bus.subscriptions`,
`bus.messages`, `bus.deliveries`) in the same database as the domain and
execution state.

* **Publish** inserts one `bus.messages` row; a trigger fans out one
  `bus.deliveries` row per subscribed consumer and `NOTIFY`s. Publishing
  happens *inside the producer's transaction*, so the sweep job row and its
  command commit together, and the signers' resolution and its event commit
  together. This is the outbox pattern with no relay process.
* **Consume** claims a delivery with `FOR UPDATE SKIP LOCKED`, holds a
  lease, and hands the handler a transaction. The handler's own writes and
  the acknowledgement commit together (the inbox). A crash before commit
  leaves the lease to expire and the delivery to be re-claimed.
* **Dedup** is `UNIQUE (topic, deduplication_key)` plus a payload hash:
  same key + same payload is a no-op, same key + different payload is a
  bug surfaced as an error.
* **Retries** push `available_at` out with exponential backoff and jitter
  (`BackoffPolicy`); after the consumer's attempt limit the delivery is
  parked as `dead`. `gum-server bus dead` lists them, `gum-server bus retry
  <message-id>` returns one to the queue. Handlers return
  `HandlerError::Permanent` for evidence that cannot be made sense of (a
  redelivery would fail identically) and `Retryable` otherwise.
* **Versioning**: each topic carries `schema_version`; a consumer refuses
  a version above the one it compiled, leaving the delivery pending until
  the consumer is upgraded. Additive changes to a variant are serde-tolerant
  and need no bump.

Why not Redis Streams or NATS JetStream:

| Need | Postgres bus | Redis Streams | NATS JetStream |
|---|---|---|---|
| Atomic "state change + publish" | same transaction | outbox table + relay process, or accept a crash window | same as Redis |
| Atomic "consume + state change + ack" | same transaction | inbox table + idempotent handler | same |
| Durability | the database we already back up | AOF/RDB tuning, or Elasticache Multi-AZ | file store, replicas |
| Extra infrastructure | none | one more managed service, one more failure mode, one more secret | same, and less common on managed hosts |
| Throughput | thousands/s on a small RDS; our peak is tens/minute | millions/s | millions/s |
| Operator query in an incident | `psql`: join a message to its job, its deposit request, its transactions | `XRANGE` + decode | `nats stream view` |

The system's entire message rate is bounded by block production on a
handful of chains and by how many sweeps and withdrawals merchants
trigger. A broker's throughput would buy nothing, and it would cost the
one property that makes every crash boundary in §6 trivial: the domain
change and the message live in one transaction. If message volume ever
becomes a Postgres problem, the `Publisher`/`Consumer` traits in `gum-bus`
are the seam to put a broker behind; the outbox/inbox tables would stay.

## 5. Typed contracts (`gum-contracts`)

One crate holds every cross-process type, so an incompatible change is a
compile error in all three binaries.

* `rpc`: `IndexerCursor`, `WatchListQuery`/`WatchList` (fingerprinted so an
  unchanged list is not re-sent), `FinalizedHeadReport`, `PaymentObservation`,
  `RecordFinalizedRange` → `RangeApplied::{Applied, AlreadyApplied,
  CursorMismatch}`, `ChainFaultReport`, `RpcError`.
* `bus`: `ExecutionCommand::{SweepBatch, WithdrawalStep}`,
  `ExecutionEvent::{SweepSubmitted, SweepFinalized, SweepAbandoned,
  WithdrawalStepFinalized, ExecutionStalled, ExecutionRejected}`,
  `ChainControl::{Halted, Resumed}`, and the evidence vocabulary
  (`SweepItemOutcome::{Settled, Returned, LateCollected, Failed}`,
  `SweepFailureCause`, `StepResult`, `AbandonReason`). The `BusMessage`
  trait binds each enum to its topic, schema version, kind and
  deduplication key.
* `correlation`: `CorrelationId`, carried as an HTTP header on RPC and as
  a column on every bus message and execution job.

The contract's guiding rule: **signers report evidence, the server decides
policy.** `SweepItemOutcome::Failed { cause: BalanceBelowAmount }` is what
the receipt proved; whether that means "retry in 30 s", "needs attention"
or "ignore" is `gum_ledger::sweeps`. This is what keeps the signers free of
deposit-request semantics and lets the state machine be tested in one
crate with `#[sqlx::test]`.

## 6. Failure, retry, recovery and idempotency, per workflow

The rule for every workflow: durable state is the only state, every
handler is idempotent on a key the producer chose, and a restarted process
runs its next pass from the database rather than from memory.

### 6.1 Payment detection (indexer → server)

Per pass, per chain:

1. `GET cursor` — where the ledger says this chain stands.
2. Re-read that block's header. **Hash differs → finality violation**:
   `POST faults`, and the pass stops. The chain stays halted (scheduler
   schedules nothing, signers prepare nothing) until an operator runs
   `gum-server chain resume <chain>` after repairing the cursor
   (`docs/runbooks/indexer-cursor-reset.md`). The halted chain is
   re-checked every pass; the fault is re-reported and deduplicated by the
   server, so no restart is needed after the repair.
3. `POST finalized-head` — the chain clock; the server expires requests it
   has outlived in that transaction.
4. `POST watch-list` with the cached fingerprint; the list is re-sent only
   when it changed.
5. Scan `[cursor+1, finality boundary]` in `log_range_size` windows, at
   most `max_ranges` per pass; each window becomes one
   `RecordFinalizedRange { expected_cursor, end_block, end_block_hash,
   observations }`.

Crash boundaries:

* Anywhere before a range is `POST`ed: nothing changed; the next pass
  re-reads the cursor and rescans the same window.
* Request sent, response lost: the next pass rescans and sends again; the
  server answers `AlreadyApplied` (cursor already at `end_block` with the
  same hash) or `CursorMismatch` (someone advanced it further), both of
  which mean "start the pass over from the fresh cursor".
* Two indexers by mistake (a deploy overlap): the compare-and-set makes
  one of them lose every race harmlessly.
* Duplicate logs: observations are keyed `(chain_id, token_address,
  transaction_hash, log_index)` (`payment_observations` primary key);
  re-inserting is a no-op.
* RPC provider or server down: `BackoffPolicy` per chain, 1 s doubling
  with jitter up to `max(idle interval, 30 s)`; other chains are
  unaffected.

Confirmation rules are per chain in `GUM_CHAINS`:
`finality_source: finalized` (Monad-shaped) trusts the node's `finalized`
tag; `latest` + `finality_confirmations` (L2-shaped) waits N blocks. Both
are enforced in one place (`gum_chain::finality_boundary`) and used by
indexer and signers alike, so a receipt the signers act on is final by the
same definition the indexer used to call a payment funded.

### 6.2 Sweep scheduling (server)

The `invoices` table is the queue: eligible = has uncollected funds, status
in `SWEEPABLE_STATUSES`, no `sweep_job_id`, no `attention_reason`,
`next_attempt` backoff elapsed, chain not halted. `schedule_sweep_job`
locks a batch `FOR UPDATE SKIP LOCKED`, inserts the `sweep_jobs` row, sets
`sweep_job_id` on the items, publishes the command — one transaction.

The loop runs every `GUM_SWEEP_SCHEDULER_INTERVAL_MS` (5 s) and is woken
by the internal RPC when a range funds a request, so scheduling latency is
normally under a second. A crash mid-pass loses nothing: uncommitted items
are still eligible, committed jobs are on the bus. Two servers running the
scheduler is safe for the same reason (`SKIP LOCKED`).

Jobs open longer than one hour are logged at `error` each pass: the signers
reconcile their own execution, so this is an alert, not a repair.

### 6.3 Execution (signers)

Model: a **job** is one command (id = the command's job id; a redelivered
command finds its row and acks). A **transaction** is one `(chain, signer,
nonce)` slot; every same-nonce fee-bump is an **attempt** under it. One
open transaction per `(chain, signer)` is a unique index
(`execution_transactions_open_lane`), which makes nonce management
trivial: the next nonce is the signer's mined count, and a signer is
either free or working on exactly one thing.

Each chain worker pass:

1. **Reconcile** every open transaction: look up receipts for every attempt
   hash; if mined, wait for finality, verify the receipt's block hash is
   still canonical, then classify (`sweep::classify` reads each item's
   result from the receipt and pinned probes) and resolve — resolution row
   update and event publish in one transaction. If unmined past
   `pending_timeout`: if the signer's chain nonce has passed ours, the nonce
   was consumed by something that is not ours → `abandoned`
   (`AbandonReason::NonceConsumed`); else replace with higher fees, up to
   `max_submissions`, after which `ExecutionStalled` is published once and
   replacement continues.
2. **Start** queued jobs on free signers unless `execution.chain_halts` has
   the chain. Build calldata, estimate fees, **sign and persist** the raw
   transaction (`transaction_attempts`), *then* broadcast, *then* mark
   broadcast.

Crash boundaries (also documented at the top of `executor.rs`):

* Before the prepared transaction commits: nothing signed exists that a
  node could have; the job is still `queued` and starts again.
* After commit, before broadcast: the raw bytes are durable. Reconcile
  finds an attempt without `broadcast_at` and resends *exactly those
  bytes*. It never signs a second transaction for a nonce that may already
  be in a mempool.
* After broadcast, before `mark_broadcast`: same; the node answers
  "already known", which is treated as success.
* Receipt seen, crash before resolution: the receipt is looked up again.
* Resolution and event publish: one transaction; both or neither.
* Uncertain submission (timeout from `eth_sendRawTransaction`): the
  attempt stays unbroadcast in our eyes; the resend is idempotent by the
  hash; if the transaction actually mined, reconcile sees it.
* KMS throttled / RPC down before a transaction exists: `jobs.attempts`
  increments, `next_attempt_at` backs off, job stays `queued`.
* Signer key removed from the pool: its open lane is still reconciled
  (receipt lookups and the unbroadcast resend need only the persisted
  bytes) but a fee bump needs the key, so remove a signer only when
  `execution.transactions` has no open row for it. A signer whose chain
  nonce shows transactions that are not ours is skipped, not used.
* Chain halted mid-flight: in-flight lanes keep being reconciled to a
  terminal state; nothing new is prepared until `Resumed`.

Health: `execution.executor_status` (per chain: running/halted/failing +
heartbeat) and `execution.signer_status` (balance, low-balance flag) are
written each pass and read by `gum-server` for `/v1/status`. The
`GUM_SIGNER_LOW_BALANCE_WEI` warning is logged with the `signer` field
so an alarm can key on it.

### 6.4 Settlement (server consumes events)

`ExecutionEventHandler` maps each event to a `gum_ledger` `apply_*`. Each
locks the job (`sweep_jobs`) or leg row; `NotOpen` means an earlier
delivery already applied it, which is acknowledged as done. Evidence the
ledger cannot interpret (an item not in the job) is `Permanent` →
dead-letter. Everything else is `Retryable`.

Outcome policy (`gum_ledger::sweeps`):

| Evidence | Effect |
|---|---|
| `Settled` | `funded → fulfilled`, `drained_at_block`, `settled_at`, webhook trigger |
| `Returned` | `expired/funded → recovered` |
| `LateCollected { settlement: Some }` | ledger the third-party settlement, then as above |
| `LateCollected { settlement: None }` on an open item | `attention_reason = settlement_unknown` |
| `Failed { TokenPaused }` | back to queue behind backoff (transient) |
| `Failed { *Blacklisted \| BalanceBelowAmount }` | `attention_reason = <cause>`; the ledger believed the address held the amount, so a contract disagreeing is something a human must look at |
| `Failed { Unknown }` | attempt counted; `retries_exhausted` after `SweepPolicy::max_attempts`, else back to queue |
| `SweepAbandoned` | job resolved `abandoned`; items back to queue, attempt counted |
| `ExecutionRejected` | job resolved `rejected`; items back to queue, attempt counted, logged at `error` (a rejection is a bug in one of the two services, and repeated ones exhaust retries into `needs_attention`) |

### 6.5 Withdrawals

Leg states live in the ledger (`withdrawals.rs`). The orchestrator
publishes one `WithdrawalStep` per transition (`transfer`, `burn`, `mint`),
polls Circle's Iris between `burned` and `attested`, and consumes
`WithdrawalStepFinalized`. `StepResult::ExecutedElsewhere` covers the
"lost previous run" case: the signers check the authorization's
precondition before signing, and if it is already consumed they report
that instead of submitting a doomed transaction. `Reverted` returns the
leg to `authorized`/`attested` behind `StepPolicy` backoff; `Expired`
fails it.

### 6.6 Reconciliation loops (the safety net)

| Loop | Where | Compares | Repairs |
|---|---|---|---|
| lane reconcile | signers, every pass | open transactions vs chain receipts/nonces | resend, replace, abandon, resolve |
| cursor header check | indexer, every pass | cursor block hash vs chain | halt chain, alert |
| finalized head | indexer → server, every `reconcile` interval | chain clock vs unbound/unpaid requests | expire |
| watch list fingerprint | indexer, every pass | cached vs server | refresh subscriptions |
| stuck jobs | server scheduler, every pass | `sweep_jobs` open > 1 h | alert |
| bus dead letters | operator, `gum-server bus dead` | — | `bus retry` |
| readiness | every service | schema version, DB, upstream | refuse traffic until true |

There is no loop that "fixes" a deposit request from chain state directly;
the only path from chain to deposit request is indexer → range → server,
and the only path from execution to deposit request is signers → event →
server. That is what makes the state machine auditable: `bus.messages`
joined on `correlation_id` is the full causal history of a request.

## 7. Observability

`gum-telemetry` is linked by all three binaries:

* `tracing` with a JSON encoder in production (`GUM_LOG_FORMAT=json`,
  the default when stdout is not a terminal) and a pretty encoder locally.
* Every line carries `service`. Span fields are flattened into events, so
  a range span opened with `chain_id`, `from_block`, `to_block` labels every
  log inside it.
* Field names are a contract (table in `crates/gum-telemetry/src/lib.rs`):
  `correlation_id`, `request_id`, `deposit_request_id`, `payment_address`,
  `chain_id`, `tx_hash`, `signer`, `job_id`, `message_type`, `message_id`,
  `attempt`, `error`, `latency_ms`.
* `correlation_id` is minted when a deposit request or withdrawal is
  created, sent as a header on indexer RPC (`gum_telemetry::correlation`),
  stored on `sweep_jobs`, every `bus.messages` row and every
  `execution.jobs` row, and put on the consumer's span before the handler
  runs. Following one deposit end to end is `grep correlation_id=<id>`
  across the three log groups, or `SELECT * FROM bus.messages WHERE
  correlation_id = $1 ORDER BY published_at`.
* Health: `/health/live` (process up) and `/health/ready`. Readiness runs
  named `ReadinessCheck`s; server and signers register `DatabaseReady`,
  which fails while any compiled migration is unapplied. The indexer has
  no dependencies to check at runtime (it refuses to *start* until the
  server answers its cursor request and the chain's deployment verifies),
  so its readiness is "the process runs". ECS uses readiness for the ALB
  and container health checks.
* Alerts: CloudWatch metric filters on exact `tracing` messages
  (`infra/main.tf`, local `log_alarms`): `finality violation; chain
  halted`, `indexer pass failed; retrying`, `transfer signal disconnected`,
  `transaction unconfirmed after the replacement limit; fees are no longer
  raised`, `signer balance is low`, `job deferred after a transient
  failure`, `message handling failed permanently; dead-lettered`, `sweep
  job open past the stuck threshold; check gum-signers`, `deposit request
  needs operator attention`. Keying alarms on the message text keeps the
  alert definitions in one file and the code free of metric plumbing; the
  cost is that renaming one of those messages is an infra change.
* Not added: OpenTelemetry/metrics exporters. The trace shape (one span
  per pass, per range, per job, per delivery) is already there, so an OTLP
  layer can be added to `gum_telemetry::init` without touching the
  services when a collector exists to receive it.

## 8. Deployment: AWS ECS Fargate, KMS stays

Evaluated:

| | Stay on AWS (ECS Fargate + RDS + KMS) | Railway (services + Postgres) + AWS KMS | Hybrid (Railway apps, RDS + KMS on AWS) |
|---|---|---|---|
| Correctness/reliability | Multi-AZ RDS, IAM task roles for KMS (no long-lived key credentials), private subnets | Managed Postgres single-region; KMS needs an IAM user's access keys stored as Railway secrets | Cross-cloud DB latency on every transaction; two networks to secure |
| Operational simplicity | Already built and applied for staging: Terraform, Cloud Map, Secrets Manager, log groups, alarms | Simpler UI; but three containers + migrate job + secrets duplicated by hand or via Railway's API | Worst of both |
| Deploy | GH Actions → ECR → targeted `migrate` task → rolling apply | `railway up` per service | mixed |
| Debugging | CloudWatch Logs Insights across log groups by `correlation_id` | Railway log viewer, per service, weaker cross-service query | split |
| Cost | ~3 small Fargate tasks + db.t4g RDS + ALB; ALB and NAT are the fixed costs | Lower fixed cost, per-usage | similar to AWS plus egress |
| Key security | KMS via task role; signers' role is the *only* principal with `kms:Sign` | Static IAM credentials leave AWS; a Railway secret leak = signing capability | same as Railway |

Decision: **stay on AWS.** The deciding factor is the last row. The whole
point of `gum-signers` as a separate process is that signing capability is
scoped to one principal with no static credentials; moving the signers off
AWS reintroduces a long-lived `kms:Sign` credential stored in a third
party's secret store. The second factor is that the infrastructure exists
and is exercised by the staging deploy workflow; Railway would be a
rewrite of `infra/` for a smaller operational win than it first appears
(three services, a migration job and ten secrets are not much simpler in
any UI). The team-size argument cuts the other way from what one might
expect: the existing Terraform is one file, and every operational action
is either `gh workflow run` or one `aws ecs` command in the runbooks.

If the calculus changes (e.g. key management moves to a non-AWS HSM),
the services are plain containers with env-var configuration and
`/health` endpoints; nothing in them knows about ECS.

What runs (all in `infra/main.tf`):

* ECS services `api` (public ALB :8080, internal listener :8081 registered
  in Cloud Map as `api.<name>.local`), `indexer` (no DB secret, no KMS,
  reaches `http://api.<name>.local:8081`), `signers` (DB secret, the only
  task role with `kms:Sign`); one task each, `deployment_minimum_healthy_percent
  = 0` for the two workers so a deploy never runs two indexers or two
  signers against one chain for longer than the rollover.
* `migrate` one-off task definition (same image as `api`, command
  `gum-server migrate`), run by `scripts/run-migrate-task.sh` before the
  services roll. Services never migrate on start; readiness refuses while
  the schema is behind, so an out-of-order deploy fails safe.
* Secrets Manager: DB URL (api, signers), `internal_token` (api,
  indexer), external API keys (api). No secret is shared with a service
  that does not need it.
* Security groups: DB ingress from `api` and `signers` only; internal
  listener ingress from `indexer` only.
* CloudWatch log group per service; alarms as in §7.
* Rollback: redeploy the previous image tag; migrations are
  forward-only and additive within a release, so the previous binary runs
  against the newer schema (readiness checks "at least" the compiled
  version).

Local development mirrors this exactly: `just dev` runs `gum-server
migrate`, then the three binaries on 3000/3010/3011/3012 with the same
env-var names (`.env.example`). There is no docker-compose for the
services themselves; the only container locally is MinIO for attachments.

## 9. Operator runbook (short form)

| Situation | Command |
|---|---|
| Apply schema | `gum-server migrate` (locally) / `scripts/run-migrate-task.sh <env>` |
| Dead-lettered deliveries | `gum-server bus dead [limit]`, then `gum-server bus retry <message-id>` |
| Chain halted by finality violation | fix cursor per `docs/runbooks/indexer-cursor-reset.md`, then `gum-server chain resume <chain-id>` |
| Deposit request `needs_attention` | `docs/runbooks/stuck-deposit-request.md`; release endpoint clears `attention_reason` |
| Sweep job open > 1 h | `SELECT * FROM execution.transactions WHERE job_id = $1`; see `docs/runbooks/stuck-deposit-request.md` |
| Follow one deposit | `SELECT * FROM bus.messages WHERE correlation_id = $1 ORDER BY published_at`; grep the same id in the three log groups |
| Restart any service | safe at any moment; see §6 for what each resumes from |

## 10. The two litmus tests

**Kill any one service and restart it five minutes later.**

* `gum-server`: API is down for five minutes (ALB 503). The indexer's
  RPCs fail and back off; its cursor does not move; nothing is lost
  because it holds nothing. The signers keep executing and publishing;
  events queue in `bus.deliveries`. On restart: migrations are already
  applied, readiness passes, the scheduler's first pass drains the funded
  queue, the event consumer drains the backlog in order, the indexer's
  next pass resumes from the unchanged cursor.
* `gum-indexer`: no payments are detected for five minutes; the finalized
  head stops advancing so no request expires. On restart: cursor read, five
  minutes of blocks scanned in `max_ranges` windows per pass, each range
  CAS-applied. Nothing to reconstruct.
* `gum-signers`: commands queue; open lanes sit in whatever state the last
  commit left. On restart: the first pass reconciles each lane (resend
  unbroadcast bytes, look up receipts, replace if past timeout), then starts
  queued jobs. A lane that mined while the process was down is classified
  from the receipt like any other.
* Postgres: everything stops (all three readiness checks fail). On
  restart, all of the above at once. Nothing was in memory only.

**Would we choose this from scratch?** Yes, with one caveat named
explicitly: a single Postgres is both the domain store and the bus, which
is the right call at tens of messages per minute and the seam
(`gum_bus::{Publisher, Consumer}`) is where a broker would go if that ever
changes. The three-process split is the minimum that separates
"can change money", "can sign" and "can only look"; fewer would merge two
of those, more would add network hops without adding a boundary that
matters.
