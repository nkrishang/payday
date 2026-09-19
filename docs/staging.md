# Staging

Staging is the middle ground between `just dev` and production: a second,
fully separate copy of the production stack that always runs the newest
commit on `main`, on Monad, Base, and Arbitrum One with real USDC and
USDT0, that you
drive from your own machine. The web app runs locally against it, `curl` and the SDK reach it
with a key you mint in that local dashboard, and the deposits it settles are
real transfers of a few cents through real RPC, real finality, real KMS
signing, real S3 scanning, and real email.

| | Local (`just dev`) | Staging | Production |
|---|---|---|---|
| Code | your working tree | every commit on `main` that passes CI | the `image_tag` in the production tfvars, applied by hand |
| API, indexer, signers, database | on this machine | AWS, `api.staging.gum.money` | AWS, `api.gum.money` |
| Chains and stablecoins | two Anvils, mock USDC and USDT | Monad, Base, Arbitrum One; Circle USDC, Tether USDT0 on Monad and Arbitrum | Monad, Base, Arbitrum One; Circle USDC, Tether USDT0 on Monad and Arbitrum |
| Contracts | bootstrapped on Anvil each run | staging's own `PaymentFactory` generation | production's generation |
| Web app | `just web` on port 3002 | `just web-staging` on port 3002 | Vercel, `gum.money` |
| Merchant sign-in | development Privy app | development Privy app | production Privy app |
| Payer and issuer codes | the local identity provider, fixed code | production Auth0 tenant, real mail | production Auth0 tenant |
| Attachments | MinIO, verdict tagged by hand | S3 and GuardDuty | S3 and GuardDuty |
| Merchant email | not sent | SES from `alerts@staging.gum.money` | SES from `alerts@gum.money` |
| Keys | Anvil accounts | staging's own KMS keys | production's KMS keys |

Staging's checkout origin is `http://127.0.0.1:3002`, the web app on your
machine, so every `deposit_url` it mints opens in your local checkout and
the local dashboard is the one origin its merchant routes and attachment
bucket accept. That is deliberate: the point is to test the web app and the
scripts you are working on against a backend that is exactly what is about
to ship. Nobody else can open those links, and that is fine for staging.

The configuration lives in `infra/environments/staging.tfvars` (tracked,
public identifiers only), `infra/environments/staging.backend.hcl`, and
`web/.env.staging`, whose `NEXT_PUBLIC_CHAINS` lists each chain's `tokens`
(`{currency, address, symbol?, decimals?}`) to match the tfvars. The
deployment workflow is `.github/workflows/deploy-staging.yml`.

## Using it

Sign in and mint a key:

```bash
just web-staging            # http://127.0.0.1:3002, pointed at staging
```

Open the dashboard, sign in with an emailed code (the development Privy app,
so the same mailbox you use locally), and mint an API key in the API key
section. It is a `gum_test_` key that only staging accepts. Issue a
deposit request from the dashboard, open its link in the same browser,
choose a network, connect a wallet holding a little of the request's
currency and gas there, and pay it; everything from the wallet attestation
to the Proof of Payment happens on the live stack.

Scripts and the SDK use the staging origin and that key:

```bash
export GUM_API_URL="https://api.staging.gum.money"
export GUM_API_KEY="gum_test_..."
curl --fail -sS "$GUM_API_URL/v1/deposit-requests" -H "Authorization: Bearer $GUM_API_KEY" | jq
```

In the SDK, pass `baseUrl: "https://api.staging.gum.money"` to `GumClient`.

The scripted end-to-end check is a real deposit that costs only gas:

```bash
export GUM_CHAIN_ID=143    # the network to pay on: 143, 8453, or 42161
export GUM_RPC_URL='https://your-rpc-endpoint'   # any HTTPS RPC for that chain
export GUM_TOKEN_ADDRESS=0x754704Bc059F8C67012fEd69BC8A327a5aafb603   # that chain's USDC
export PAYER_KEY='0x...'      # a wallet holding at least 0.01 USDC and some gas there
just live-smoke
```

Run it once per network; only the three exports change. `GUM_CURRENCY=USDT`
runs it in USDT0 instead: the request is pinned to `GUM_CHAIN_ID` (Monad
or Arbitrum One) and `GUM_TOKEN_ADDRESS` names that chain's USDT0
contract (the Monad address is the default), so run it once more on each
chain that serves USDT.

It issues a request with the wallet-attestation add-on, paid out to the paying
wallet itself, and binds that wallet with an EIP-712 attestation signed by
`cast` exactly as
the checkout does, transfers the stablecoin, waits for finalized settlement,
checks that the payout arrived and the deposit address is empty, and
validates the Proof of Payment. `LATE_TRANSFER=1` also sends one base unit
after settlement and waits for it to be recovered into Gum's recovery
custody. `GUM_ATTESTOR=0x…` makes
it check the proof's signer against the address you published. The same
script runs against production with `GUM_API_URL=https://api.gum.money`
and a live key; that is the launch check in the production runbook. Before
the first withdrawal on a chain, `just forge-fork` runs the forwarder's fork
tests and the USDT0 authorization fork tests (`MonadUsdt0ForkTest`,
`ArbitrumUsdt0ForkTest`) against the real USDT0 contracts on Monad and
Arbitrum.

Webhooks: staging delivers to any public HTTPS endpoint, and cannot reach
your machine. Use a request-capture service or a tunnel for a local
receiver.

Operator commands (`docs/runbooks/`) work against staging with
`AWS_REGION=eu-north-1` and the cluster name `gum-staging`; every
Terraform command needs `-var-file=environments/staging.tfvars` and an init
against `environments/staging.backend.hcl`.

## What deploys, and when

Every push to `main` runs CI. When CI succeeds, the `Deploy staging`
workflow checks out that commit, builds the three images (`api`,
`indexer`, `signers`) under its immutable `git-<sha>` tag (or skips the
build when the tag already exists), pushes them to staging's ECR, registers
the `migrate` task definition for that tag and runs it once
(`scripts/run-migrate-task.sh`), and only then applies the rest of the
staging Terraform with that tag. The services roll over through the usual
ECS deployment: the API with the circuit breaker, the indexer and signers
each stopping the old task before the new one starts (each is a singleton).
The workflow then waits for the three services to stabilize and checks
`/health`. Nothing in it can reach the production state or account role.

Before the stack exists, the workflow does nothing: it exits early until
the `AWS_STAGING_DEPLOY_ROLE_ARN` and `STAGING_RPC_URLS` secrets are set
(bootstrap, below).

To redeploy an older commit that is on `main`, run the workflow by hand
from the Actions tab with that commit's SHA. It refuses any commit that is
not an ancestor of `main`. Reverting on `main` is the other way back.

The workflow assumes a role that the staging Terraform itself creates
(`github_repository` in the tfvars): an OIDC trust for exactly this
repository's `main` branch, carrying `AdministratorAccess`, since a Terraform
apply of this stack touches IAM, KMS, RDS, ECS, and S3. The trust policy is
the control. Production sets no `github_repository` and therefore has no
such role.

## Schema changes

Gum is pre-release, so the schema is three baseline files in
`crates/gum-schema/migrations` (`0001_application.sql`, `0002_bus.sql`,
`0003_execution.sql`), edited in place; there are no incremental migrations
to write. A database that applied the old files refuses the edited ones
(their checksums differ), so the deploy workflow compares the migrations
directory between the deployed commit and the one it is shipping and, when
they differ, replaces staging's RDS instance in the targeted apply that
registers the migrate task. The migrate task then creates the schema from
scratch before any service rolls; no service migrates on start, and each
one's `/health/ready` refuses while the schema is behind its binary. That
is also why the staging tfvars turn off deletion protection and the final
snapshot.

Everything on staging is disposable, including its accounts and keys, and a
schema change resets all of it: sign in again and mint a new key
afterwards. To force a reset by hand:

```bash
terraform -chdir=infra apply -var-file=environments/staging.tfvars \
  -var image_tag=git-<sha> -replace=aws_db_instance.this \
  -target=aws_ecs_task_definition.migrate
scripts/run-migrate-task.sh gum-staging
terraform -chdir=infra apply -var-file=environments/staging.tfvars \
  -var image_tag=git-<sha>
```

The first apply is targeted so that the schema exists before anything
else is touched: every service's `/health/ready` refuses against an empty
database, and ECS restarts the tasks until the migrate task has run.

## Bootstrap (once)

The stack is created by hand the first time, because the workflow's own
role is one of the things it creates. All of this mirrors the production
runbook with staging's names; do it from a shell authenticated to the
Gum AWS account.

1. **DNS.** Create a Route53 public hosted zone for `api.staging.gum.money`
   and, in Vercel's DNS settings for `gum.money`, add its four nameservers
   as `NS` records named `api.staging`. Put the zone ID in
   `infra/environments/staging.tfvars` as `route53_zone_id`. Wait for
   `dig NS api.staging.gum.money` to return the Route53 names.
2. **Contracts.** Deploy a `PaymentFactory` and `BatchSweeper` generation
   for staging on Monad, Base, and Arbitrum One exactly as in the
   production runbook §2, from one fresh deployment wallet with a little
   gas on each chain so the addresses match everywhere, and record the
   addresses and runtime code hashes in each `chains` entry of the tfvars,
   beside that chain's `tokens` (USDC everywhere, USDT0 on Monad and
   Arbitrum One, the addresses from the production runbook's table).
   Never point staging at production's contracts: every deposit address is
   derived from its factory, and the two databases must not share one.
3. **Start blocks.** `cast block-number` on each chain immediately before
   the first apply, into that entry's `start_block`.
4. **State and images.** With `TF_VAR_rpc_urls` exported (a JSON object of
   endpoints keyed by chain id, as in the production runbook §6):

   ```bash
   terraform -chdir=infra init -reconfigure -backend-config=environments/staging.backend.hcl
   terraform -chdir=infra apply -var-file=environments/staging.tfvars \
     -target=aws_ecr_repository.api -target=aws_ecr_repository.indexer \
     -target=aws_ecr_repository.signers
   api_repo=$(terraform -chdir=infra output -raw api_ecr_repository_url)
   indexer_repo=$(terraform -chdir=infra output -raw indexer_ecr_repository_url)
   signers_repo=$(terraform -chdir=infra output -raw signers_ecr_repository_url)
   scripts/push-images.sh eu-north-1 "$api_repo" "$indexer_repo" "$signers_repo" "git-$(git rev-parse HEAD)"
   terraform -chdir=infra plan -var-file=environments/staging.tfvars \
     -var "image_tag=git-$(git rev-parse HEAD)" \
     -target=aws_ecs_task_definition.migrate -out=migrate.tfplan
   terraform -chdir=infra apply migrate.tfplan
   scripts/run-migrate-task.sh gum-staging
   terraform -chdir=infra plan -var-file=environments/staging.tfvars \
     -var "image_tag=git-$(git rev-parse HEAD)" -out=staging.tfplan
   terraform -chdir=infra apply staging.tfplan
   ```

   Review the plan as for production. Because `infra/.terraform` is shared,
   run `terraform init -reconfigure -backend-config=backend.hcl` before the
   next production command.
5. **Signers.** Derive and fund every sweep signer in the pool
   (`sweep_signer_count`, two on staging) with a little MON on Monad and
   ETH on Base and Arbitrum One, and derive the attestation signer, as in
   the production runbook §8; note
   staging's attestor address in the team's records, since proofs from
   staging are signed by it and `GUM_ATTESTOR` in the smoke test checks
   it.
6. **Email.** Add the three `CNAME`s from
   `terraform -chdir=infra output notification_dkim_records` in Vercel DNS
   (they are names under `staging.gum.money`). SES verifies the identity
   once they resolve; the region's SES sandbox limits apply to staging the
   same way.
7. **Hand over to the workflow.** In the GitHub repository, create an
   environment named `staging` and two repository secrets:
   `AWS_STAGING_DEPLOY_ROLE_ARN`, from
   `terraform -chdir=infra output -raw github_deploy_role_arn`, and
   `STAGING_RPC_URLS`, the same JSON object you exported as
   `TF_VAR_rpc_urls`.
   A third, `STAGING_RESEND_API_KEY`, is optional: with it, staging emails
   payers their deposit requests through Resend; without it, those emails
   queue unsent.
   Commit the filled-in tfvars. The next merge to `main` deploys itself; or
   run the workflow by hand once from the Actions tab to confirm.
8. **Verify.** `just web-staging`, sign in, mint a key, and run
   `just live-smoke`. Confirm the SNS alarm subscription mail for
   `gum-staging`.

## Cost and guardrails

Staging is billed like a small production: one Fargate task each for the
API, indexer, and signers, a single-AZ `db.t4g.small`, the ALB, WAF, and
the public IPs. Its two sweep signers need only a little gas each on each chain, and idle
chains cost the indexer two RPC calls every five minutes. Keep only what a test needs in
the payer wallet; the payout defaults to that same wallet, so a smoke run
costs gas alone.

Staging keys are `gum_test_`, production keys `gum_live_`; neither is
accepted by the other environment. Staging's database is not backed up
beyond RDS's own automated backups and is meant to be reset. Do not send it
anything you would mind losing, and do not treat its Proofs of Payment as
Gum's: they are signed by staging's attestor, not the published one.
