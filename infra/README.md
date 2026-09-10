# AWS deployment

Small production-oriented stack: a two-AZ VPC, public-IP Fargate API and indexer tasks, HTTPS ALB, WAF rate limiting, private encrypted PostgreSQL RDS, ECR, Secrets Manager, CloudWatch with email alarms, a private versioned S3 bucket for deposit request attachments scanned by GuardDuty Malware Protection, and four KMS keys: three secp256k1 keys (the sweep signer, the Proof of Payment attestation signer, and a legacy recovery wallet kept only until its balance is returned) and one symmetric key encrypting attachments. The indexer has no inbound rule and is fixed at one task. Public ECS subnets avoid NAT Gateway cost; the API accepts traffic only from the ALB, but public IPs and unrestricted outbound remain a deliberate cost/security tradeoff.

The contract generation is pinned: `factory_code_hash` and
`batch_sweeper_code_hash` are the keccak256 of the runtime bytecode at
`factory_address` and `batch_sweeper_address`
(`cast keccak "$(cast code <ADDRESS> --rpc-url <RPC_URL>)"`), deployed together
as one generation. Both services compare them with the chain at startup and
refuse to start on a mismatch, which is why the API task now also reads the RPC
secret. The `recovery` KMS key is legacy: deposits now return excess and late
funds to the payer's own attested wallet, so gatewayd no longer reads its
address; keep it only until any balance it holds has been returned by hand
(see `docs/production-runbook.md`). The API task definition's
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

The networks a payer may pay on are the `chains` list: one entry per chain
with its USDC contract, the contract generation's addresses and code hashes,
finality policy, block time, log range, scan mode, and explorer origin,
passed to both tasks as `PAYDAY_CHAINS`. The paid RPC endpoints are the
`rpc_urls` map, keyed by chain id (export `TF_VAR_rpc_urls` rather than
writing them to a file); each becomes its own Secrets Manager secret,
injected as `PAYDAY_RPC_URL_<chain_id>`. Gatewayd links addresses and
settlement transactions through each chain's explorer; every `deposit_url`
points at the checkout on `checkout_base_url`.

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
export TF_VAR_rpc_urls='{"143":"https://…","8453":"https://…","42161":"https://…"}'   # one paid endpoint per chain id
terraform init -backend-config=backend.hcl
terraform fmt -check -recursive
terraform validate
terraform apply -target=aws_ecr_repository.api -target=aws_ecr_repository.indexer
# Build and push Dockerfile's API/indexer targets using image_tag now.
terraform plan -out=deploy.tfplan
terraform apply deploy.tfplan
# Derive the Proof of Payment attestor address and publish it as the trusted attestor:
AWS_KMS_KEY_ID="$(terraform output -raw attestation_kms_key_arn)" cast wallet address --aws
```

Review the plan, especially Route53, IAM, RDS, and deletion settings. No factory address, code hash, or USDC start block is defaulted. Retrieve generated values from Secrets Manager rather than Terraform output.
After apply, confirm the AWS SNS subscription sent to `alarm_email`; alarms do not deliver until it is confirmed.
The stack creates an SES identity for `notification_domain_name` with Easy
DKIM. Its three CNAMEs are the `notification_dkim_records` output; add them in
the DNS provider hosting that domain (Vercel for `payday.sh`), since the API's
Route53 zone covers only `domain_name`. Before launch, also move the SES
account out of the sandbox in this region and verify a test message from
`notification_from_address` reaches an external recipient.

Payers named with an email on a deposit request are emailed their link
through Resend, not SES. Supply the key as `TF_VAR_resend_api_key` (never in
a tfvars file); the stack stores it in Secrets Manager and passes it to the
API as `PAYDAY_RESEND_API_KEY` together with `payer_email_from`. Left empty,
those emails queue unsent.

## Sandbox deployment

The same architecture can be instantiated independently for the testnets.
See `terraform.sandbox.tfvars.example` and `../docs/sandbox.md`. Use a separate
backend state key and `name`; never plan sandbox variables against production
state. The examples document planning only and do not change external state.

## Deposit request attachments

`<name>-invoice-attachments` holds one PDF per deposit request (S3 bucket names are
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
finalization and refers to it on every later read. Issuing a deposit request
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
and `kms:GetPublicKey`/`kms:Sign` with the attestation key alone. Indexer execution can read only database/RPC secrets. The
indexer task role can only `kms:GetPublicKey` and `kms:Sign` on the signer
key. No role at all can sign with the legacy recovery key. RDS
connections use hostname and certificate verification against the
checksum-pinned AWS global RDS CA bundle in the image. Secrets Manager version
rotation is not observed by running ECS tasks. Force a new API deployment after
rotating the webhook key, and retain prior application key material until
ciphertext associated with its key ID has been re-encrypted.

KMS does not return an Ethereum address. Derive it from `GetPublicKey` (uncompressed secp256k1 public key, Keccak-256, last 20 bytes) and independently verify it before use. KMS signatures also require application-side Ethereum digest/signature normalization. The same derivation on `attestation_kms_key_arn` gives the Proof of Payment attestor address; publish it so merchants can verify proofs against it (`gateway_core::verify_proof`).

WAF request sampling is disabled because samples can contain the bearer `Authorization` header. Fatal indexer safety errors and loss of its database lock exit the process and publish a log-derived CloudWatch alarm. RDS Multi-AZ, ALB, WAF, public IPv4 addresses, Container Insights, logs, Secrets Manager, and KMS incur ongoing charges. Public IPv4 and cross-AZ traffic are billed. This stack has no autoscaling, VPC endpoints, bastion, or automatic finality-reorg recovery.

## Destroy protection

RDS deletion protection defaults to true and final snapshots default on, so normal `terraform destroy` intentionally fails. All four KMS keys (sweep signer, recovery, attestation, attachments) have Terraform `prevent_destroy`; the legacy recovery key may still hold USDC recovered before deposits returned funds to the payer's own wallet, so confirm it is empty before removing that guard and the resource, and the attachment key is the only way to read stored PDFs. The attachment bucket is not force-destroyed: empty it deliberately, after confirming no deposit request still references its objects, before a teardown. For a deliberate teardown, preserve required data, set `db_deletion_protection = false`, apply that change, and separately review removal of the KMS lifecycle guards before destroying. KMS deletion has a 30-day waiting period; secret recovery and retained snapshots may continue to incur cost.
