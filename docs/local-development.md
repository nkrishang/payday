# Local development and configuration

## System overview

The API creates a deposit request; its counterfactual CREATE3 deposit address is
derived once the payer attests, from the hosted page, the wallet they will
pay from (`POST /v1/payer/deposit-requests/{id}/wallet/challenge` and `/attest`),
because the address commits to that wallet as its recovery term and to the
signature through the salt. The payer transfers the request's currency to
that address. `gum-indexer` reads finalized ranges of
`Transfer(address,address,uint256)` logs from every stablecoin contract
configured on the chain and reports each range, with its observations, to
`gum-server`'s internal listener; the server credits only the request's own
token and advances deposit requests from `created` to `funded` when
cumulative transfers reach the requested amount, or to `expired` when the
finalized block timestamp passes the deadline. The indexer has no database
and no keys ([`architecture.md`](architecture.md)).

`gum-server`'s scheduler groups funded requests into sweep jobs and
publishes one `SweepBatch` command per job on the Postgres bus;
`gum-signers` executes it against `BatchSweeper`. For an address without
code it calls `PaymentFactory.execute`, which deploys `DepositRequest` at the
counterfactual address; before expiry the constructor pays the beneficiary
exactly the requested amount and sends any remainder back to the payer's
wallet, and after expiry it sends the whole balance to that wallet. For an
address that already has code it calls `Deposit.recover`, which forwards
anything that arrived later to the payer's wallet. The finalized receipt
decides the outcome, which the signers report as evidence in a
`SweepFinalized` event and the server turns into transitions: `Settled` →
`fulfilled` (with the overpayment recovered), `Returned` → `recovered`,
`LateCollected` → late funds collected, and `Failed` → retried or
`attention_reason` set after the signers read `paused()`,
`isBlacklisted()`, and `balanceOf()` on the token at the receipt block.
Every nonzero recovery is written to the `recovered_funds` ledger in the
transaction that applies the event, and each ledger row raises a
`deposit_request.recovered_funds` webhook.

The recovery wallet is the payer's attested wallet, never a configured or
requested value: `gum-server` rejects a create request that carries
`refund_address`.

```
created → funded → fulfilled
   │         │
   └► expired└──► recovered
```

Being inside a sweep job (`sweep_job_id`) and needing an operator
(`attention_reason`, shown publicly as `needs_attention`) are flags beside
the status, not statuses; see `architecture.md` §1.

A deposit request is denominated in one `currency`, `USDC` (default) or
`USDT`, and only the exact contract the chain's `GUM_CHAINS` entry lists
for that currency is credited, on that chain; a transfer of another
configured stablecoin to the address stays there for `recover(address)` to
return. Every served stablecoin uses six decimal places. Production
configures Circle's native USDC proxy per network and Tether's USDT0 on
Monad and Arbitrum One; the local setup deploys mintable six-decimal
`MockStablecoin` fixtures with the same `paused()`/`isBlacklisted()` views
on its two Anvils.

A USDC deposit request has no chain until the payer chooses one at the wallet
step: the request offers every registry network serving USDC, the challenge names the
chosen `chain_id`, and the binding writes the chain, token, factory, and
address together. A USDT request has no 1:1 bridge, so it must pin
`chain_id` at creation to a chain that serves USDT. The local stack runs two chains so that step has a real
choice: Anvil on 8545 (chain 31337, the Monad-shaped `finalized` path,
serving USDC and USDT) and Anvil on 8546 (chain 31338, the L2-shaped
`latest` plus confirmations path, USDC only).

## Indexing and finality

The acquisition path has two halves over standard EVM JSON-RPC:

The indexer runs one worker per `GUM_CHAINS` entry, each with its own
RPC (`GUM_RPC_URL_<chain_id>`), cursor, advisory lock, and signal,
watching every token contract the entry lists; `indexer_cursor` and
`indexer_status` are keyed by chain alone.

- **The reconciler** (the only writer) runs a pass every
  `GUM_INDEXER_RECONCILE_INTERVAL_MS` and immediately on a wake. A pass is
  one boundary header read (the entry's `finality_source`, `finalized` or
  `latest`, minus `finality_confirmations` blocks), one header read to
  verify the stored cursor hash, then, if the chain has anything to watch,
  per bounded range one range-end header read, one `eth_getLogs` filtered
  to the chain's token addresses (one address array), the `Transfer` topic,
  and the watched recipients
  (500 per call), and a second range-end read that proves nothing
  moved while the logs were fetched. A chain with nothing to watch
  fast-forwards its cursor instead and sleeps
  `GUM_INDEXER_IDLE_INTERVAL_MS`. Deposits are classified by the
  `blockTimestamp` the log itself carries, so a block with transfers costs
  no extra request. Up to `GUM_INDEXER_MAX_RANGES_PER_TICK` ranges drain
  per pass, so a backlog clears independently of the cadence.
- **The transfer signal** is a WebSocket `eth_subscribe` on the same node
  (`GUM_RPC_WS_URL_<chain_id>`, derived from the HTTP URL when unset)
  for transfers of the chain's configured tokens whose recipient is one of
  our payment addresses, one subscription per chain, held
  only while the chain has something to watch. On Monad it uses `monadLogs`
  and wakes the reconciler when a matching log reaches the `Finalized`
  commit state; Anvil, Base, and Arbitrum lack `monadLogs`, so the signal
  falls back to the standard `logs` subscription and the reconciler waits
  a block time at a time until the block is behind the boundary. The signal
  never writes: a missed or duplicated notification costs latency, not
  correctness, and while the socket is down the reconciler simply runs on
  `GUM_INDEXER_POLL_INTERVAL_MS`.

Observations, deposit request projections, and the hash-bearing cursor commit
atomically. Every transfer to a known deposit request address is retained: `credited`
transfers count toward the amount, `late` transfers (after settlement, expiry,
or a block) are queued for recovery, and zero-value transfers are `error`.
Each observation records the block and transaction index of the sweep that
collected it, so a lagging cursor can never re-queue funds a finalized sweep
already moved, including when deposit and sweep transactions share a block.

Log ranges start at the entry's `log_range_size` (100 on Monad, QuickNode's
cap there; 10,000 on the L2s), halve when the provider reports a
range/result-size error, and grow after a successful response. A one-block failure is retried rather than
skipped.

## Prerequisites

- Rust, Foundry (`anvil`, `cast`, `forge`), Docker (it runs PostgreSQL and
  the MinIO attachment store), `just`, the PostgreSQL client, and `jq`.
- Node.js 20 or newer, for the TypeScript SDK and the `gum.money` web app.

Copy `.env.example` to `.env` if you want to override the checked-in local
defaults. Start PostgreSQL, MinIO, Anvil, the development identity provider,
`gum-server`, and the indexer with multiplexed logs:

```bash
just dev
```

In another shell, mint a local account with an API key:

```bash
just seed                      # or: just seed you@example.test
```

Merchant sign-in is Privy's, which has no local stand-in, so the seed writes
an account and a key straight into the runner's database
(`scripts/local-api-key.sh`) and prints the key; export it as
`GUM_API_KEY`. To sign in through the browser instead, run the web app
below: it signs in against the real development Privy app and the code
arrives in your mailbox. Each run starts from a clean database,
attachment store, and Anvil chains so their indexed histories cannot drift.
After the bootstrap deploys the contracts on both Anvils, the runner reads
their runtime bytecode from each chain and builds `GUM_CHAINS` from it
(overriding any `.env` value), because all three services verify the deployed
contract generation, and each token's `decimals()`, `name()`, and
`version()`, on every chain at startup and refuse to start on a mismatch.

To open a created deposit, run the hosted checkout in a third shell:

```bash
npm ci
cp web/.env.example web/.env.local
just web
```

It serves `http://127.0.0.1:3002`, which is what `GUM_PUBLIC_BASE_URL`
points at, so the `deposit_url` the API returns opens the real checkout and the
gateway accepts the dashboard's cross-origin requests (the merchant routes
answer only that origin). Port 3002 rather than 3001, which belongs to the
development identity provider. See [web/README.md](../web/README.md).

For the dashboard, `web/.env.local` also needs `NEXT_PUBLIC_PRIVY_APP_ID`
(the development Privy app, the same id the runner gives `gum-server` as
`GUM_PRIVY_APP_ID`; `http://127.0.0.1:3002` must be among that app's
allowed domains) and `NEXT_PUBLIC_ATTACHMENT_UPLOAD_ORIGIN` (the local MinIO,
`http://127.0.0.1:9000`, so the page may PUT PDFs there). The example file
carries working local values. Payer and issuer-mailbox codes still come from
the development identity provider, printed in its log.

## Local Anvil end-to-end run

`scripts/e2e-anvil.sh` runs the complete flow (the payer's wallet binding,
signed with `cast` exactly as a wallet signs EIP-712 typed data; exact,
partial, and batched deposits; an overpayment split between the beneficiary
and the payer's wallet; late transfers; third-party execution; a paused
token; a blacklisted beneficiary and its operator release; an expired partial
deposit returned automatically and completed late; a gated request whose
wallet step follows its email verification; funds from a stranger's wallet
flagged and refused a proof; the `recovered_funds` ledger and its webhook
events; a request bound and settled on the second chain; a challenge for a
chain the request does not offer refused with `422`; a create carrying
`chain_id` refused with `400`; an idle chain fast-forwarding without a
scan; and a deposit sent to an address on the wrong chain, refused by the
contract and returned to the payer by hand exactly as
`docs/runbooks/wrong-network-deposit.md` does it) against two fresh Anvils
started with `--slots-in-an-epoch 1 --block-time 1`, which makes the
node's `finalized` tag advance like a real chain, and a fresh MinIO
container standing in for the attachment bucket. Use it whenever the contracts or
the worker change:

```bash
just e2e
```

For a manual session, the key below is Anvil's public development account #0
key. Never use it on a real network.

### 1. Start Anvil

```bash
anvil --chain-id 31337 --slots-in-an-epoch 1 --mixed-mining --block-time 1
```

### 2. Bootstrap PaymentFactory and the mock stablecoins

Run against a fresh Anvil. The script is safe to repeat on the same node, but
it will not redeploy changed contract code over existing addresses: restart
Anvil after editing a contract, and recompute the code hashes below, because
the services refuse to start against a generation that differs from the one
they were configured for.

```bash
forge script foundry/script/Bootstrap.s.sol:BootstrapScript \
  --rpc-url http://127.0.0.1:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --broadcast
```

It deploys:

- `PaymentFactory`: `0x5FbDB2315678afecb367f032d93F642f64180aa3`
- `MockStablecoin` as USDC: `0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512`
- `BatchSweeper`: `0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0`
- `MockStablecoin` as USDT: `0xCf7Ed3AccA5a467e9e704C703E8D87F634fB0Fc9`
  (account #0's nonce-3 CREATE address; `GUM_USDT_ADDRESS` to the
  runner)

`MockStablecoin` (`foundry/src/MockStablecoin.sol`, formerly `MockUSDC`)
takes its name and symbol in the constructor. Anvil accounts #0 and #1 are
topped up to 1,000,000 of each test stablecoin. Account #1 is
the end-to-end suite's payer, whose wallet every request is bound to; account
#5 plays the stranger who pays from an unattested wallet. The runner lists
both tokens on the first chain and USDC alone on the second, and hands the
Relay stand-in both addresses as `RELAY_STUB_USDC` and `RELAY_STUB_USDT`.

Pin the deployed generation for the services (the runner does this for
you, per chain, when it builds `GUM_CHAINS`); these are the two hashes
each registry entry carries:

```bash
cast keccak "$(cast code 0x5FbDB2315678afecb367f032d93F642f64180aa3 --rpc-url http://127.0.0.1:8545)"   # factory_code_hash
cast keccak "$(cast code 0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0 --rpc-url http://127.0.0.1:8545)"   # batch_sweeper_code_hash
```

### 3. Start the local attachment store

MinIO stands in for the S3 attachment bucket (the runner does this for you).
Nothing scans local uploads, so `just dev` also runs a small stand-in for
GuardDuty: a few seconds after a PDF lands it stamps the clean verdict
finalize is waiting on, as described under
[Attachments and the scan tag](#attachments-and-the-scan-tag).

```bash
docker run -d --rm --name gum-minio \
  -e MINIO_ROOT_USER=gum-local -e MINIO_ROOT_PASSWORD=gum-local -e MINIO_BROWSER=off \
  -p 127.0.0.1:9000:9000 quay.io/minio/minio server /data
curl -fsS http://127.0.0.1:9000/minio/health/live
docker run --rm --network host \
  -e MC_HOST_local=http://gum-local:gum-local@127.0.0.1:9000 \
  quay.io/minio/mc mb --ignore-existing local/gum-attachments-local
```

`.env.example` carries the matching `GUM_ATTACHMENT_*` and `AWS_*` values.

### 4. Build and start the services manually

Ensure `.env` contains the database URL, `GUM_CHAINS` (one entry per
Anvil, with the fixture addresses, its `tokens`, the two code hashes above,
finality settings, and start block; `scripts/local-runner.sh`'s
`chain_entry` builds one), `GUM_RPC_URL_31337` and `GUM_RPC_URL_31338`, signer and
attestation keys, attachment store, and identity settings (the
`.env.example` hash placeholders are zero and will be refused).
Start the identity provider:

```bash
cargo build --workspace
set -a; source .env; set +a
./target/debug/gum-dev-identity
```

Apply the schema, then start `gum-server` in another shell (services never
migrate on start; `/health/ready` on the internal listener,
`http://127.0.0.1:3010/health/ready`, answers 503 `schema behind` until
the migration has run):

```bash
set -a; source .env; set +a
./target/debug/gum-server migrate
./target/debug/gum-server
```

Then mint an account and key with `scripts/local-api-key.sh` (it reads
`DATABASE_URL`), or sign in through the web app against the development Privy
app. Production merchant sign-in is Privy too; see `docs/authentication.md`.

In two more terminals, once `gum-server` is ready:

```bash
set -a; source .env; set +a
GUM_INDEXER_POLL_INTERVAL_MS=1000 ./target/debug/gum-indexer   # health on 127.0.0.1:3011
```

```bash
set -a; source .env; set +a
./target/debug/gum-signers                                         # health on 127.0.0.1:3012
```

`scripts/local-runner.sh` does all of this in order for `just dev`.

### 5. Create a deposit request

Expirations must be at least ten minutes and at most a year ahead.

```bash
curl -fsS http://127.0.0.1:3000/v1/deposit-requests \
  -H "Authorization: Bearer $GUM_API_KEY" \
  -H "Content-Type: application/json" -H "Idempotency-Key: local-1" \
  -d '{"amount": "1.5", "payout_address": "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
       "issuer": {"name": "Local"}, "payer": {"name": "Payer"},
       "payer_policy": {"mode": "permissionless"}, "expires_in": 3600}'
```

The response's `currency` is `USDC` (pass `"currency": "USDT"` with
`"chain_id": "31337"` for a USDT request, which the first Anvil alone
serves); `chain`, `token`, and `address` are null and `networks` lists
both Anvils: bind the payer's wallet first, as the hosted checkout does
(challenge with `{"wallet", "chain_id": "31337"}`, sign the typed data with
`cast wallet sign --data --from-file`, attest; `scripts/e2e-anvil.sh`'s
`bind_payer_wallet` is the reference). `recovery_address` is then that
wallet; there is no flag to choose it, nor one to choose the chain.

Copy `id` and, after the binding, `address` from `GET /v1/deposit-requests/{id}`,
then transfer 1.5 USDC (`1500000` atomic units) from the bound wallet:

The `self_settlement` object contains the factory and salt needed for anyone
to settle the deposit request on-chain if Gum is unavailable.

```bash
cast send 0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512 \
  'transfer(address,uint256)' <payment_address> 1500000 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --rpc-url http://127.0.0.1:8545

curl -fsS "http://127.0.0.1:3000/v1/deposit-requests/<id>" \
  -H "Authorization: Bearer $GUM_API_KEY" | jq
```

The status should reach `settled` with `settlement_tx_hash` set. Verify the
deposit address was emptied and the Deposit contract was deployed:

```bash
cast call 0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512 \
  'balanceOf(address)(uint256)' <payment_address> --rpc-url http://127.0.0.1:8545
cast call <payment_address> 'settled()(bool)' --rpc-url http://127.0.0.1:8545
```

## Attachments and the scan tag

`POST /v1/attachments` returns a presigned PUT aimed at MinIO; upload the PDF
with the returned headers, then call `POST /v1/attachments/{id}/finalize`.
Finalize answers `409 attachment_scan_pending` until the object carries the
tag GuardDuty Malware Protection writes in production, and
`422 attachment_rejected` for any other verdict.

Under `just dev`, `local-runner.sh`'s `start_scan_stub` polls the bucket and
stamps a clean verdict on every upload within a couple of seconds, so a
manual upload through the browser finalizes almost immediately instead of
sitting out finalize's 2-minute poll timeout. It only fills in the tag when
none is present yet, so stamping one by hand first still walks the rejection
path — set any other value on the object key,
`uploads/<account_id>/<attachment_id>.pdf`, before the stub gets to it:

```bash
docker run --rm --network host \
  -e MC_HOST_local=http://gum-local:gum-local@127.0.0.1:9000 \
  quay.io/minio/mc tag set local/gum-attachments-local/uploads/<account_id>/<attachment_id>.pdf \
  'GuardDutyMalwareScanStatus=THREATS_FOUND'
```

`mc tag list` on the same path shows the tags. `scripts/e2e-anvil.sh` controls
verdicts the same way, deterministically, through its own `tag_object_scanned`
helper — reach for `just e2e` instead when the rejection path needs reliable
coverage rather than a race against the stub. No lifecycle rule runs locally,
so abandoned
uploads stay until the container is removed.

## Automated tests

```bash
DATABASE_URL=postgresql:///gateway?user="$USER" cargo test --workspace
cargo build -p gum-core --bin derive-address
forge test
just web-check   # SDK and web app: build, types, lint, unit tests
```

Coverage includes the payer wallet binding and Proof of Payment v2 (the
EIP-712 digest is pinned against viem in `web/lib/payer-attestation.test.ts`);
exact, partial, and overpayment funding; the overpayment split between
beneficiary and the payer's wallet and the `recovered_funds` ledger;
deployment code-hash and bound-factory verification at startup; finality-tag
and confirmation gating; multi-range draining; range replay idempotency; chain
isolation; expiry by block timestamp; settlement, recovery, third-party
execution, and late-fund collection through finalized receipts; failure
classification (paused, blacklisted, underfunded, unknown); same-nonce fee
replacement, abandoned nonces, and stalled lanes; bus delivery, retry,
dead-letter and idempotency; crash-boundary recovery in the signers; API validation;
CREATE3 address parity; and `BatchSweeper` under the production gas budget.

## Configuration

- `DATABASE_URL`
- `GUM_API_KEY` — CLI-only per-account bearer key for deposit requests
- `GUM_PRIVY_APP_ID` — the Privy app merchants sign in to; `gum-server`
  verifies dashboard sessions against its published keys, fetched at startup
  (see `docs/authentication.md`). `just dev` sets the development app; unset,
  only API keys authenticate, which is how `just e2e` runs
- `GUM_PRIVY_APP_SECRET` — optional; unrelated to session verification.
  Enables `POST /v1/wallets/pregenerate`, which the sign-up dialog calls the
  moment a merchant submits their email so Privy can create the embedded
  wallet in parallel with sending the code, instead of only starting once the
  code is verified (`crates/gum-server/src/pregenerated_wallet.rs`). Unset
  anywhere, sign-in still creates the wallet itself; the dialog just waits
  slightly longer for it. Rate-limited to one request per email per 30
  seconds, in memory, bounded the same way the proof cache is
- `GUM_PAYER_AUTH0_ISSUER`, `GUM_PAYER_AUTH0_AUDIENCE`,
  `GUM_PAYER_AUTH0_CLIENT_ID`, `GUM_PAYER_REF_MASTER_KEY` — the payer
  email-verification audience and the payer-reference key, set together or not
  at all; `just dev` and `just e2e` point them at the development provider's
  `gum-payer-local` client and audience
- `GUM_HOSTED_CHECKOUT_ORIGIN` — the one browser origin the payer
  verification writes answer to; defaults to `GUM_PUBLIC_BASE_URL`
- `GUM_RESEND_API_KEY` — optional; the Resend key that emails a payer
  named with an `email` on a deposit request their link to it, from
  `GUM_PAYER_EMAIL_FROM` (default `Gum <contact@gum.money>`). Unset,
  as `just dev` leaves it, the emails are queued in `notification_outbox`
  and never sent; set it against a Resend domain you control to see the
  real message
- `GUM_ADMIN_REVIEWER_ID` — recorded as the operator on
  `POST /v1/admin/deposit-requests/{id}/release`; defaults to `operator`
- `GUM_DEV_IDENTITY` — set to `1` only locally to permit a loopback HTTP
  issuer; non-loopback HTTP issuers remain rejected
- `GUM_DEV_IDENTITY_BIND` — loopback socket for the development provider
- `GUM_DEV_IDENTITY_ISSUER` — token issuer; must exactly match
  `GUM_PAYER_AUTH0_ISSUER`
- `GUM_DEV_IDENTITY_OTP` — the code the development provider emails for
  every payer or issuer-mailbox verification, instead of a random one it only
  prints to its log (`DEV IDENTITY OTP <email> <code>`). Local convenience
  only; the provider refuses to bind anything but loopback, and Auth0 issues
  the real codes
- `GUM_CHAINS` — the network registry all three services read: a JSON array
  of `{chain_id, tokens, factory, batch_sweeper, factory_code_hash,
  batch_sweeper_code_hash, start_block, finality_source,
  finality_confirmations, block_time_ms, log_range_size,
  signer_low_balance_wei?, explorer_base_url?, cctp?}`, in the order the
  checkout offers networks. `tokens` is
  `[{"currency":"USDC","address":"0x…"},{"currency":"USDT","address":"0x…"}]`,
  the currencies served on that chain by their exact contracts (Circle's
  native USDC proxy and Tether's USDT0 in production); a currency absent
  there is not offered on that chain, and at least one chain must list
  USDC. The code hashes are keccak256 of the runtime bytecode at the two
  addresses
  (`cast keccak "$(cast code <ADDRESS> --rpc-url <RPC_URL>)"`); the
  services compare them with each live chain at startup, also checking that
  `BatchSweeper.factory()` is the entry's `factory` and that each token's
  `decimals()`, `name()`, and `version()` match its currency (a missing
  `version()` getter, as on USDT0, is tolerated by trying `"1"` then `"2"`
  against `DOMAIN_SEPARATOR`), and refuse to start on
  a mismatch. `just dev` and `just e2e` build the two local entries from
  the running Anvils (`build_chain_registry` in `scripts/local-runner.sh`).
  `signer_low_balance_wei` overrides `GUM_SIGNER_LOW_BALANCE_WEI` for
  that chain. `cctp` is `{domain, token_messenger, message_transmitter,
  forwarder, forwarder_code_hash}`, Circle's CCTP V2 and the deployed
  `WithdrawalForwarder`; it requires USDC among the chain's `tokens`, and
  a chain without it (the local Anvils) serves same-chain withdrawals only.
  `finality_source` is `finalized` or `latest` (see
  `docs/indexer-architecture.md`)
- `GUM_RPC_URL_<chain_id>` — one HTTPS endpoint per registry chain
  (QuickNode in production, an Anvil locally); read by all three services
  (`gum-server` uses it for deployment verification and read-only chain
  queries; it never signs)
- `GUM_RPC_WS_URL_<chain_id>` — WebSocket endpoint for that chain's
  transfer signal; unset derives it from the HTTP URL (`https` → `wss`,
  `http` → `ws`, same host, path and token), `off` disables the signal
- `GUM_ONBOARDING_CHAIN_ID` — the chain the dashboard onboarding demo
  deposit binds and pays on; defaults to the first registry entry
- `GUM_PUBLIC_BASE_URL` — origin serving the hosted checkout, which is where
  deposit links point; `http://127.0.0.1:3002`
  locally, `https://gum.money` in production. Must be a bare origin, and HTTPS
  unless it is loopback
- `GUM_INDEXER_MAX_RANGES_PER_TICK` — ranges drained per pass, default 20
- `GUM_INDEXER_RECONCILE_INTERVAL_MS` — reconcile cadence while a chain
  has something to watch and its transfer signal is connected, default
  60000; the local runner uses 1000
- `GUM_INDEXER_POLL_INTERVAL_MS` — the indexer's pass cadence while the
  signal is disconnected or disabled, default 2000
- `GUM_INDEXER_IDLE_INTERVAL_MS` — pass cadence for a chain with nothing
  to watch, whose passes fast-forward the cursor without scanning, default
  300000; the local runner uses 2000
- `GUM_INDEXER_LATE_WATCH_DAYS` — how long a settled address stays in the
  watch list, which the signal subscribes to and every range scan is
  filtered by, default 365; a late transfer outside the window is not seen
  and is returned by hand (`docs/runbooks/wrong-network-deposit.md`)
- `GUM_INDEXER_RPC_MAX_RPS` — paces outgoing RPC calls under the provider's
  requests-per-second budget, default 40; 0 disables pacing (see
  [quicknode-rpc-limits.md](runbooks/quicknode-rpc-limits.md))
- `GUM_SWEEP_PENDING_TIMEOUT_SECS` — seconds without a receipt before a
  helper transaction is replaced on the same nonce, default 60
- `GUM_SWEEP_MAX_SUBMISSIONS` — replacements of one transaction before
  `gum-signers` publishes `ExecutionStalled` and stops raising fees
  (reconciliation continues), default 5
- `GUM_SWEEP_MAX_ATTEMPTS` — unclassified attempts before `gum-signers`
  fails a job permanently, default 8. The server-side counterpart is
  `SweepPolicy::max_attempts` (5): unexplained item failures before the
  deposit request gets `attention_reason = retries_exhausted`
- `GUM_SWEEP_SCHEDULER_INTERVAL_MS` — how often `gum-server` turns
  funded requests into sweep jobs when not woken by the indexer, default 5000
- `GUM_SIGNERS_POLL_INTERVAL_MS` — `gum-signers` pass cadence when not
  woken by a command, default 2000; the local runner uses 250
- `GUM_SIGNERS_RPC_MAX_RPS` — as `GUM_INDEXER_RPC_MAX_RPS`, for the
  signers, default 20
- `GUM_INTERNAL_BIND_ADDR`, `GUM_SERVER_INTERNAL_URL`,
  `GUM_INTERNAL_TOKEN` — `gum-server`'s internal listener, where the
  indexer finds it, and the bearer token both read
- `GUM_INDEXER_LISTEN_ADDR`, `GUM_SIGNERS_LISTEN_ADDR` — the two
  workers' `/health` listeners
- `GUM_CCTP_IRIS_URL` — Circle's attestation service the withdrawal relayer
  polls for a bridge leg's burn, default `https://iris-api.circle.com`; the
  local Anvil chains have no `cctp` block, so nothing polls it there
- `GUM_RELAY_URL` and `GUM_RELAY_API_KEY` — Relay (relay.link), for
  paying a deposit request from another network; `gum-server` reads them. Unset key:
  the hosted checkout does not offer it. `just dev` and `just e2e` point
  them at `scripts/relay-stub.mjs`, a stand-in on port 4020 that quotes a
  transfer of the local USDC or USDT (`RELAY_STUB_USDC`, `RELAY_STUB_USDT`)
  to its solver on the second Anvil and fills it on the first
- `GUM_SIGNER_LOW_BALANCE_WEI` — threshold for the low-balance warning,
  default 0.05 native tokens; a chain's `signer_low_balance_wei` overrides it
- `GUM_SIGNER_KEYS` — the local/Anvil sweep signer pool, comma-separated
  private keys; every key keeps one helper transaction in flight, so N keys
  let N sweep batches or withdrawal steps run at once per chain. The runner
  uses Anvil account #0 plus mnemonic accounts #10 and #11 (Anvil starts
  with twelve accounts). Mutually exclusive with KMS
- `GUM_KMS_KEY_IDS` — production AWS KMS secp256k1 key ARNs,
  comma-separated, one per pool signer; `gum-signers` uses its ambient ECS
  task role for `kms:GetPublicKey` and `kms:Sign` on each. Only
  `gum-signers` reads either signer variable
- `GUM_ATTACHMENT_BUCKET` — S3 bucket holding deposit request PDFs;
  `gum-attachments-local` on the runner's MinIO
- `GUM_ATTACHMENT_S3_ENDPOINT`, `GUM_ATTACHMENT_S3_FORCE_PATH_STYLE` —
  optional endpoint override and path-style addressing, set locally to reach
  MinIO at `http://127.0.0.1:9000`; unset in production
- `GUM_ATTACHMENT_DOWNLOAD_TTL_SECS` — lifetime of signed download URLs,
  default 300
- `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION` — MinIO's root
  credentials locally (`gum-local`, region `us-east-1`); the ambient ECS
  task role in production
- `GUM_ATTESTATION_SIGNER_KEY` — local key signing Proof of Payment
  verification attestations (Anvil account #6, whose address
  `0x976EA74026E726554dB657fA54763abd0C3a0aa9` is the local trusted attestor);
  mutually exclusive with `GUM_ATTESTATION_KMS_KEY_ID`, the production KMS
  secp256k1 key ARN. Exactly one is required
- `GUM_ONBOARDING_PAYER_KEY` — local key that pays the dashboard onboarding
  walkthrough's one self-issued deposit request (Anvil account #1); mutually
  exclusive with `GUM_ONBOARDING_PAYER_KMS_KEY_ID`, the production KMS
  key. Unlike attestation, both may be unset in any environment, including
  production — that simply disables `POST /v1/deposit-requests/{id}/onboarding-deposit`

The AWS deployment procedure is in `docs/production-runbook.md`; its
Terraform source is under `infra/`.

## Current constraints

- One exact contract per currency per configured chain; for USDC the payer
  chooses the chain, the merchant does not, while a USDT request is pinned
  to a chain serving USDT at creation. A deposit sent to an address on another
  supported chain is refused by the contract and returned by hand
  (`docs/runbooks/wrong-network-deposit.md`).
- A finalized cursor hash mismatch requires operator intervention; there is no
  automatic finalized-reorg rollback.
- One transaction is in flight per pool signer per chain; replacements
  share its nonce. After `GUM_SWEEP_MAX_SUBMISSIONS` unconfirmed
  submissions `gum-signers` stops raising that lane's fees and alarms
  (`ExecutionStalled`) while still reconciling it every pass; the other
  signers and block indexing continue.
- Deposit requests with an `attention_reason` are released by an operator
  (`docs/runbooks/stuck-deposit-request.md`); the scheduler never picks
  them up on its own.
- Deposit listing is cursor-paginated and bounded to 100 records per request.
