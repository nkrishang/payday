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
counterfactual address; before expiry the constructor pays the beneficiary
exactly the invoice amount and sends any remainder to the Payday recovery
wallet, and after expiry it sends the whole balance to that wallet. For an
address that already has code it calls `Payment.recover`, which forwards
anything that arrived later to the recovery wallet. The finalized receipt
decides the outcome: `Settled` → `fulfilled` (with a `Recovered` remainder
when overpaid), standalone `Recovered` → `recovered`, `SweepRecovered` → late
funds collected, and `SweepFailed` → retried or `blocked` after reading
`paused()`, `isBlacklisted()`, and `balanceOf()` on the token. Every nonzero
recovery is written to the `recovered_funds` ledger in the transaction that
resolves the batch, and each ledger row raises a `payment.recovered_funds`
webhook.

The recovery wallet is platform-controlled: `gatewayd` stamps
`PAYDAY_RECOVERY_ADDRESS` on every invoice and rejects a create request that
carries `refund_address`.

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

- Rust, Foundry (`anvil`, `cast`, `forge`), Docker (it runs PostgreSQL and
  the MinIO attachment store), `just`, the PostgreSQL client, and `jq`.
- Node.js 20 or newer, for the TypeScript SDK and the `payday.sh` web app.

Copy `.env.example` to `.env` if you want to override the checked-in local
defaults. Start PostgreSQL, MinIO, Anvil, the development identity provider,
`gatewayd`, and the indexer with multiplexed logs:

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
`PAYDAY_API_KEY`. To sign in through the browser instead, run the web app
below: it signs in against the real development Privy app and the code
arrives in your mailbox. Each run starts from a clean database,
attachment store, and Anvil chain so their indexed histories cannot drift.
After the bootstrap deploys the contracts, the runner reads their runtime
bytecode from the chain and exports `PAYDAY_FACTORY_CODE_HASH` and
`PAYDAY_BATCH_SWEEPER_CODE_HASH` from it (overriding any `.env` value),
because both services verify the deployed contract generation at startup and
refuse to start on a mismatch.

To open a created payment, run the hosted checkout in a third shell:

```bash
npm ci
cp web/.env.example web/.env.local
just web
```

It serves `http://127.0.0.1:3002`, which is what `PAYDAY_PUBLIC_BASE_URL`
points at, so the `payment_url` the CLI prints opens the real checkout and the
gateway accepts the dashboard's cross-origin requests (the merchant routes
answer only that origin). Port 3002 rather than 3001, which belongs to the
development identity provider. See [web/README.md](../web/README.md).

For the dashboard, `web/.env.local` also needs `NEXT_PUBLIC_PRIVY_APP_ID`
(the development Privy app, the same id the runner gives `gatewayd` as
`PAYDAY_PRIVY_APP_ID`; `http://127.0.0.1:3002` must be among that app's
allowed domains) and `NEXT_PUBLIC_ATTACHMENT_UPLOAD_ORIGIN` (the local MinIO,
`http://127.0.0.1:9000`, so the page may PUT PDFs there). The example file
carries working local values. Payer and issuer-mailbox codes still come from
the development identity provider, printed in its log.

## Local Anvil end-to-end run

`scripts/e2e-anvil.sh` runs the complete flow (exact, partial, and batched
payments; an overpayment split between the beneficiary and the Payday recovery
wallet; late transfers; third-party execution; a paused token; a blacklisted
beneficiary and its operator release; an expired partial payment recovered
automatically and completed late; the `recovered_funds` ledger and its
webhook events) against a fresh Anvil started with
`--slots-in-an-epoch 1 --block-time 1`, which makes the node's
`finalized` tag advance like a real chain, and a fresh MinIO container
standing in for the attachment bucket. Use it whenever the contracts or
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

### 2. Bootstrap PaymentFactory and MockUSDC

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
- `MockUSDC`: `0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512`
- `BatchSweeper`: `0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0`

Anvil accounts #0 and #1 are topped up to 1,000,000 test USDC. Account #5,
`0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc`, is the local Payday recovery
wallet (`PAYDAY_RECOVERY_ADDRESS`).

Pin the deployed generation for both services (the runner does this for you):

```bash
export PAYDAY_FACTORY_CODE_HASH="$(cast keccak "$(cast code 0x5FbDB2315678afecb367f032d93F642f64180aa3 --rpc-url http://127.0.0.1:8545)")"
export PAYDAY_BATCH_SWEEPER_CODE_HASH="$(cast keccak "$(cast code 0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0 --rpc-url http://127.0.0.1:8545)")"
```

### 3. Start the local attachment store

MinIO stands in for the S3 attachment bucket (the runner does this for you).
Nothing scans local uploads, so `just dev` also runs a small stand-in for
GuardDuty: a few seconds after a PDF lands it stamps the clean verdict
finalize is waiting on, as described under
[Attachments and the scan tag](#attachments-and-the-scan-tag).

```bash
docker run -d --rm --name payday-minio \
  -e MINIO_ROOT_USER=payday-local -e MINIO_ROOT_PASSWORD=payday-local -e MINIO_BROWSER=off \
  -p 127.0.0.1:9000:9000 minio/minio server /data
curl -fsS http://127.0.0.1:9000/minio/health/live
docker run --rm --network host \
  -e MC_HOST_local=http://payday-local:payday-local@127.0.0.1:9000 \
  minio/mc mb --ignore-existing local/payday-attachments-local
```

`.env.example` carries the matching `PAYDAY_ATTACHMENT_*` and `AWS_*` values.

### 4. Build and start the services manually

Ensure `.env` contains the local addresses, recovery address, database URL,
RPC URL, finality settings, start block, signer and attestation keys,
attachment store, and identity settings, and that the two code hashes above
are exported (the `.env.example` placeholders are zero and will be refused).
Start the identity provider:

```bash
cargo build --workspace
set -a; source .env; set +a
./target/debug/payday-dev-identity
```

Start `gatewayd` in another shell so it fetches the local signing key and
applies the database schema:

```bash
set -a; source .env; set +a
./target/debug/gatewayd
```

Then mint an account and key with `scripts/local-api-key.sh` (it reads
`DATABASE_URL`), or sign in through the web app against the development Privy
app. Production merchant sign-in is Privy too; see `docs/authentication.md`.

In another terminal:

```bash
set -a; source .env; set +a
PAYDAY_INDEXER_POLL_INTERVAL_MS=1000 ./target/debug/gateway-indexer
```

### 5. Create a payment

Expirations must be at least ten minutes and at most a year ahead.

```bash
./target/debug/payday --json create \
  --chain-id 31337 \
  --token 0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512 \
  --payout 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 \
  --expires-in 3600 \
  --amount 1.5
```

The response's `recovery_address` is the configured Payday recovery wallet;
there is no flag to choose it.

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
  -e MC_HOST_local=http://payday-local:payday-local@127.0.0.1:9000 \
  minio/mc tag set local/payday-attachments-local/uploads/<account_id>/<attachment_id>.pdf \
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
cargo build -p gateway-core --bin derive-address
forge test
just web-check   # SDK and web app: build, types, lint, unit tests
```

Coverage includes exact, partial, and overpayment funding; the overpayment
split between beneficiary and recovery and the `recovered_funds` ledger;
deployment code-hash and bound-factory verification at startup; finality-tag
and confirmation gating; multi-range draining; range replay idempotency; chain
isolation; expiry by block timestamp; settlement, recovery, third-party
execution, and late-fund collection through finalized receipts; failure
classification (paused, blacklisted, underfunded, unknown); same-nonce fee
replacement, abandoned nonces, and the paused sweep worker; API validation;
CREATE3 address parity; and `BatchSweeper` under the production gas budget.

## Configuration

- `DATABASE_URL`
- `PAYDAY_API_KEY` — CLI-only per-account bearer key for payment requests
- `PAYDAY_PRIVY_APP_ID` — the Privy app merchants sign in to; `gatewayd`
  verifies dashboard sessions against its published keys, fetched at startup
  (see `docs/authentication.md`). `just dev` sets the development app; unset,
  only API keys authenticate, which is how `just e2e` runs
- `PAYDAY_PAYER_AUTH0_ISSUER`, `PAYDAY_PAYER_AUTH0_AUDIENCE`,
  `PAYDAY_PAYER_AUTH0_CLIENT_ID`, `PAYDAY_PAYER_REF_MASTER_KEY` — the payer
  email-verification audience and the payer-reference key, set together or not
  at all; `just dev` and `just e2e` point them at the development provider's
  `payday-payer-local` client and audience
- `PAYDAY_HOSTED_CHECKOUT_ORIGIN` — the one browser origin the payer
  verification writes answer to; defaults to `PAYDAY_PUBLIC_BASE_URL`
- `PAYDAY_DIDIT_API_KEY`, `PAYDAY_DIDIT_WORKFLOW_ID`,
  `PAYDAY_DIDIT_WEBHOOK_SECRET` (optional `PAYDAY_DIDIT_BASE_URL`) — the
  identity provider, set together or not at all; `just dev` leaves them unset,
  so identity start answers `verification_unavailable` locally unless you
  export a Didit sandbox key, and the hosted callback then needs a public URL
- `PAYDAY_ADMIN_REVIEWER_ID` — recorded as the reviewer on
  `POST /v1/admin/verifications/{id}/decision`; defaults to `operator`
- `PAYDAY_DEV_IDENTITY` — set to `1` only locally to permit a loopback HTTP
  issuer; non-loopback HTTP issuers remain rejected
- `PAYDAY_DEV_IDENTITY_BIND` — loopback socket for the development provider
- `PAYDAY_DEV_IDENTITY_ISSUER` — token issuer; must exactly match
  `PAYDAY_PAYER_AUTH0_ISSUER`
- `PAYDAY_DEV_IDENTITY_OTP` — the code the development provider emails for
  every payer or issuer-mailbox verification, instead of a random one it only
  prints to its log (`DEV IDENTITY OTP <email> <code>`). Local convenience
  only; the provider refuses to bind anything but loopback, and Auth0 issues
  the real codes
- `PAYDAY_CHAIN_ID`
- `PAYDAY_FACTORY_ADDRESS`
- `PAYDAY_BATCH_SWEEPER_ADDRESS`
- `PAYDAY_FACTORY_CODE_HASH`, `PAYDAY_BATCH_SWEEPER_CODE_HASH` — keccak256 of
  the runtime bytecode at the two addresses
  (`cast keccak "$(cast code <ADDRESS> --rpc-url <RPC_URL>)"`); both services
  compare them with the live chain at startup, also checking that
  `BatchSweeper.factory()` is `PAYDAY_FACTORY_ADDRESS`, and refuse to start on
  a mismatch. `just dev` and `just e2e` compute them from the running chain
- `PAYDAY_RECOVERY_ADDRESS` — the Payday recovery wallet `gatewayd` stamps on
  every invoice; a nonzero address, Anvil account #5 locally, the recovery KMS
  key's address in production
- `PAYDAY_USDC_ADDRESS` — exact Circle native-USDC proxy in production
- `PAYDAY_PUBLIC_BASE_URL` — origin serving the hosted checkout, which is where
  payment links point and where `GET /pay/{id}` redirects; `http://127.0.0.1:3002`
  locally, `https://payday.sh` in production. Must be a bare origin, and HTTPS
  unless it is loopback
- `PAYDAY_EXPLORER_BASE_URL` — optional HTTPS explorer origin; production
  Monad uses `https://monadvision.com`, while Anvil leaves it unset
- `PAYDAY_USDC_START_BLOCK` — required; the block to start indexing from on a
  fresh database (the current block at first deployment)
- `PAYDAY_RPC_URL` — QuickNode HTTPS URL in production, Anvil locally; read by
  both services (`gatewayd` uses it for deployment verification and skips it,
  along with the recovery address and code hashes, when
  `PAYDAY_STATUS_ONLY=true`)
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
- `PAYDAY_ATTACHMENT_BUCKET` — S3 bucket holding invoice PDFs;
  `payday-attachments-local` on the runner's MinIO
- `PAYDAY_ATTACHMENT_S3_ENDPOINT`, `PAYDAY_ATTACHMENT_S3_FORCE_PATH_STYLE` —
  optional endpoint override and path-style addressing, set locally to reach
  MinIO at `http://127.0.0.1:9000`; unset in production
- `PAYDAY_ATTACHMENT_DOWNLOAD_TTL_SECS` — lifetime of signed download URLs,
  default 300
- `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION` — MinIO's root
  credentials locally (`payday-local`, region `us-east-1`); the ambient ECS
  task role in production
- `PAYDAY_ATTESTATION_SIGNER_KEY` — local key signing Proof of Payment
  verification attestations (Anvil account #6, whose address
  `0x976EA74026E726554dB657fA54763abd0C3a0aa9` is the local trusted attestor);
  mutually exclusive with `PAYDAY_ATTESTATION_KMS_KEY_ID`, the production KMS
  secp256k1 key ARN. Exactly one is required unless `PAYDAY_STATUS_ONLY=true`
- `PAYDAY_ONBOARDING_PAYER_KEY` — local key that pays the dashboard onboarding
  walkthrough's one self-issued deposit request (Anvil account #1); mutually
  exclusive with `PAYDAY_ONBOARDING_PAYER_KMS_KEY_ID`, the production KMS
  key. Unlike attestation, both may be unset in any environment, including
  production — that simply disables `POST /v1/payments/{id}/onboarding-payment`

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
- Payment listing is cursor-paginated and bounded to 100 records per request.
