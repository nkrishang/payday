# USDC payment indexer architecture

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

This makes the owned acquisition path small enough that QuickNode Streams does
not currently justify its vendor coupling or per-block cost. Streams would
outsource polling, retries, ordering, and backfill, but not the business-critical
work: finality policy, idempotent accounting, hash verification, projection
updates, correction handling, and irreversible sweep safety.

Use QuickNode Streams instead when multi-chain scale, pathological provider log
limits, sustained backfill lag, or indexer on-call cost becomes material. If that
happens, preserve the same durable ledger and finality gate.

## What changes from native currency

Native currency has no event for an ordinary transfer, and full blocks omit
internal calls. USDC has a standard event emitted by the token contract:

```solidity
event Transfer(address indexed from, address indexed to, uint256 value);
```

The event provides exactly the payment facts needed:

- `log.address`: token identity;
- `topic1`: sender;
- `topic2`: recipient payment address;
- `data`: raw atomic amount;
- block hash/number, transaction hash/index, and log index: ordering and
  idempotency.

Consequences:

- no full block downloads;
- no traces for contract-wallet payments;
- no transaction receipt calls;
- no recurring `balanceOf` Multicalls;
- no scans over all open invoices;
- multiple partial payments and overpayment are naturally represented as
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
- finality policy;
- enabled/halted state and configuration version.

USDC is upgradeable. Calls execute through a proxy and logs remain emitted from
the stable proxy address, so implementation upgrades do not require changing the
log filter. Monitor the proxy's `Upgraded` event and halt at an unreviewed upgrade
until its transfer/log invariants have been checked.

The API should either remove `token_address` from invoice creation or require it
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
4. The recipient is an active payment address for that chain and token.
5. The value is nonzero and valid `uint256` data.
6. The containing block is at the configured finalized boundary.
7. The observation has not already been recorded.

The recommended business policy is to count any genuine nonzero inbound USDC
credit, including a mint with `from == address(0)`, because the resulting USDC is
spendable by the payment contract. If payer identity or compliance policy
requires a nonzero sender, make that an explicit product rule rather than an
indexer assumption.

Use `(chain_id, token_address, tx_hash, log_index)` as the observation identity.
A transaction can emit multiple USDC transfers, so transaction hash alone is not
unique.

## Acquisition loop

For each enabled chain/asset:

1. Acquire a PostgreSQL advisory lock for ingestion leadership.
2. Load the durable finalized cursor `{number, hash, config_version}`.
3. Query the configured finality boundary.
4. Verify the stored cursor hash still matches the provider's canonical hash.
5. Request logs for `cursor + 1 .. finalized_head` in bounded ranges.
6. Sort results by block number, transaction index, and log index.
7. Decode and validate every log strictly.
8. Intersect unique recipients with known invoice addresses in one indexed DB
   query; status controls projection transitions, not ledger retention.
9. Commit observations, projections, status changes, and cursor advancement in
   one database transaction per range.

The implementation uses adaptive range sizing up to a configured ceiling (100
blocks by default). It grows the range by 25% after success and halves it for
QuickNode HTTP 413, block-range, response-size, or result-count errors. It never
treats a provider limit, timeout, malformed response, or suspicious response as
an empty range. A failure at one block is retried and alerted rather than skipped:
availability degradation is safer than silently losing a payment.

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
invoices.

### RPC filter

Filter at the provider by:

- exact USDC proxy address;
- exact `Transfer` topic0.

Initially fetch all USDC transfers in each range and intersect recipients in one
set-based database query. This avoids synchronizing an unbounded dynamic address
watchlist with the provider.

If USDC log volume becomes a measured bottleneck, maintain the active address set
locally and split it across OR filters for indexed `topic2`. The database remains
authoritative, and the indexer must refresh registrations after fetching a range
but before committing it so an address disclosed after its invoice commit cannot
be missed.

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
- invoice ID;
- amount `NUMERIC(78, 0)`;
- observed timestamp.

Enforce uniqueness on
`(chain_id, token_address, tx_hash, log_index)`. Add an invoice foreign key and
indexes for ledger replay by invoice and block.

### Invoice projection

Add:

- `confirmed_received NUMERIC(78, 0) NOT NULL DEFAULT 0`;
- `funded_at_block` and `funded_at_block_hash`;
- optionally the observation identity that crossed the funding threshold.

Use a unique `(chain_id, token_address, payment_address)` index and a partial
index over active invoice payment addresses.

`payment_observations` is the audit source of truth. Invoice totals and status are
rebuildable projections.

## Atomic processing

Do not hold a DB transaction across RPC calls. After a complete range is fetched
and validated, one transaction must:

1. lock and revalidate the cursor;
2. insert observations idempotently;
3. increment affected invoice totals in event order;
4. transition `created -> funded` at the earliest observation where confirmed
   cumulative credit reaches the invoice amount;
5. record the crossing block hash/number and observed cumulative amount;
6. advance the cursor to a verified range-end block hash.

The cursor advances if and only if all range effects commit. A crash before
commit replays safely; a crash after commit resumes at the next block.

Invoice creation must continue to commit before the payment address is disclosed
in the API response. This guarantees the invoice can be found before any client
could include a payment in a later block.

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
operator. Do not automatically reverse a finalized invoice because its funds may
already have been swept irreversibly.

Finalized-only processing avoids canonical/orphan block tables and rollback
machinery. Add those only if provisional states become a product requirement.

## Partial payments and overpayment

Sum all finalized observations for an invoice in canonical order. Partial
payments across transactions or blocks accumulate. Multiple transfers in one
transaction remain distinct by log index. Fund at the first observation where
the cumulative amount reaches the requested amount.

Before expiration, all USDC at the payment address when execution occurs belongs
to the beneficiary. The Payment constructor requires a balance of at least the
invoice amount and transfers the full balance, so an overpayment present before
execution is not stranded. After expiration, execution instead transfers the
complete balance to the invoice's recovery address without requiring the invoice
amount. Both the expiration timestamp and recovery address are committed into
the deterministic address.

Factory execution is permissionless, so anyone can recover an expired partial
payment. The indexer only automatically executes finalized-funded invoices;
operators or a separate automation path must execute expired underfunded
invoices.

Transfers sent after the Payment contract has executed can still become
stranded: constructor-based recovery runs only at deployment. Stop presenting
the address after funding and use a different deployed-contract design if those
late transfers must be recoverable.

## Sweep architecture under USDC

Sweep only finalized-funded invoices. The funding finality gate does not make the
sweep transaction final.

The implementation persists a submitted transaction hash before receipt polling,
then persists its inclusion block/hash when mined. It keeps the invoice
`deploying` and marks it `fulfilled` only after that detection passes the same
confirmation depth and Payment code remains present. A restart resumes receipt
polling or recovers permissionless execution from deterministic code presence.
Persisted signer nonce ownership and replacement history remain required before
running multiple sweep workers.

The native-token failure classifier cannot be reused. For USDC, a failed CREATE3
deployment with no code may mean:

- USDC is globally paused;
- payment address or beneficiary is blacklisted;
- the payment address became underfunded;
- an unreviewed proxy implementation changed behavior;
- the RPC or submission path failed.

ERC-20 receipt does not invoke a beneficiary callback, so `receiver_rejected` is
not an accurate reason. Classify with targeted `balanceOf`, pause/blacklist reads,
code checks, and a simulation at a pinned block. Infrastructure failures remain
retryable and alertable rather than becoming a terminal invoice failure.

Use a separate per-chain/per-signer leader lock. Persist nonce ownership before
submission so replicas cannot race retries. Prefer a direct safe ERC-20 transfer
from the Payment contract over self-approval followed by `transferFrom`.

## Build versus QuickNode

### Owned `eth_getLogs` indexer — recommended now

Benefits:

- few RPC calls because many finalized blocks are queried per range;
- provider-native filtering by one contract and one event;
- portable across EVM RPC vendors;
- exact control over finality and failure policy;
- no billing for every empty block beyond ordinary RPC usage;
- small implementation surface for one chain and one asset.

Costs:

- adaptive range/retry logic;
- cursor and provider hash checks;
- fallback routing and monitoring;
- backfill throughput and provider-limit operations.

These are bounded concerns now that traces, full blocks, receipts, and balance
reconciliation are gone.

### QuickNode Streams — preferred managed upgrade path

Use the **Logs** dataset and a server-side filter for only the exact USDC proxy
and `Transfer` topic. Deliver to an authenticated webhook, not directly into the
application's projection tables. The webhook must validate HMAC/mTLS, commit the
ledger and cursor transaction, then return 2xx.

Streams provides sequential delivery, configurable batching, historical
backfill, retries, and reorg correction restreams. Configure `fix_block_reorgs`
and an appropriate `keep_distance_from_tip`.

However:

- filtering does not reduce billing; credits are charged per block processed;
- `keep_distance_from_tip` is probabilistic, not economic finality;
- correction ranges and reorg metadata must still be applied idempotently;
- QuickNode recommends independent hash-continuity verification;
- webhook failures retry and eventually pause the stream, requiring alerts and
  operational resume;
- a QuickNode-side dynamic payment-address watchlist creates a consistency race,
  so filter only by USDC and match recipients in our database.

QuickNode documentation:

- [Streams logs and other data sources](https://www.quicknode.com/docs/streams/data-sources)
- [Streams backfilling](https://www.quicknode.com/docs/streams/backfilling)
- [Streams reorg handling](https://www.quicknode.com/docs/streams/reorg-handling)
- [Streams webhook delivery](https://www.quicknode.com/docs/streams/destinations/webhooks)
- [Streams billing](https://www.quicknode.com/docs/streams/billing)

### QuickNode Webhooks — not a correctness source

QuickNode Webhooks may be economical for low-volume contract-event alerts because
it bills per delivered payload. It advertises retries and automatic reorg
handling, but lacks Streams' explicit historical backfill, batching, ordered
correction protocol, and detailed reorg controls. It is appropriate for
notifications, not the authoritative payment ledger.

### When to switch to Streams

Reconsider when any of these become true:

- several chains or USDC assets are supported;
- a single block can exceed reliable provider log limits;
- catch-up lag or provider quirks create recurring on-call work;
- provisional low-latency indexing already requires a reversible block ledger;
- managed-delivery support/SLA value exceeds Streams' per-block cost;
- Kafka/object-storage fanout is needed for multiple consumers.

Switching acquisition does not change the database ledger, event identity,
finality gate, sweep rules, or halt-on-finalized-reorg invariant.

## Verification and operations

Required tests:

- `transfer`, `transferFrom`, relayed/internal-call transfer detection;
- reverted and zero-value transfers ignored;
- multiple USDC logs in one transaction;
- same-block and cross-block partial payment, exact payment, and overpayment;
- wrong token, bridged USDC, wrong chain, and fake `Transfer` emitter ignored;
- duplicate range replay and crashes around every cursor transaction boundary;
- adaptive range shrinking, provider failover, and provider disagreement;
- invoice creation concurrent with range ingestion;
- unreviewed USDC proxy upgrade halts ingestion;
- finalized cursor hash mismatch halts ingestion;
- USDC pause, source/beneficiary blacklist, and underfunded sweep classification;
- sweep submission crash, replacement, third-party execution, and finalization;
- full projection rebuild equals materialized invoice totals.

Monitor:

- finalized-head and cursor lag;
- logs and ranges processed, range size, response bytes, and provider errors;
- provider hash disagreement;
- observations, funded invoices, partial-payment age, and unmatched USDC logs;
- chain halted state and proxy upgrades;
- oldest funded-unswept invoice;
- sweep nonce, receipt, and finality lag.

Hard invariants:

- only exact allowlisted native-USDC logs are credited;
- every observation is durable at most once;
- cursor and range effects commit atomically;
- only finalized cumulative credit funds an invoice;
- only finalized funding can authorize a sweep;
- payment addresses are disclosed only after invoice commit;
- unreviewed asset upgrades and finalized hash mismatches halt processing;
- projections can be rebuilt from the observation ledger.
