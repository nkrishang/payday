# AGENTS.md — Stablecoin Gateway

> This file is the project constitution. It governs every directory in this
> repository. Read it before assisting. Update it only when a decision changes.

## Mission

This project serves two goals simultaneously. Any assistance that advances one
at the expense of the other fails the project.

1. **Build a credible, production-oriented stablecoin gateway** — a Rust +
   Solidity monorepo demonstrating real systems engineering: reliable blockchain
   indexing, crash-safe settlement, ergonomic APIs, and a high-quality test
   suite that runs locally.
2. **Teach the user to originate a substantial Rust system from scratch.** The
   user is a Solidity expert and beginner-to-intermediate at Rust (core concepts
   learned; has contributed to a large Rust codebase collaboratively, but has
   not originated one). The goal is to close that gap.

Quickly shipping AI-generated code that works would satisfy goal 1 superficially
but actively defeat goal 2. Optimize for the user's understanding and
demonstrated engineering judgment, not feature count or speed.

## How agents MUST assist

This section overrides any default agent behavior that would conflict with it.

- **The user writes all implementation and test code manually.** Agents MUST NOT
  modify tracked project files, generate patches, or provide complete
  feature/module/file implementations unless the user explicitly asks.
- Agents MAY: inspect code, review it, diagnose failures, explain concepts,
  propose designs, compare options, suggest commands, define acceptance tests,
  and write small **illustrative** snippets.
- Illustrative snippets MUST teach one concept and MUST NOT amount to a drop-in
  feature or full file. Before a snippet, explain the relevant Rust concept and
  the mental model; after it, explain why it works and what the user should
  implement next.
- Prefer a **concrete recommendation** over an unranked option dump. Expose
  material tradeoffs and let the user decide, but do not present five options
  and walk away.
- Review code the user wrote instead of replacing it wholesale. Point out what is
  good, what is risky, and what to learn from.
- Treat the user as a Solidity expert. Explain Rust ownership, traits, async,
  error handling, concurrency, and library idioms at beginner-to-intermediate
  depth. Do not assume familiarity with originating Rust project structure.
- Do not turn every interaction into a quiz. Use Socratic questions only when
  they genuinely improve understanding.
- Never conceal complexity to keep a milestone easy. Instead, separate the
  initial implementation from later hardening and explain the boundary.
- Ground every response in the two goals above.

A useful response pattern when the user faces a design or implementation problem:

1. Restate the invariant or problem.
2. Explain the relevant Rust/system concept.
3. Present realistic options with tradeoffs.
4. Recommend one and justify it against project requirements.
5. Give manual implementation steps (not code to paste).
6. Define tests and observable success criteria.
7. Offer to review what the user writes.

## Product flow

The end-to-end lifecycle of a single invoice. Architecture and component names
may change; this flow is the product.

1. A user (merchant/beneficiary) calls an HTTP endpoint on the gateway to create
   a payment invoice, specifying chain, payment token, and amount.
2. The gateway persists the invoice and returns a unique invoice ID plus the
   full payment instructions — critically, the **counterfactual CREATE2
   address** of the payment contract that does not exist on-chain yet.
3. The user (or any client) can query invoice status and perform permitted
   update actions via the API using the invoice ID.
4. A factory contract can deploy child "payment contracts" at deterministic
   addresses. The address is unique per (beneficiary, amount, token, salt).
5. Anyone — the payer, a third party, anyone — can transfer the requisite
   token amount to the not-yet-deployed deterministic address. No on-chain
   interaction with our system is required to fund it.
6. The Rust backend indexes the chain, watching for transfers to known payment
   addresses. It must handle finality, reorgs, and missed logs.
7. When the backend confirms the counterfactual address holds the required
   amount, it submits a transaction to the factory to deploy the payment
   contract at that address.
8. The payment contract's constructor atomically forwards its full token
   balance to the beneficiary. Deployment reverts if the balance is insufficient
   or the transfer fails.
9. The backend confirms the deployment transaction, marks the invoice as
   fulfilled, and exposes the updated status through the API.
10. Throughout, the CLI and HTTP API let all participants (merchant, payer,
    relayer/operator) query and act on invoice state ergonomically.

Key properties of this flow: the payment address is usable before any contract
exists there; funding and deployment are decoupled and can be performed by
different parties; the backend is a reactive observer and relayer, not a custodian
of funds; and every state transition must survive crashes, reorgs, and duplicate
execution.

## Product and protocol invariants

These are durable facts. Architecture may change; these should not.

- An invoice is scoped by **chain ID, factory, token, beneficiary, amount, and
  salt**. The Rust and Solidity implementations MUST derive the same CREATE2
  address from these.
- Amounts are **integer token base units** — never `f32`/`f64`, never floats.
- Once a payment address is issued, its economic fields (recipient, token,
  amount, chain, factory, salt) are **immutable**. Changing any creates a
  replacement invoice.
- Anyone may fund the counterfactual payment address before deployment.
- An observed transfer is not a finalized payment. Payment observation, chain
  confirmation/finality, deployment submission, deployment confirmation, and
  fulfillment are **distinct states** with distinct transitions.
- Deployment MUST revert unless the counterfactual address currently holds at
  least the required token amount. On success, the constructor forwards the full
  balance atomically and reverts if the transfer fails. Without this guard, a
  premature deployment consumes the CREATE2 address and prevents later
  forwarding.
- Chain events and jobs are processed **at-least-once**. Every state transition
  MUST be idempotent.
- The **database (PostgreSQL) is the durable source of truth**, not process
  memory.
- Expiry and cancellation are not automatically enforceable on-chain. The
  project MUST define what happens if an expired/cancelled address is funded.
- Invoice IDs MUST NOT be treated as authorization merely because they are hard
  to guess.
- Token support: the system MUST NOT silently claim to support arbitrary
  ERC-20s. Fee-on-transfer, rebasing, callback-capable, no-return, and
  malicious tokens require explicit handling or allowlisting/rejection.

## Decision status

Prevent reopening settled choices or presenting guesses as architecture.

**Accepted:**
- Rust + Axum + Tokio for the backend.
- Solidity + Foundry for contracts.
- API-first; primary UI is CLI (clap), not a web app.
- Modular monolith: one deployable initially, explicit module boundaries, split
  only when scaling/isolation demands it.
- PostgreSQL + SQLx for persistence (real Postgres, not SQLite).
- Alloy (not ethers-rs) for Ethereum RPC and contract interaction.
- `thiserror` for typed domain/application errors; `anyhow`-style context only
  for binary startup orchestration.
- Structured `tracing` for observability; never log secrets or private keys.

**Provisional (recommend as defaults, await an ADR to confirm):**
- Workspace layout (see Architecture). Extract crates only at real boundaries.

**Open (surface these; do not assume answers):**
- Supported chains and the meaning of finality on each.
- Token allowlisting policy and nonstandard-token behavior.
- Factory deployment permissions (permissionless deployment is attractive for
  liveness given immutable parameters, but must be a deliberate decision).
- Invoice authentication and authorization model.
- Expiry, cancellation, underpayment, overpayment, and late-payment semantics.
- Relayer signer and nonce management strategy.
- API versioning and OpenAPI strategy.
- Whether server and workers run in one process or separate deployables.
- Task runner choice (make/just/cargo aliases) for local commands.

When an open decision is resolved, record the rationale in `docs/adr/` and move
only the result here.

## Architecture

**Dependency direction:**

```
HTTP / CLI / DB / chain adapters  ->  application / domain logic
```

Handlers validate and translate requests, then invoke application operations.
They MUST NOT contain SQL, transaction lifecycle, or blockchain workflow logic.

**Provisional workspace:**

```
crates/
  gateway-core/   domain types, invariants, state transitions
  gatewayd/       Axum server, workers, SQLx + Alloy adapters
  gateway-cli/    HTTP client and local operator/user workflows
foundry/          Foundry project (Solidity contracts)
tests/e2e/        black-box end-to-end tests
docs/adr/         architecture decision records
```

Keep chain and storage adapters as modules inside `gatewayd` initially. Extract
additional crates only at a real dependency boundary or reuse case. Avoid
"one crate per noun" and trait abstractions for every implementation.

## Reliability and data-flow rules

These rules separate production quality from a toy. They describe **outcomes and
questions**, not mandatory patterns. Recommend the least complex design that
satisfies each.

For every state-changing operation, the agent MUST be able to answer:
- What invariant is protected?
- What if it runs twice? (idempotency)
- What if the process crashes before/after each external side effect?
- What if two instances act concurrently?
- What if the RPC result is stale or the chain reorgs?
- What is durable? What does the client observe? How is it tested?

**Invoice creation:** persist the invoice before returning a usable payment
address. Store a chain/block anchor so indexing cannot miss a payment made
immediately after the response. Support API idempotency keys. Enforce uniqueness
with database constraints, not only application checks.

**Indexing:** store canonical block number AND hash, not just height. Detect
parent/hash mismatches and handle reorgs. Deduplicate logs by chain, tx, and
log identity. Use logs as discovery signals, but reconcile against canonical
token balances and contract code. Avoid scanning every transfer of a
high-volume stablecoin without recipient filtering. Persist the index cursor;
make replay safe.

**Settlement jobs:** persist deployment work before executing it. Assume workers
can crash after every external side effect. Prevent duplicate transitions with
constraints, leases, or compare-and-set. Do NOT hold a DB transaction open
across RPC calls. Before deploying, recheck canonical balance and whether code
already exists at the child address. Detect externally-made deployments if
deployment is permissionless.

**Transaction submission:** the crash window between broadcasting and recording
a tx MUST be addressed (e.g., persist signed tx / durable nonce intent before
broadcasting). Require multi-instance-safe nonce allocation, receipt and
replacement tracking, distinction between submitted/included/reverted/replaced/
confirmed/finalized, no blind retry of state-changing calls, and bounded
retry/backoff only for classified transient failures.

**Async behavior:** no detached Tokio tasks with invisible failures. Supervise
long-running tasks; propagate fatal failures. Support cancellation and graceful
shutdown. Bound concurrency and queues. Timeout RPC and external operations.
Avoid blocking work on async executors. Avoid holding mutex guards across
`.await` unless justified.

## Rust quality conventions

Encode principles, not a style encyclopedia.

- Typed domain/application errors. Convert to stable HTTP/CLI representations at
  boundaries.
- Avoid `unwrap`/`expect`, unchecked casts, and silent defaults in runtime
  paths. Startup assertions are acceptable only for proven invariants.
- Preserve error sources; add actionable context.
- Validate external input once at the owning boundary.
- Prefer precise types and enums over strings, booleans, and illegal optional
  combinations.
- Never represent token quantities as floats.
- Keep API DTOs distinct from database rows and internal domain models.
- Add traits only at a meaningful seam, not automatically for every service.
- Recommend the simplest design that satisfies current invariant/restart/
  concurrency/failure requirements. Every new abstraction or infrastructure
  component MUST solve a named present problem. Production quality does not mean
  maximum abstraction.

## API and CLI expectations

The API MUST expose complete payment instructions, not only an opaque ID:
invoice ID, chain ID, token contract + metadata, amount as decimal string in
base units, payment address, beneficiary, expiry/policy, current observed and
finalized settlement info, and relevant transaction hashes.

- Version the public API. Use stable machine-readable error codes. Paginate
  lists. Require idempotency for invoice creation. Never expose DB rows
  directly. Authentication/tenant isolation must be deliberate.
- The CLI MUST exercise the HTTP API, not reach into the database. Support named
  local profiles (merchant, payer, relayer, operator). Provide script-friendly
  output including JSON. Keep secrets out of args, shell history, and logs. The
  full local flow must be understandable without a browser.

## Contracts and Rust integration

The Solidity and Rust sides MUST share a specification and cross-language
fixtures for: salt construction, constructor/init-code encoding, CREATE2
derivation, chain ID and factory identity, address normalization, and token
amount encoding.

The factory SHOULD emit sufficient events to reconcile deployments. Contract
behavior MUST be minimal and explicit: validate inputs, reject premature
deployment, revert atomically on failed transfer, define duplicate-deployment
behavior, and decide deliberately whether deployment is permissionless.

Threat model MUST include: malicious payers, callers racing deployment,
unsupported tokens, relayer compromise, RPC dishonesty/outage, reorgs,
duplicate execution, and database/process crashes.

## Testing and local development

Testing is central to the portfolio value. At every milestone, the user MUST be
able to run the full flow built so far locally, masquerading as all
participants.

**Test layers:**

1. **Solidity (Foundry) unit/fuzz/invariant tests** — CREATE2 parity,
   pre-funding, under/exact/excess payment, partial transfers, duplicate
   deployment, wrong inputs, failed/no-return/false-return transfers,
   unsupported token behavior, permissionless caller behavior, factory/child
   invariants.
2. **Rust unit tests** — state transitions, amount/address parsing, finality
   calculations, retry classification, API error mapping, CREATE2 fixtures
   shared with Solidity.
3. **DB integration tests (real Postgres)** — migrations from empty DB, unique
   constraints, idempotency, concurrent worker claims, rollback, cursor replay,
   duplicate event processing, restart-safe jobs.
4. **RPC/indexer integration tests** — paginated logs, duplicate/out-of-order
   responses, RPC timeouts, canonical hash mismatch/reorg, missed-log
   reconciliation, reverted/replaced/dropped deployment txs.
5. **Black-box E2E tests** — launch Postgres, Anvil, contracts, gateway, and
   CLI/API. Use distinct deterministic Anvil identities for merchant, payer,
   relayer, factory/token deployer, and an untrusted third party. Mine blocks
   explicitly; poll with bounded timeouts; avoid arbitrary sleeps. The happy
   path uses real HTTP, Postgres, Anvil, and contracts. Reserve mocks for
   targeted failure injection.

Also exercise: partial/excess payment, payment immediately after invoice
creation, restart at every workflow stage, duplicate worker execution, reorg
before finality, RPC outage/recovery, deployment revert/replacement, and funding
after expiry/cancellation.

A one-command local flow is a goal, but the task runner is an open decision.

## Definition of done (for every meaningful feature)

- [ ] Domain invariant is named.
- [ ] Happy path works locally end-to-end.
- [ ] Invalid input and authorization are addressed.
- [ ] Retry, duplicate delivery, restart, and concurrency behavior are
      understood and tested.
- [ ] DB migration/constraint implications are handled.
- [ ] Errors and structured observability exist.
- [ ] Unit/integration/E2E coverage matches the risk.
- [ ] Feature is manually testable via CLI or HTTP.
- [ ] Relevant ADRs and docs are updated.
- [ ] No secrets, panics, float amounts, or in-memory-only durable state
      introduced.
