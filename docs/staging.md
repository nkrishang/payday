# Staging

Staging is the middle ground between `just dev` and production: a second,
fully separate copy of the production stack that always runs the newest
commit on `main`, on Monad mainnet with real USDC, that you drive from your
own machine. The web app runs locally against it, `curl` and the SDK reach it
with a key you mint in that local dashboard, and the deposits it settles are
real transfers of a few cents through real RPC, real finality, real KMS
signing, real S3 scanning, and real email.

| | Local (`just dev`) | Staging | Production |
|---|---|---|---|
| Code | your working tree | every commit on `main` that passes CI | the `image_tag` in the production tfvars, applied by hand |
| API, indexer, database | on this machine | AWS, `api.staging.payday.sh` | AWS, `api.payday.sh` |
| Chain and USDC | Anvil, mock USDC | Monad mainnet, Circle USDC | Monad mainnet, Circle USDC |
| Contracts | bootstrapped on Anvil each run | staging's own `PaymentFactory` generation | production's generation |
| Web app | `just web` on port 3002 | `just web-staging` on port 3002 | Vercel, `payday.sh` |
| Merchant sign-in | development Privy app | development Privy app | production Privy app |
| Payer and issuer codes | the local identity provider, fixed code | production Auth0 tenant, real mail | production Auth0 tenant |
| Attachments | MinIO, verdict tagged by hand | S3 and GuardDuty | S3 and GuardDuty |
| Merchant email | not sent | SES from `alerts@staging.payday.sh` | SES from `alerts@payday.sh` |
| Keys | Anvil accounts | staging's own KMS keys | production's KMS keys |

Staging's checkout origin is `http://127.0.0.1:3002`, the web app on your
machine, so every `deposit_url` it mints opens in your local checkout and
the local dashboard is the one origin its merchant routes and attachment
bucket accept. That is deliberate: the point is to test the web app and the
scripts you are working on against a backend that is exactly what is about
to ship. Nobody else can open those links, and that is fine for staging.

The configuration lives in `infra/environments/staging.tfvars` (tracked,
public identifiers only), `infra/environments/staging.backend.hcl`, and
`web/.env.staging`. The deployment workflow is
`.github/workflows/deploy-staging.yml`.

## Using it

Sign in and mint a key:

```bash
just web-staging            # http://127.0.0.1:3002, pointed at staging
```

Open the dashboard, sign in with an emailed code (the development Privy app,
so the same mailbox you use locally), and mint an API key in the API key
section. It is a `payday_test_` key that only staging accepts. Issue a
deposit request from the dashboard, open its link in the same browser,
connect a wallet holding a little USDC and MON, and pay it; everything from
the wallet attestation to the Proof of Payment happens on the live stack.

Scripts and the SDK use the staging origin and that key:

```bash
export PAYDAY_API_URL="https://api.staging.payday.sh"
export PAYDAY_API_KEY="payday_test_..."
curl --fail -sS "$PAYDAY_API_URL/v1/deposit-requests" -H "Authorization: Bearer $PAYDAY_API_KEY" | jq
```

In the SDK, pass `baseUrl: "https://api.staging.payday.sh"` to `PaydayClient`.

The scripted end-to-end check is a real deposit that costs only gas:

```bash
export PAYDAY_RPC_URL='https://your-quicknode-endpoint'   # any HTTPS Monad RPC
export PAYER_KEY='0x...'      # a wallet holding at least 0.01 USDC and some MON
just live-smoke
```

It issues a permissionless request paid out to the paying wallet itself,
binds that wallet with an EIP-712 attestation signed by `cast` exactly as
the checkout does, transfers the USDC, waits for finalized settlement,
checks that the payout arrived and the deposit address is empty, and
validates the Proof of Payment. `LATE_TRANSFER=1` also sends one base unit
after settlement and waits for it to come back. `PAYDAY_ATTESTOR=0x…` makes
it check the proof's signer against the address you published. The same
script runs against production with `PAYDAY_API_URL=https://api.payday.sh`
and a live key; that is the launch check in the production runbook.

Webhooks: staging delivers to any public HTTPS endpoint, and cannot reach
your machine. Use a request-capture service or a tunnel for a local
receiver.

Operator commands (`docs/runbooks/`) work against staging with
`AWS_REGION=eu-north-1` and the cluster name `payday-staging`; every
Terraform command needs `-var-file=environments/staging.tfvars` and an init
against `environments/staging.backend.hcl`.

## What deploys, and when

Every push to `main` runs CI. When CI succeeds, the `Deploy staging`
workflow checks out that commit, builds both images under its immutable
`git-<sha>` tag (or skips the build when the tag already exists), pushes
them to staging's ECR, and applies the staging Terraform with that tag.
The API and indexer roll over through the usual ECS deployment: the API
with the circuit breaker, the indexer stopping the old task before the new
one starts. The workflow then waits for both services to stabilize and
checks `/health`. Nothing in it can reach the production state or account
role.

Before the stack exists, the workflow does nothing: it exits early until
the `AWS_STAGING_DEPLOY_ROLE_ARN` and `STAGING_RPC_URL` secrets are set
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

Payday is pre-release, so the schema is one baseline file,
`crates/gateway-db/migrations/0001_initial_schema.sql`, edited in place;
there are no incremental migrations to write. A database that applied the
old file refuses the edited one (its checksum differs), so the deploy
workflow compares the migrations directory between the deployed commit and
the one it is shipping and, when they differ, replaces staging's RDS
instance in the same apply. The new tasks then create the schema from
scratch. That is also why the staging tfvars turn off deletion protection
and the final snapshot.

Everything on staging is disposable, including its accounts and keys, and a
schema change resets all of it: sign in again and mint a new key
afterwards. To force a reset by hand:

```bash
terraform -chdir=infra apply -var-file=environments/staging.tfvars \
  -var image_tag=git-<sha> -replace=aws_db_instance.this
```

## Bootstrap (once)

The stack is created by hand the first time, because the workflow's own
role is one of the things it creates. All of this mirrors the production
runbook with staging's names; do it from a shell authenticated to the
Payday AWS account.

1. **DNS.** Create a Route53 public hosted zone for `api.staging.payday.sh`
   and, in Vercel's DNS settings for `payday.sh`, add its four nameservers
   as `NS` records named `api.staging`. Put the zone ID in
   `infra/environments/staging.tfvars` as `route53_zone_id`. Wait for
   `dig NS api.staging.payday.sh` to return the Route53 names.
2. **Contracts.** Deploy a `PaymentFactory` and `BatchSweeper` generation
   for staging on Monad mainnet exactly as in the production runbook §2,
   from a deployment wallet with a little MON, and record the two
   addresses and the two runtime code hashes in the tfvars. Never point
   staging at production's contracts: every deposit address is derived from
   its factory, and the two databases must not share one.
3. **Start block.** `cast block-number` immediately before the first apply,
   into `usdc_start_block`.
4. **State and images.** With `TF_VAR_rpc_url` exported:

   ```bash
   terraform -chdir=infra init -reconfigure -backend-config=environments/staging.backend.hcl
   terraform -chdir=infra apply -var-file=environments/staging.tfvars \
     -target=aws_ecr_repository.api -target=aws_ecr_repository.indexer
   api_repo=$(terraform -chdir=infra output -raw api_ecr_repository_url)
   indexer_repo=$(terraform -chdir=infra output -raw indexer_ecr_repository_url)
   scripts/push-images.sh eu-north-1 "$api_repo" "$indexer_repo" "git-$(git rev-parse HEAD)"
   terraform -chdir=infra plan -var-file=environments/staging.tfvars \
     -var "image_tag=git-$(git rev-parse HEAD)" -out=staging.tfplan
   terraform -chdir=infra apply staging.tfplan
   ```

   Review the plan as for production. Because `infra/.terraform` is shared,
   run `terraform init -reconfigure -backend-config=backend.hcl` before the
   next production command.
5. **Signers.** Derive and fund the sweep signer with a little MON, and
   derive the attestation signer, as in the production runbook §8; note
   staging's attestor address in the team's records, since proofs from
   staging are signed by it and `PAYDAY_ATTESTOR` in the smoke test checks
   it.
6. **Email.** Add the three `CNAME`s from
   `terraform -chdir=infra output notification_dkim_records` in Vercel DNS
   (they are names under `staging.payday.sh`). SES verifies the identity
   once they resolve; the region's SES sandbox limits apply to staging the
   same way.
7. **Hand over to the workflow.** In the GitHub repository, create an
   environment named `staging` and two repository secrets:
   `AWS_STAGING_DEPLOY_ROLE_ARN`, from
   `terraform -chdir=infra output -raw github_deploy_role_arn`, and
   `STAGING_RPC_URL`, the same endpoint you exported as `TF_VAR_rpc_url`.
   A third, `STAGING_RESEND_API_KEY`, is optional: with it, staging emails
   payers their deposit requests through Resend; without it, those emails
   queue unsent.
   Commit the filled-in tfvars. The next merge to `main` deploys itself; or
   run the workflow by hand once from the Actions tab to confirm.
8. **Verify.** `just web-staging`, sign in, mint a key, and run
   `just live-smoke`. Confirm the SNS alarm subscription mail for
   `payday-staging`.

## Cost and guardrails

Staging is billed like a small production: one Fargate task each for the
API and indexer, a single-AZ `db.t4g.small`, the ALB, WAF, and the public
IPs. Its sweep signer needs only a few MON. Keep only what a test needs in
the payer wallet; the payout defaults to that same wallet, so a smoke run
costs gas alone.

Staging keys are `payday_test_`, production keys `payday_live_`; neither is
accepted by the other environment. Staging's database is not backed up
beyond RDS's own automated backups and is meant to be reset. Do not send it
anything you would mind losing, and do not treat its Proofs of Payment as
Payday's: they are signed by staging's attestor, not the published one.
