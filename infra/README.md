# AWS deployment

Small production-oriented stack: a two-AZ VPC, public-IP Fargate API and indexer tasks, HTTPS ALB, WAF rate limiting, private encrypted PostgreSQL RDS, ECR, Secrets Manager, CloudWatch with email alarms, a private versioned S3 bucket for invoice attachments scanned by GuardDuty Malware Protection, and four KMS keys: three secp256k1 keys (the sweep signer, the Payday recovery wallet, and the Proof of Payment attestation signer) and one symmetric key encrypting attachments. The indexer has no inbound rule and is fixed at one task. Public ECS subnets avoid NAT Gateway cost; the API accepts traffic only from the ALB, but public IPs and unrestricted outbound remain a deliberate cost/security tradeoff.

The contract generation is pinned: `factory_code_hash` and
`batch_sweeper_code_hash` are the keccak256 of the runtime bytecode at
`factory_address` and `batch_sweeper_address`
(`cast keccak "$(cast code <ADDRESS> --rpc-url <RPC_URL>)"`), deployed together
as one generation. Both services compare them with the chain at startup and
refuse to start on a mismatch, which is why the API task now also reads the RPC
secret. `recovery_address` is the address of the recovery key, derived with
`cast wallet address --aws` from `recovery_kms_key_arn` after a targeted apply
of that key; see `docs/production-runbook.md`. It has no safe placeholder:
leave it unset until the key exists, and the API task definition's
precondition fails the plan until it is set.

The API task also requires the Privy app merchants sign in to
(`privy_app_id`, public; gatewayd verifies dashboard sessions against that
app's published keys), an externally configured Auth0 tenant issuer, and the
payer verification API and Native application once they exist
(`payer_auth0_audience` and `payer_auth0_client_id`, set together; while
empty, the payer settings and the generated `PAYDAY_PAYER_REF_MASTER_KEY`
secret are not passed to the task and verification is unavailable). Terraform
passes these non-secret identifiers to ECS; Privy, the embedded email OTP
connection, and the payer application setup are documented in
`docs/authentication.md`. `admin_reviewer_id` names who is recorded on
operator decisions.

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
terraform apply -target=aws_ecr_repository.api -target=aws_ecr_repository.indexer \
  -target=aws_kms_key.recovery -target=aws_kms_alias.recovery
# Build and push Dockerfile's API/indexer targets using image_tag now.
# Derive the recovery wallet and set recovery_address in terraform.tfvars:
AWS_KMS_KEY_ID="$(terraform output -raw recovery_kms_key_arn)" cast wallet address --aws
terraform plan -out=deploy.tfplan
terraform apply deploy.tfplan
# Derive the Proof of Payment attestor address and publish it as the trusted attestor:
AWS_KMS_KEY_ID="$(terraform output -raw attestation_kms_key_arn)" cast wallet address --aws
```

Review the plan, especially Route53, IAM, RDS, and deletion settings. No factory address, code hash, or USDC start block is defaulted, and `recovery_address` defaults to null so the plan fails closed until the real key's address is set. Retrieve generated values from Secrets Manager rather than Terraform output.
After apply, confirm the AWS SNS subscription sent to `alarm_email`; alarms do not deliver until it is confirmed.
The stack verifies `notification_domain_name` with SES Easy DKIM. Before launch,
also move the SES account out of the sandbox in this region and verify a test
message from `notification_from_address` reaches an external recipient.

## Sandbox deployment

The same architecture can be instantiated independently for Monad testnet.
See `terraform.sandbox.tfvars.example` and `../docs/sandbox.md`. Use a separate
backend state key and `name`; never plan sandbox variables against production
state. The examples document planning only and do not change external state.

## Invoice attachments

`<name>-invoice-attachments` holds one PDF per invoice (S3 bucket names are
global; change `name` if it is taken). It is private, versioned, encrypted
with `aws_kms_key.attachments`, and answers browser preflights only from
`checkout_base_url`, where the dashboard lives. gatewayd never proxies upload
bytes: it signs a presigned PUT with the API task role and the client uploads
to `uploads/<account_id>/<attachment_id>.pdf`. Objects are never moved. The
web deployment needs the bucket's virtual-hosted origin
(`https://<attachment_bucket_name>.s3.<region>.amazonaws.com`, from the
`attachment_bucket_name` output) as `NEXT_PUBLIC_ATTACHMENT_UPLOAD_ORIGIN`,
so the dashboard's Content-Security-Policy admits the upload.

A GuardDuty Malware Protection plan scans every new object through
`aws_iam_role.malware_protection` and tags it
`GuardDutyMalwareScanStatus=<verdict>`; Malware Protection for S3 works
without the GuardDuty detector being enabled. The bucket policy denies
`s3:GetObject` on any object not tagged `NO_THREATS_FOUND` to every principal
except that scanner role and the API task role. The task role is exempt
because `HeadObject` is `s3:GetObject` too: gatewayd must be able to answer
`409 attachment_scan_pending` for an object the scanner has not reached, and
it re-checks the tag in code before it streams anything or signs a download.
GuardDuty enables EventBridge notifications on the bucket and writes a
`malware-protection-resource-validation-object` probe when the plan is
created; both happen outside Terraform, so never manage
`aws_s3_bucket_notification` for this bucket.

Cleanup is tag-based so the gateway never has to move objects. The presigned
PUT carries `x-amz-tagging: payday-upload=pending` and `If-None-Match: *` as
signed headers, so no upload can omit the tag and no key can be written twice
(a replayed PUT fails with 412); gatewayd pins the version it hashed at
finalization and refers to it on every later read. Issuing an invoice
rewrites the tag to `payday-upload=attached` (keeping the GuardDuty tag); an
issuance that fails leaves the tag `pending`, so the object still expires.
Finalize deletes an object it rejects. The lifecycle rule expires objects
under `uploads/` still tagged `pending` seven days after upload, and their
noncurrent versions seven days later; an attached PDF cannot match it, and an
upload that expired unused is refused at issuance. The zero-byte delete
markers it leaves behind are harmless.

The task role may put, tag, head, get, read tags on, and delete objects under
`uploads/` only, and use the attachment key only through S3 in this region.

## Secrets and operational notes

Terraform state contains the RPC URL, payer-link signing secret, and generated
database password/URL in plaintext within encrypted state despite `sensitive`
markings. Secure state,
plans, CI logs, and access accordingly; never commit `terraform.tfvars`,
`backend.hcl`, or plans. ECS injects infrastructure secrets at task startup.
The API execution role can read only the database, RPC, webhook encryption,
and generated operator credential secrets; its task role can send mail only
from the verified SES identity, work the attachment bucket as described above,
and `kms:GetPublicKey`/`kms:Sign` with the attestation key alone. The status
execution role can read only the database secret. Indexer execution can read only database/RPC secrets. The
indexer task role can only `kms:GetPublicKey` and `kms:Sign` on the signer
key. No role at all can sign with the recovery key: recovered funds are
returned by an operator by hand, never by the stack. RDS
connections use hostname and certificate verification against the
checksum-pinned AWS global RDS CA bundle in the image. Secrets Manager version
rotation is not observed by running ECS tasks. Force a new API deployment after
rotating the webhook key, and retain prior application key material until
ciphertext associated with its key ID has been re-encrypted.

KMS does not return an Ethereum address. Derive it from `GetPublicKey` (uncompressed secp256k1 public key, Keccak-256, last 20 bytes) and independently verify it before use. KMS signatures also require application-side Ethereum digest/signature normalization. The same derivation on `attestation_kms_key_arn` gives the Proof of Payment attestor address; publish it so merchants can verify proofs against it (`gateway_core::verify_proof`).

WAF request sampling is disabled because samples can contain the bearer `Authorization` header. Fatal indexer safety errors and loss of its database lock exit the process and publish a log-derived CloudWatch alarm. RDS Multi-AZ, ALB, WAF, public IPv4 addresses, Container Insights, logs, Secrets Manager, and KMS incur ongoing charges. Public IPv4 and cross-AZ traffic are billed. This stack has no autoscaling, VPC endpoints, bastion, or automatic finality-reorg recovery.

## Destroy protection

RDS deletion protection defaults to true and final snapshots default on, so normal `terraform destroy` intentionally fails. All four KMS keys (sweep signer, recovery, attestation, attachments) have Terraform `prevent_destroy`; the recovery key holds custody of recovered USDC, so confirm the wallet is empty and every `recovered_funds` row has been returned before removing that guard, and the attachment key is the only way to read stored PDFs. The attachment bucket is not force-destroyed: empty it deliberately, after confirming no invoice still references its objects, before a teardown. For a deliberate teardown, preserve required data, set `db_deletion_protection = false`, apply that change, and separately review removal of the KMS lifecycle guards before destroying. KMS deletion has a 30-day waiting period; secret recovery and retained snapshots may continue to incur cost.
