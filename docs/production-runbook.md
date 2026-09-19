# Deployment runbook (Vercel + AWS + Monad, Base, Arbitrum One)

This is the deployment path for one operator: the web app on Vercel, every
backend service on AWS through the Terraform in `infra/`, the Auth0 tenant
through the Terraform in `auth0/`, and the two contracts, at the same
addresses, on each network a payer may choose: Monad, Base, and Arbitrum
One.
Docker images contain the Rust services; AWS KMS owns the non-exportable
sweep key and Proof of Payment attestation key. Do not accept a real deposit
until the final end-to-end test in this runbook succeeds.

Gum is pre-release with no real users, and this release treats the
database accordingly: the schema ships as one baseline migration and there
is no upgrade path from any earlier pre-release database. Every existing
environment is recreated empty (§7).

One thing Gum holds and two it never does: the sweep signer holds only
gas (MON on Monad, ETH on Base and Arbitrum One); the requested amount moves directly from the deposit address to the
merchant; and overpayment remainders, expired balances, and late transfers
go back on-chain to the payer's own attested wallet, which is every deposit
address's recovery term. Gum custodies no stablecoin.

## What runs where

| Component | Runs on | Public name | Source |
|---|---|---|---|
| Landing page, hosted checkout (`/pay/{id}`), merchant dashboard (`/dashboard`) | Vercel project rooted at `web/` | `gum.money`, `www.gum.money` | `web/` |
| Merchant and payer API (`gum-server`) | ECS Fargate service `api` behind ALB + WAF | `api.gum.money` | `crates/gum-server` |
| Stablecoin indexer (`gum-indexer`) | ECS Fargate service `indexer`, one task running one read-only chain observer per network; no database, no keys; reports to the API's internal listener over Cloud Map DNS | none | `crates/gum-indexer` |
| Transaction executor (`gum-signers`) | ECS Fargate service `signers`, one task; the only principal that can `kms:Sign` with the sweep pool; consumes execution commands from the Postgres bus and publishes execution events; no inbound access | none | `crates/gum-signers` |
| Database (domain tables, `bus.*`, `execution.*`) | RDS PostgreSQL, private subnets, TLS to the pinned RDS CA; reachable from `api` and `signers` only | none | `crates/gum-schema/migrations` |
| Schema migrations | ECS task definition `migrate` (the `api` image running `gum-server migrate`), run once per deploy by `scripts/run-migrate-task.sh` before the services roll | none | `crates/gum-schema` |
| Deposit request attachments (PDF) | S3 bucket `gum-invoice-attachments` scanned by GuardDuty Malware Protection | virtual-hosted bucket URL, browser PUT only | `infra/` |
| Signing keys | KMS secp256k1 keys: sweep signer, attestation signer; a symmetric key for attachments; a legacy recovery key pending removal | none | `infra/` |
| Merchant notification email | SES identity for `gum.money` | `alerts@gum.money` | `infra/` |
| Payer deposit request email | Resend, with the API's own key (`resend_api_key`) | `contact@gum.money` | `infra/` |
| Merchant sign-in | Privy app (email code, embedded wallet, identity token) | | `docs/authentication.md` |
| Payer and issuer-mailbox codes | Auth0 tenant (Terraform in `auth0/`) sending through Resend | | `auth0/README.md` |
| Contracts | `PaymentFactory` + `BatchSweeper`, one generation, at the same addresses on Monad, Base, and Arbitrum One | | `foundry/` |
| RPC | One QuickNode paid endpoint per network, each its own Secrets Manager secret | | |
| DNS | `gum.money` at Vercel DNS; a Route53 public hosted zone for `api.gum.money` delegated from it | | `infra/` |

The `api` service (and the `migrate` task) runs the `gum-server` image, the
`indexer` service the `gum-indexer` image, and the `signers` service the
`gum-signers` image. All three are targets of the root `Dockerfile` and are
tagged `git-<full SHA>`. How the three divide the work, and what each one
does when killed and restarted, is [`architecture.md`](architecture.md).

## Accounts and assets the operator must provide

1. **AWS account** with billing enabled. Protect the root user with MFA, use
   IAM Identity Center or another non-root administrator identity for daily
   work, and configure the AWS CLI with short-lived SSO credentials.
2. **Vercel account** with a project for `web/` (§9) and the `gum.money`
   domain attached to it.
3. **The `gum.money` domain at Vercel**, where it is registered and where its
   DNS is hosted. Only `api.gum.money` is delegated to a Route53 hosted zone
   (§5); everything else, including the SES DKIM records, is a record in
   Vercel DNS.
4. **QuickNode account** with a paid HTTPS endpoint for each of Monad
   Mainnet, Base Mainnet, and Arbitrum One. The indexer needs
   `eth_blockNumber`, `eth_getLogs`, block lookup, call, transaction
   submission, receipt methods, and `eth_subscribe("logs")` over the
   derived `wss://` URL.
5. **GitHub repository.** This repository already satisfies that
   requirement; GitHub Actions runs CI.
6. **A fresh deployment wallet**, funded with a little MON on Monad and a
   little ETH on Base and on Arbitrum One. It must have sent no transaction
   on any of the three chains (nonce 0 everywhere) so the generation lands
   at the same addresses on each (§2); an encrypted Foundry keystore
   created for this purpose is the simplest way to guarantee that. The KMS
   sweep signer also needs a deliberately small gas balance on each chain
   after deployment.
7. **A wallet with a small amount of native USDC** on each network, and of
   USDT0 on Monad and Arbitrum One, for the production smoke deposits. Both
   can come from a supported exchange or bridge.
8. **A Privy app** for merchant sign-in, an **Auth0 tenant**, and a **Resend
   account** for the embedded passwordless email code that payers and issuer
   mailboxes prove themselves with, all configured as described in
   [authentication.md](authentication.md). Resend must verify a
   Gum-owned sending domain before real payers verify. Apply the
   reviewable Auth0 attack-protection root and complete its launch checks in
   [`auth0/README.md`](../auth0/README.md); enabled tenant defaults alone are
   not deployment evidence.
9. Optionally a **WalletConnect (Reown) project ID** so the checkout can
   offer phone wallets.

No Docker Hub, Terraform Cloud, separate PostgreSQL vendor, Circle account, or
third-party key-management account is required.

## Production hostnames and DNS

- `https://gum.money` (and `www`) is the Vercel-hosted web app: landing
  page, hosted checkout, and dashboard. Every `deposit_url` points here, the
  API allows merchant-route CORS from exactly this origin, and the attachment
  bucket answers browser preflights from exactly this origin
  (`checkout_base_url`).
- `https://api.gum.money` is the public API behind AWS WAF and the
  Application Load Balancer. Merchant servers and the SDK call it with an API
  key; the dashboard and checkout call it from the browser.
- The indexer and the database have no public hostname.

`gum.money` stays at Vercel: Vercel is the registrar, hosts the zone, and
serves the web app on the apex and `www` with no records to add by hand.
Terraform needs Route53 only for `api.gum.money`: it creates the alias
record and the ACM validation record in the zone named by `route53_zone_id`,
which is a hosted zone for `api.gum.money` delegated from Vercel DNS with
four `NS` records (§5). Terraform touches nothing else in DNS. The one other
record set the stack needs, the three SES DKIM `CNAME`s under `gum.money`,
is published as a Terraform output and added in Vercel DNS by hand (§10).

Wait until `dig NS api.gum.money` returns the Route53 nameservers before the
full Terraform apply, because ACM cannot validate the certificate until the
delegation is publicly visible.

## Fixed values per network

The `chains` list in the tfvars carries these; the payer sees the networks
in that order. Every served stablecoin has six decimals on all of them.

| | Monad | Base | Arbitrum One |
|---|---|---|---|
| Chain ID | `143` | `8453` | `42161` |
| Gas token | `MON` | `ETH` | `ETH` |
| Circle native USDC (`USDC`) | `0x754704Bc059F8C67012fEd69BC8A327a5aafb603` | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | `0xaf88d065e77c8cC2239327C5EDb3A432268e5831` |
| Tether USDT0 (`USDT`) | `0xe7cd86e13AC4309349F30B3435a9d337750fC82D` | not served | `0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9` |
| `finality_source` / `finality_confirmations` | `finalized` / `0` | `latest` / `10` | `latest` / `40` |
| `block_time_ms` | `300` | `2000` | `250` |
| `log_range_size` | `100` | `10000` | `10000` |
| Explorer | `https://monadvision.com` | `https://basescan.org` | `https://arbiscan.io` |

- [Monad full finality](https://docs.monad.xyz/monad-arch/consensus/block-states):
  the node's `finalized` tag is "irreversible without a hard fork", so
  Monad uses it with no margin: a margin would cost a second header read
  per pass and buy nothing on this chain. `finalized` trails `latest` by
  two blocks (about 600 ms). `latest` on Monad is the speculatively
  executed proposed block and is never used for commits.
- Base and Arbitrum One settle on `latest` minus a confirmation depth
  (about 20 and 10 seconds): their `finalized` tag means L1 finality, ten
  to twenty minutes behind, and the product decision is to trust the
  sequencer's ordering, as exchange deposits do, with the margin absorbing
  the sequencer's own reorgs. On every chain `eth_getLogs` is filtered to
  the addresses Gum is watching, 500 per call, with every configured
  token contract in one address array, so RPC spend follows Gum's
  activity and not the chain's stablecoin volume. One worker, one cursor,
  and one socket serve a chain however many currencies it lists.
- Detection is push-driven on every chain: while a chain has something to
  watch, the indexer holds a WebSocket to that chain's QuickNode endpoint
  (`wss://` derived from `GUM_RPC_URL_<chain_id>`) subscribed to
  transfers of its configured tokens to its own payment addresses
  (`monadLogs` on Monad, `logs` elsewhere), and wakes a range scan the
  moment one lands. The scan also
  runs every `GUM_INDEXER_RECONCILE_INTERVAL_MS` (60 s) as the backstop
  and the only writer; the socket has no ledger authority. A chain with
  nothing to watch holds no socket and only advances its cursor every
  `GUM_INDEXER_IDLE_INTERVAL_MS` (5 min). Expect about 0.9k calls a day
  per idle chain, ~14k for an active Monad and ~7–9k for an active L2
  (`docs/runbooks/quicknode-rpc-limits.md`), and `transfer signal
  connected` with the `chain_id` in the indexer log once a chain becomes
  active.
- [Monad RPC differences](https://docs.monad.xyz/reference/rpc-differences):
  QuickNode allows 100 blocks per `eth_getLogs` (`log_range_size`),
  `eth_getTransactionByHash` returns nothing for a transaction still in
  flight, and the `pending` tag reads like `latest`. `gum-signers`
  therefore treats a helper transaction without a receipt after
  `GUM_SWEEP_PENDING_TIMEOUT_SECS` as replaceable on the same nonce and
  detects a consumed nonce from the signer's mined transaction count.
- Monad bills the gas *limit*: every helper transaction reserves
  `100k + 400k × items` gas of MON from the pool signer that sends it.
  Base and Arbitrum bill gas used plus the L1 data fee.

Reconfirm every USDC address against
[Circle's official contract-address page](https://developers.circle.com/stablecoins/usdc-contract-addresses)
and every USDT0 address against
[Tether's USDT0 deployments page](https://docs.usdt0.to/technical-documentation/deployments)
before every new production environment. USDT0 is Tether's omnichain USDT,
backed 1:1 by USDT locked on Ethereum; Base carries USDC only, because
Base's USDT is a bridge wrapper without EIP-3009 and is not served as a
deposit currency (payers may still pay from it through Relay). Both services
also read each contract's `decimals()`, `name()`, and `version()` at
startup (a missing `version()` getter is tolerated by trying `"1"` then
`"2"` against `DOMAIN_SEPARATOR`; USDT0 has none, and hashes under the name
`USDT0` on Monad and `USD₮0` on Arbitrum with version `"1"`) and refuse to
start if the chain disagrees with the configured currency.

## Order of operations

1. Install operator tools and verify the AWS account.
2. Deploy `PaymentFactory` and `BatchSweeper` to each network, from one
   fresh key.
3. Configure Privy, Resend, and Auth0.
4. Create Terraform state storage.
5. Delegate `api.gum.money` to Route53 (already done for the current
   deployment).
6. Choose the first index block and configure Terraform.
7. Bootstrap ECR, push images, and create the AWS stack against an empty
   database.
8. Verify and fund the KMS signers; publish the attestor address.
9. Deploy the web app to Vercel and attach the domain.
10. Verify attachment scanning and email delivery, then run the end-to-end
    test.

Steps 2 through 5 are independent of each other and can be done in any
order; everything after depends on all of them.

## 1. Install operator tools

Install Docker, Terraform 1.10+, AWS CLI v2, Rust, Foundry, Node 22 with npm,
and optionally the Vercel CLI. Authenticate with AWS and confirm the intended
account before creating anything:

```bash
aws configure sso
aws sso login
aws sts get-caller-identity
```

Never run production Terraform while authenticated to an account you have not
explicitly verified.

## 2. Deploy PaymentFactory and BatchSweeper to each network

`PaymentFactory` and its immutable `BatchSweeper` helper have no owner or
privileged administrative key. Use a dedicated deployment wallet rather than
the KMS sweep key, and retain its transaction record even though it has no
post-deployment authority.

The generation must sit at the **same addresses on every chain**. A deposit
address commits to the chain the payer chose, and the `Payment` contract
refuses to route funds anywhere else; if a payer nevertheless sends the
token to that address on another network, it can be returned only by deploying the
`Payment` there through a factory at the identical address
(`runbooks/wrong-network-deposit.md`). The deployment script enforces the
precondition: the deployer must have nonce 0 on the target chain, so the
factory lands at its nonce-0 CREATE address and the sweeper at nonce 1.
Use one fresh key and run the script once per chain, in any order.

Every counterfactual deposit address is derived from the factory address, so
a factory can never be replaced once a real deposit request exists: deploy the
final `DepositRequest`/`PaymentFactory` code before the first production
deposit request, and treat any later contract change as a new deployment with
its own database.

`PaymentFactory` and `BatchSweeper` are one **contract generation**: the
factory embeds `DepositRequest`'s creation code and the sweeper is bound to one
factory. Always deploy the two together with the same script, and never point
a new factory at an old sweeper. Both services pin the generation by the
keccak256 of each contract's runtime bytecode and by the sweeper's bound
factory, and refuse to start if the chain disagrees; a change of generation
needs a fresh database, not a configuration edit. The token is not part of
the generation: `PaymentFactory`, `BatchSweeper`, and `Payment` take it per
call, so adding a currency to a live chain is a `tokens` entry in that
chain's configuration, with no redeploy and no backfill (no request could
have offered the currency before it was configured).

For an encrypted Foundry keystore:

```bash
cast wallet import gum-deployer --interactive
```

Fund the displayed address with enough gas for two contract deployments on
each chain (MON, ETH, ETH). Confirm it is fresh everywhere, then deploy
chain by chain:

```bash
export MONAD_RPC_URL='https://your-quicknode-monad-endpoint'
export BASE_RPC_URL='https://your-quicknode-base-endpoint'
export ARBITRUM_RPC_URL='https://your-quicknode-arbitrum-endpoint'
for rpc in "$MONAD_RPC_URL" "$BASE_RPC_URL" "$ARBITRUM_RPC_URL"; do
  cast nonce "$(cast wallet address --account gum-deployer)" --rpc-url "$rpc"   # must print 0
done

GUM_CHAIN_ID=143 forge script foundry/script/PaymentFactory.s.sol:PaymentFactoryScript \
  --rpc-url "$MONAD_RPC_URL" --account gum-deployer --broadcast
GUM_CHAIN_ID=8453 forge script foundry/script/PaymentFactory.s.sol:PaymentFactoryScript \
  --rpc-url "$BASE_RPC_URL" --account gum-deployer --broadcast
GUM_CHAIN_ID=42161 forge script foundry/script/PaymentFactory.s.sol:PaymentFactoryScript \
  --rpc-url "$ARBITRUM_RPC_URL" --account gum-deployer --broadcast
```

The script aborts if the RPC chain ID is not `GUM_CHAIN_ID` or the
deployer's nonce is not 0. Save both resulting addresses, the deployment
transactions, and both code hashes from each run; the addresses must be
identical across the three chains, and the code hashes normally are too.
Verify that code exists at each address on each chain and recompute the
hashes from the chain, which is what the services will compare against:

```bash
for rpc in "$MONAD_RPC_URL" "$BASE_RPC_URL" "$ARBITRUM_RPC_URL"; do
  cast keccak "$(cast code <FACTORY_ADDRESS> --rpc-url "$rpc")"
  cast keccak "$(cast code <BATCH_SWEEPER_ADDRESS> --rpc-url "$rpc")"
  cast call <BATCH_SWEEPER_ADDRESS> 'factory()(address)' --rpc-url "$rpc"
done
```

An empty `0x` result means deployment verification failed; stop there. The
`factory()` call must return the factory you just deployed. Configure the
addresses as `factory` and `batch_sweeper` and the hashes as
`factory_code_hash` and `batch_sweeper_code_hash` in each entry of the
`chains` list in Terraform.

### The WithdrawalForwarder

USDC withdrawals that cross networks burn through `WithdrawalForwarder`
(`foundry/src/WithdrawalForwarder.sol`), a stateless, ownerless contract
bound to Circle's `TokenMessengerV2` (`0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d`
on Monad, Base, and Arbitrum). CCTP, the forwarder, and its 10,000,000
per-message limit exist for USDC only: a USDT withdrawal moves the
destination network's balance in one same-chain leg, and balances on other
networks are withdrawn separately to an address there. It is deployed like
the factory, from a
second fresh key at nonce 0, so it has one address everywhere and one runtime
code hash to pin:

```bash
cast wallet import gum-forwarder-deployer --interactive
for rpc in "$MONAD_RPC_URL" "$BASE_RPC_URL" "$ARBITRUM_RPC_URL"; do
  cast nonce "$(cast wallet address --account gum-forwarder-deployer)" --rpc-url "$rpc"   # must print 0
done
GUM_CHAIN_ID=143 forge script foundry/script/WithdrawalForwarder.s.sol:WithdrawalForwarderScript \
  --rpc-url "$MONAD_RPC_URL" --account gum-forwarder-deployer --broadcast
GUM_CHAIN_ID=8453 forge script foundry/script/WithdrawalForwarder.s.sol:WithdrawalForwarderScript \
  --rpc-url "$BASE_RPC_URL" --account gum-forwarder-deployer --broadcast
GUM_CHAIN_ID=42161 forge script foundry/script/WithdrawalForwarder.s.sol:WithdrawalForwarderScript \
  --rpc-url "$ARBITRUM_RPC_URL" --account gum-forwarder-deployer --broadcast
for rpc in "$MONAD_RPC_URL" "$BASE_RPC_URL" "$ARBITRUM_RPC_URL"; do
  cast keccak "$(cast code <FORWARDER_ADDRESS> --rpc-url "$rpc")"
done
```

Put the address and hash in each chain's `cctp` block in `terraform.tfvars`,
with Circle's domain (Monad 15, Base 6, Arbitrum 3) and the two CCTP
contracts, which are the same on all three chains. A `cctp` block requires
USDC among that chain's `tokens`. Both services verify the
forwarder's code hash at startup like the factory's. A chain without a
`cctp` block still serves same-network withdrawals; `POST /v1/withdrawals`
answers `withdrawals_unavailable` when a USDC leg would have to bridge from
or to it. Before the first production withdrawal, run the fork tests
against the live chains (`just forge-fork`, which also proves the USDT0
contracts accept the withdrawal authorization) and one real withdrawal on
staging (`docs/staging.md`).

## 3. Configure Privy, Resend, and Auth0

Follow [authentication.md](authentication.md) §1 through §3, in that order.
The outputs this runbook needs:

- the Privy **App ID** (public), with `https://gum.money` among the app's
  allowed domains, email login only, embedded wallets created on login, and
  identity tokens enabled;
- the Auth0 tenant issuer, `https://<tenant-domain>/`;
- the `Gum Payer` API identifier (`https://api.gum.money/payer`) and the
  `Gum Payer Verification` application's client ID, both created by the
  Terraform in `auth0/` (`terraform -chdir=auth0 apply`, with its own state
  key as described in [`auth0/README.md`](../auth0/README.md));
- a Resend sending domain verified and connected to Auth0 as its email
  provider. The API needs a Resend API key of its own as well (§6): it
  emails payers their deposit requests from `contact@gum.money`, so the
  verified domain must cover that address.

The payer verification settings are optional to Terraform as a group: leave
`payer_auth0_audience` and `payer_auth0_client_id` unset and the API still
serves permissionless and merchant-session deposit requests, but gated
requests and issuer-mailbox verification answer `503
verification_unavailable`. Set both before launch.

## 4. Create protected Terraform state storage

Create one private S3 bucket for Terraform state. Enable versioning, default
encryption, and Block Public Access. The bucket is intentionally not created
by any Terraform state here. Create `infra/backend.hcl` and `auth0/backend.hcl`
as shown in `infra/README.md` and `auth0/README.md`, with distinct keys; both
files are ignored by Git.

For one operator, S3 native state locking (`use_lockfile = true`) is
sufficient. Restrict bucket access to the deployment identity and recovery
administrator. Terraform stores generated credentials and the RPC URL in its
encrypted state: treat state files and saved plans as secrets.

## 5. Delegate `api.gum.money` to Route53

This exists already for the current deployment; `route53_zone_id` in the
untracked `infra/terraform.tfvars` is that zone. For a new environment,
create a public hosted zone for the API name:

```bash
aws route53 create-hosted-zone --name api.gum.money \
  --caller-reference "gum-api-$(date +%s)" \
  --query '{zone: HostedZone.Id, nameservers: DelegationSet.NameServers}'
```

Record the zone ID (drop the `/hostedzone/` prefix) as `route53_zone_id`.
In Vercel's DNS settings for `gum.money`, add four `NS` records with name
`api`, one per Route53 nameserver. Do not change the nameservers of
`gum.money` itself. Wait for `dig NS api.gum.money` to return the Route53
names.

## 6. Choose the first index block and configure Terraform

Immediately before the first deployment, record the current block on each
chain:

```bash
cast block-number --rpc-url "$MONAD_RPC_URL"
cast block-number --rpc-url "$BASE_RPC_URL"
cast block-number --rpc-url "$ARBITRUM_RPC_URL"
```

Use each value as that entry's `start_block`. No deposit requests can
predate the first launch, so scanning older transfers would waste RPC
requests without finding a payable deposit request.

```bash
cp infra/terraform.tfvars.example infra/terraform.tfvars
```

Replace every placeholder in `terraform.tfvars`, including:

- AWS region and alert email
- `route53_zone_id`, the `api.gum.money` zone from §5
- `checkout_base_url = "https://gum.money"`, the Vercel-hosted site
- `image_tag = "git-<full commit SHA>"` of the commit you will build in §7
- the `chains` list: one entry per network with the deployed `factory`
  and `batch_sweeper`, their `factory_code_hash` and
  `batch_sweeper_code_hash` from §2, that chain's `start_block`, its
  `tokens` (`[{currency = "USDC", address = …}, {currency = "USDT",
  address = …}]`), and the fixed values from the table above. Both tasks receive
  the list as `GUM_CHAINS`, whose entries carry
  `tokens: [{"currency":"USDC","address":"0x…"},{"currency":"USDT","address":"0x…"}]`
  and `start_block`. Each `tokens` address must be the issuer's **canonical
  deployment** of that currency on that chain (Circle's native USDC;
  Tether's USDT0), never a bridged look-alike: startup verifies each
  contract's decimals and its own EIP-712 domain, but no on-chain call can
  tell *which* issuer an address belongs to — both are self-consistent — so
  a swapped address starts cleanly and mislabels every request. Copy the
  addresses from the issuer's own documentation (see
  `docs/deposit-safety.md`) and double-check them before applying
- `privy_app_id`, `auth0_issuer`, `payer_auth0_audience`, and
  `payer_auth0_client_id` from §3
- `admin_reviewer_id`, who operator decisions are recorded against

`notification_domain_name` and `notification_from_address` default to
`gum.money` and `alerts@gum.money`. Supply the RPC URLs, one per chain id,
without writing them to the tfvars file; Terraform stores each as its own
Secrets Manager secret and injects it as `GUM_RPC_URL_<chain_id>`:

```bash
export TF_VAR_rpc_urls='{"143":"'"$MONAD_RPC_URL"'","8453":"'"$BASE_RPC_URL"'","42161":"'"$ARBITRUM_RPC_URL"'"}'
export TF_VAR_resend_api_key="$RESEND_API_KEY"
```

The Resend key is optional to Terraform: leave it unset and the stack passes
no `GUM_RESEND_API_KEY`, so the API queues each payer's deposit request
email but sends none until a key is configured. `payer_email_from` defaults
to `Gum <contact@gum.money>`.

## 7. Bootstrap ECR, push images, and create the AWS stack

The ECR repositories must exist before images can be pushed, while ECS cannot
start until those images exist:

```bash
terraform -chdir=infra init -backend-config=backend.hcl
terraform -chdir=infra apply \
  -target=aws_ecr_repository.api \
  -target=aws_ecr_repository.indexer \
  -target=aws_ecr_repository.signers

api_repo=$(terraform -chdir=infra output -raw api_ecr_repository_url)
indexer_repo=$(terraform -chdir=infra output -raw indexer_ecr_repository_url)
signers_repo=$(terraform -chdir=infra output -raw signers_ecr_repository_url)
tag="git-$(git rev-parse HEAD)"

scripts/push-images.sh <AWS_REGION> "$api_repo" "$indexer_repo" "$signers_repo" "$tag"
```

The push script requires exactly `git-<full HEAD SHA>` and refuses a dirty
worktree. Commit and review every source change before building. This makes
the image-to-source relationship and rollback deterministic.

### The database starts empty, and nothing migrates on start

The schema is `crates/gum-schema/migrations/` (`0001_application.sql`,
`0002_bus.sql`, `0003_execution.sql`), embedded in the `gum-server` image
and applied only by `gum-server migrate`. No service applies migrations
when it starts; instead every service's `/health/ready` refuses (`schema
behind`) until the schema is at the version its binary expects, so a
service that rolls before the migration ran fails its health check and the
deployment circuit breaker stops it. There is no migration from any earlier
pre-release schema.

- **New environment:** Terraform creates the RDS instance with an empty
  `gateway` database. Register the `migrate` task definition and run it
  before the services can become healthy:

  ```bash
  terraform -chdir=infra apply -target=aws_ecs_task_definition.migrate
  scripts/run-migrate-task.sh <NAME>    # runs `gum-server migrate`, waits, prints its log
  ```

- **Existing pre-release environment:** scale the `api`, `indexer` and
  `signers` services to zero, then empty the database through the
  procedure in [`db-access.md`](runbooks/db-access.md):

  ```sql
  DROP SCHEMA public CASCADE; CREATE SCHEMA public;
  DROP SCHEMA bus CASCADE;
  DROP SCHEMA execution CASCADE;
  ```

  This discards every pre-release account, key, deposit request, and
  observation, which is the intent. Run the migrate task as above, then
  restore the desired counts with the full apply below. Pre-release
  attachments left in the S3 bucket are orphaned by this: empty the
  `uploads/` prefix as well.

### Full apply

```bash
terraform -chdir=infra fmt -check
terraform -chdir=infra validate
terraform -chdir=infra plan -out=deploy.tfplan
terraform -chdir=infra apply deploy.tfplan
```

Review the plan before applying it. In particular, reject unexplained database
replacement or destruction, IAM permissions broader than the named secrets,
the attachment bucket's `uploads/` prefix, and the named KMS keys (only the
API task role may sign with the attestation key, only the **signers** task
role with the sweep pool, and nothing with the legacy recovery key), a
database secret attached to the indexer task, an indexer or signers count
other than one, or plaintext or non-HTTPS endpoints.

AWS creates the TLS certificate and DNS records, ALB and WAF, the three ECS
services and the `migrate` task definition, the Cloud Map namespace the
indexer resolves `api.<name>.local` through, the RDS database, Secrets
Manager values (including the generated `internal_token` the indexer
presents to the API), alarms, KMS keys, the SES identity, the attachment
bucket, and its GuardDuty Malware Protection plan. All three services
verify the contract generation against the chain as they start: if a task
loops on a code-hash or bound-factory refusal, the tfvars and the
deployment disagree. Fix the values, never the check. WAF request
sampling is disabled so the bearer header is not retained in samples. RDS
connections verify the server certificate against the checksum-pinned AWS
RDS CA bundle in the container. Confirm the SNS subscription link sent to
the configured alert email; alarms do not deliver until it is confirmed.

The stack still declares a `recovery` KMS key. It is legacy: before deposits
returned excess funds to the payer's own wallet, it was every deposit
request's recovery term. Nothing reads its address any more and no task role
can sign with it. Keep it only until any balance it holds from that period
has been returned by hand, then remove its `prevent_destroy` guard and the
resource.

## 8. Verify and fund the KMS signers

### Sweep signer pool

`gum-signers` executes with `sweep_signer_count` KMS keys (five in
production), each an address that keeps one transaction in flight per
chain, so several sweep batches and withdrawal steps run at once on every
chain. At boot it logs one `configured signer` line per key and chain with
the Ethereum address derived from the KMS public key:

```bash
aws logs tail /ecs/gum/signers --since 15m \
  --filter-pattern 'configured signer'
```

Independently derive every address with Foundry's AWS KMS support:

```bash
for arn in $(terraform -chdir=infra output -json kms_key_arns | jq -r '.[]'); do
  AWS_KMS_KEY_ID="$arn" cast wallet address --aws
done
```

The addresses must match the log, in order. Each is one address on every
chain, and each sweeps on every chain, so fund **every** address on each
chain: only enough MON on Monad and ETH on Base and Arbitrum One for expected
sweeps, spread over the pool (free signers are offered richest first, so
they drain evenly). `gum-signers` warns per signer and chain below the
chain's `signer_low_balance_wei` (`GUM_SIGNER_LOW_BALANCE_WEI` as the
fallback); the `signer balance is low` line names the address and drives
the `signers-low-balance` alarm. The signers do not custody stablecoins;
they pay gas to invoke the permissionless factory. Current balances are
also in `execution.signer_status` and in `GET /v1/status`.

Raising `sweep_signer_count` adds keys at the end of the pool; lowering it
is refused by Terraform's `prevent_destroy`. A key removed from the pool
while it has an open row in `execution.transactions` can still have its
receipts looked up but cannot be fee-bumped, so change the pool only when
`SELECT count(*) FROM execution.transactions WHERE resolved_at IS NULL AND
signer = <address>` is zero, or wait for that lane to resolve.

### Attestation signer

Every Proof of Payment carries a verification attestation signed with the
attestation KMS key. Only the API task role can sign with it and it holds no
funds. Derive its address the same way and verify the derivation
independently:

```bash
export AWS_KMS_KEY_ID="$(terraform -chdir=infra output -raw attestation_kms_key_arn)"
cast wallet address --aws
```

Publish that address as Gum's trusted attestor, in the API documentation
and wherever proofs are downloaded, so merchants and auditors can hand it to
whatever runs `gum_core::verify_proof`; an attestation signed by anything
else must fail verification. The address changes only if the key is
replaced, which changes the trust anchor of every earlier proof, so treat
replacement as an announced cut-over, never as routine rotation.

## 9. Deploy the web app to Vercel

The web app is `web/`, a Next.js app inside the npm workspace at the
repository root; it depends on `@gum/sdk` from `sdk/typescript`, which its
`prebuild` hook builds. Every configuration value is `NEXT_PUBLIC_` and is
inlined into the browser bundle at build time, so nothing secret belongs in
Vercel and a changed value needs a redeploy.

1. Create a Vercel project from the GitHub repository with:
   - **Root Directory** `web`, with "Include source files outside of the Root
     Directory" left enabled (the SDK and the root lockfile live above it);
   - **Framework** Next.js;
   - **Install Command** `cd .. && npm ci`;
   - **Build Command** `cd .. && npm run build --workspace @gum/web`;
   - **Node.js** 22, matching CI;
   - the production branch set to `main`.
2. Set these environment variables for the Production environment (they are
   the same values CI builds with, see `.github/workflows/ci.yml`, plus the
   two that depend on this deployment):

   | Variable | Production value |
   |---|---|
   | `NEXT_PUBLIC_GUM_API_URL` | `https://api.gum.money` |
   | `NEXT_PUBLIC_CHAINS` | the same three networks as `chains`, as a JSON array of `{id, name, rpcUrl, tokens, explorerUrl, confirmation}`, each `tokens` entry `{currency, address, symbol?, decimals?}` matching that chain's `GUM_CHAINS` tokens, with *public* RPCs (`https://rpc.monad.xyz`, `https://mainnet.base.org`, `https://arb1.arbitrum.io/rpc`), never the QuickNode endpoints; `web/.env.staging` has the exact value |
   | `NEXT_PUBLIC_PRIVY_APP_ID` | the production Privy app ID from §3 |
   | `NEXT_PUBLIC_ATTACHMENT_UPLOAD_ORIGIN` | `https://<attachment_bucket_name>.s3.<region>.amazonaws.com`, from `terraform -chdir=infra output -raw attachment_bucket_name` |
   | `NEXT_PUBLIC_PAYER_APPEAL_EMAIL` | a monitored support address |
   | `NEXT_PUBLIC_WALLETCONNECT_PROJECT_ID` | optional; empty offers injected wallets only |

   Do not set `GUM_PRIVY_STUB` (test-only; it replaces Privy with a
   fake).
3. Deploy `main`. The first production deployment must be the same commit as
   the images pushed in §7, or a later one whose API contract they still
   satisfy.
4. Under the project's **Domains**, add `gum.money` and `www.gum.money`
   (redirecting `www` to the apex). Because Vercel also hosts the zone, it
   creates the records itself and reports the domain valid at once.
5. Confirm in Privy that `https://gum.money` is among the app's allowed
   domains. Preview deployments get their own `*.vercel.app` origins: the
   checkout works there because the payer API is public and CORS-open, but
   the dashboard does not, since Privy allows only the listed domains and
   `gum-server` accepts merchant-route requests from `checkout_base_url` only.
   Test dashboard changes locally or on production.

Vercel redeploys on every push to `main`. Because the web app and the API
share one contract through `@gum/sdk`, deploy API changes that remove or
rename fields before the web change that stops sending them, and web changes
that need new fields after the API that provides them.

## 10. Verify, then run the end-to-end test

### Attachment scanning

Terraform created the attachment bucket and a GuardDuty Malware Protection
plan for it; Malware Protection for S3 does not require the GuardDuty
detector to be enabled. Confirm the plan is active with tagging on, and that
GuardDuty turned on the bucket's EventBridge notifications:

```bash
aws guardduty list-malware-protection-plans
aws guardduty get-malware-protection-plan --malware-protection-plan-id <id> \
  --query '{status: Status, tagging: Actions.Tagging.Status}'
aws s3api get-bucket-notification-configuration \
  --bucket "$(terraform -chdir=infra output -raw attachment_bucket_name)"
```

`status` must be `ACTIVE`, `tagging` `ENABLED`, and the notification
configuration must contain `EventBridgeConfiguration`. Then upload a small
PDF through the dashboard (or the SDK's `attachments.upload`) and watch
finalization move from `409 attachment_scan_pending` to an attachment
descriptor within a few minutes; `aws s3api get-object-tagging` on
`uploads/<account_id>/<attachment_id>.pdf` then shows
`GuardDutyMalwareScanStatus=NO_THREATS_FOUND`. Uploading the EICAR test file
must end in `422 attachment_rejected`. Malware findings appear in the
GuardDuty console; nothing in Gum alarms on them.

### Merchant email

Terraform created an SES identity for `gum.money` with Easy DKIM, but its
DNS lives at Vercel, so add the three `CNAME`s yourself:

```bash
terraform -chdir=infra output notification_dkim_records
```

Create each as a `CNAME` in Vercel's DNS settings for `gum.money` (the name
is the `<token>._domainkey` part; the value is the `<token>.dkim.amazonses.com`
host). SES marks the identity verified within an hour of the records
resolving; confirm in **SES → Identities**. Then move the SES account out of
the sandbox in this region and confirm a test message from
`alerts@gum.money` reaches an external mailbox. Deposit creation refuses to create additional unnotifiable
deposits for an account without a verified email, and
`gum-notification-missing-contact` alarms if a deposit request snapshots
no contact.

### Sign in and test the API

Sign in at `https://gum.money` with an emailed code and mint an API key in
the dashboard's API key section ([authentication.md](authentication.md)
§5). Then, with that key:

```bash
export GUM_API_KEY="<the key, from your secret store>"
curl --fail "https://api.gum.money/health"
curl --fail -sS "https://api.gum.money/v1/deposit-requests" \
  -H "Authorization: Bearer $GUM_API_KEY" \
  -H 'Content-Type: application/json' \
  -H "Idempotency-Key: launch-check-$(date +%s)" \
  -d '{"amount":"0.01","payout_address":"<YOUR_PAYOUT_ADDRESS>",
       "issuer":{"name":"Gum"},"payer":{"name":"Launch check"},
       "payer_policy":{"mode":"permissionless"},"expires_in":3600}' | jq
```

The response's `chain`, `token`, and `address` are null until a wallet is
bound, and `networks` lists the three chains. Open the returned
`deposit_url` (on `gum.money`) in a browser, choose a network, connect the
wallet you will pay from (the page switches it to that chain), and sign the
attestation; `GET /v1/deposit-requests/{id}` then carries `chain`, `token`,
`address`, `payer_wallet`, and `recovery_address` (the same wallet), and a
`deposit_request.ready` webhook fires. Pay exactly 0.01 native USDC to that
address from that wallet on that chain. Repeat once per network, and once
more with `"currency":"USDT","chain_id":"143"` (and `42161`) paid in USDT0,
before accepting real deposits. Confirm that:

1. `GET /v1/deposit-requests/{id}` progresses `awaiting_deposit → deposited →
   settled`, with `received_base_units`, `settlement_tx_hash`, `settled_at`,
   and `settled_block` set.
2. The beneficiary receives exactly the requested amount of the request's
   currency.
3. `balanceOf(payment_address)` becomes zero.
4. `cast call payment_address 'settled()(bool)'` returns `true`.
5. A second, small deposit to the same address comes back to the paying
   wallet within a minute while the status stays `settled`, and
   `recovered_funds` records it with reason `late_transfer` (see the
   [smoke test](runbooks/end-to-end-smoke-test.md)).
6. API, indexer, and signers logs contain no repeated errors, and
   `/health/ready` on each answers 200.
7. CloudWatch alarms and RDS backups are configured.
8. `GET /v1/deposit-requests/{id}/proof` returns a proof whose attestation
   `signer` is the address from §8, and `gum_core::verify_proof` accepts
   it with that address as the trusted attestor.

Do not advertise or depend on the service until this succeeds.

## Updates and rollback

### Backend

The `Deploy staging` workflow is the reference procedure; production
follows the same order by hand. For each application update:

1. Build and push the three `git-<SHA>` images (§7).
2. Register the new `migrate` task definition and run it:
   `terraform -chdir=infra apply -target=aws_ecs_task_definition.migrate`
   then `scripts/run-migrate-task.sh <NAME>`. The task exits non-zero and
   the deploy stops here if a migration fails; no service has rolled yet.
3. Change `image_tag`, review `terraform plan`, and apply it. ECS's
   deployment circuit breaker rolls back failed task startups. The
   `indexer` and `signers` services deploy with minimum healthy 0%, so the
   old task stops before the new one starts: two signers never share a
   nonce stream, and two indexers never race the cursor (the cursor is
   compare-and-set, so a race would be harmless, only wasteful).
4. `aws ecs wait services-stable --services api indexer signers`.

A manual rollback sets `image_tag` to a previous known-good image and
applies again (see below for the schema constraint). Secrets Manager
rotation is not observed by running tasks; force a new deployment after
rotating a secret ([secrets-rotation.md](runbooks/secrets-rotation.md)
says which services read which secret).

### Schema changes

Migrations live in `crates/gum-schema/migrations/` and are applied only by
the `migrate` task (`gum-server migrate`), never by a starting service.
Every service's readiness check compares the applied versions with the set
its binary embeds and refuses to become ready while any is missing, so the
ordering above is enforced, not just documented. Until the first real
deposit is accepted, the three files are edited in place; a schema change
recreates every database, which the staging deploy does on its own
([staging.md](staging.md)) and production does as in §7. Once real data
exists, the files are frozen and a change is a new numbered file, reviewed
for compatibility with the image still running while the new one starts.

#### Rolling back after a new migration has applied

The readiness check only requires that every version the image embeds is
applied; it does not object to versions it has never heard of. So an
older image runs against a newer schema as long as the migration was
additive, and a rollback is setting `image_tag` to the previous image and
applying. Do not delete rows from `_sqlx_migrations`. If a migration was
not additive (a dropped or renamed column the old image reads), the
rollback is rolling forward to a fixed image built from the post-migration
commit.

#### Concurrent index builds

A post-freeze migration that builds an index on a large table runs with
`-- no-transaction` and `CREATE INDEX CONCURRENTLY` so the build never takes
a write lock on the table (the current files hold none). The cost of that
is atomicity: a failed build leaves an invalid index and does not record
the version, so every later `migrate` run retries the migration and fails
again — loudly, on purpose: such a migration deliberately omits `IF NOT
EXISTS` so a retry cannot skip a leftover invalid index and record itself as
applied. Recover in this order:

1. Confirm no build is still running:
   `SELECT pid, query FROM pg_stat_activity WHERE query ILIKE '%CREATE INDEX%';`
2. Find the invalid index:
   `SELECT indexrelid::regclass FROM pg_index WHERE NOT indisvalid;`
3. Drop it: `DROP INDEX CONCURRENTLY <name>;`
4. Run the migrate task again.

If a version was ever recorded while its index is invalid (should be
impossible with these migrations, but check
`SELECT version, success FROM _sqlx_migrations ORDER BY version;` against
the invalid index's name), dropping the index is not enough: rebuild it
manually with the same `CREATE INDEX CONCURRENTLY` statement the migration
file carries.

### Web

Vercel deploys `main` automatically and keeps every earlier deployment; a
rollback is "Promote to Production" on a previous deployment or a revert on
`main`. A change to any `NEXT_PUBLIC_` value needs a redeploy because the
value is inlined at build time.

### Contract generation changes

A change to `DepositRequest`, `PaymentFactory`, or `BatchSweeper` is a new
generation, not an update: existing deposit requests are committed to the
old factory and would never match the new one. Deploy the factory and
sweeper together on every chain from a new fresh key (§2), record the new
addresses and code hashes, recreate the database, update every `chains`
entry's `factory`, `batch_sweeper`, `factory_code_hash`, and
`batch_sweeper_code_hash` together, and apply. A
build configured for one generation refuses to start against another, so a
half-updated configuration fails closed rather than settling against the
wrong contracts.

### Adding a currency

Adding a stablecoin to a live chain is a configuration change, not a
generation change: append `{currency, address}` to that chain's `tokens`
(and the matching entry to `NEXT_PUBLIC_CHAINS`), verify the address against
the issuer's page, and apply. The contracts take the token per call, the
database needs no backfill, and the indexer's single per-chain cursor
simply starts watching the new contract. A currency served on no chain
answers `422 unsupported_currency`.

## Sandbox

The same stack can be instantiated a second time against the testnets
(Monad testnet, Base Sepolia, Arbitrum Sepolia), see [sandbox.md](sandbox.md)
and `infra/terraform.sandbox.tfvars.example`. It needs its own Terraform
state key, `name`, Privy app, Auth0 tenant, RPC endpoints, Route53 zone (`api.sandbox.gum.money`, delegated from Vercel DNS
like production's), and Vercel project (for `sandbox.gum.money`), and it
sends merchant email from `sandbox.gum.money` so its SES identity and DKIM
records never collide with production's. Never plan sandbox variables
against production state.

## Backups and recurring operations

- Keep RDS deletion protection enabled and periodically test point-in-time
  restoration into a non-production database.
- Each service raises its own log-derived alarms (a halted chain, a
  stalled transaction lane, a low signer balance, dead-lettered bus
  deliveries, a sweep job open past an hour, a deposit request needing
  attention, sustained retryable failures); see
  [daily-monitoring.md](runbooks/daily-monitoring.md). Nothing exits on a
  stuck transaction: `gum-signers` keeps reconciling it every pass and
  `gum-server` keeps the deposit request in its job until a terminal event
  arrives.
- Rotate account API keys from the dashboard's API key section as described
  in [secrets-rotation.md](runbooks/secrets-rotation.md). The previous key
  has a 24-hour grace period; revoking invalidates current and grace-period
  keys immediately.
- The `recovered_funds` ledger records every amount returned to a payer's
  wallet; nothing is held, so there is nothing to return by hand. See
  "Reconciling returned funds" in
  [stuck-deposit-request.md](runbooks/stuck-deposit-request.md).
- Attached PDFs stay in the versioned, KMS-encrypted attachment bucket for as
  long as their deposit request; the lifecycle rule removes only uploads that
  were never attached (still tagged `gum-upload=pending`) after seven
  days. Include the bucket in the backup discipline applied to the database.
- Keep the `indexer` and `signers` services at one task each. A second
  indexer is harmless (every range it reports is a compare-and-set on the
  server-owned cursor) but wasteful; a second signers task would contend
  for the same signer lanes, which the `execution_transactions_open_lane`
  unique index makes fail loudly rather than double-spend a nonce.
- A finalized cursor hash mismatch halts the chain and requires operator
  investigation ([indexer-fatal-halt.md](runbooks/indexer-fatal-halt.md));
  the indexer reports the fault rather than silently skipping or rewriting
  observations, and nothing is scheduled or signed on that chain until
  `gum-server chain resume <chain-id>`. `indexer_cursor` holds one row per
  chain, whatever the chain's currencies.
