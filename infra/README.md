# AWS deployment

Small production-oriented stack: a two-AZ VPC, public-IP Fargate API and indexer tasks, HTTPS ALB, WAF rate limiting, private encrypted PostgreSQL RDS, ECR, Secrets Manager, CloudWatch with email alarms, and a secp256k1 KMS signing key. The indexer has no inbound rule and is fixed at one task. Public ECS subnets avoid NAT Gateway cost; the API accepts traffic only from the ALB, but public IPs and unrestricted outbound remain a deliberate cost/security tradeoff.

The API task also requires an externally configured Auth0 tenant issuer, API
audience, and Native application client ID. Terraform passes these non-secret
identifiers to ECS; embedded email OTP and connection setup are documented in
`docs/authentication.md`.

The same ALB and certificate serve `payment_domain_name` and
`status_domain_name`. The status hostname routes to an independent ECS service
and target group whose `/live` check reports process health without requiring
the database; the public status response checks database-backed state lazily.
Gatewayd embeds the
cacheable payment page, signs each scoped payer URL with a generated Secrets
Manager value, and links Monad addresses and settlement transactions through
the configured explorer origin.

## Remote state bootstrap

Create a versioned, encrypted, public-access-blocked S3 bucket (and optionally a DynamoDB lock table) separately. Do **not** add that bucket to this state. Then create `backend.hcl` (untracked) such as:

```hcl
bucket         = "company-payday-terraform-state"
key            = "production/payday.tfstate"
region         = "us-east-1"
encrypt        = true
use_lockfile   = true
```

Grant tightly scoped access to the deployment principal. S3 native lockfiles require a recent Terraform version; use `dynamodb_table` instead where organizational tooling still requires it.

## Deploy

Build both root Dockerfile targets, authenticate Docker to ECR, and push the **same immutable** Git-SHA tag to the two repository URLs. On the first deployment, create only ECR first, push images, and then apply the reviewed full plan (targeted apply is only for this bootstrap):

```bash
cd infra
cp terraform.tfvars.example terraform.tfvars   # replace every example value
export TF_VAR_rpc_url='https://your-paid-provider.example/...'
terraform init -backend-config=backend.hcl
terraform fmt -check -recursive
terraform validate
terraform apply -target=aws_ecr_repository.api -target=aws_ecr_repository.indexer
# Build and push Dockerfile's API/indexer targets using image_tag now.
terraform plan -out=deploy.tfplan
terraform apply deploy.tfplan
```

Review the plan, especially Route53, IAM, RDS, and deletion settings. No factory address or USDC start block is defaulted. Retrieve generated values from Secrets Manager rather than Terraform output.
After apply, confirm the AWS SNS subscription sent to `alarm_email`; alarms do not deliver until it is confirmed.
The stack verifies `notification_domain_name` with SES Easy DKIM. Before launch,
also move the SES account out of the sandbox in this region and verify a test
message from `notification_from_address` reaches an external recipient.

## Sandbox deployment

The same architecture can be instantiated independently for Monad testnet.
See `terraform.sandbox.tfvars.example` and `../docs/sandbox.md`. Use a separate
backend state key and `name`; never plan sandbox variables against production
state. The examples document planning only and do not change external state.

## Secrets and operational notes

Terraform state contains the RPC URL, payer-link signing secret, and generated
database password/URL in plaintext within encrypted state despite `sensitive`
markings. Secure state,
plans, CI logs, and access accordingly; never commit `terraform.tfvars`,
`backend.hcl`, or plans. ECS injects infrastructure secrets at task startup.
The API execution role can read only the database, payer-link, webhook
encryption, and generated operator credential secrets; its task role can send
mail only from the verified SES identity. The status execution role can read
only the database secret. Indexer execution can read only database/RPC secrets.
The indexer task role can only `kms:GetPublicKey` and `kms:Sign` on its key. RDS
connections use hostname and certificate verification against the
checksum-pinned AWS global RDS CA bundle in the image. Secrets Manager version
rotation is not observed by running ECS tasks. Force a new API deployment after
rotating the webhook key, and retain prior application key material until
ciphertext associated with its key ID has been re-encrypted.

KMS does not return an Ethereum address. Derive it from `GetPublicKey` (uncompressed secp256k1 public key, Keccak-256, last 20 bytes) and independently verify it before use. KMS signatures also require application-side Ethereum digest/signature normalization.

WAF request sampling is disabled because samples can contain the bearer `Authorization` header. Fatal indexer safety errors and loss of its database lock exit the process and publish a log-derived CloudWatch alarm. RDS Multi-AZ, ALB, WAF, public IPv4 addresses, Container Insights, logs, Secrets Manager, and KMS incur ongoing charges. Public IPv4 and cross-AZ traffic are billed. This stack has no autoscaling, VPC endpoints, bastion, or automatic finality-reorg recovery.

## Destroy protection

RDS deletion protection defaults to true and final snapshots default on, so normal `terraform destroy` intentionally fails. The non-exportable KMS signing key also has Terraform `prevent_destroy`. For a deliberate teardown, preserve required data, set `db_deletion_protection = false`, apply that change, and separately review removal of the KMS lifecycle guard before destroying. KMS deletion has a 30-day waiting period; secret recovery and retained snapshots may continue to incur cost.
