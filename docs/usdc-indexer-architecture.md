# USDC deposit indexer architecture

## Decision

Support only Circle-issued native USDC and own a **confirmation-gated ERC-20 log
indexer**. Use a QuickNode standard EVM endpoint as the primary JSON-RPC
provider. Add an independent fallback provider when production availability or
cross-provider verification requirements justify it.

Do not ingest full blocks. Query contiguous block ranges with `eth_getLogs`,
filtered by the exact USDC proxy address and
`Transfer(address,address,uint256)` topic. A successful USDC transfer always
emits this log, including transfers initiated inside smart-contract calls,
`transferFrom`, and relayed authorization flows. Reverted execution leaves no
logs, so receipts and traces are unnecessary.

Detection latency comes from a push path, not from the poll cadence: a
WebSocket `monadLogs` subscription filtered to USDC transfers addressed to our
own payment addresses wakes the range scan the moment such a transfer
finalizes. The scan is the only writer and also runs on a slow timer, so the
request budget is set by the `eth_getLogs` range cap and the block rate, and
nothing else. See "Acquisition loop" for the two halves and "Build versus
QuickNode" for why the managed products lose on this chain.

## What changes from native currency

Native currency has no event for an ordinary transfer, and full blocks omit
internal calls. USDC has a standard event emitted by the token contract:

```solidity
event Transfer(address indexed from, address indexed to, uint256 value);
```

The event provides exactly the deposit request facts needed:

- `log.address`: token identity;
- `topic1`: sender;
- `topic2`: recipient deposit address;
- `data`: raw atomic amount;
- block hash/number, transaction hash/index, and log index: ordering and
  idempotency.

Consequences:

- no full block downloads;
- no traces for contract-wallet deposits;
- no transaction receipt calls;
- no recurring `balanceOf` Multicalls;
- no scans over all open deposit requests;
- multiple partial deposits and overpayment are naturally represented as
  separate immutable observations.

## Asset identity and configuration

USDC is identified by `(chain_id, proxy_address)`, never by symbol, name, or
implementation address. Circle publishes a different authoritative native-USDC
address per chain. Bridged assets such as `USDC.e` are not interchangeable with
Circle-issued native USDC.

Keep a reviewed chain/asset registry containing:

- chain ID and genesis hash;
- Circle native-USDC proxy address;
- USDC deployment/start block;
- expected decimals (`6`), verified on-chain at onboarding and startup;
- PaymentFactory address and expected code identity;
- BatchSweeper address and expected code identity;
- finality policy;
- enabled/halted state and configuration version.

USDC is upgradeable. Calls execute through a proxy and logs remain emitted from
the stable proxy address, so implementation upgrades do not require changing the
log filter. Monitor the proxy's `Upgraded` event and halt at an unreviewed upgrade
until its transfer/log invariants have been checked.

The API should either remove `token_address` from deposit request creation or require it
to equal the configured native-USDC proxy exactly. Parse and display six decimal
places while storing and comparing only integer atomic units.

Authoritative references:

- [Circle USDC contract addresses](https://developers.circle.com/stablecoins/usdc-contract-addresses)
- [Circle stablecoin EVM contracts](https://github.com/circlefin/stablecoin-evm)

## Funding predicate

Credit an observation only when:

1. The log belongs to the configured chain.
2. `log.address` exactly equals the allowlisted USDC proxy.
3. `topic0` equals `keccak256("Transfer(address,address,uint256)")`.
4. The recipient is a known deposit address for that chain and token.
5. The value is valid `uint256` data.
6. The containing block is at the configured finalized boundary.
7. The containing block's timestamp is no later than the deposit request deadline.
8. The observation has not already been recorded.

Any genuine nonzero inbound USDC credit to a `created` or `funded` deposit request,
including a mint with `from == address(0)`, is credited because the resulting
USDC is spendable by the deposit contract. A zero-value transfer is retained
with an `error` disposition. A transfer to a deposit request in any other status is
retained with a `late` disposition: it never counts toward the amount, but it
sits at the address and is queued for return to the payer through
`Deposit.recover`. A nonzero transfer from any wallet but the deposit request's
attested payer wallet is credited too, but flags the deposit request
`likely_unsolicited_at` once, at its chain time.
Every nonzero observation also records the block and transaction index of the
sweep that collected it, so the ledger always says which funds are still at the
address. If payer identity or compliance policy requires a nonzero sender, make
that an explicit product rule rather than an indexer assumption.

Use `(chain_id, token_address, tx_hash, log_index)` as the observation identity.
A transaction can emit multiple USDC transfers, so transaction hash alone is not
unique.

## Acquisition loop

Block acquisition and sweeping run as independently scheduled workers. The
deposit request table is their durable queue: the acquisition worker atomically commits
finalized observations, `funded` transitions, and `expired` transitions by
block timestamp, while the sweep worker claims eligible rows without delaying
the next log poll. It sends one BatchSweeper transaction for each claimed group
of up to 20 deposit requests; the finalized receipt's events determine each deposit request's
outcome (see "Sweep architecture under USDC").

The finality boundary is the node's `finalized` tag (minus an optional margin,
0 on Monad where the tag is irreversible without a hard fork); each pass
drains every range up to it, so catch-up throughput does not depend on the
cadence. State reads that classify outcomes pin a block number whose canonical
hash was verified first, so nothing relies on EIP-1898 block-hash parameters
being supported by the provider.

Acquisition has two halves that never trust each other:

- **The reconciler** is the only writer. It runs a pass every
  `PAYDAY_INDEXER_RECONCILE_INTERVAL_MS` (60 s in production) and immediately
  when the signal wakes it.
- **The transfer signal** (`signal.rs`) holds a WebSocket to the same node
  subscribed to `monadLogs` with the USDC address, the `Transfer` topic, and
  the current payment addresses in `topics[2]` (chunked, 500 per
  subscription; the node accepts thousands). Every matching log is delivered
  once per commit state; the `Finalized`/`Verified` deliveries record the
  block in a lock-free "highest target" cell and nudge the reconciler. A
  node without `monadLogs` (Anvil) gets the standard `logs` subscription,
  which fires at proposal. The signal keeps the socket alive with one
  `eth_chainId` every 30 s (verified against the expected chain id), reconnects
  with backoff, and re-subscribes when the watch list changes by chunk diff:
  chunks whose address set is unchanged keep their live subscription, added
  chunks subscribe before removed chunks unsubscribe (Alloy keys a
  subscription by its request, so an unchanged chunk must never be
  resubscribed and then unsubscribed), and it flips a
  health flag the reconciler reads to choose its cadence: 60 s while
  connected, `PAYDAY_INDEXER_POLL_INTERVAL_MS` while not.

For each pass:

1. Acquire a PostgreSQL advisory lock for ingestion leadership (at startup).
2. Read the `finalized` header; it is the boundary.
3. Load the durable finalized cursor and verify its hash still matches the
   provider's canonical header at that height.
4. For each bounded range up to `PAYDAY_INDEXER_MAX_RANGES_PER_TICK`: read
   the range-end header, request `eth_getLogs` for the USDC `Transfer`
   topic, sort by block number, transaction index, and log index, decode and
   validate strictly, then read the range-end header again and require the
   same hash (a change while the logs were in flight is a reorg below
   finality, reported and retried, never committed).
5. Take each transfer's block timestamp from the log's own `blockTimestamp`
   (served by Monad and Anvil; its absence is a transient error, never a
   silent zero). No header is read per transfer-bearing block.
6. Intersect unique recipients with known deposit request addresses in one
   indexed DB query; status controls projection transitions, not ledger
   retention.
7. Commit observations, projections, status changes, and cursor advancement in
   one database transaction per range.
8. If the pass was a wake whose block is not yet indexed — because finality
   has not reached it, or because the per-pass range budget stopped the pass
   short of it — re-read and pass again every 300 ms for up to 16 attempts;
   Monad finalizes within two blocks, Anvil within two seconds. The timer
   covers whatever the attempts do not.

A trust-boundary note on the range-end header pair: two reads of the same
hash prove the range end did not move while the logs were in flight, not
that each returned log belongs to that canonical ancestry. A provider that
answered with a stale or forked log page between two stable header reads
would not be rejected here; the pass trusts the finalized-RPC contract
(QuickNode's `finalized` is irreversible) and keeps the older per-block
header fan-out out of the request budget. Deep suspicion is served by the
cursor-hash and finalized-reorg halts, not by more header reads.

The watch list the signal subscribes to is every bound address whose request
is not `fulfilled`/`recovered`, every address with uncollected funds, and
every address whose request changed within `PAYDAY_INDEXER_LATE_WATCH_DAYS`.
The reconciler fingerprints that set every two seconds with one indexed
aggregate (`invoices_watch_open`, `invoices_watch_recent`,
`invoices_sweep_queue`) and loads the list only when the fingerprint moves.
The list bounds only the fast path: the reconciler fetches every USDC transfer
in a range and ledgers a late transfer to any address ever bound.

The implementation uses adaptive range sizing up to a configured ceiling (100
blocks by default). It grows the range by 25% after success and halves it for
QuickNode HTTP 413, block-range, response-size, or result-count errors. It never
treats a provider limit, timeout, malformed response, or suspicious response as
an empty range. A failure at one block is retried and alerted rather than skipped:
availability degradation is safer than silently losing a deposit request.

Why the database-side intersection is race-free without a provider-side watch
list: every block in a range was finalized before the pass read the boundary,
and a deposit request bound after that read was disclosed to its payer after
every block in the range was mined, so no transfer to it can be in the range.
The signal's list can therefore be stale at worst, which delays detection to
the next timer pass and nothing more.

The argument covers the supported flow, where the payer learns the address
only from the API response that follows the binding commit. An address is
derivable by anyone who can compute the binding inputs — the attestation
challenge exposes them — so a client that prefunds its own deposit address
before binding commits can land a transfer in a range that is scanned before
the request exists. That transfer's recipient matches no request, so it is
skipped before anything is persisted, and the cursor never rewinds to
re-read it once the binding commits. Supporting prefunded addresses needs
registration before derivation inputs are disclosed, or a dedicated
historical reconciliation path; widening the subscription list cannot fix it.

RPC errors retain method, JSON-RPC code, message, and retryability. QuickNode
429, `-32007`, `-32012`, limit, unavailable, network, and internal failures are
infrastructure errors. Contract failure is recognized only from explicit EVM
execution-revert responses (QuickNode code `3` / `-32015`, compatible Geth
`-32000` execution-reverted responses, or decodable revert data).

Error references:

- [JSON-RPC 2.0 error object](https://www.jsonrpc.org/specification#error_object)
- [QuickNode Ethereum error reference](https://www.quicknode.com/docs/ethereum/error-references)

A fallback provider improves availability by default, not correctness. Compare
finalized boundary hashes between independent providers. For higher assurance,
verify matched logs against the second provider before funding high-value
deposit requests.

### RPC filter

Filter at the provider by:

- exact USDC proxy address;
- exact `Transfer` topic0.

The reconciler fetches all USDC transfers in each range and intersects
recipients in one set-based database query; the call count is fixed by the
range cap either way, and this keeps the correctness path free of any
provider-side address list. Measured on Monad mainnet, USDC carries 16–32
transfers per 100 blocks, so the responses are small.

Only the transfer signal's subscription filters by recipient (`topics[2]`),
because notifications are billed per message; that list is a latency
optimisation with no ledger authority (see "Acquisition loop").

## Database model

### `chain_assets`

- chain ID and genesis hash;
- USDC proxy and deployment block;
- decimals;
- finality configuration;
- config version;
- running/halted state.

### `indexer_cursor`

- chain ID and token address;
- last finalized block number and hash;
- config version;
- updated timestamp.

### `payment_observations`

- chain ID and token address;
- block number/hash;
- transaction hash/index;
- log index;
- sender and recipient;
- deposit request ID;
- amount `NUMERIC(78, 0)`;
- disposition (`credited`, `error`, or `blocked`) and reason;
- observed timestamp.

Enforce uniqueness on
`(chain_id, token_address, tx_hash, log_index)`. Add a deposit request foreign key and
indexes for ledger replay by deposit request and block.

### Deposit request projection

Add:

- `confirmed_received NUMERIC(78, 0) NOT NULL DEFAULT 0`;
- `funded_at_block` and `funded_at_block_hash`;
- optionally the observation identity that crossed the funding threshold.

Use a unique `(chain_id, token_address, payment_address)` index and a partial
index over active deposit request addresses.

`payment_observations` is the audit source of truth. Deposit request totals and status are
rebuildable projections. Every log addressed to a known deposit request is retained:
only `credited` observations affect automatic funding, while `error` and
`blocked` observations form an indexed manual-review queue.

## Atomic processing

Do not hold a DB transaction across RPC calls. After a complete range is fetched
and validated, one transaction must:

1. lock and revalidate the cursor;
2. insert observations idempotently;
3. increment affected deposit request totals in event order;
4. transition `created -> funded` at the earliest observation where confirmed
   cumulative credit reaches the requested amount;
5. record the crossing block hash/number and observed cumulative amount;
6. advance the cursor to a verified range-end block hash.

The cursor advances if and only if all range effects commit. A crash before
commit replays safely; a crash after commit resumes at the next block.

Deposit request creation must continue to commit before the deposit address is disclosed
in the API response. This guarantees the deposit request can be found before any client
could include a deposit request in a later block.

## Finality and reorg policy

The implemented first version processes only logs behind a configured
confirmation depth. It has no unconfirmed projection and never sweeps from the
tip. This is portable across QuickNode-supported EVM chains, but the depth must
be reviewed for each chain and is not equivalent to economic finality.

Finality is chain-specific:

- Prefer a meaningful RPC `finalized` tag in a future chain-specific finality
  adapter where it is reliably supported.
- Do not treat `safe` as finality for an irreversible sweep.
- For an L2, define whether settlement requires sequencer confirmation, L1
  inclusion, or L1 finalization. The chain's business risk policy—not a generic
  block count—chooses the boundary.
- If the product needs faster UX, add a separate `payment_detected` provisional
  projection. It must be reversible and must not authorize sweeping.

Before each advancement, compare the stored finalized cursor hash with the
canonical provider response. A mismatch at or below the finalized cursor is an
exceptional finalized reorg or provider inconsistency: halt the chain and page an
operator. Do not automatically reverse a finalized deposit request because its funds may
already have been swept irreversibly.

Finalized-only processing avoids canonical/orphan block tables and rollback
machinery. Add those only if provisional states become a product requirement.

## Partial deposits and overpayment

Sum all finalized observations for a deposit request in canonical order. Partial
deposits across transactions or blocks accumulate. Multiple transfers in one
transaction remain distinct by log index. Fund at the first observation where
the cumulative amount reaches the requested amount.

Before expiration, the deposit request constructor requires a balance of at least the
requested amount, transfers exactly that amount to the beneficiary, and sends any
remainder back to the payer's attested wallet, so an overpayment present
before execution is neither stranded nor forwarded to the merchant. After
expiration, execution instead transfers the complete balance to that wallet
without requiring the requested amount. Both the expiration timestamp and the
recovery wallet are committed into the deterministic address; the wallet is
the one the payer attested when the address was derived, not a merchant or
platform choice. A deposit request without a bound wallet has no address and nothing
to sweep.

Factory execution is permissionless, so anyone can recover an expired partial
deposit; the indexer also does it automatically once the deposit request is `expired`,
reporting the outcome as `recovered`.

Transfers sent after the Deposit contract has executed are forwarded to the
payer's wallet by `Deposit.recover`, which anyone may call and which the
sweep worker calls automatically; they are never credited to the deposit request. The
API still describes the address as single-use so merchants do not present it
after settlement.

Every nonzero amount the payer's wallet receives back — an overpayment remainder,
an expired balance, or a late transfer — is a `recovered_funds` row keyed by
deposit request, transaction, and reason, inserted in the transaction that finalizes
the batch, so a replayed receipt cannot double-count and the ledger is never
ahead of or behind the deposit request state. A trigger raises one
`deposit_request.recovered_funds` webhook per row.

## Sweep architecture under USDC

Every deposit request with uncollected funds at its deposit address is queued,
whatever its status: `funded` (settle), `expired` (recover the balance), and
`fulfilled`/`recovered` (forward a late transfer). One helper transaction is in
flight per signer. Each exact signed transaction is persisted in its
`sweep_batches` outbox before broadcast, so a crash can only cause an
idempotent resend of the same bytes. The row owns the nonce, raw transactions,
and every hash submitted for it, so a receipt for any submission resolves the
batch. A batch
without a receipt after the pending timeout is replaced on the same nonce with
fees bumped by 12.5%, and after the configured number of submissions the sweep
worker pauses and alarms while block indexing continues. A mined nonce ahead of
the batch's nonce without a visible receipt means nothing it sent can mine any
more (Monad returns no receipt for an in-flight transaction and forgets dropped
ones), so the batch is abandoned and its deposit requests re-queued.

The finalized receipt is the single source of truth, including for reverted
transactions; no batch is released from a merely unfinalized revert. The worker
checks the receipt block's canonical hash before and after its pinned
classification reads. `BatchSweeper` deploys `DepositRequest` through the factory for
an address without code and calls
`Deposit.recover` for one that already has code; it never attempts the CREATE2
collision that a second `execute` would hit, which burns every unit of gas
forwarded to it. Per item the receipt carries one of:

- `Settled` from the deposit address: the deployment paid the beneficiary
  exactly the requested amount (`fulfilled`). An overpaid deployment also emits
  `Recovered` for the remainder in the same receipt; the parser combines the
  two regardless of event order and rejects a duplicate or conflicting pair as
  a malformed receipt;
- `Recovered` alone from the deposit address: the deployment paid the whole
  balance back to the payer's wallet after expiry (`recovered`);
- `SweepRecovered` from the helper: the contract pre-existed and `recover`
  forwarded the reported amount; an open deposit request in this position was executed
  by someone else and `Deposit.settled()` at the receipt block says how;
- `SweepFailed` from the helper: `execute` or `recover` reverted. Solady's
  CREATE3 reduces every constructor failure to `DeploymentFailed()`, so the
  revert bytes cannot classify the cause; the worker reads `paused()`,
  `isBlacklisted()` for the deposit address and its destination, `balanceOf`,
  and code presence at the receipt block instead. A paused token or an
  unclassified revert is retried behind exponential backoff up to a ceiling; a
  blacklisted destination, a balance below the credited amount, or an
  exhausted ceiling blocks the deposit request with a reason an operator can act on.

Finalization marks every nonzero observation before the receipt's exact
`(block number, transaction index)` position as collected and recomputes the
deposit request's uncollected count from the ledger. A transfer indexed later from a
position the drain already covered is recorded as collected on insert, while a
later transaction in the same block remains queued. The lag between the two
loops therefore cannot re-queue funds a finalized sweep already moved. A
drained deposit request's status changes only when it was open; late collections leave
`fulfilled`/`recovered` untouched.

Expiry is decided by chain time: each observation is classified against its
own block timestamp, independent of range boundaries. An open deposit request whose
deadline precedes the timestamp of a committed range's end block becomes
`expired` in the same commit, and its balance is recovered through the same
batch path. The API
refuses deadlines closer than ten minutes so a deposit request always has room to
settle before the contract starts routing to recovery.

Use a separate per-chain/per-signer leader lock. Persist nonce ownership and the
exact signed bytes before broadcast so replicas cannot race retries and a crash
cannot lose the only copy of an already-submitted transaction. Prefer a direct
safe ERC-20 transfer from the Deposit contract over self-approval followed by
`transferFrom`.

## Build versus QuickNode

Measured facts that decide this (Monad mainnet, September 2026):

- Monad produces a block every ~300 ms: ~288k blocks a day, ~8.6M a month.
- QuickNode bills every Monad method at 30 credits, WebSocket notifications
  per delivered message, and Streams per block processed.
- `eth_getLogs` is capped at 100 blocks per call on QuickNode's Monad
  endpoint; logs carry `blockTimestamp`; `topics[2]` accepts an OR-array.
- `finalized` trails `latest` by exactly two blocks.

### Owned reconciler plus WebSocket signal — in production

Idle baseline, about 14k calls a day and ~13M credits a month: 1,440
reconcile passes a day at the 60 s cadence (a finalized header plus a cursor
check each — 2,880 calls), 2,880 ranges × 3 calls, and 2,880 keepalives.
Payment activity adds wakes on top — each wake is a pass over
still-unindexed ranges, and a pass that a wake interrupts at a partially
filled range repeats its range-end headers — so the true spend scales weakly
with payment volume instead of being flat. It stays far under the previous
fixed-cadence poll with a header read per transfer-bearing block, which cost
~130k calls a day (~117M credits a month) at a 5 s latency, and whose header
fan-out is what tripped the requests-per-second budget. Detection latency is
one block of finality.

Benefits:

- the request budget is set by the range cap and the block rate, not by
  how fast payments must be noticed;
- provider-native filtering: one contract and one event on the scan, plus
  the recipient set on the subscription, so idle chains cost nothing extra;
- exact control over finality and failure policy; the socket has no ledger
  authority, so its outages degrade latency only;
- portable: any node with `eth_subscribe("logs")` runs the same code, and
  `monadLogs` is a per-node upgrade, not a dependency.

Costs:

- adaptive range/retry logic, cursor and provider hash checks;
- one long-lived socket to keep alive, re-subscribe, and alarm on;
- backfill throughput bounded by the 100-block cap (a day of backlog is
  2,880 ranges).

### QuickNode Streams — rejected on this chain

Streams (Logs dataset to a webhook) would outsource polling, ordering,
retries, and backfill, but it bills per block processed **regardless of
filtering**: 8.6M Monad blocks a month at 30 or more credits each is
260M+ credits, an order of magnitude above the owned path, before the
webhook receiver is built. Its reorg handling is a block delay
(`keep_distance_from_tip`), not finality, and correction restreams still
have to be applied idempotently by us. Reconsider only on a chain with a
slow block rate or if QuickNode changes Streams to bill filtered output.

### QuickNode Webhooks — a weaker version of the signal

Webhooks bill 30 credits per delivered payload, which is the same cost shape
as our subscription, but offer template filters only (no dynamic recipient
list of our own), no backfill, and no finality semantics. Everything they
could contribute, the `monadLogs` subscription already does with a tighter
filter and no inbound endpoint to secure.

### If the fast path is ever removed

Switching acquisition does not change the database ledger, event identity,
finality gate, sweep rules, or halt-on-finalized-reorg invariant: set
`PAYDAY_RPC_WS_URL=off` and the reconciler runs on
`PAYDAY_INDEXER_POLL_INTERVAL_MS` alone, at the cost of latency and calls.

## Verification and operations

Required tests:

- `transfer`, `transferFrom`, relayed/internal-call transfer detection;
- reverted and zero-value transfers ignored;
- multiple USDC logs in one transaction;
- same-block and cross-block partial deposit, exact deposit, and overpayment;
- wrong token, bridged USDC, wrong chain, and fake `Transfer` emitter ignored;
- duplicate range replay and crashes around every cursor transaction boundary;
- adaptive range shrinking, provider failover, and provider disagreement;
- deposit request creation concurrent with range ingestion;
- unreviewed USDC proxy upgrade halts ingestion;
- finalized cursor hash mismatch halts ingestion;
- USDC pause, source/beneficiary blacklist, and underfunded sweep classification;
- sweep submission crash, replacement, third-party execution, and finalization;
- full projection rebuild equals materialized deposit request totals.

Monitor:

- finalized-head and cursor lag;
- logs and ranges processed, range size, response bytes, and provider errors;
- provider hash disagreement;
- observations, funded deposit requests, partial-deposit age, and unmatched USDC logs;
- chain halted state and proxy upgrades;
- oldest funded-unswept deposit request;
- sweep nonce, receipt, and finality lag.

Hard invariants:

- only exact allowlisted native-USDC logs are credited;
- every observation is durable at most once;
- cursor and range effects commit atomically;
- only finalized cumulative credit funds a deposit request;
- only finalized funding can authorize a sweep;
- deposit addresses are disclosed only after deposit request commit;
- unreviewed asset upgrades and finalized hash mismatches halt processing;
- projections can be rebuilt from the observation ledger.
