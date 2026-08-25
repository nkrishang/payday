# Production runbook (AWS + Monad)

This is the first-production deployment path for one operator. Terraform owns
the AWS resources; Docker images contain the two Rust services; AWS KMS owns the
non-exportable sweep key. Do not accept a real payment until the final end-to-end
test in this runbook succeeds.

## Accounts and assets the operator must provide

Required:

1. **AWS account** with billing enabled. Protect the root user with MFA, use IAM
   Identity Center or another non-root administrator identity for daily work,
   and configure the AWS CLI with short-lived SSO credentials.
2. **The existing `payday.sh` domain and a public Route53 hosted zone for
   `api.payday.sh`.** Keep the domain registered and its parent DNS hosted at
   Vercel. Delegate only `api.payday.sh` to Route53 as described below.
3. **QuickNode account** with a paid Monad Mainnet HTTPS endpoint. The indexer
   needs standard `eth_blockNumber`, `eth_getLogs`, block lookup, call,
   transaction submission, and receipt methods.
4. **GitHub account/repository.** This repository already satisfies that
   requirement. GitHub Actions runs CI and creates CLI releases from `v*` tags.
5. **A Monad deployment wallet with MON.** Prefer a hardware wallet. A separate
   encrypted Foundry keystore is acceptable for the ownerless factory's one-time
   deployment. The AWS KMS sweep address also needs a deliberately small MON gas
   balance after deployment.
6. **A wallet with a small amount of native Monad USDC** for the production
   smoke payment. Circle Mint is not required; USDC can come from a supported
   exchange or bridge.

No Docker Hub, Terraform Cloud, separate PostgreSQL vendor, Circle account, or
third-party key-management account is required.

## Production hostnames

- `https://api.payday.sh` is the public API behind AWS WAF and the Application
  Load Balancer. The CLI uses this as `GATEWAY_API_URL`.
- The indexer/sweeper and PostgreSQL database have no public hostname or inbound
  internet access.
- `payday.sh` and `www.payday.sh` remain available for a Vercel-hosted website or
  documentation. They are not required to run the payment service.

Create a **public hosted zone named `api.payday.sh`** in Route53. AWS assigns
four authoritative nameservers. In Vercel's DNS settings for `payday.sh`, add
four separate `NS` records with name `api`, one for each AWS nameserver. Do not
change the nameservers for the whole `payday.sh` domain. Use the Route53 hosted
zone ID as `route53_zone_id`; Terraform then creates the API alias and ACM
certificate-validation records inside that delegated zone.

Wait until `dig NS api.payday.sh` returns the four AWS nameservers before the
full Terraform apply, because ACM cannot validate the certificate until the
delegation is publicly visible.

## Fixed Monad values

- Chain ID: `143`
- Native gas token: `MON`
- [Circle native USDC](https://www.circle.com/multi-chain-usdc/monad):
  `0x754704Bc059F8C67012fEd69BC8A327a5aafb603`
- USDC decimals: `6`
- [Monad full finality](https://docs.monad.xyz/monad-arch/consensus/block-states):
  block `N` is finalized when `N+2` is proposed, so this stack sets
  `GATEWAY_FINALITY_CONFIRMATIONS=2`.

Reconfirm the USDC address against Circle's official contract-address page
before every new production environment.

## 1. Install operator tools

Install Docker, Terraform 1.10+, AWS CLI v2, Rust, and Foundry. Authenticate with
AWS and confirm the intended account before creating anything:

```bash
aws configure sso
aws sso login
aws sts get-caller-identity
```

Never run production Terraform while authenticated to an account you have not
explicitly verified.

## 2. Deploy PaymentFactory and BatchSweeper to Monad

`PaymentFactory` and its immutable `BatchSweeper` helper have no owner or
privileged administrative key. Use a dedicated
deployment wallet rather than the KMS sweep key, and retain its transaction
record even though it has no post-deployment authority.

For an encrypted Foundry keystore:

```bash
cast wallet import payday-deployer --interactive
```

Fund the displayed address with enough MON for two contract deployments. Then:

```bash
export GATEWAY_CHAIN_ID=143
export MONAD_RPC_URL='https://your-quicknode-endpoint'

forge script foundry/script/PaymentFactory.s.sol:PaymentFactoryScript \
  --rpc-url "$MONAD_RPC_URL" \
  --account payday-deployer \
  --broadcast
```

The script aborts if the RPC chain ID is not 143. Save both resulting addresses
and deployment transactions. Verify that code exists at each address:

```bash
cast code <FACTORY_ADDRESS> --rpc-url "$MONAD_RPC_URL"
cast code <BATCH_SWEEPER_ADDRESS> --rpc-url "$MONAD_RPC_URL"
```

An empty `0x` result means deployment verification failed; stop there. Configure
the helper address as `batch_sweeper_address` in Terraform.

## 3. Create protected Terraform state storage

Create one private S3 bucket for Terraform state. Enable versioning, default
encryption, and Block Public Access. The bucket is intentionally not created by
the main Terraform state. Create `infra/backend.hcl` as shown in
`infra/README.md`; it is ignored by Git.

For one operator, S3 native state locking (`use_lockfile = true`) is sufficient.
Restrict bucket access to the deployment identity and recovery administrator.

## 4. Choose the first index block

Immediately before the first deployment, record the current block:

```bash
cast block-number --rpc-url "$MONAD_RPC_URL"
```

Use that value as `usdc_start_block`. No invoices can predate the first launch,
so scanning all historical USDC transfers would waste RPC requests without
finding a payable invoice.

## 5. Configure Terraform

```bash
cp infra/terraform.tfvars.example infra/terraform.tfvars
git_sha=$(git rev-parse HEAD)
```

Replace every placeholder in `terraform.tfvars`, including:

- AWS region
- API hostname and Route53 zone ID
- alert email
- `image_tag = "git-<full commit SHA>"`
- deployed factory address
- current `usdc_start_block`

Supply the RPC URL without writing it to the tfvars file:

```bash
export TF_VAR_rpc_url="$MONAD_RPC_URL"
```

Terraform stores generated credentials and the RPC URL in its encrypted state.
Treat state files and saved plans as secrets.

## 6. Bootstrap ECR and push images

The repositories must exist before images can be pushed, while ECS cannot start
until those images exist:

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
worktree. Commit and review every source change before building. This makes the
image-to-source relationship and rollback deterministic.

## 7. Create the complete AWS stack

```bash
terraform -chdir=infra fmt -check
terraform -chdir=infra validate
terraform -chdir=infra plan -out=deploy.tfplan
terraform -chdir=infra apply deploy.tfplan
```

Review the plan before applying it. In particular, reject unexplained database
replacement/destruction, IAM permissions broader than the named secrets and KMS
key, a worker count other than one, or plaintext/non-HTTPS endpoints.

AWS creates the TLS certificate, DNS record, ALB/WAF, ECS services, RDS database,
Secrets Manager values, alarms, and KMS key. WAF request sampling is disabled so
the bearer header is not retained in samples. RDS verifies its server hostname
and certificate against the checksum-pinned AWS global RDS CA bundle in the
container. Confirm the SNS subscription link sent to the configured alert email.

## 8. Fund and verify the KMS sweep signer

The indexer logs the Ethereum address derived from the KMS public key. Read it
from CloudWatch:

```bash
aws logs tail /ecs/payday/indexer --since 15m \
  --filter-pattern 'configured sweep signer'
```

Independently derive the same address with Foundry's AWS KMS support:

```bash
export AWS_KMS_KEY_ID="$(terraform -chdir=infra output -raw kms_key_arn)"
cast wallet address --aws
```

The two addresses must match. Fund it with only enough MON for expected sweeps
and maintain a low-balance alert operationally. The signer does not custody
USDC; it pays gas to invoke the permissionless factory.

## 9. Configure and test the CLI

Retrieve the generated API key without printing it in shared logs:

```bash
export GATEWAY_API_URL="$(terraform -chdir=infra output -raw api_url)"
api_key_secret="$(terraform -chdir=infra output -raw api_key_secret_arn)"
export GATEWAY_API_KEY="$(aws secretsmanager get-secret-value \
  --secret-id "$api_key_secret" --query SecretString --output text)"

curl --fail "$GATEWAY_API_URL/health"
cargo run --release -p gateway-cli -- invoice create \
  --chain-id 143 \
  --token 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  --beneficiary <YOUR_BENEFICIARY_ADDRESS> \
  --expiration-timestamp "$(($(date +%s) + 3600))" \
  --recovery <YOUR_RECOVERY_ADDRESS> \
  --amount 0.01
```

Pay exactly 0.01 native USDC to the returned payment address. Confirm that:

1. CLI status progresses `created → funded → deploying → fulfilled`.
2. The beneficiary receives the USDC.
3. `balanceOf(payment_address)` becomes zero.
4. `cast code payment_address` is non-empty.
5. API and indexer logs contain no repeated errors.
6. CloudWatch alarms and RDS backups are configured.

Do not advertise or depend on the service until this succeeds.

## Updates and rollback

For each application update, build and push a new `git-<SHA>` tag, change
`image_tag`, review `terraform plan`, and apply it. ECS's deployment circuit
breaker rolls back failed task startups. The indexer deployment stops the old
task before starting the new one so two sweep nonce owners never overlap. A
manual rollback sets `image_tag` to a previous known-good image and applies
again. Database migrations are embedded and run at service startup, so schema
changes require a separately reviewed forward/backward compatibility plan.

To publish CLI binaries after CI is green, create and push a signed `v*` tag.
The release workflow builds Linux, macOS Intel/Apple Silicon, and Windows assets
and creates the GitHub Release.

## Backups and recurring operations

- Keep RDS deletion protection enabled and periodically test point-in-time
  restoration into a non-production database.
- Monitor indexer task count, fatal-worker alarm, target health, database
  capacity, RPC errors, signer MON balance, oldest unfulfilled invoice, and
  indexer cursor lag. Permanent safety errors and loss of the retained database
  advisory-lock connection terminate the worker instead of appearing healthy.
- Rotate the API key by updating Secrets Manager and restarting API tasks; this
  single-operator version does not provide overlapping-key rotation.
- The retained PostgreSQL advisory lock rejects a second indexer even if someone
  bypasses ECS and starts another task. Keep the ECS service at one task as an
  additional control.
- A finalized cursor hash mismatch requires operator investigation; the worker
  intentionally exits and alerts rather than silently skipping or rewriting
  observations.
