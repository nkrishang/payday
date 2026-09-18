data "aws_availability_zones" "available" { state = "available" }
data "aws_caller_identity" "current" {}

locals {
  # Payer links point at the hosted checkout; gum-server validates this as a
  # bare HTTPS origin and allows merchant-route CORS from it alone.
  checkout_base_url = var.checkout_base_url

  azs = slice(data.aws_availability_zones.available.names, 0, 2)
  # The chain registry all three services read: every network a payer may pay
  # on, with the contract generation deployed there. The RPC URL for each
  # is a secret (PAYDAY_RPC_URL_<chain_id>), never part of this JSON.
  common_environment = [
    { name = "PAYDAY_CHAINS", value = jsonencode(var.chains) },
    { name = "RUST_LOG", value = "info" }
  ]
  rpc_url_secrets = [for id, secret in aws_secretsmanager_secret.rpc_url :
    { name = "PAYDAY_RPC_URL_${id}", valueFrom = secret.arn }
  ]
  # Payer email verification needs its own Auth0 API and application (see
  # docs/authentication.md). Until both identifiers exist the settings are
  # omitted as a group, and gum-server answers verification_unavailable.
  payer_verification_enabled = var.payer_auth0_audience != "" && var.payer_auth0_client_id != ""
  payer_environment = local.payer_verification_enabled ? [
    { name = "PAYDAY_PAYER_AUTH0_ISSUER", value = var.auth0_issuer },
    { name = "PAYDAY_PAYER_AUTH0_AUDIENCE", value = var.payer_auth0_audience },
    { name = "PAYDAY_PAYER_AUTH0_CLIENT_ID", value = var.payer_auth0_client_id },
    { name = "PAYDAY_HOSTED_CHECKOUT_ORIGIN", value = local.checkout_base_url }
  ] : []
  payer_secrets = local.payer_verification_enabled ? [
    { name = "PAYDAY_PAYER_REF_MASTER_KEY", valueFrom = aws_secretsmanager_secret.payer_ref_master_key.arn }
  ] : []
  # Who operator decisions are recorded against.
  identity_environment = [
    { name = "PAYDAY_ADMIN_REVIEWER_ID", value = var.admin_reviewer_id }
  ]
  identity_secrets = []
}

resource "aws_vpc" "this" {
  cidr_block           = "10.42.0.0/16"
  enable_dns_hostnames = true
  enable_dns_support   = true
}

resource "aws_internet_gateway" "this" { vpc_id = aws_vpc.this.id }

resource "aws_subnet" "public" {
  count                   = 2
  vpc_id                  = aws_vpc.this.id
  availability_zone       = local.azs[count.index]
  cidr_block              = cidrsubnet(aws_vpc.this.cidr_block, 8, count.index)
  map_public_ip_on_launch = true
}

resource "aws_subnet" "private" {
  count             = 2
  vpc_id            = aws_vpc.this.id
  availability_zone = local.azs[count.index]
  cidr_block        = cidrsubnet(aws_vpc.this.cidr_block, 8, count.index + 10)
}

resource "aws_route_table" "public" { vpc_id = aws_vpc.this.id }
resource "aws_route" "internet" {
  route_table_id         = aws_route_table.public.id
  destination_cidr_block = "0.0.0.0/0"
  gateway_id             = aws_internet_gateway.this.id
}
resource "aws_route_table_association" "public" {
  count          = 2
  subnet_id      = aws_subnet.public[count.index].id
  route_table_id = aws_route_table.public.id
}

resource "aws_security_group" "alb" {
  name_prefix = "${var.name}-alb-"
  vpc_id      = aws_vpc.this.id
  ingress {
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }
  ingress {
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }
  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "aws_security_group" "api" {
  name_prefix = "${var.name}-api-"
  vpc_id      = aws_vpc.this.id
  ingress {
    from_port       = var.api_port
    to_port         = var.api_port
    protocol        = "tcp"
    security_groups = [aws_security_group.alb.id]
  }
  # The internal listener: the indexer's RPC and the health endpoints. Only
  # the indexer may reach it; it is never behind the ALB.
  ingress {
    from_port       = var.internal_port
    to_port         = var.internal_port
    protocol        = "tcp"
    security_groups = [aws_security_group.indexer.id]
  }
  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "aws_security_group" "indexer" {
  name_prefix = "${var.name}-indexer-"
  vpc_id      = aws_vpc.this.id
  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}
resource "aws_security_group" "signers" {
  name_prefix = "${var.name}-signers-"
  vpc_id      = aws_vpc.this.id
  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "aws_security_group" "db" {
  name_prefix = "${var.name}-db-"
  vpc_id      = aws_vpc.this.id
  ingress {
    from_port       = 5432
    to_port         = 5432
    protocol        = "tcp"
    security_groups = [aws_security_group.api.id, aws_security_group.signers.id]
  }
  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "random_password" "db" {
  length  = 32
  special = false
}


resource "aws_db_subnet_group" "this" {
  name       = var.name
  subnet_ids = aws_subnet.private[*].id
}

resource "aws_db_instance" "this" {
  identifier                   = var.name
  engine                       = "postgres"
  engine_version               = "16"
  instance_class               = var.db_instance_class
  allocated_storage            = var.db_allocated_storage
  max_allocated_storage        = var.db_max_allocated_storage
  storage_type                 = "gp3"
  storage_encrypted            = true
  db_name                      = "gateway"
  username                     = "gateway"
  password                     = random_password.db.result
  port                         = 5432
  multi_az                     = var.db_multi_az
  publicly_accessible          = false
  deletion_protection          = var.db_deletion_protection
  skip_final_snapshot          = var.db_skip_final_snapshot
  final_snapshot_identifier    = var.db_skip_final_snapshot ? null : "${var.name}-final"
  backup_retention_period      = 7
  auto_minor_version_upgrade   = true
  db_subnet_group_name         = aws_db_subnet_group.this.name
  vpc_security_group_ids       = [aws_security_group.db.id]
  performance_insights_enabled = true
}

resource "aws_secretsmanager_secret" "database_url" { name = "${var.name}/database-url" }
resource "aws_secretsmanager_secret_version" "database_url" {
  secret_id     = aws_secretsmanager_secret.database_url.id
  secret_string = "postgresql://gateway:${random_password.db.result}@${aws_db_instance.this.address}:5432/gateway?sslmode=verify-full&sslrootcert=/usr/local/share/ca-certificates/aws-rds-global-bundle.pem"
}
# One RPC secret per chain, keyed by chain id. The keys come from the
# non-sensitive `chains` list (a sensitive map cannot drive for_each); the
# precondition on the indexer task definition requires rpc_urls to cover
# every one of them.
resource "aws_secretsmanager_secret" "rpc_url" {
  for_each = toset([for c in var.chains : tostring(c.chain_id)])
  name     = "${var.name}/rpc-url-${each.key}"
}
resource "aws_secretsmanager_secret_version" "rpc_url" {
  for_each      = toset([for c in var.chains : tostring(c.chain_id)])
  secret_id     = aws_secretsmanager_secret.rpc_url[each.key].id
  secret_string = var.rpc_urls[each.key]
}

# Webhook signing secrets need to be recoverable across worker restarts while
# remaining encrypted at rest. This key encrypts them before they reach Postgres.
resource "random_id" "webhook_encryption_key" { byte_length = 32 }
resource "aws_secretsmanager_secret" "webhook_encryption_key" {
  name = "${var.name}/webhook-encryption-key"
}
resource "aws_secretsmanager_secret_version" "webhook_encryption_key" {
  secret_id     = aws_secretsmanager_secret.webhook_encryption_key.id
  secret_string = random_id.webhook_encryption_key.b64_std
}

# Derives the merchant-scoped payer references stored against verified
# sessions; rotating it changes every reference derived from then on.
resource "random_id" "payer_ref_master_key" { byte_length = 32 }
resource "aws_secretsmanager_secret" "payer_ref_master_key" {
  name = "${var.name}/payer-ref-master-key"
}
resource "aws_secretsmanager_secret_version" "payer_ref_master_key" {
  secret_id     = aws_secretsmanager_secret.payer_ref_master_key.id
  secret_string = random_id.payer_ref_master_key.b64_std
}

# The payer's deposit request email goes out through Resend with a key the
# operator supplies. Empty means not configured: no secret, no setting, and
# gum-server leaves those emails queued.
locals {
  resend_enabled = nonsensitive(var.resend_api_key != "")
}
resource "aws_secretsmanager_secret" "resend_api_key" {
  count = local.resend_enabled ? 1 : 0
  name  = "${var.name}/resend-api-key"
}
resource "aws_secretsmanager_secret_version" "resend_api_key" {
  count         = local.resend_enabled ? 1 : 0
  secret_id     = aws_secretsmanager_secret.resend_api_key[0].id
  secret_string = var.resend_api_key
}
locals {
  payer_email_environment = local.resend_enabled ? [
    { name = "PAYDAY_PAYER_EMAIL_FROM", value = var.payer_email_from }
  ] : []
  payer_email_secrets = local.resend_enabled ? [
    { name = "PAYDAY_RESEND_API_KEY", valueFrom = aws_secretsmanager_secret.resend_api_key[0].arn }
  ] : []
}

# Paying a deposit request from another network goes through Relay with a
# key the operator supplies; both services read it (the API quotes, the
# indexer follows). Empty means not offered: no secret, no setting, and the
# hosted checkout does not show the option.
locals {
  relay_enabled = nonsensitive(var.relay_api_key != "")
}
resource "aws_secretsmanager_secret" "relay_api_key" {
  count = local.relay_enabled ? 1 : 0
  name  = "${var.name}/relay-api-key"
}
resource "aws_secretsmanager_secret_version" "relay_api_key" {
  count         = local.relay_enabled ? 1 : 0
  secret_id     = aws_secretsmanager_secret.relay_api_key[0].id
  secret_string = var.relay_api_key
}
locals {
  relay_secrets = local.relay_enabled ? [
    { name = "PAYDAY_RELAY_API_KEY", valueFrom = aws_secretsmanager_secret.relay_api_key[0].arn }
  ] : []
}

# Privy wallet pregeneration is a latency optimization for email sign-up:
# empty means gum-server answers pregenerate with 503 and sign-in still
# creates the wallet itself (config.rs's PrivyConfig doc comment).
locals {
  privy_enabled = nonsensitive(var.privy_app_secret != "")
}
resource "aws_secretsmanager_secret" "privy_app_secret" {
  count = local.privy_enabled ? 1 : 0
  name  = "${var.name}/privy-app-secret"
}
resource "aws_secretsmanager_secret_version" "privy_app_secret" {
  count         = local.privy_enabled ? 1 : 0
  secret_id     = aws_secretsmanager_secret.privy_app_secret[0].id
  secret_string = var.privy_app_secret
}
locals {
  privy_secrets = local.privy_enabled ? [
    { name = "PAYDAY_PRIVY_APP_SECRET", valueFrom = aws_secretsmanager_secret.privy_app_secret[0].arn }
  ] : []
}

resource "random_password" "admin_bearer" {
  length  = 48
  special = false
}
# The bearer the indexer presents on gum-server's internal listener. Shared
# by exactly those two services; rotating it is a new random_password.
resource "random_password" "internal_token" {
  length  = 48
  special = false
}
resource "aws_secretsmanager_secret" "internal_token" { name = "${var.name}/internal-token" }
resource "aws_secretsmanager_secret_version" "internal_token" {
  secret_id     = aws_secretsmanager_secret.internal_token.id
  secret_string = random_password.internal_token.result
}

resource "aws_secretsmanager_secret" "admin_bearer" { name = "${var.name}/admin-bearer" }
resource "aws_secretsmanager_secret_version" "admin_bearer" {
  secret_id     = aws_secretsmanager_secret.admin_bearer.id
  secret_string = random_password.admin_bearer.result
}

# The sweep signer pool: `sweep_signer_count` keys, each an address the
# indexer keeps one helper transaction in flight on, so sweeps and
# withdrawal steps run side by side. Every key must be funded with gas on
# every chain (docs/production-runbook.md §8). Index 0 is the original
# single signer, kept at its address and alias.
resource "aws_kms_key" "signer" {
  count                    = var.sweep_signer_count
  description              = "${var.name} Ethereum transaction signer ${count.index}"
  key_usage                = "SIGN_VERIFY"
  customer_master_key_spec = "ECC_SECG_P256K1"
  deletion_window_in_days  = 30
  enable_key_rotation      = false
  lifecycle {
    prevent_destroy = true
  }
}
moved {
  from = aws_kms_key.signer
  to   = aws_kms_key.signer[0]
}
resource "aws_kms_alias" "signer" {
  count         = var.sweep_signer_count
  name          = count.index == 0 ? "alias/${var.name}-signer" : "alias/${var.name}-signer-${count.index}"
  target_key_id = aws_kms_key.signer[count.index].key_id
}
moved {
  from = aws_kms_alias.signer
  to   = aws_kms_alias.signer[0]
}

# Gum's dedicated recovery wallet: the recovery term committed into every
# deposit address, whatever verification the payer completed. Nothing in the
# stack signs with it: recovered funds (overpayment remainders, expired
# balances, late transfers) are reviewed and returned by hand, so no task
# role is granted kms:Sign.
resource "aws_kms_key" "recovery" {
  description              = "Payday recovery wallet; manual operator use only"
  key_usage                = "SIGN_VERIFY"
  customer_master_key_spec = "ECC_SECG_P256K1"
  deletion_window_in_days  = 30
  enable_key_rotation      = false
  lifecycle {
    prevent_destroy = true
  }
}
resource "aws_kms_alias" "recovery" {
  name          = "alias/${var.name}-recovery"
  target_key_id = aws_kms_key.recovery.key_id
}

# Signs the Payday-attested verification result carried in every Proof of
# Payment. It is deliberately neither the sweep signer nor the recovery key: a
# proof consumer trusts this address for attestations only, and it can never
# move funds.
resource "aws_kms_key" "attestation" {
  description              = "${var.name} Proof of Payment attestation signer"
  key_usage                = "SIGN_VERIFY"
  customer_master_key_spec = "ECC_SECG_P256K1"
  deletion_window_in_days  = 30
  enable_key_rotation      = false
  lifecycle {
    prevent_destroy = true
  }
}
resource "aws_kms_alias" "attestation" {
  name          = "alias/${var.name}-attestation"
  target_key_id = aws_kms_key.attestation.key_id
}

# Pays the dashboard onboarding walkthrough's one self-issued deposit request
# per account. Deliberately neither the sweep signer (whose key lives in the
# indexer's task role, not this one) nor the attestation key (which can never
# move funds): the demo payer is the one task-role key that broadcasts a
# transfer, so it is funded by hand with a little gas and USDC.
resource "aws_kms_key" "onboarding_payer" {
  description              = "${var.name} onboarding demo payer"
  key_usage                = "SIGN_VERIFY"
  customer_master_key_spec = "ECC_SECG_P256K1"
  deletion_window_in_days  = 30
  enable_key_rotation      = false
  lifecycle {
    prevent_destroy = true
  }
}
resource "aws_kms_alias" "onboarding_payer" {
  name          = "alias/${var.name}-onboarding-payer"
  target_key_id = aws_kms_key.onboarding_payer.key_id
}

resource "aws_ecr_repository" "api" {
  name                 = "${var.name}-api"
  image_tag_mutability = "IMMUTABLE"
  image_scanning_configuration { scan_on_push = true }
}
resource "aws_ecr_repository" "indexer" {
  name                 = "${var.name}-indexer"
  image_tag_mutability = "IMMUTABLE"
  image_scanning_configuration { scan_on_push = true }
}
resource "aws_ecr_repository" "signers" {
  name                 = "${var.name}-signers"
  image_tag_mutability = "IMMUTABLE"
  image_scanning_configuration { scan_on_push = true }
}

resource "aws_cloudwatch_log_group" "api" {
  name              = "/ecs/${var.name}/api"
  retention_in_days = 30
}
resource "aws_cloudwatch_log_group" "indexer" {
  name              = "/ecs/${var.name}/indexer"
  retention_in_days = 30
}
resource "aws_cloudwatch_log_group" "signers" {
  name              = "/ecs/${var.name}/signers"
  retention_in_days = 30
}

resource "aws_ecs_cluster" "this" {
  name = var.name
  setting {
    name  = "containerInsights"
    value = "enabled"
  }
}

resource "aws_iam_role" "api_execution" {
  name               = "${var.name}-api-execution"
  assume_role_policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Principal = { Service = "ecs-tasks.amazonaws.com" }, Action = "sts:AssumeRole" }] })
}
resource "aws_iam_role" "indexer_execution" {
  name               = "${var.name}-indexer-execution"
  assume_role_policy = aws_iam_role.api_execution.assume_role_policy
}
resource "aws_iam_role_policy_attachment" "api_execution" {
  role       = aws_iam_role.api_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}
resource "aws_iam_role_policy_attachment" "indexer_execution" {
  role       = aws_iam_role.indexer_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}
resource "aws_iam_role" "signers_execution" {
  name               = "${var.name}-signers-execution"
  assume_role_policy = aws_iam_role.api_execution.assume_role_policy
}
resource "aws_iam_role_policy_attachment" "signers_execution" {
  role       = aws_iam_role.signers_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

resource "aws_iam_role_policy" "api_secrets" {
  role   = aws_iam_role.api_execution.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Action = ["secretsmanager:GetSecretValue"], Resource = concat([aws_secretsmanager_secret.database_url.arn, aws_secretsmanager_secret.internal_token.arn, aws_secretsmanager_secret.webhook_encryption_key.arn, aws_secretsmanager_secret.admin_bearer.arn, aws_secretsmanager_secret.payer_ref_master_key.arn], values(aws_secretsmanager_secret.rpc_url)[*].arn, aws_secretsmanager_secret.resend_api_key[*].arn, aws_secretsmanager_secret.privy_app_secret[*].arn, aws_secretsmanager_secret.relay_api_key[*].arn) }] })
}
# The indexer reads chains and talks to gum-server: no database credential,
# no signing key.
resource "aws_iam_role_policy" "indexer_secrets" {
  role   = aws_iam_role.indexer_execution.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Action = ["secretsmanager:GetSecretValue"], Resource = concat([aws_secretsmanager_secret.internal_token.arn], values(aws_secretsmanager_secret.rpc_url)[*].arn) }] })
}
resource "aws_iam_role_policy" "signers_secrets" {
  role   = aws_iam_role.signers_execution.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Action = ["secretsmanager:GetSecretValue"], Resource = concat([aws_secretsmanager_secret.database_url.arn], values(aws_secretsmanager_secret.rpc_url)[*].arn) }] })
}

resource "aws_iam_role" "api_task" {
  name               = "${var.name}-api-task"
  assume_role_policy = aws_iam_role.api_execution.assume_role_policy
}
resource "aws_sesv2_email_identity" "notifications" {
  email_identity = var.notification_domain_name
}
# The DKIM CNAMEs live under notification_domain_name (payday.sh), whose DNS
# is hosted at Vercel, not in the API's Route53 zone; they are published as
# the notification_dkim_records output and added there by hand.
resource "aws_iam_role_policy" "api_ses" {
  role   = aws_iam_role.api_task.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Action = ["ses:SendEmail"], Resource = aws_sesv2_email_identity.notifications.arn }] })
}
resource "aws_iam_role_policy" "api_attestation_kms" {
  role   = aws_iam_role.api_task.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Action = ["kms:GetPublicKey", "kms:Sign"], Resource = aws_kms_key.attestation.arn }] })
}
resource "aws_iam_role_policy" "api_onboarding_payer_kms" {
  role   = aws_iam_role.api_task.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Action = ["kms:GetPublicKey", "kms:Sign"], Resource = aws_kms_key.onboarding_payer.arn }] })
}
# The indexer's task role has no permissions at all: it is a read-only
# observer of chains and a client of gum-server.
resource "aws_iam_role" "indexer_task" {
  name               = "${var.name}-indexer-task"
  assume_role_policy = aws_iam_role.api_execution.assume_role_policy
}
# gum-signers is the only process that can sign with the sweep pool.
resource "aws_iam_role" "signers_task" {
  name               = "${var.name}-signers-task"
  assume_role_policy = aws_iam_role.api_execution.assume_role_policy
}
resource "aws_iam_role_policy" "signers_kms" {
  role   = aws_iam_role.signers_task.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Action = ["kms:GetPublicKey", "kms:Sign"], Resource = concat(aws_kms_key.signer[*].arn, [aws_kms_key.onboarding_payer.arn]) }] })
}

# ---------------------------------------------------------------------------
# Deposit request attachments. One PDF per deposit request, uploaded straight to S3 through a
# presigned PUT signed by the API task role, scanned by GuardDuty Malware
# Protection, and finalized by gum-server only once the managed tag
# GuardDutyMalwareScanStatus=NO_THREATS_FOUND is present.
#
# Object layout: every upload lands at uploads/<account_id>/<attachment_id>.pdf
# and is never moved. The presigned PUT stamps the tag payday-upload=pending on
# it (a signed header, so no upload can omit it); issuing a deposit request rewrites
# that tag to payday-upload=attached before the deposit request commits. The lifecycle
# rule expires only objects still tagged pending, so infrastructure can never
# expire an attached PDF, and an abandoned upload is gone after seven days
# without gum-server having to track it.
# ---------------------------------------------------------------------------

resource "aws_kms_key" "attachments" {
  description             = "${var.name} deposit request attachment encryption"
  deletion_window_in_days = 30
  enable_key_rotation     = true
  lifecycle {
    prevent_destroy = true
  }
}
resource "aws_kms_alias" "attachments" {
  name          = "alias/${var.name}-attachments"
  target_key_id = aws_kms_key.attachments.key_id
}

resource "aws_s3_bucket" "attachments" {
  bucket = "${var.name}-invoice-attachments"
}
resource "aws_s3_bucket_ownership_controls" "attachments" {
  bucket = aws_s3_bucket.attachments.id
  rule {
    object_ownership = "BucketOwnerEnforced"
  }
}
resource "aws_s3_bucket_public_access_block" "attachments" {
  bucket                  = aws_s3_bucket.attachments.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}
resource "aws_s3_bucket_versioning" "attachments" {
  bucket = aws_s3_bucket.attachments.id
  versioning_configuration {
    status = "Enabled"
  }
}
resource "aws_s3_bucket_server_side_encryption_configuration" "attachments" {
  bucket = aws_s3_bucket.attachments.id
  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm     = "aws:kms"
      kms_master_key_id = aws_kms_key.attachments.arn
    }
    bucket_key_enabled = true
  }
}
resource "aws_s3_bucket_lifecycle_configuration" "attachments" {
  bucket = aws_s3_bucket.attachments.id
  rule {
    id     = "expire-unattached-uploads"
    status = "Enabled"
    filter {
      and {
        prefix = "uploads/"
        tags   = { payday-upload = "pending" }
      }
    }
    expiration {
      days = 7
    }
    # Versioning turns the expiration above into a delete marker; the version
    # underneath is still tagged pending and goes seven days later.
    noncurrent_version_expiration {
      noncurrent_days = 7
    }
  }
  depends_on = [aws_s3_bucket_versioning.attachments]
}
# The dashboard uploads from the browser, so the bucket must answer its
# preflight. The presigned signature, not CORS, is what constrains the request.
resource "aws_s3_bucket_cors_configuration" "attachments" {
  bucket = aws_s3_bucket.attachments.id
  cors_rule {
    allowed_origins = [local.checkout_base_url]
    allowed_methods = ["PUT"]
    allowed_headers = ["*"]
    expose_headers  = ["ETag"]
    max_age_seconds = 3600
  }
}

# GuardDuty scans every new object through this role, then tags it with the
# verdict. The permissions are the AWS prerequisite policy for a bucket
# encrypted with a customer managed key.
resource "aws_iam_role" "malware_protection" {
  name               = "${var.name}-attachments-malware-protection"
  assume_role_policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Principal = { Service = "malware-protection-plan.guardduty.amazonaws.com" }, Action = "sts:AssumeRole" }] })
}
resource "aws_iam_role_policy" "malware_protection" {
  role = aws_iam_role.malware_protection.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "ManageScanTriggerRule"
        Effect    = "Allow"
        Action    = ["events:PutRule", "events:DeleteRule", "events:PutTargets", "events:RemoveTargets"]
        Resource  = "arn:aws:events:${var.aws_region}:${data.aws_caller_identity.current.account_id}:rule/DO-NOT-DELETE-AmazonGuardDutyMalwareProtectionS3*"
        Condition = { StringLike = { "events:ManagedBy" = "malware-protection-plan.guardduty.amazonaws.com" } }
      },
      {
        Sid      = "MonitorScanTriggerRule"
        Effect   = "Allow"
        Action   = ["events:DescribeRule", "events:ListTargetsByRule"]
        Resource = "arn:aws:events:${var.aws_region}:${data.aws_caller_identity.current.account_id}:rule/DO-NOT-DELETE-AmazonGuardDutyMalwareProtectionS3*"
      },
      {
        Sid      = "TagScanVerdicts"
        Effect   = "Allow"
        Action   = ["s3:PutObjectTagging", "s3:GetObjectTagging", "s3:PutObjectVersionTagging", "s3:GetObjectVersionTagging"]
        Resource = "${aws_s3_bucket.attachments.arn}/*"
      },
      {
        Sid      = "EnableBucketEvents"
        Effect   = "Allow"
        Action   = ["s3:PutBucketNotification", "s3:GetBucketNotification"]
        Resource = aws_s3_bucket.attachments.arn
      },
      {
        Sid      = "PutValidationObject"
        Effect   = "Allow"
        Action   = "s3:PutObject"
        Resource = "${aws_s3_bucket.attachments.arn}/malware-protection-resource-validation-object"
      },
      {
        Sid      = "CheckBucketOwnership"
        Effect   = "Allow"
        Action   = "s3:ListBucket"
        Resource = aws_s3_bucket.attachments.arn
      },
      {
        Sid      = "ReadObjectsToScan"
        Effect   = "Allow"
        Action   = ["s3:GetObject", "s3:GetObjectVersion"]
        Resource = "${aws_s3_bucket.attachments.arn}/*"
      },
      {
        Sid       = "DecryptObjectsToScan"
        Effect    = "Allow"
        Action    = ["kms:GenerateDataKey", "kms:Decrypt"]
        Resource  = aws_kms_key.attachments.arn
        Condition = { StringLike = { "kms:ViaService" = "s3.${var.aws_region}.amazonaws.com" } }
      }
    ]
  })
}
resource "aws_guardduty_malware_protection_plan" "attachments" {
  role = aws_iam_role.malware_protection.arn
  protected_resource {
    s3_bucket {
      bucket_name = aws_s3_bucket.attachments.id
    }
  }
  actions {
    tagging {
      status = "ENABLED"
    }
  }
  # GuardDuty validates the role by writing a probe object at creation, so the
  # permissions above and the bucket policy must already be in place.
  depends_on = [aws_iam_role_policy.malware_protection, aws_s3_bucket_policy.attachments]
}

# Nobody reads an object before its scan came back clean, except the scanner
# itself and the API task role, which checks the verdict tag in code before it
# streams anything (HEAD is s3:GetObject too, so the role must be exempt to
# report a pending scan at all). Presigned downloads are signed by the task
# role and are only ever issued for attached, clean objects.
resource "aws_s3_bucket_policy" "attachments" {
  bucket = aws_s3_bucket.attachments.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "DenyInsecureTransport"
        Effect    = "Deny"
        Principal = "*"
        Action    = "s3:*"
        Resource  = [aws_s3_bucket.attachments.arn, "${aws_s3_bucket.attachments.arn}/*"]
        Condition = { Bool = { "aws:SecureTransport" = "false" } }
      },
      {
        Sid       = "DenyReadsUntilScanIsClean"
        Effect    = "Deny"
        Principal = "*"
        Action    = ["s3:GetObject", "s3:GetObjectVersion"]
        Resource  = "${aws_s3_bucket.attachments.arn}/*"
        Condition = {
          StringNotEquals = { "s3:ExistingObjectTag/GuardDutyMalwareScanStatus" = "NO_THREATS_FOUND" }
          ArnNotEquals    = { "aws:PrincipalArn" = [aws_iam_role.api_task.arn, aws_iam_role.malware_protection.arn] }
        }
      }
    ]
  })
  depends_on = [aws_s3_bucket_public_access_block.attachments]
}

# What gum-server may do with uploads, and nothing else: sign PUTs that carry the
# pending tag, inspect and stream objects, read and rewrite tags, and delete
# rejected uploads. No ListBucket, nothing outside uploads/.
resource "aws_iam_role_policy" "api_attachments" {
  role = aws_iam_role.api_task.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid      = "ManageUploads"
        Effect   = "Allow"
        Action   = ["s3:PutObject", "s3:PutObjectTagging", "s3:PutObjectVersionTagging", "s3:GetObject", "s3:GetObjectVersion", "s3:GetObjectTagging", "s3:GetObjectVersionTagging", "s3:DeleteObject"]
        Resource = "${aws_s3_bucket.attachments.arn}/uploads/*"
      },
      {
        Sid       = "UseAttachmentKeyThroughS3"
        Effect    = "Allow"
        Action    = ["kms:GenerateDataKey", "kms:Decrypt"]
        Resource  = aws_kms_key.attachments.arn
        Condition = { StringEquals = { "kms:ViaService" = "s3.${var.aws_region}.amazonaws.com" } }
      }
    ]
  })
}

resource "aws_ecs_task_definition" "api" {
  family                   = "${var.name}-api"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.api_cpu
  memory                   = var.api_memory
  execution_role_arn       = aws_iam_role.api_execution.arn
  task_role_arn            = aws_iam_role.api_task.arn
  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "X86_64"
  }
  container_definitions = jsonencode([{
    name                   = "api", image = "${aws_ecr_repository.api.repository_url}:${var.image_tag}", essential = true,
    readonlyRootFilesystem = true,
    portMappings = [
      { containerPort = var.api_port, protocol = "tcp" },
      { containerPort = var.internal_port, protocol = "tcp", name = "internal" }
    ],
    stopTimeout = 120,
    environment = concat(local.common_environment, [
      { name = "PAYDAY_BIND_ADDR", value = "0.0.0.0:${var.api_port}" },
      { name = "PAYDAY_INTERNAL_BIND_ADDR", value = "0.0.0.0:${var.internal_port}" },
      { name = "PAYDAY_PRIVY_APP_ID", value = var.privy_app_id },
      { name = "PAYDAY_API_KEY_PREFIX", value = var.api_key_prefix },
      { name = "PAYDAY_PUBLIC_BASE_URL", value = local.checkout_base_url },
      { name = "PAYDAY_NOTIFICATION_FROM_ADDRESS", value = var.notification_from_address },
      { name = "PAYDAY_ATTACHMENT_BUCKET", value = aws_s3_bucket.attachments.id },
      { name = "PAYDAY_ATTESTATION_KMS_KEY_ID", value = aws_kms_key.attestation.arn },
      { name = "PAYDAY_ONBOARDING_PAYER_KMS_KEY_ID", value = aws_kms_key.onboarding_payer.arn },
      { name = "PAYDAY_RECOVERY_ADDRESS", value = var.recovery_address }
    ], local.payer_environment, local.payer_email_environment, local.identity_environment),
    # The recovery address is the custody term committed into every payment
    # contract at deployment; a wrong or zero address would misroute every
    # recovery, so the definition refuses to apply without one.
    lifecycle {
      precondition {
        condition     = can(regex("^0x[0-9a-fA-F]{40}$", var.recovery_address)) && lower(var.recovery_address) != "0x0000000000000000000000000000000000000000"
        error_message = "recovery_address must be a nonzero 20-byte EVM address, derived from the recovery KMS key's public key (see infra/README.md)."
      }
    },
    # The API verifies the deployed contract generation on every chain at
    # startup, so it reads each chain through the same RPC secrets as the indexer.
    secrets = concat([
      { name = "DATABASE_URL", valueFrom = aws_secretsmanager_secret.database_url.arn },
      { name = "PAYDAY_INTERNAL_TOKEN", valueFrom = aws_secretsmanager_secret.internal_token.arn },
      { name = "PAYDAY_WEBHOOK_ENCRYPTION_KEY", valueFrom = aws_secretsmanager_secret.webhook_encryption_key.arn },
      { name = "PAYDAY_ADMIN_BEARER_SECRET", valueFrom = aws_secretsmanager_secret.admin_bearer.arn }
    ], local.rpc_url_secrets, local.payer_secrets, local.payer_email_secrets, local.identity_secrets, local.privy_secrets, local.relay_secrets),
    logConfiguration = { logDriver = "awslogs", options = { "awslogs-group" = aws_cloudwatch_log_group.api.name, "awslogs-region" = var.aws_region, "awslogs-stream-prefix" = "api" } }
  }])
}

resource "aws_ecs_task_definition" "indexer" {
  family                   = "${var.name}-indexer"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.indexer_cpu
  memory                   = var.indexer_memory
  execution_role_arn       = aws_iam_role.indexer_execution.arn
  task_role_arn            = aws_iam_role.indexer_task.arn
  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "X86_64"
  }
  container_definitions = jsonencode([{
    name                   = "indexer", image = "${aws_ecr_repository.indexer.repository_url}:${var.image_tag}", essential = true,
    readonlyRootFilesystem = true,
    stopTimeout            = 120,
    environment = concat(local.common_environment, [
      { name = "PAYDAY_SERVER_INTERNAL_URL", value = "http://${local.api_internal_hostname}:${var.internal_port}" },
      { name = "PAYDAY_INDEXER_LISTEN_ADDR", value = "0.0.0.0:${var.health_port}" },
      { name = "PAYDAY_INDEXER_POLL_INTERVAL_MS", value = tostring(var.indexer_poll_interval_ms) },
      { name = "PAYDAY_INDEXER_RECONCILE_INTERVAL_MS", value = tostring(var.indexer_reconcile_interval_ms) },
      { name = "PAYDAY_INDEXER_IDLE_INTERVAL_MS", value = tostring(var.indexer_idle_interval_ms) }
    ]),
    secrets          = concat([{ name = "PAYDAY_INTERNAL_TOKEN", valueFrom = aws_secretsmanager_secret.internal_token.arn }], local.rpc_url_secrets),
    logConfiguration = { logDriver = "awslogs", options = { "awslogs-group" = aws_cloudwatch_log_group.indexer.name, "awslogs-region" = var.aws_region, "awslogs-stream-prefix" = "indexer" } }
  }])

  lifecycle {
    precondition {
      condition     = alltrue([for c in var.chains : contains(keys(var.rpc_urls), tostring(c.chain_id))])
      error_message = "rpc_urls must carry an endpoint for every chain in chains, keyed by decimal chain id."
    }
  }
}

resource "aws_ecs_task_definition" "signers" {
  family                   = "${var.name}-signers"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.signers_cpu
  memory                   = var.signers_memory
  execution_role_arn       = aws_iam_role.signers_execution.arn
  task_role_arn            = aws_iam_role.signers_task.arn
  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "X86_64"
  }
  container_definitions = jsonencode([{
    name                   = "signers", image = "${aws_ecr_repository.signers.repository_url}:${var.image_tag}", essential = true,
    readonlyRootFilesystem = true,
    # A helper transaction that is signed but not yet broadcast is durable;
    # the stop timeout only lets an in-progress RPC call finish.
    stopTimeout = 120,
    environment = concat(local.common_environment, [
      { name = "PAYDAY_KMS_KEY_IDS", value = join(",", aws_kms_key.signer[*].arn) },
      { name = "PAYDAY_ONBOARDING_PAYER_KMS_KEY_ID", value = aws_kms_key.onboarding_payer.arn },
      { name = "PAYDAY_SIGNERS_LISTEN_ADDR", value = "0.0.0.0:${var.health_port}" }
    ]),
    secrets          = concat([{ name = "DATABASE_URL", valueFrom = aws_secretsmanager_secret.database_url.arn }], local.rpc_url_secrets),
    logConfiguration = { logDriver = "awslogs", options = { "awslogs-group" = aws_cloudwatch_log_group.signers.name, "awslogs-region" = var.aws_region, "awslogs-stream-prefix" = "signers" } }
  }])
}

# `gum-server migrate` as a one-off task: the deploy runs it with the new
# image before the services roll (docs/production-runbook.md). Same image,
# same credentials, no ports.
resource "aws_ecs_task_definition" "migrate" {
  family                   = "${var.name}-migrate"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = 256
  memory                   = 512
  execution_role_arn       = aws_iam_role.api_execution.arn
  task_role_arn            = aws_iam_role.api_task.arn
  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "X86_64"
  }
  container_definitions = jsonencode([{
    name                   = "migrate", image = "${aws_ecr_repository.api.repository_url}:${var.image_tag}", essential = true,
    readonlyRootFilesystem = true,
    command                = ["migrate"],
    environment            = [{ name = "RUST_LOG", value = "info" }],
    secrets                = [{ name = "DATABASE_URL", valueFrom = aws_secretsmanager_secret.database_url.arn }],
    logConfiguration       = { logDriver = "awslogs", options = { "awslogs-group" = aws_cloudwatch_log_group.api.name, "awslogs-region" = var.aws_region, "awslogs-stream-prefix" = "migrate" } }
  }])
}

# Private DNS so the indexer finds gum-server's internal listener by name
# (api.<name>.local). Cloud Map registers each api task's private IP as it
# starts; the record's short TTL means a rolled task disappears within a
# minute, and the indexer's HTTP retries cover the gap.
resource "aws_service_discovery_private_dns_namespace" "this" {
  name = "${var.name}.local"
  vpc  = aws_vpc.this.id
}
resource "aws_service_discovery_service" "api" {
  name = "api"
  dns_config {
    namespace_id   = aws_service_discovery_private_dns_namespace.this.id
    routing_policy = "MULTIVALUE"
    dns_records {
      type = "A"
      ttl  = 10
    }
  }
  health_check_custom_config {}
}
locals {
  api_internal_hostname = "${aws_service_discovery_service.api.name}.${aws_service_discovery_private_dns_namespace.this.name}"
}

resource "aws_lb" "api" {
  name               = var.name
  load_balancer_type = "application"
  subnets            = aws_subnet.public[*].id
  security_groups    = [aws_security_group.alb.id]
}
resource "aws_lb_target_group" "api" {
  name        = "${var.name}-api"
  port        = var.api_port
  protocol    = "HTTP"
  target_type = "ip"
  vpc_id      = aws_vpc.this.id
  health_check {
    path    = "/health"
    matcher = "200-399"
  }
}
resource "aws_acm_certificate" "api" {
  domain_name       = var.domain_name
  validation_method = "DNS"
  lifecycle { create_before_destroy = true }
}
resource "aws_route53_record" "validation" {
  for_each = { for dvo in aws_acm_certificate.api.domain_validation_options : dvo.domain_name => { name = dvo.resource_record_name, record = dvo.resource_record_value, type = dvo.resource_record_type } }
  zone_id  = var.route53_zone_id
  name     = each.value.name
  type     = each.value.type
  records  = [each.value.record]
  ttl      = 60
}
resource "aws_acm_certificate_validation" "api" {
  certificate_arn         = aws_acm_certificate.api.arn
  validation_record_fqdns = [for r in aws_route53_record.validation : r.fqdn]
}
resource "aws_lb_listener" "http" {
  load_balancer_arn = aws_lb.api.arn
  port              = 80
  protocol          = "HTTP"
  default_action {
    type = "redirect"
    redirect {
      port        = "443"
      protocol    = "HTTPS"
      status_code = "HTTP_301"
    }
  }
}
resource "aws_lb_listener" "https" {
  load_balancer_arn = aws_lb.api.arn
  port              = 443
  protocol          = "HTTPS"
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-2021-06"
  certificate_arn   = aws_acm_certificate_validation.api.certificate_arn
  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.api.arn
  }
}
resource "aws_route53_record" "api" {
  zone_id = var.route53_zone_id
  name    = var.domain_name
  type    = "A"
  alias {
    name                   = aws_lb.api.dns_name
    zone_id                = aws_lb.api.zone_id
    evaluate_target_health = true
  }
}
resource "aws_ecs_service" "api" {
  name            = "api"
  cluster         = aws_ecs_cluster.this.id
  task_definition = aws_ecs_task_definition.api.arn
  desired_count   = var.api_desired_count
  launch_type     = "FARGATE"
  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }
  network_configuration {
    subnets          = aws_subnet.public[*].id
    security_groups  = [aws_security_group.api.id]
    assign_public_ip = true
  }
  load_balancer {
    target_group_arn = aws_lb_target_group.api.arn
    container_name   = "api"
    container_port   = var.api_port
  }
  service_registries {
    registry_arn = aws_service_discovery_service.api.arn
  }
  depends_on = [aws_lb_listener.https]
}
# One indexer task: the ledger's compare-and-set cursor makes a second one
# safe, merely wasteful, so the deployment may briefly overlap.
resource "aws_ecs_service" "indexer" {
  name                               = "indexer"
  cluster                            = aws_ecs_cluster.this.id
  task_definition                    = aws_ecs_task_definition.indexer.arn
  desired_count                      = 1
  launch_type                        = "FARGATE"
  deployment_minimum_healthy_percent = 0
  deployment_maximum_percent         = 100
  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }
  network_configuration {
    subnets          = aws_subnet.public[*].id
    security_groups  = [aws_security_group.indexer.id]
    assign_public_ip = true
  }
}
# One signers task. Two would be safe for correctness (every lane is a row
# lock and every job an idempotent command) but would race for the same
# signer nonces and lose to each other; the deployment therefore stops the
# old task before starting the new one.
resource "aws_ecs_service" "signers" {
  name                               = "signers"
  cluster                            = aws_ecs_cluster.this.id
  task_definition                    = aws_ecs_task_definition.signers.arn
  desired_count                      = 1
  launch_type                        = "FARGATE"
  deployment_minimum_healthy_percent = 0
  deployment_maximum_percent         = 100
  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }
  network_configuration {
    subnets          = aws_subnet.public[*].id
    security_groups  = [aws_security_group.signers.id]
    assign_public_ip = true
  }
}

resource "aws_wafv2_web_acl" "api" {
  name  = var.name
  scope = "REGIONAL"
  default_action {
    allow {}
  }
  rule {
    name     = "ip-rate-limit"
    priority = 1
    action {
      block {}
    }
    statement {
      rate_based_statement {
        limit              = var.waf_rate_limit
        aggregate_key_type = "IP"
      }
    }
    visibility_config {
      cloudwatch_metrics_enabled = true
      metric_name                = "${var.name}-rate-limit"
      sampled_requests_enabled   = false
    }
  }
  visibility_config {
    cloudwatch_metrics_enabled = true
    metric_name                = var.name
    sampled_requests_enabled   = false
  }
}
resource "aws_wafv2_web_acl_association" "api" {
  resource_arn = aws_lb.api.arn
  web_acl_arn  = aws_wafv2_web_acl.api.arn
}

resource "aws_sns_topic" "alarms" {
  name = "${var.name}-alarms"
}

resource "aws_sns_topic_subscription" "alarm_email" {
  topic_arn = aws_sns_topic.alarms.arn
  protocol  = "email"
  endpoint  = var.alarm_email
}

resource "aws_cloudwatch_metric_alarm" "unhealthy" {
  alarm_name          = "${var.name}-unhealthy-targets"
  namespace           = "AWS/ApplicationELB"
  metric_name         = "UnHealthyHostCount"
  statistic           = "Maximum"
  period              = 60
  evaluation_periods  = 2
  threshold           = 0
  comparison_operator = "GreaterThanThreshold"
  treat_missing_data  = "notBreaching"
  dimensions          = { LoadBalancer = aws_lb.api.arn_suffix, TargetGroup = aws_lb_target_group.api.arn_suffix }
  alarm_actions       = [aws_sns_topic.alarms.arn]
}
resource "aws_cloudwatch_metric_alarm" "api_tasks" {
  alarm_name          = "${var.name}-api-task-count"
  namespace           = "ECS/ContainerInsights"
  metric_name         = "RunningTaskCount"
  statistic           = "Minimum"
  period              = 60
  evaluation_periods  = 2
  threshold           = var.api_desired_count
  comparison_operator = "LessThanThreshold"
  treat_missing_data  = "breaching"
  dimensions          = { ClusterName = aws_ecs_cluster.this.name, ServiceName = aws_ecs_service.api.name }
  alarm_actions       = [aws_sns_topic.alarms.arn]
}
resource "aws_cloudwatch_metric_alarm" "indexer_tasks" {
  alarm_name          = "${var.name}-indexer-task-count"
  namespace           = "ECS/ContainerInsights"
  metric_name         = "RunningTaskCount"
  statistic           = "Minimum"
  period              = 60
  evaluation_periods  = 2
  threshold           = 1
  comparison_operator = "LessThanThreshold"
  treat_missing_data  = "breaching"
  dimensions          = { ClusterName = aws_ecs_cluster.this.name, ServiceName = aws_ecs_service.indexer.name }
  alarm_actions       = [aws_sns_topic.alarms.arn]
}
resource "aws_cloudwatch_metric_alarm" "signers_tasks" {
  alarm_name          = "${var.name}-signers-task-count"
  namespace           = "ECS/ContainerInsights"
  metric_name         = "RunningTaskCount"
  statistic           = "Minimum"
  period              = 60
  evaluation_periods  = 2
  threshold           = 1
  comparison_operator = "LessThanThreshold"
  treat_missing_data  = "breaching"
  dimensions          = { ClusterName = aws_ecs_cluster.this.name, ServiceName = aws_ecs_service.signers.name }
  alarm_actions       = [aws_sns_topic.alarms.arn]
}
resource "aws_cloudwatch_log_metric_filter" "notification_failures" {
  name           = "${var.name}-notification-delivery-failures"
  log_group_name = aws_cloudwatch_log_group.api.name
  pattern        = "\"merchant notification delivery failed; retrying\""
  metric_transformation {
    name      = "NotificationDeliveryFailures"
    namespace = var.name
    value     = "1"
  }
}
resource "aws_cloudwatch_metric_alarm" "notification_failures" {
  alarm_name          = "${var.name}-notification-delivery-failures"
  alarm_description   = "Merchant email or webhook delivery failed repeatedly; inspect gum-server logs and notification_outbox"
  namespace           = var.name
  metric_name         = aws_cloudwatch_log_metric_filter.notification_failures.metric_transformation[0].name
  statistic           = "Sum"
  period              = 300
  evaluation_periods  = 1
  threshold           = 5
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"
  alarm_actions       = [aws_sns_topic.alarms.arn]
  ok_actions          = [aws_sns_topic.alarms.arn]
}
resource "aws_cloudwatch_log_metric_filter" "notification_missing_contact" {
  name           = "${var.name}-notification-missing-contact"
  log_group_name = aws_cloudwatch_log_group.api.name
  pattern        = "\"blocked payment notification has no merchant email\""
  metric_transformation {
    name      = "NotificationMissingContact"
    namespace = var.name
    value     = "1"
  }
}
resource "aws_cloudwatch_metric_alarm" "notification_missing_contact" {
  alarm_name          = "${var.name}-notification-missing-contact"
  alarm_description   = "A blocked deposit request cannot notify its merchant by email; recover the account contact and contact the merchant manually"
  namespace           = var.name
  metric_name         = aws_cloudwatch_log_metric_filter.notification_missing_contact.metric_transformation[0].name
  statistic           = "Sum"
  period              = 60
  evaluation_periods  = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"
  alarm_actions       = [aws_sns_topic.alarms.arn]
}
# Conditions the services log but do not exit on. Each is logged on every
# pass while it holds, so an alarm stays raised until the condition clears.
# The patterns are the exact `tracing` messages; the log lines carry the
# chain_id, job id and signer as structured fields.
locals {
  log_alarms = {
    indexer_chain_halted = {
      log_group   = aws_cloudwatch_log_group.indexer.name
      pattern     = "\"finality violation; chain halted\""
      period      = 60
      threshold   = 0
      description = "A finalized block changed hash. The chain is halted: nothing is scanned or swept on it until an operator repairs the cursor and runs `gum-server chain resume <chain-id>` (docs/architecture.md)"
    }
    indexer_pass_failing = {
      log_group   = aws_cloudwatch_log_group.indexer.name
      pattern     = "\"indexer pass failed; retrying\""
      period      = 300
      threshold   = 10
      description = "Sustained RPC or gum-server failures in the indexer; payments are detected late until it clears"
    }
    indexer_signal_down = {
      log_group   = aws_cloudwatch_log_group.indexer.name
      pattern     = "?\"transfer signal disconnected\" ?\"transfer signal connection failed\""
      period      = 600
      threshold   = 0
      description = "The indexer's WebSocket transfer signal keeps failing; payments are still detected on the poll cadence at the old request cost. Check the RPC endpoint's WebSocket status"
    }
    signers_stalled = {
      log_group   = aws_cloudwatch_log_group.signers.name
      pattern     = "\"transaction unconfirmed after the replacement limit; fees are no longer raised\""
      period      = 60
      threshold   = 0
      description = "A helper transaction was replaced past the limit without mining; the signers keep re-broadcasting. See docs/runbooks/stuck-deposit-request.md"
    }
    signers_low_balance = {
      log_group   = aws_cloudwatch_log_group.signers.name
      pattern     = "\"signer balance is low\""
      period      = 300
      threshold   = 0
      description = "A KMS sweep signer is below its chain's low-balance level; the log line names the address, fund it"
    }
    signers_failing = {
      log_group   = aws_cloudwatch_log_group.signers.name
      pattern     = "?\"job deferred after a transient failure\" ?\"reconciling transaction failed\" ?\"broadcast failed; the signed transaction stays durable for the next pass\""
      period      = 300
      threshold   = 10
      description = "Sustained RPC, KMS or database failures in the signers"
    }
    server_dead_letter = {
      log_group   = aws_cloudwatch_log_group.api.name
      pattern     = "?\"dead-lettering\" ?\"message handling failed permanently; dead-lettered\""
      period      = 60
      threshold   = 0
      description = "A bus delivery was parked for an operator: `gum-server bus dead`, then `gum-server bus retry <message-id>` once the cause is fixed"
    }
    server_sweep_stuck = {
      log_group   = aws_cloudwatch_log_group.api.name
      pattern     = "?\"sweep job open past the stuck threshold; check gum-signers\" ?\"execution stalled\""
      period      = 300
      threshold   = 0
      description = "A sweep job has had no terminal event for over an hour, or the signers reported it stalled"
    }
    server_attention = {
      log_group   = aws_cloudwatch_log_group.api.name
      pattern     = "\"deposit request needs operator attention\""
      period      = 300
      threshold   = 0
      description = "A deposit request was marked for manual attention; follow docs/runbooks/stuck-deposit-request.md"
    }
    signers_dead_letter = {
      log_group   = aws_cloudwatch_log_group.signers.name
      pattern     = "\"message handling failed permanently; dead-lettered\""
      period      = 60
      threshold   = 0
      description = "A sweep command was dead-lettered by the signers: `gum-server bus dead`"
    }
  }
}

resource "aws_cloudwatch_log_metric_filter" "service" {
  for_each       = local.log_alarms
  name           = "${var.name}-${replace(each.key, "_", "-")}"
  log_group_name = each.value.log_group
  pattern        = each.value.pattern
  metric_transformation {
    name      = title(replace(each.key, "_", ""))
    namespace = var.name
    value     = "1"
  }
}

resource "aws_cloudwatch_metric_alarm" "service" {
  for_each            = local.log_alarms
  alarm_name          = "${var.name}-${replace(each.key, "_", "-")}"
  alarm_description   = each.value.description
  namespace           = var.name
  metric_name         = aws_cloudwatch_log_metric_filter.service[each.key].metric_transformation[0].name
  statistic           = "Sum"
  period              = each.value.period
  evaluation_periods  = 1
  threshold           = each.value.threshold
  comparison_operator = "GreaterThanThreshold"
  treat_missing_data  = "notBreaching"
  alarm_actions       = [aws_sns_topic.alarms.arn]
  ok_actions          = [aws_sns_topic.alarms.arn]
}

resource "aws_cloudwatch_metric_alarm" "db_cpu" {
  alarm_name          = "${var.name}-db-high-cpu"
  namespace           = "AWS/RDS"
  metric_name         = "CPUUtilization"
  statistic           = "Average"
  period              = 300
  evaluation_periods  = 3
  threshold           = 80
  comparison_operator = "GreaterThanThreshold"
  dimensions          = { DBInstanceIdentifier = aws_db_instance.this.identifier }
  alarm_actions       = [aws_sns_topic.alarms.arn]
}
resource "aws_cloudwatch_metric_alarm" "db_storage" {
  alarm_name          = "${var.name}-db-low-storage"
  namespace           = "AWS/RDS"
  metric_name         = "FreeStorageSpace"
  statistic           = "Minimum"
  period              = 300
  evaluation_periods  = 1
  threshold           = 5368709120
  comparison_operator = "LessThanThreshold"
  dimensions          = { DBInstanceIdentifier = aws_db_instance.this.identifier }
  alarm_actions       = [aws_sns_topic.alarms.arn]
}

# Continuous deployment of main from GitHub Actions (docs/staging.md). The
# OIDC provider is one per AWS account, so only the environment that sets
# github_repository declares it. The role trusts exactly that repository's
# main branch; it carries AdministratorAccess because a Terraform apply of
# this stack touches IAM, KMS, RDS, ECS, S3, and more, which is why the trust
# policy, not the permissions, is the control.
resource "aws_iam_openid_connect_provider" "github" {
  count           = var.github_repository != "" ? 1 : 0
  url             = "https://token.actions.githubusercontent.com"
  client_id_list  = ["sts.amazonaws.com"]
  thumbprint_list = ["6938fd4d98bab03faadb97b34396831e3780aea1", "1c58a3a8518e8759bf075b76b750d4f2df264fcd"]
}
resource "aws_iam_role" "github_deploy" {
  count = var.github_repository != "" ? 1 : 0
  name  = "${var.name}-github-deploy"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Federated = aws_iam_openid_connect_provider.github[0].arn }
      Action    = "sts:AssumeRoleWithWebIdentity"
      Condition = {
        StringEquals = { "token.actions.githubusercontent.com:aud" = "sts.amazonaws.com" }
        StringLike   = { "token.actions.githubusercontent.com:sub" = "repo:${var.github_repository}:ref:refs/heads/main" }
      }
    }]
  })
}
resource "aws_iam_role_policy_attachment" "github_deploy" {
  count      = var.github_repository != "" ? 1 : 0
  role       = aws_iam_role.github_deploy[0].name
  policy_arn = "arn:aws:iam::aws:policy/AdministratorAccess"
}
