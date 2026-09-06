# Deployment runbook (Vercel + AWS + Monad)

This is the deployment path for one operator: the web app on Vercel, every
backend service on AWS through the Terraform in `infra/`, the Auth0 tenant
through the Terraform in `auth0/`, and the two contracts on Monad mainnet.
Docker images contain the Rust services; AWS KMS owns the non-exportable
sweep key and Proof of Payment attestation key. Do not accept a real deposit
until the final end-to-end test in this runbook succeeds.

Payday is pre-release with no real users, and this release treats the
database accordingly: the schema ships as one baseline migration and there
is no upgrade path from any earlier pre-release database. Every existing
environment is recreated empty (§7).

One thing Payday holds and two it never does: the sweep signer holds only MON
for gas; the requested amount moves directly from the deposit address to the
merchant; and overpayment remainders, expired balances, and late transfers
go back on-chain to the payer's own attested wallet, which is every deposit
address's recovery term. Payday custodies no USDC.

## What runs where

| Component | Runs on | Public name | Source |
|---|---|---|---|
| Landing page, hosted checkout (`/pay/{id}`), merchant dashboard (`/dashboard`) | Vercel project rooted at `web/` | `payday.sh`, `www.payday.sh` | `web/` |
| Merchant and payer API (`gatewayd`) | ECS Fargate service `api` behind ALB + WAF | `api.payday.sh` | `crates/gatewayd` |
| USDC indexer and sweep worker | ECS Fargate service `indexer`, one task, no inbound access | none | `crates/gateway-indexer` |
| Database | RDS PostgreSQL, private subnets, TLS to the pinned RDS CA | none | `crates/gateway-db/migrations` |
| Deposit request attachments (PDF) | S3 bucket `payday-invoice-attachments` scanned by GuardDuty Malware Protection | virtual-hosted bucket URL, browser PUT only | `infra/` |
| Signing keys | KMS secp256k1 keys: sweep signer, attestation signer; a symmetric key for attachments; a legacy recovery key pending removal | none | `infra/` |
| Merchant notification email | SES identity for `payday.sh` | `alerts@payday.sh` | `infra/` |
| Merchant sign-in | Privy app (email code, embedded wallet, identity token) | | `docs/authentication.md` |
| Payer and issuer-mailbox codes | Auth0 tenant (Terraform in `auth0/`) sending through Resend | | `auth0/README.md` |
| Contracts | `PaymentFactory` + `BatchSweeper`, one generation, on Monad mainnet | | `foundry/` |
| Monad RPC | QuickNode paid endpoint, stored in Secrets Manager | | |
| DNS | `payday.sh` at Vercel DNS; a Route53 public hosted zone for `api.payday.sh` delegated from it | | `infra/` |

The `api` service runs the `gatewayd` image and the `indexer` service runs
the `gateway-indexer` image. Both are built from the root `Dockerfile` and
tagged `git-<full SHA>`.

## Accounts and assets the operator must provide

1. **AWS account** with billing enabled. Protect the root user with MFA, use
   IAM Identity Center or another non-root administrator identity for daily
   work, and configure the AWS CLI with short-lived SSO credentials.
2. **Vercel account** with a project for `web/` (§9) and the `payday.sh`
   domain attached to it.
3. **The `payday.sh` domain at Vercel**, where it is registered and where its
   DNS is hosted. Only `api.payday.sh` is delegated to a Route53 hosted zone
   (§5); everything else, including the SES DKIM records, is a record in
   Vercel DNS.
4. **QuickNode account** with a paid Monad Mainnet HTTPS endpoint. The
   indexer needs `eth_blockNumber`, `eth_getLogs`, block lookup, call,
   transaction submission, and receipt methods.
5. **GitHub repository.** This repository already satisfies that
   requirement; GitHub Actions runs CI.
6. **A Monad deployment wallet with MON**, preferably a hardware wallet. A
   separate encrypted Foundry keystore is acceptable for the ownerless
   factory's one-time deployment. The KMS sweep signer also needs a
   deliberately small MON balance after deployment.
7. **A wallet with a small amount of native Monad USDC** for the production
   smoke deposit. USDC can come from a supported exchange or bridge.
8. **A Privy app** for merchant sign-in, an **Auth0 tenant**, and a **Resend
   account** for the embedded passwordless email code that payers and issuer
   mailboxes prove themselves with, all configured as described in
   [authentication.md](authentication.md). Resend must verify a
   Payday-owned sending domain before real payers verify. Apply the
   reviewable Auth0 attack-protection root and complete its launch checks in
   [`auth0/README.md`](../auth0/README.md); enabled tenant defaults alone are
   not deployment evidence.
9. Optionally a **WalletConnect (Reown) project ID** so the checkout can
   offer phone wallets.

No Docker Hub, Terraform Cloud, separate PostgreSQL vendor, Circle account, or
third-party key-management account is required.

## Production hostnames and DNS

- `https://payday.sh` (and `www`) is the Vercel-hosted web app: landing
  page, hosted checkout, and dashboard. Every `deposit_url` points here, the
  API allows merchant-route CORS from exactly this origin, and the attachment
  bucket answers browser preflights from exactly this origin
  (`checkout_base_url`).
- `https://api.payday.sh` is the public API behind AWS WAF and the
  Application Load Balancer. Merchant servers and the SDK call it with an API
  key; the dashboard and checkout call it from the browser.
- The indexer and the database have no public hostname.

`payday.sh` stays at Vercel: Vercel is the registrar, hosts the zone, and
serves the web app on the apex and `www` with no records to add by hand.
Terraform needs Route53 only for `api.payday.sh`: it creates the alias
record and the ACM validation record in the zone named by `route53_zone_id`,
which is a hosted zone for `api.payday.sh` delegated from Vercel DNS with
four `NS` records (§5). Terraform touches nothing else in DNS. The one other
record set the stack needs, the three SES DKIM `CNAME`s under `payday.sh`,
is published as a Terraform output and added in Vercel DNS by hand (§10).

Wait until `dig NS api.payday.sh` returns the Route53 nameservers before the
full Terraform apply, because ACM cannot validate the certificate until the
delegation is publicly visible.

## Fixed Monad values

- Chain ID: `143`
- Native gas token: `MON`
- [Circle native USDC](https://www.circle.com/multi-chain-usdc/monad):
  `0x754704Bc059F8C67012fEd69BC8A327a5aafb603`
- USDC decimals: `6`
- [Monad full finality](https://docs.monad.xyz/monad-arch/consensus/block-states):
  the node's `finalized` tag is "irreversible without a hard fork", so this
  stack sets `PAYDAY_FINALITY_SOURCE=finalized` and subtracts
  `PAYDAY_FINALITY_CONFIRMATIONS=2` more blocks as a margin against replica
  skew behind the provider's load balancer. `latest` on Monad is the
  speculatively executed proposed block and is never used for commits.
- [Monad RPC differences](https://docs.monad.xyz/reference/rpc-differences):
  QuickNode allows 100 blocks per `eth_getLogs` (`PAYDAY_LOG_RANGE_SIZE`),
  `eth_getTransactionByHash` returns nothing for a transaction still in
  flight, and the `pending` tag reads like `latest`. The sweep worker
  therefore treats a helper transaction without a receipt after
  `PAYDAY_SWEEP_PENDING_TIMEOUT_SECS` as replaceable on the same nonce and
  detects a consumed nonce from the signer's mined transaction count.
- Monad bills the gas *limit*: every helper transaction reserves
  `100k + 400k × items` gas of MON from the sweep signer.

Reconfirm the USDC address against Circle's official contract-address page
before every new production environment.

## Order of operations

1. Install operator tools and verify the AWS account.
2. Deploy `PaymentFactory` and `BatchSweeper` to Monad.
3. Configure Privy, Resend, and Auth0.
4. Create Terraform state storage.
5. Delegate `api.payday.sh` to Route53 (already done for the current
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

## 2. Deploy PaymentFactory and BatchSweeper to Monad

`PaymentFactory` and its immutable `BatchSweeper` helper have no owner or
privileged administrative key. Use a dedicated deployment wallet rather than
the KMS sweep key, and retain its transaction record even though it has no
post-deployment authority.

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
needs a fresh database, not a configuration edit.

For an encrypted Foundry keystore:

```bash
cast wallet import payday-deployer --interactive
```

Fund the displayed address with enough MON for two contract deployments. Then:

```bash
export PAYDAY_CHAIN_ID=143
export MONAD_RPC_URL='https://your-quicknode-endpoint'

forge script foundry/script/PaymentFactory.s.sol:PaymentFactoryScript \
  --rpc-url "$MONAD_RPC_URL" \
  --account payday-deployer \
  --broadcast
```

The script aborts if the RPC chain ID is not 143. Save both resulting
addresses, both deployment transactions, and both code hashes from the script
output. Verify that code exists at each address and recompute the hashes from
the chain, which is what the services will compare against:

```bash
cast code <FACTORY_ADDRESS> --rpc-url "$MONAD_RPC_URL"
cast code <BATCH_SWEEPER_ADDRESS> --rpc-url "$MONAD_RPC_URL"
cast keccak "$(cast code <FACTORY_ADDRESS> --rpc-url "$MONAD_RPC_URL")"
cast keccak "$(cast code <BATCH_SWEEPER_ADDRESS> --rpc-url "$MONAD_RPC_URL")"
cast call <BATCH_SWEEPER_ADDRESS> 'factory()(address)' --rpc-url "$MONAD_RPC_URL"
```

An empty `0x` result means deployment verification failed; stop there. The
`factory()` call must return the factory you just deployed. Configure the
addresses as `factory_address` and `batch_sweeper_address` and the hashes as
`factory_code_hash` and `batch_sweeper_code_hash` in Terraform.

## 3. Configure Privy, Resend, and Auth0

Follow [authentication.md](authentication.md) §1 through §3, in that order.
The outputs this runbook needs:

- the Privy **App ID** (public), with `https://payday.sh` among the app's
  allowed domains, email login only, embedded wallets created on login, and
  identity tokens enabled;
- the Auth0 tenant issuer, `https://<tenant-domain>/`;
- the `Payday Payer` API identifier (`https://api.payday.sh/payer`) and the
  `Payday Payer Verification` application's client ID, both created by the
  Terraform in `auth0/` (`terraform -chdir=auth0 apply`, with its own state
  key as described in [`auth0/README.md`](../auth0/README.md));
- a Resend sending domain verified and connected to Auth0 as its email
  provider. The Resend API key lives only in Auth0.

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

## 5. Delegate `api.payday.sh` to Route53

This exists already for the current deployment; `route53_zone_id` in the
untracked `infra/terraform.tfvars` is that zone. For a new environment,
create a public hosted zone for the API name:

```bash
aws route53 create-hosted-zone --name api.payday.sh \
  --caller-reference "payday-api-$(date +%s)" \
  --query '{zone: HostedZone.Id, nameservers: DelegationSet.NameServers}'
```

Record the zone ID (drop the `/hostedzone/` prefix) as `route53_zone_id`.
In Vercel's DNS settings for `payday.sh`, add four `NS` records with name
`api`, one per Route53 nameserver. Do not change the nameservers of
`payday.sh` itself. Wait for `dig NS api.payday.sh` to return the Route53
names.

## 6. Choose the first index block and configure Terraform

Immediately before the first deployment, record the current block:

```bash
cast block-number --rpc-url "$MONAD_RPC_URL"
```

Use that value as `usdc_start_block`. No deposit requests can predate the
first launch, so scanning older USDC transfers would waste RPC requests
without finding a payable deposit request.

```bash
cp infra/terraform.tfvars.example infra/terraform.tfvars
```

Replace every placeholder in `terraform.tfvars`, including:

- AWS region and alert email
- `route53_zone_id`, the `api.payday.sh` zone from §5
- `checkout_base_url = "https://payday.sh"`, the Vercel-hosted site
- `image_tag = "git-<full commit SHA>"` of the commit you will build in §7
- deployed `factory_address` and `batch_sweeper_address`, with their
  `factory_code_hash` and `batch_sweeper_code_hash` from §2
- current `usdc_start_block`
- `privy_app_id`, `auth0_issuer`, `payer_auth0_audience`, and
  `payer_auth0_client_id` from §3
- `admin_reviewer_id`, who operator decisions are recorded against

`notification_domain_name` and `notification_from_address` default to
`payday.sh` and `alerts@payday.sh`. Supply the RPC URL without writing it to the tfvars file:

```bash
export TF_VAR_rpc_url="$MONAD_RPC_URL"
```

## 7. Bootstrap ECR, push images, and create the AWS stack

The ECR repositories must exist before images can be pushed, while ECS cannot
start until those images exist:

```bash
terraform -chdir=infra init -backend-config=backend.hcl
terraform -chdir=infra apply \
  -target=aws_ecr_repository.api \
  -target=aws_ecr_repository.indexer

api_repo=$(terraform -chdir=infra output -raw api_ecr_repository_url)
indexer_repo=$(terraform -chdir=infra output -raw indexer_ecr_repository_url)
tag="git-$(git rev-parse HEAD)"

scripts/push-images.sh <AWS_REGION> "$api_repo" "$indexer_repo" "$tag"
```

The push script requires exactly `git-<full HEAD SHA>` and refuses a dirty
worktree. Commit and review every source change before building. This makes
the image-to-source relationship and rollback deterministic.

### The database starts empty

The schema is a single baseline, `crates/gateway-db/migrations/0001_initial_schema.sql`,
embedded in both service images and applied by whichever service connects
first. There is no migration from any earlier pre-release schema: a database
that ran the old 21-file chain refuses the baseline outright, because its
`_sqlx_migrations` table disagrees with the embedded set.

- **New environment:** nothing to do. Terraform creates the RDS instance
  with an empty `gateway` database and the first task to start applies the
  baseline (its log says `database migrations applied`).
- **Existing pre-release environment:** scale the `api` and `indexer`
  services to zero, then empty the database through the
  procedure in [`db-access.md`](runbooks/db-access.md):

  ```sql
  DROP SCHEMA public CASCADE;
  CREATE SCHEMA public;
  ```

  This discards every pre-release account, key, deposit request, and
  observation, which is the intent. Restore the desired counts with the
  full apply below; the services recreate the schema on start. Pre-release
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
API task role may sign with the attestation key, only the indexer task role
with the sweep key, and nothing with the legacy recovery key), an indexer
count other than one, or plaintext or non-HTTPS endpoints.

AWS creates the TLS certificate and DNS records, ALB and WAF, the two ECS
services, the RDS database, Secrets Manager values, alarms, KMS keys, the SES
identity, the attachment bucket, and its GuardDuty Malware Protection plan.
Both services verify the contract generation against the chain as they
start: if a task loops on a code-hash or bound-factory refusal, the tfvars
and the deployment disagree. Fix the values, never the check. WAF request
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

### Sweep signer

The indexer logs the Ethereum address derived from the KMS public key:

```bash
aws logs tail /ecs/payday/indexer --since 15m \
  --filter-pattern 'configured sweep signer'
```

Independently derive the same address with Foundry's AWS KMS support:

```bash
export AWS_KMS_KEY_ID="$(terraform -chdir=infra output -raw kms_key_arn)"
cast wallet address --aws
```

The two addresses must match. Fund it with only enough MON for expected
sweeps; the worker alarms below `PAYDAY_SIGNER_LOW_BALANCE_WEI`. The signer
does not custody USDC; it pays gas to invoke the permissionless factory.

### Attestation signer

Every Proof of Payment carries a verification attestation signed with the
attestation KMS key. Only the API task role can sign with it and it holds no
funds. Derive its address the same way and verify the derivation
independently:

```bash
export AWS_KMS_KEY_ID="$(terraform -chdir=infra output -raw attestation_kms_key_arn)"
cast wallet address --aws
```

Publish that address as Payday's trusted attestor, in the API documentation
and wherever proofs are downloaded, so merchants and auditors can hand it to
whatever runs `gateway_core::verify_proof`; an attestation signed by anything
else must fail verification. The address changes only if the key is
replaced, which changes the trust anchor of every earlier proof, so treat
replacement as an announced cut-over, never as routine rotation.

### Onboarding demo payer (optional)

The dashboard's onboarding walkthrough can pay one self-issued deposit
request per account from a Payday-funded wallet. Terraform does not
provision that key; the endpoint is disabled unless `gatewayd` is given
`PAYDAY_ONBOARDING_PAYER_KMS_KEY_ID` (a KMS key the API task role may sign
with, funded with a little MON and USDC). Leave it off for launch unless the
walkthrough is wanted.

## 9. Deploy the web app to Vercel

The web app is `web/`, a Next.js app inside the npm workspace at the
repository root; it depends on `@payday/sdk` from `sdk/typescript`, which its
`prebuild` hook builds. Every configuration value is `NEXT_PUBLIC_` and is
inlined into the browser bundle at build time, so nothing secret belongs in
Vercel and a changed value needs a redeploy.

1. Create a Vercel project from the GitHub repository with:
   - **Root Directory** `web`, with "Include source files outside of the Root
     Directory" left enabled (the SDK and the root lockfile live above it);
   - **Framework** Next.js;
   - **Install Command** `cd .. && npm ci`;
   - **Build Command** `cd .. && npm run build --workspace @payday/web`;
   - **Node.js** 22, matching CI;
   - the production branch set to `main`.
2. Set these environment variables for the Production environment (they are
   the same values CI builds with, see `.github/workflows/ci.yml`, plus the
   two that depend on this deployment):

   | Variable | Production value |
   |---|---|
   | `NEXT_PUBLIC_PAYDAY_API_URL` | `https://api.payday.sh` |
   | `NEXT_PUBLIC_CHAIN_ID` | `143` |
   | `NEXT_PUBLIC_CHAIN_NAME` | `Monad` |
   | `NEXT_PUBLIC_RPC_URL` | a public Monad RPC such as `https://rpc.monad.xyz`, never the QuickNode endpoint |
   | `NEXT_PUBLIC_USDC_ADDRESS` | `0x754704Bc059F8C67012fEd69BC8A327a5aafb603` |
   | `NEXT_PUBLIC_EXPLORER_BASE_URL` | `https://monadvision.com` |
   | `NEXT_PUBLIC_PRIVY_APP_ID` | the production Privy app ID from §3 |
   | `NEXT_PUBLIC_ATTACHMENT_UPLOAD_ORIGIN` | `https://<attachment_bucket_name>.s3.<region>.amazonaws.com`, from `terraform -chdir=infra output -raw attachment_bucket_name` |
   | `NEXT_PUBLIC_PAYER_APPEAL_EMAIL` | a monitored support address |
   | `NEXT_PUBLIC_WALLETCONNECT_PROJECT_ID` | optional; empty offers injected wallets only |

   Do not set `NEXT_PUBLIC_NATIVE_SYMBOL` (it defaults to MON) or
   `PAYDAY_PRIVY_STUB` (test-only; it replaces Privy with a fake).
3. Deploy `main`. The first production deployment must be the same commit as
   the images pushed in §7, or a later one whose API contract they still
   satisfy.
4. Under the project's **Domains**, add `payday.sh` and `www.payday.sh`
   (redirecting `www` to the apex). Because Vercel also hosts the zone, it
   creates the records itself and reports the domain valid at once.
5. Confirm in Privy that `https://payday.sh` is among the app's allowed
   domains. Preview deployments get their own `*.vercel.app` origins: the
   checkout works there because the payer API is public and CORS-open, but
   the dashboard does not, since Privy allows only the listed domains and
   `gatewayd` accepts merchant-route requests from `checkout_base_url` only.
   Test dashboard changes locally or on production.

Vercel redeploys on every push to `main`. Because the web app and the API
share one contract through `@payday/sdk`, deploy API changes that remove or
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
GuardDuty console; nothing in Payday alarms on them.

### Merchant email

Terraform created an SES identity for `payday.sh` with Easy DKIM, but its
DNS lives at Vercel, so add the three `CNAME`s yourself:

```bash
terraform -chdir=infra output notification_dkim_records
```

Create each as a `CNAME` in Vercel's DNS settings for `payday.sh` (the name
is the `<token>._domainkey` part; the value is the `<token>.dkim.amazonses.com`
host). SES marks the identity verified within an hour of the records
resolving; confirm in **SES → Identities**. Then move the SES account out of
the sandbox in this region and confirm a test message from
`alerts@payday.sh` reaches an external mailbox. Deposit creation refuses to create additional unnotifiable
deposits for an account without a verified email, and
`payday-notification-missing-contact` alarms if a deposit request snapshots
no contact.

### Sign in and test the API

Sign in at `https://payday.sh` with an emailed code and mint an API key in
the dashboard's API key section ([authentication.md](authentication.md)
§5). Then, with that key:

```bash
export PAYDAY_API_KEY="<the key, from your secret store>"
curl --fail "https://api.payday.sh/health"
curl --fail -sS "https://api.payday.sh/v1/deposit-requests" \
  -H "Authorization: Bearer $PAYDAY_API_KEY" \
  -H 'Content-Type: application/json' \
  -H "Idempotency-Key: launch-check-$(date +%s)" \
  -d '{"amount":"0.01","payout_address":"<YOUR_PAYOUT_ADDRESS>",
       "issuer":{"name":"Payday"},"payer":{"name":"Launch check"},
       "payer_policy":{"mode":"permissionless"},"expires_in":3600}' | jq
```

The response's `address` is null until a wallet is bound. Open the returned
`deposit_url` (on `payday.sh`) in a browser, connect the wallet you will pay
from, and sign the attestation; `GET /v1/deposit-requests/{id}` then carries
`address`, `payer_wallet`, and `recovery_address` (the same wallet), and a
`deposit_request.ready` webhook fires. Pay exactly 0.01 native USDC to that
address from that wallet. Confirm that:

1. `GET /v1/deposit-requests/{id}` progresses `awaiting_deposit → deposited →
   settled`, with `received_base_units`, `settlement_tx_hash`, `settled_at`,
   and `settled_block` set.
2. The beneficiary receives exactly the USDC amount.
3. `balanceOf(payment_address)` becomes zero.
4. `cast call payment_address 'settled()(bool)'` returns `true`.
5. A second, small deposit to the same address comes back to the paying
   wallet within a minute while the status stays `settled`, and
   `recovered_funds` records it with reason `late_transfer` (see the
   [smoke test](runbooks/end-to-end-smoke-test.md)).
6. API and indexer logs contain no repeated errors.
7. CloudWatch alarms and RDS backups are configured.
8. `GET /v1/deposit-requests/{id}/proof` returns a proof whose attestation
   `signer` is the address from §8, and `gateway_core::verify_proof` accepts
   it with that address as the trusted attestor.

Do not advertise or depend on the service until this succeeds.

## Updates and rollback

### Backend

For each application update, build and push a new `git-<SHA>` tag, change
`image_tag`, review `terraform plan`, and apply it. ECS's deployment circuit
breaker rolls back failed task startups. The indexer deployment stops the old
task before starting the new one so two sweep nonce owners never overlap. A
manual rollback sets `image_tag` to a previous known-good image and applies
again. Secrets Manager rotation is not observed by running tasks; force a new
deployment after rotating a secret.

### Schema changes

Migrations are embedded and run at service startup. Until the first real
deposit is accepted, the schema stays one baseline file,
`0001_initial_schema.sql`, edited in place; a schema change recreates every
database, which the staging deploy does on its own
([staging.md](staging.md)) and production does as in §7. Once real data
exists, the baseline is frozen and a change is a new numbered file,
reviewed for compatibility with the image still running while the new one
starts.

### Web

Vercel deploys `main` automatically and keeps every earlier deployment; a
rollback is "Promote to Production" on a previous deployment or a revert on
`main`. A change to any `NEXT_PUBLIC_` value needs a redeploy because the
value is inlined at build time.

### Contract generation changes

A change to `DepositRequest`, `PaymentFactory`, or `BatchSweeper` is a new
generation, not an update: existing deposit requests are committed to the
old factory and would never match the new one. Deploy the factory and
sweeper together (§2), record the new addresses and code hashes, recreate
the database, update `factory_address`, `batch_sweeper_address`,
`factory_code_hash`, and `batch_sweeper_code_hash` together, and apply. A
build configured for one generation refuses to start against another, so a
half-updated configuration fails closed rather than settling against the
wrong contracts.

## Sandbox

The same stack can be instantiated a second time against Monad testnet, see
[sandbox.md](sandbox.md) and `infra/terraform.sandbox.tfvars.example`. It
needs its own Terraform state key, `name`, Privy app, Auth0 tenant, RPC
endpoint, Route53 zone (`api.sandbox.payday.sh`, delegated from Vercel DNS
like production's), and Vercel project (for `sandbox.payday.sh`), and it
sends merchant email from `sandbox.payday.sh` so its SES identity and DKIM
records never collide with production's. Never plan sandbox variables
against production state.

## Backups and recurring operations

- Keep RDS deletion protection enabled and periodically test point-in-time
  restoration into a non-production database.
- The worker raises its own alarms for a paused sweep, a low signer balance,
  cursor lag, a stale sweep backlog, and sustained retryable failures; see
  [daily-monitoring.md](runbooks/daily-monitoring.md). Permanent safety
  errors in the block indexer and loss of the retained database
  advisory-lock connection terminate the worker instead of appearing healthy;
  a stuck helper transaction only pauses the sweep worker.
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
  were never attached (still tagged `payday-upload=pending`) after seven
  days. Include the bucket in the backup discipline applied to the database.
- The retained PostgreSQL advisory lock rejects a second indexer even if
  someone bypasses ECS and starts another task. Keep the ECS service at one
  task as an additional control.
- A finalized cursor hash mismatch requires operator investigation; the
  worker intentionally exits and alerts rather than silently skipping or
  rewriting observations.
