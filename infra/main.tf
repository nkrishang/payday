data "aws_availability_zones" "available" { state = "available" }
data "aws_caller_identity" "current" {}

locals {
  # Payer links point at the hosted checkout. gatewayd validates this as a bare
  # HTTPS origin and redirects its own /pay/{id} to it, so links shared before
  # the checkout moved keep working.
  checkout_base_url = var.checkout_base_url != "" ? var.checkout_base_url : "https://${var.payment_domain_name}"

  azs = slice(data.aws_availability_zones.available.names, 0, 2)
  certificate_validation_zone_ids = {
    (var.domain_name)         = var.route53_zone_id
    (var.payment_domain_name) = var.payment_route53_zone_id
    (var.status_domain_name)  = var.route53_zone_id
  }
  common_environment = [
    { name = "PAYDAY_CHAIN_ID", value = tostring(var.chain_id) },
    { name = "PAYDAY_FACTORY_ADDRESS", value = var.factory_address },
    { name = "PAYDAY_BATCH_SWEEPER_ADDRESS", value = var.batch_sweeper_address },
    { name = "PAYDAY_FACTORY_CODE_HASH", value = var.factory_code_hash },
    { name = "PAYDAY_BATCH_SWEEPER_CODE_HASH", value = var.batch_sweeper_code_hash },
    { name = "PAYDAY_USDC_ADDRESS", value = var.usdc_address },
    { name = "RUST_LOG", value = "info" }
  ]
  # The dashboard's Auth0 application is optional until it exists. Omit the
  # variable rather than pass an empty value, so gatewayd sees the same absence
  # as a deployment that has no dashboard.
  dashboard_environment = var.dashboard_auth0_client_id == "" ? [] : [
    { name = "PAYDAY_DASHBOARD_AUTH0_CLIENT_ID", value = var.dashboard_auth0_client_id }
  ]
  # Payer email verification needs its own Auth0 API and application (see
  # docs/authentication.md). Until both identifiers exist the settings are
  # omitted as a group, and gatewayd answers verification_unavailable.
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

resource "aws_security_group" "db" {
  name_prefix = "${var.name}-db-"
  vpc_id      = aws_vpc.this.id
  ingress {
    from_port       = 5432
    to_port         = 5432
    protocol        = "tcp"
    security_groups = [aws_security_group.api.id, aws_security_group.indexer.id]
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
resource "aws_secretsmanager_secret" "rpc_url" { name = "${var.name}/rpc-url" }
resource "aws_secretsmanager_secret_version" "rpc_url" {
  secret_id     = aws_secretsmanager_secret.rpc_url.id
  secret_string = var.rpc_url
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

# Derives the merchant-scoped payer references; rotating it unlinks every
# stored identity credential from the mailboxes that earned them.
resource "random_id" "payer_ref_master_key" { byte_length = 32 }
resource "aws_secretsmanager_secret" "payer_ref_master_key" {
  name = "${var.name}/payer-ref-master-key"
}
resource "aws_secretsmanager_secret_version" "payer_ref_master_key" {
  secret_id     = aws_secretsmanager_secret.payer_ref_master_key.id
  secret_string = random_id.payer_ref_master_key.b64_std
}

resource "random_password" "admin_bearer" {
  length  = 48
  special = false
}
resource "aws_secretsmanager_secret" "admin_bearer" { name = "${var.name}/admin-bearer" }
resource "aws_secretsmanager_secret_version" "admin_bearer" {
  secret_id     = aws_secretsmanager_secret.admin_bearer.id
  secret_string = random_password.admin_bearer.result
}

resource "aws_kms_key" "signer" {
  description              = "${var.name} Ethereum transaction signer"
  key_usage                = "SIGN_VERIFY"
  customer_master_key_spec = "ECC_SECG_P256K1"
  deletion_window_in_days  = 30
  enable_key_rotation      = false
  lifecycle {
    prevent_destroy = true
  }
}
resource "aws_kms_alias" "signer" {
  name          = "alias/${var.name}-signer"
  target_key_id = aws_kms_key.signer.key_id
}

# The recovery wallet takes custody of overpayment remainders, expired
# balances, and late transfers. Nothing in the stack signs with it: recovered
# funds are reviewed and returned by hand, so no task role is granted kms:Sign.
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

resource "aws_cloudwatch_log_group" "api" {
  name              = "/ecs/${var.name}/api"
  retention_in_days = 30
}
resource "aws_cloudwatch_log_group" "status" {
  name              = "/ecs/${var.name}/status"
  retention_in_days = 30
}
resource "aws_cloudwatch_log_group" "indexer" {
  name              = "/ecs/${var.name}/indexer"
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
resource "aws_iam_role" "status_execution" {
  name               = "${var.name}-status-execution"
  assume_role_policy = aws_iam_role.api_execution.assume_role_policy
}
resource "aws_iam_role" "indexer_execution" {
  name               = "${var.name}-indexer-execution"
  assume_role_policy = aws_iam_role.api_execution.assume_role_policy
}
resource "aws_iam_role_policy_attachment" "api_execution" {
  role       = aws_iam_role.api_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}
resource "aws_iam_role_policy_attachment" "status_execution" {
  role       = aws_iam_role.status_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}
resource "aws_iam_role_policy_attachment" "indexer_execution" {
  role       = aws_iam_role.indexer_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

resource "aws_iam_role_policy" "api_secrets" {
  role   = aws_iam_role.api_execution.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Action = ["secretsmanager:GetSecretValue"], Resource = [aws_secretsmanager_secret.database_url.arn, aws_secretsmanager_secret.rpc_url.arn, aws_secretsmanager_secret.webhook_encryption_key.arn, aws_secretsmanager_secret.admin_bearer.arn, aws_secretsmanager_secret.payer_ref_master_key.arn] }] })
}
resource "aws_iam_role_policy" "status_secrets" {
  role   = aws_iam_role.status_execution.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Action = ["secretsmanager:GetSecretValue"], Resource = aws_secretsmanager_secret.database_url.arn }] })
}
resource "aws_iam_role_policy" "indexer_secrets" {
  role   = aws_iam_role.indexer_execution.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Action = ["secretsmanager:GetSecretValue"], Resource = [aws_secretsmanager_secret.database_url.arn, aws_secretsmanager_secret.rpc_url.arn] }] })
}

resource "aws_iam_role" "api_task" {
  name               = "${var.name}-api-task"
  assume_role_policy = aws_iam_role.api_execution.assume_role_policy
}
resource "aws_sesv2_email_identity" "notifications" {
  email_identity = var.notification_domain_name
}
resource "aws_route53_record" "ses_dkim" {
  count   = 3
  zone_id = var.route53_zone_id
  name    = "${aws_sesv2_email_identity.notifications.dkim_signing_attributes[0].tokens[count.index]}._domainkey.${var.notification_domain_name}"
  type    = "CNAME"
  ttl     = 300
  records = ["${aws_sesv2_email_identity.notifications.dkim_signing_attributes[0].tokens[count.index]}.dkim.amazonses.com"]
}
resource "aws_iam_role_policy" "api_ses" {
  role   = aws_iam_role.api_task.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Action = ["ses:SendEmail"], Resource = aws_sesv2_email_identity.notifications.arn }] })
}
resource "aws_iam_role_policy" "api_attestation_kms" {
  role   = aws_iam_role.api_task.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Action = ["kms:GetPublicKey", "kms:Sign"], Resource = aws_kms_key.attestation.arn }] })
}
resource "aws_iam_role" "indexer_task" {
  name               = "${var.name}-indexer-task"
  assume_role_policy = aws_iam_role.api_execution.assume_role_policy
}
resource "aws_iam_role_policy" "indexer_kms" {
  role   = aws_iam_role.indexer_task.id
  policy = jsonencode({ Version = "2012-10-17", Statement = [{ Effect = "Allow", Action = ["kms:GetPublicKey", "kms:Sign"], Resource = aws_kms_key.signer.arn }] })
}

# ---------------------------------------------------------------------------
# Invoice attachments. One PDF per invoice, uploaded straight to S3 through a
# presigned PUT signed by the API task role, scanned by GuardDuty Malware
# Protection, and finalized by gatewayd only once the managed tag
# GuardDutyMalwareScanStatus=NO_THREATS_FOUND is present.
#
# Object layout: every upload lands at uploads/<account_id>/<attachment_id>.pdf
# and is never moved. The presigned PUT stamps the tag payday-upload=pending on
# it (a signed header, so no upload can omit it); issuing an invoice rewrites
# that tag to payday-upload=attached before the invoice commits. The lifecycle
# rule expires only objects still tagged pending, so infrastructure can never
# expire an attached PDF, and an abandoned upload is gone after seven days
# without gatewayd having to track it.
# ---------------------------------------------------------------------------

resource "aws_kms_key" "attachments" {
  description             = "${var.name} invoice attachment encryption"
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

# What gatewayd may do with uploads, and nothing else: sign PUTs that carry the
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
        Action   = ["s3:PutObject", "s3:PutObjectTagging", "s3:GetObject", "s3:GetObjectVersion", "s3:GetObjectTagging", "s3:GetObjectVersionTagging", "s3:DeleteObject"]
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
    portMappings           = [{ containerPort = var.api_port, protocol = "tcp" }],
    environment = concat(local.common_environment, [
      { name = "PAYDAY_BIND_ADDR", value = "0.0.0.0:${var.api_port}" },
      { name = "PAYDAY_AUTH0_ISSUER", value = var.auth0_issuer },
      { name = "PAYDAY_AUTH0_AUDIENCE", value = var.auth0_audience },
      { name = "PAYDAY_AUTH0_CLIENT_ID", value = var.auth0_client_id },
      { name = "PAYDAY_API_KEY_PREFIX", value = var.api_key_prefix },
      { name = "PAYDAY_PUBLIC_BASE_URL", value = local.checkout_base_url },
      { name = "PAYDAY_EXPLORER_BASE_URL", value = var.explorer_base_url },
      { name = "PAYDAY_STATUS_INDEXER_STALE_SECONDS", value = tostring(var.status_indexer_stale_seconds) },
      { name = "PAYDAY_NOTIFICATION_FROM_ADDRESS", value = var.notification_from_address },
      { name = "PAYDAY_RECOVERY_ADDRESS", value = var.recovery_address },
      { name = "PAYDAY_ATTACHMENT_BUCKET", value = aws_s3_bucket.attachments.id },
      { name = "PAYDAY_ATTESTATION_KMS_KEY_ID", value = aws_kms_key.attestation.arn }
    ], local.dashboard_environment, local.payer_environment),
    # The API verifies the deployed contract generation at startup, so it reads
    # the chain through the same RPC secret as the indexer.
    secrets = concat([
      { name = "DATABASE_URL", valueFrom = aws_secretsmanager_secret.database_url.arn },
      { name = "PAYDAY_RPC_URL", valueFrom = aws_secretsmanager_secret.rpc_url.arn },
      { name = "PAYDAY_WEBHOOK_ENCRYPTION_KEY", valueFrom = aws_secretsmanager_secret.webhook_encryption_key.arn },
      { name = "PAYDAY_ADMIN_BEARER_SECRET", valueFrom = aws_secretsmanager_secret.admin_bearer.arn }
    ], local.payer_secrets),
    logConfiguration = { logDriver = "awslogs", options = { "awslogs-group" = aws_cloudwatch_log_group.api.name, "awslogs-region" = var.aws_region, "awslogs-stream-prefix" = "api" } }
  }])

  # The recovery wallet is committed into every payment address, so the API
  # must never start with a placeholder; fail the plan rather than the task.
  lifecycle {
    precondition {
      condition     = var.recovery_address != null
      error_message = "recovery_address must be set to the recovery KMS key's address (derive it with cast wallet address --aws from recovery_kms_key_arn) before the API task can be deployed"
    }
  }
}

resource "aws_ecs_task_definition" "status" {
  family                   = "${var.name}-status"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = 256
  memory                   = 512
  execution_role_arn       = aws_iam_role.status_execution.arn
  runtime_platform {
    operating_system_family = "LINUX"
    cpu_architecture        = "X86_64"
  }
  container_definitions = jsonencode([{
    name                   = "status", image = "${aws_ecr_repository.api.repository_url}:${var.image_tag}", essential = true,
    readonlyRootFilesystem = true,
    portMappings           = [{ containerPort = var.api_port, protocol = "tcp" }],
    environment = concat(local.common_environment, [
      { name = "PAYDAY_BIND_ADDR", value = "0.0.0.0:${var.api_port}" },
      { name = "PAYDAY_STATUS_ONLY", value = "true" },
      { name = "PAYDAY_STATUS_INDEXER_STALE_SECONDS", value = tostring(var.status_indexer_stale_seconds) }
    ]),
    secrets          = [{ name = "DATABASE_URL", valueFrom = aws_secretsmanager_secret.database_url.arn }],
    logConfiguration = { logDriver = "awslogs", options = { "awslogs-group" = aws_cloudwatch_log_group.status.name, "awslogs-region" = var.aws_region, "awslogs-stream-prefix" = "status" } }
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
      { name = "PAYDAY_KMS_KEY_ID", value = aws_kms_key.signer.arn },
      { name = "PAYDAY_USDC_START_BLOCK", value = tostring(var.usdc_start_block) },
      { name = "PAYDAY_FINALITY_SOURCE", value = "finalized" },
      { name = "PAYDAY_FINALITY_CONFIRMATIONS", value = tostring(var.finality_confirmations) },
      { name = "PAYDAY_LOG_RANGE_SIZE", value = tostring(var.log_range_size) },
      { name = "PAYDAY_INDEXER_POLL_INTERVAL_MS", value = tostring(var.indexer_poll_interval_ms) }
    ]),
    secrets          = [{ name = "DATABASE_URL", valueFrom = aws_secretsmanager_secret.database_url.arn }, { name = "PAYDAY_RPC_URL", valueFrom = aws_secretsmanager_secret.rpc_url.arn }],
    logConfiguration = { logDriver = "awslogs", options = { "awslogs-group" = aws_cloudwatch_log_group.indexer.name, "awslogs-region" = var.aws_region, "awslogs-stream-prefix" = "indexer" } }
  }])
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
resource "aws_lb_target_group" "status" {
  name        = "${var.name}-status"
  port        = var.api_port
  protocol    = "HTTP"
  target_type = "ip"
  vpc_id      = aws_vpc.this.id
  health_check {
    path    = "/live"
    matcher = "200-399"
  }
}

resource "aws_acm_certificate" "api" {
  domain_name               = var.domain_name
  subject_alternative_names = [var.payment_domain_name, var.status_domain_name]
  validation_method         = "DNS"
  lifecycle { create_before_destroy = true }
}
resource "aws_route53_record" "validation" {
  for_each = { for dvo in aws_acm_certificate.api.domain_validation_options : dvo.domain_name => { name = dvo.resource_record_name, record = dvo.resource_record_value, type = dvo.resource_record_type } }
  zone_id  = local.certificate_validation_zone_ids[each.key]
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
resource "aws_lb_listener_rule" "status" {
  listener_arn = aws_lb_listener.https.arn
  priority     = 10
  action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.status.arn
  }
  condition {
    host_header { values = [var.status_domain_name] }
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
resource "aws_route53_record" "payment" {
  zone_id = var.payment_route53_zone_id
  name    = var.payment_domain_name
  type    = "A"
  alias {
    name                   = aws_lb.api.dns_name
    zone_id                = aws_lb.api.zone_id
    evaluate_target_health = true
  }
}

resource "aws_route53_record" "status" {
  zone_id = var.route53_zone_id
  name    = var.status_domain_name
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
  depends_on = [aws_lb_listener.https]
}
resource "aws_ecs_service" "status" {
  name            = "status"
  cluster         = aws_ecs_cluster.this.id
  task_definition = aws_ecs_task_definition.status.arn
  desired_count   = 1
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
    target_group_arn = aws_lb_target_group.status.arn
    container_name   = "status"
    container_port   = var.api_port
  }
  depends_on = [aws_lb_listener_rule.status]
}
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
resource "aws_cloudwatch_metric_alarm" "status_unhealthy" {
  alarm_name          = "${var.name}-status-unhealthy-targets"
  namespace           = "AWS/ApplicationELB"
  metric_name         = "UnHealthyHostCount"
  statistic           = "Maximum"
  period              = 60
  evaluation_periods  = 2
  threshold           = 0
  comparison_operator = "GreaterThanThreshold"
  treat_missing_data  = "breaching"
  dimensions          = { LoadBalancer = aws_lb.api.arn_suffix, TargetGroup = aws_lb_target_group.status.arn_suffix }
  alarm_actions       = [aws_sns_topic.alarms.arn]
}
resource "aws_cloudwatch_metric_alarm" "status_tasks" {
  alarm_name          = "${var.name}-status-task-count"
  namespace           = "ECS/ContainerInsights"
  metric_name         = "RunningTaskCount"
  statistic           = "Minimum"
  period              = 60
  evaluation_periods  = 2
  threshold           = 1
  comparison_operator = "LessThanThreshold"
  treat_missing_data  = "breaching"
  dimensions          = { ClusterName = aws_ecs_cluster.this.name, ServiceName = aws_ecs_service.status.name }
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
resource "aws_cloudwatch_log_metric_filter" "indexer_fatal" {
  name           = "${var.name}-indexer-fatal"
  log_group_name = aws_cloudwatch_log_group.indexer.name
  pattern        = "\"indexer fatal\""
  metric_transformation {
    name      = "IndexerFatal"
    namespace = var.name
    value     = "1"
  }
}
resource "aws_cloudwatch_metric_alarm" "indexer_fatal" {
  alarm_name          = "${var.name}-indexer-fatal"
  namespace           = var.name
  metric_name         = "IndexerFatal"
  statistic           = "Sum"
  period              = 60
  evaluation_periods  = 1
  threshold           = 0
  comparison_operator = "GreaterThanThreshold"
  treat_missing_data  = "notBreaching"
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
  alarm_description   = "Merchant email or webhook delivery failed repeatedly; inspect gatewayd logs and notification_outbox"
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
  alarm_description   = "A blocked payment cannot notify its merchant by email; recover the account contact and contact the merchant manually"
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
# Worker conditions that do not end the process. Each is logged on every
# pass while it holds, so the alarm stays raised until the condition clears.
locals {
  indexer_log_alarms = {
    sweep_paused = {
      pattern     = "\"sweep worker paused\""
      period      = 60
      description = "The sweep worker cannot resolve its in-flight batch; see docs/runbooks/stuck-invoice.md"
    }
    signer_low_balance = {
      pattern     = "\"sweep signer balance low\""
      period      = 300
      description = "The KMS sweep signer is below PAYDAY_SIGNER_LOW_BALANCE_WEI; fund it"
    }
    cursor_lagging = {
      pattern     = "\"indexer cursor lagging\""
      period      = 60
      description = "The block indexer trails finality by more than a thousand blocks; follow docs/runbooks/public-status-incident.md"
    }
    sweep_backlog_stale = {
      pattern     = "\"sweep backlog stale\""
      period      = 300
      description = "Collectable funds have waited more than fifteen minutes"
    }
    retryable_failures = {
      pattern     = "?\"sweep pass failed\" ?\"indexer poll failed\""
      period      = 300
      description = "Sustained retryable RPC or database failures in the worker"
    }
  }
}

resource "aws_cloudwatch_log_metric_filter" "indexer" {
  for_each       = local.indexer_log_alarms
  name           = "${var.name}-indexer-${replace(each.key, "_", "-")}"
  log_group_name = aws_cloudwatch_log_group.indexer.name
  pattern        = each.value.pattern
  metric_transformation {
    name      = "Indexer${title(replace(each.key, "_", ""))}"
    namespace = var.name
    value     = "1"
  }
}

resource "aws_cloudwatch_metric_alarm" "indexer" {
  for_each            = local.indexer_log_alarms
  alarm_name          = "${var.name}-indexer-${replace(each.key, "_", "-")}"
  alarm_description   = each.value.description
  namespace           = var.name
  metric_name         = aws_cloudwatch_log_metric_filter.indexer[each.key].metric_transformation[0].name
  statistic           = "Sum"
  period              = each.value.period
  evaluation_periods  = 1
  threshold           = each.key == "retryable_failures" ? 10 : 0
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
