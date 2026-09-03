variable "name" {
  description = "Short name used to prefix resources."
  type        = string
  default     = "payday"
  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{1,20}$", var.name))
    error_message = "name must be 2-21 lowercase letters, digits, or hyphens, beginning with a letter."
  }
}

variable "aws_region" {
  type    = string
  default = "us-east-1"
}

variable "domain_name" {
  description = "Existing Route53 DNS name to use for the API (api.payday.sh in production)."
  type        = string
  validation {
    condition     = length(var.domain_name) <= 253 && can(regex("^([a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?\\.)+[a-zA-Z]{2,63}$", var.domain_name))
    error_message = "domain_name must be a valid fully qualified DNS name without a scheme or path."
  }
}

variable "payment_domain_name" {
  description = "Public hostname used for payer checkout links (pay.payday.sh in production)."
  type        = string
  validation {
    condition     = length(var.payment_domain_name) <= 253 && can(regex("^([a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?\\.)+[a-zA-Z]{2,63}$", var.payment_domain_name))
    error_message = "payment_domain_name must be a valid fully qualified DNS name without a scheme or path."
  }
}

variable "payment_route53_zone_id" {
  description = "ID of the public Route53 hosted zone containing payment_domain_name."
  type        = string
}

variable "checkout_base_url" {
  description = <<-EOT
    Origin serving the hosted checkout at /pay/{id}, which is where every
    payment_url points. In production this is the Vercel-hosted site at
    https://payday.sh. Leave empty to keep links on payment_domain_name, which
    this service answers with a 301 to this origin.
  EOT
  type        = string
  default     = ""
  validation {
    condition     = var.checkout_base_url == "" || can(regex("^https://[^/?#]+$", var.checkout_base_url))
    error_message = "checkout_base_url must be an HTTPS origin without a trailing slash, path, query, or fragment."
  }
}

variable "explorer_base_url" {
  description = "Explorer origin for the configured chain; Monad production uses MonadVision."
  type        = string
  default     = "https://monadvision.com"
  validation {
    condition     = can(regex("^https://[^/?#]+/?$", var.explorer_base_url))
    error_message = "explorer_base_url must be an HTTPS origin without a path, query, or fragment."
  }
}

variable "status_domain_name" {
  description = "Public status page DNS name."
  type        = string
  default     = "status.payday.sh"
}

variable "status_indexer_stale_seconds" {
  description = "Age at which an unchanged payment cursor makes public status degraded."
  type        = number
  default     = 120
  validation {
    condition     = var.status_indexer_stale_seconds >= 30 && floor(var.status_indexer_stale_seconds) == var.status_indexer_stale_seconds
    error_message = "status_indexer_stale_seconds must be an integer of at least 30."
  }
}

variable "auth0_issuer" {
  description = "Auth0 tenant issuer URL, including https://."
  type        = string
  validation {
    condition     = can(regex("^https://[^[:space:]]+/?$", var.auth0_issuer))
    error_message = "auth0_issuer must be an HTTPS URL."
  }
}

variable "auth0_audience" {
  description = "Identifier of the Auth0 API configured for api.payday.sh."
  type        = string
  validation {
    condition     = length(trimspace(var.auth0_audience)) > 0
    error_message = "auth0_audience must not be empty."
  }
}

variable "auth0_client_id" {
  description = "Public client ID of the Auth0 Native application used by payday."
  type        = string
  validation {
    condition     = length(trimspace(var.auth0_client_id)) > 0
    error_message = "auth0_client_id must not be empty."
  }
}

variable "dashboard_auth0_client_id" {
  description = <<-EOT
    Public client ID of the Auth0 Single Page Application the merchant
    dashboard signs in with; see docs/authentication.md. Leave empty until it
    exists: gatewayd then accepts only API keys and the CLI's Native
    application, and the variable is not passed to the task at all.
  EOT
  type        = string
  default     = ""
}

variable "payer_auth0_audience" {
  description = <<-EOT
    Identifier of the Auth0 API payers verify their mailbox against
    (https://api.payday.sh/payer); see docs/authentication.md. Leave empty
    with payer_auth0_client_id until both exist: the payer settings are then
    not passed to the task at all and verification answers unavailable.
  EOT
  type        = string
  default     = ""
}

variable "payer_auth0_client_id" {
  description = "Public client ID of the Auth0 Native application gatewayd exchanges payer codes with (auth0/payer.tf)."
  type        = string
  default     = ""
  validation {
    condition     = (var.payer_auth0_client_id == "") == (var.payer_auth0_audience == "")
    error_message = "payer_auth0_audience and payer_auth0_client_id must be set together."
  }
}

variable "didit_workflow_id" {
  description = <<-EOT
    The pinned Didit workflow for payer identity verification (document,
    liveness, face match; declines expected-name mismatches); see
    docs/authentication.md. Leave empty with the two Didit secrets until they
    exist: the identity settings are then not passed to the task at all and
    identity start answers unavailable.
  EOT
  type        = string
  default     = ""
}

variable "didit_api_key" {
  description = "Didit API key. Prefer TF_VAR_didit_api_key from a secure environment."
  type        = string
  default     = ""
  sensitive   = true
}

variable "didit_webhook_secret" {
  description = "Shared secret of the Didit webhook destination that points at /v1/webhooks/identity. Prefer TF_VAR_didit_webhook_secret."
  type        = string
  default     = ""
  sensitive   = true
  validation {
    condition     = (var.didit_workflow_id == "") == (var.didit_api_key == "") && (var.didit_workflow_id == "") == (var.didit_webhook_secret == "")
    error_message = "didit_workflow_id, didit_api_key, and didit_webhook_secret must be set together."
  }
}

variable "admin_reviewer_id" {
  description = "Recorded as the reviewer on manual verification decisions taken through the operator API."
  type        = string
  default     = "operator"
}

variable "route53_zone_id" {
  description = "ID of the public Route53 hosted zone containing the API domain_name."
  type        = string
}

variable "image_tag" {
  description = "Immutable image tag in git-<full lowercase commit SHA> form."
  type        = string
  validation {
    condition     = can(regex("^git-[0-9a-f]{40}$", var.image_tag))
    error_message = "image_tag must be git- followed by the full 40-character lowercase commit SHA."
  }
}

variable "chain_id" {
  type = number
  validation {
    condition     = var.chain_id > 0 && floor(var.chain_id) == var.chain_id
    error_message = "chain_id must be a positive integer."
  }
}

variable "factory_address" {
  type = string
  validation {
    condition     = can(regex("^0x[0-9a-fA-F]{40}$", var.factory_address))
    error_message = "factory_address must be a 20-byte 0x-prefixed EVM address."
  }
}

variable "batch_sweeper_address" {
  type = string
  validation {
    condition     = can(regex("^0x[0-9a-fA-F]{40}$", var.batch_sweeper_address))
    error_message = "batch_sweeper_address must be a 20-byte 0x-prefixed EVM address."
  }
}

variable "factory_code_hash" {
  description = <<-EOT
    keccak256 of the runtime bytecode deployed at factory_address, computed
    with: cast keccak "$(cast code <factory_address> --rpc-url <rpc_url>)".
    PaymentFactory and BatchSweeper are deployed together as one contract
    generation; both services compare the live code with this hash at startup
    and refuse to start on a mismatch.
  EOT
  type        = string
  validation {
    condition     = can(regex("^0x[0-9a-fA-F]{64}$", var.factory_code_hash))
    error_message = "factory_code_hash must be a 32-byte 0x-prefixed keccak256 hash."
  }
}

variable "batch_sweeper_code_hash" {
  description = <<-EOT
    keccak256 of the runtime bytecode deployed at batch_sweeper_address,
    computed with: cast keccak "$(cast code <batch_sweeper_address> --rpc-url <rpc_url>)".
    Deploy the sweeper with the factory it is bound to, as one generation;
    never point a new factory at an old sweeper.
  EOT
  type        = string
  validation {
    condition     = can(regex("^0x[0-9a-fA-F]{64}$", var.batch_sweeper_code_hash))
    error_message = "batch_sweeper_code_hash must be a 32-byte 0x-prefixed keccak256 hash."
  }
}

variable "recovery_address" {
  description = <<-EOT
    Payday's custodial recovery wallet: the Ethereum address of the recovery
    KMS key (cast wallet address --aws with recovery_kms_key_arn). gatewayd
    stamps it on every invoice; overpayment remainders, expired balances, and
    late transfers land here and are returned manually by the operator.
    Null until the key exists: there is no safe placeholder, because every
    invoice commits this address into its payment address, so the API task
    definition refuses to plan while it is unset.
  EOT
  type        = string
  default     = null
  nullable    = true
  validation {
    condition     = var.recovery_address == null || can(regex("^0x[0-9a-fA-F]{40}$", var.recovery_address))
    error_message = "recovery_address must be a 20-byte 0x-prefixed EVM address."
  }
}

variable "usdc_address" {
  type = string
  validation {
    condition     = can(regex("^0x[0-9a-fA-F]{40}$", var.usdc_address))
    error_message = "usdc_address must be a 20-byte 0x-prefixed EVM address."
  }
}

variable "usdc_start_block" {
  type = number
  validation {
    condition     = var.usdc_start_block >= 0 && floor(var.usdc_start_block) == var.usdc_start_block
    error_message = "usdc_start_block must be a non-negative integer."
  }
}

variable "finality_confirmations" {
  description = "Blocks subtracted from the node's finalized tag before a range is committed; a small margin against replica skew behind the provider's load balancer."
  type        = number
  default     = 2
  validation {
    condition     = var.finality_confirmations >= 0 && var.finality_confirmations <= 10000 && floor(var.finality_confirmations) == var.finality_confirmations
    error_message = "finality_confirmations must be an integer from 0 through 10000."
  }
}

variable "log_range_size" {
  type    = number
  default = 100
  validation {
    condition     = var.log_range_size >= 1 && var.log_range_size <= 10000 && floor(var.log_range_size) == var.log_range_size
    error_message = "log_range_size must be an integer from 1 through 10000."
  }
}

variable "indexer_poll_interval_ms" {
  description = "Idle poll interval in milliseconds for both worker loops. Each tick drains every finalized range up to PAYDAY_INDEXER_MAX_RANGES_PER_TICK. Increasing this reduces idle polling but not catch-up traffic: indexing uses about 90 eth_getLogs calls/hour at QuickNode's 100-block cap, plus finality/cursor reads and one header lookup per distinct transfer-bearing block."
  type        = number
  default     = 5000
  validation {
    condition     = var.indexer_poll_interval_ms >= 1000 && var.indexer_poll_interval_ms <= 300000 && floor(var.indexer_poll_interval_ms) == var.indexer_poll_interval_ms
    error_message = "indexer_poll_interval_ms must be an integer from 1000 through 300000."
  }
}

variable "rpc_url" {
  description = "Paid HTTPS JSON-RPC endpoint. WARNING: sensitive values remain in Terraform state."
  type        = string
  sensitive   = true
  validation {
    condition     = can(regex("^https://[^[:space:]]+$", var.rpc_url))
    error_message = "rpc_url must be an HTTPS URL."
  }
}

variable "api_desired_count" {
  type    = number
  default = 1
  validation {
    condition     = var.api_desired_count >= 1 && floor(var.api_desired_count) == var.api_desired_count
    error_message = "api_desired_count must be a positive integer."
  }
}

variable "api_cpu" {
  type    = number
  default = 256
}
variable "api_memory" {
  type    = number
  default = 512
}
variable "indexer_cpu" {
  type    = number
  default = 256
}
variable "indexer_memory" {
  type    = number
  default = 512
}
variable "api_port" {
  type    = number
  default = 8080
}

variable "api_key_prefix" {
  description = "Prefix issued on account API keys for this deployment."
  type        = string
  default     = "payday_live_"
  validation {
    condition     = contains(["payday_live_", "payday_test_"], var.api_key_prefix)
    error_message = "api_key_prefix must be exactly payday_live_ or payday_test_."
  }
}

variable "db_instance_class" {
  type    = string
  default = "db.t4g.small"
}
variable "db_allocated_storage" {
  type    = number
  default = 20
}
variable "db_max_allocated_storage" {
  type    = number
  default = 100
}
variable "db_multi_az" {
  type    = bool
  default = true
}
variable "db_deletion_protection" {
  type    = bool
  default = true
}
variable "db_skip_final_snapshot" {
  type    = bool
  default = false
}

variable "waf_rate_limit" {
  description = "Maximum requests per five-minute period per source IP."
  type        = number
  default     = 2000
  validation {
    condition     = var.waf_rate_limit >= 100
    error_message = "waf_rate_limit must be at least 100."
  }
}

variable "alarm_email" {
  description = "Email address that receives CloudWatch alarm notifications; AWS sends a confirmation link after apply."
  type        = string
  validation {
    condition     = can(regex("^[^@[:space:]]+@[^@[:space:]]+\\.[^@[:space:]]+$", var.alarm_email))
    error_message = "alarm_email must be a valid email address."
  }
}

variable "notification_from_address" {
  description = "Sender address used for merchant payout notifications. Its domain must equal notification_domain_name."
  type        = string
  default     = "alerts@payday.sh"
  validation {
    condition     = can(regex("^[^@[:space:]]+@[^@[:space:]]+\\.[^@[:space:]]+$", var.notification_from_address))
    error_message = "notification_from_address must be a valid email address."
  }
}

variable "notification_domain_name" {
  description = "SES Easy DKIM domain for merchant notification email."
  type        = string
  default     = "payday.sh"
  validation {
    condition     = endswith(var.notification_from_address, "@${var.notification_domain_name}")
    error_message = "notification_from_address must belong to notification_domain_name."
  }
}
