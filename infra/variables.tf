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

variable "checkout_base_url" {
  description = <<-EOT
    Origin serving the hosted checkout at /pay/{id}, which is where every
    deposit_url points: the Vercel-hosted site, https://payday.sh in
    production. Staging points at the web app running on the operator's
    machine, so a loopback HTTP origin is also accepted (docs/staging.md).
  EOT
  type        = string
  validation {
    condition     = can(regex("^https://[^/?#]+$", var.checkout_base_url)) || can(regex("^http://(127\\.0\\.0\\.1|localhost)(:[0-9]{1,5})?$", var.checkout_base_url))
    error_message = "checkout_base_url must be an HTTPS origin (or a loopback HTTP origin) without a trailing slash, path, query, or fragment."
  }
}


variable "privy_app_id" {
  description = <<-EOT
    Public id of the Privy app merchants sign in to (see
    docs/authentication.md). gatewayd verifies dashboard sessions — Privy
    identity tokens — against this app's published keys; the web app is built
    with the same id as NEXT_PUBLIC_PRIVY_APP_ID. Public, like every value in
    this file; there is no Privy secret anywhere in Payday.
  EOT
  type        = string
  validation {
    condition     = can(regex("^[A-Za-z0-9_-]{1,64}$", var.privy_app_id))
    error_message = "privy_app_id must be a Privy app id (letters, digits, - and _)."
  }
}

variable "auth0_issuer" {
  description = "Auth0 tenant issuer URL, including https://; the tenant payers and issuer mailboxes prove themselves against."
  type        = string
  validation {
    condition     = can(regex("^https://[^[:space:]]+/?$", var.auth0_issuer))
    error_message = "auth0_issuer must be an HTTPS URL."
  }
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

variable "admin_reviewer_id" {
  description = "Recorded as the operator on decisions taken through the operator API."
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










variable "indexer_poll_interval_ms" {
  description = "Sweep worker cadence, and the block indexer's reconcile cadence only while its WebSocket transfer signal is disconnected. While the signal is connected the indexer reconciles every indexer_reconcile_interval_ms and immediately on a wake, so this value no longer sets the request budget."
  type        = number
  default     = 5000
  validation {
    condition     = var.indexer_poll_interval_ms >= 1000 && var.indexer_poll_interval_ms <= 300000 && floor(var.indexer_poll_interval_ms) == var.indexer_poll_interval_ms
    error_message = "indexer_poll_interval_ms must be an integer from 1000 through 300000."
  }
}

variable "indexer_reconcile_interval_ms" {
  description = "Block indexer reconcile cadence, per chain, while the chain has something to watch and its transfer signal is connected. Payments are detected by the signal within a block of finality regardless; this timer keeps the cursor moving and catches anything the socket missed. Each pass costs one boundary read, one cursor check, and three calls per range (two range-end headers and one eth_getLogs), so at 60 s an active Monad chain runs about 14k calls/day. A chain with nothing to watch uses indexer_idle_interval_ms instead."
  type        = number
  default     = 60000
  validation {
    condition     = var.indexer_reconcile_interval_ms >= 1000 && var.indexer_reconcile_interval_ms <= 300000 && floor(var.indexer_reconcile_interval_ms) == var.indexer_reconcile_interval_ms
    error_message = "indexer_reconcile_interval_ms must be an integer from 1000 through 300000."
  }
}


variable "indexer_idle_interval_ms" {
  description = "Block indexer cadence, per chain, while the chain has nothing to watch (no open bound request, nothing uncollected, nothing recently settled). Such a pass costs two calls and fast-forwards the cursor without scanning, so three idle chains at five minutes cost under 2k calls/day between them."
  type        = number
  default     = 300000
  validation {
    condition     = var.indexer_idle_interval_ms >= 1000 && var.indexer_idle_interval_ms <= 3600000 && floor(var.indexer_idle_interval_ms) == var.indexer_idle_interval_ms
    error_message = "indexer_idle_interval_ms must be an integer from 1000 through 3600000."
  }
}

variable "chains" {
  description = <<-EOT
    Every network a payer may pay on, in the order the checkout offers them,
    each with the contract generation deployed there. Becomes PAYDAY_CHAINS on
    both services. The factory and sweeper are one generation deployed at the
    same addresses on every chain (foundry/script/PaymentFactory.s.sol insists
    on a fresh deployer key); the code hashes are keccak256 of the runtime
    bytecode, cast keccak "$(cast code <ADDRESS> --rpc-url <RPC_URL>)", and
    both services compare them with the chain at startup and refuse to start on
    a mismatch. usdc is Circle's native USDC proxy on that chain, never a
    bridged variant. finality_source is "finalized" (Monad: irreversible) or
    "latest" with finality_confirmations blocks of margin (Base, Arbitrum:
    seconds, trusting the sequencer). Every range scan is filtered to the
    addresses Payday watches, so log_range_size is only the provider's cap.
    Set usdc_start_block to the chain's block just before the services first
    run there.
  EOT
  type = list(object({
    chain_id                = number
    usdc                    = string
    factory                 = string
    batch_sweeper           = string
    factory_code_hash       = string
    batch_sweeper_code_hash = string
    usdc_start_block        = number
    finality_source         = string
    finality_confirmations  = number
    block_time_ms           = number
    log_range_size          = number
    explorer_base_url       = optional(string)
  }))
  validation {
    condition     = length(var.chains) > 0
    error_message = "chains must list at least one network."
  }
  validation {
    condition     = length(distinct([for c in var.chains : c.chain_id])) == length(var.chains)
    error_message = "chains must not list a chain id twice."
  }
  validation {
    condition = alltrue([for c in var.chains :
      c.chain_id > 0 && floor(c.chain_id) == c.chain_id
      && can(regex("^0x[0-9a-fA-F]{40}$", c.usdc))
      && can(regex("^0x[0-9a-fA-F]{40}$", c.factory))
      && can(regex("^0x[0-9a-fA-F]{40}$", c.batch_sweeper))
      && can(regex("^0x[0-9a-fA-F]{64}$", c.factory_code_hash))
      && can(regex("^0x[0-9a-fA-F]{64}$", c.batch_sweeper_code_hash))
      && c.factory_code_hash != "0x0000000000000000000000000000000000000000000000000000000000000000"
      && c.usdc_start_block >= 0
      && contains(["finalized", "latest"], c.finality_source)
      && c.finality_confirmations >= 0 && c.finality_confirmations <= 10000
      && c.block_time_ms >= 1
      && c.log_range_size >= 1 && c.log_range_size <= 10000
      && (c.explorer_base_url == null || can(regex("^https://[^/?#]+/?$", c.explorer_base_url)))
    ])
    error_message = "Every chain needs 20-byte addresses, non-zero 32-byte code hashes, finality_source finalized|latest, a positive block time, a log range from 1 through 10000, and an HTTPS explorer origin if any."
  }
}

variable "rpc_urls" {
  description = "Paid HTTPS JSON-RPC endpoint per chain, keyed by decimal chain id (one for every entry of chains). Supply as TF_VAR_rpc_urls. WARNING: sensitive values remain in Terraform state."
  type        = map(string)
  sensitive   = true
  validation {
    condition     = alltrue([for id, url in var.rpc_urls : can(regex("^[0-9]+$", id)) && can(regex("^https://[^[:space:]]+$", url))])
    error_message = "rpc_urls keys must be decimal chain ids and values HTTPS URLs."
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

variable "resend_api_key" {
  description = <<-EOT
    Resend API key the API emails payers their deposit requests with, from
    payer_email_from. Leave empty to queue those emails without sending
    them. Supply as TF_VAR_resend_api_key. WARNING: sensitive values remain
    in Terraform state.
  EOT
  type        = string
  default     = ""
  sensitive   = true
}

variable "privy_app_secret" {
  description = <<-EOT
    Privy app secret the API pregenerates merchant wallets with during email
    sign-up. Leave empty to skip pregeneration: gatewayd answers pregenerate
    with 503 and sign-in still creates the wallet itself. Supply as
    TF_VAR_privy_app_secret. Note that Terraform state will contain it.
  EOT
  type        = string
  default     = ""
  sensitive   = true
}

variable "payer_email_from" {
  description = "From header of the payer's deposit request email; Resend must have verified its domain."
  type        = string
  default     = "Payday <contact@payday.sh>"
  validation {
    condition     = can(regex("^([^<>]+<)?[^@<>[:space:]]+@[^@<>[:space:]]+\\.[^@<>[:space:]]+>?$", var.payer_email_from))
    error_message = "payer_email_from must be an email address, optionally as Name <address>."
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

variable "github_repository" {
  description = <<-EOT
    GitHub repository (owner/name) whose main branch deploys this environment
    through GitHub Actions. Set only for staging (docs/staging.md): it
    creates the account's GitHub OIDC provider and a deploy role that the
    workflow assumes, so at most one environment per AWS account may set it.
    Empty, as in production, creates nothing and every apply is manual.
  EOT
  type        = string
  default     = ""
  validation {
    condition     = var.github_repository == "" || can(regex("^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$", var.github_repository))
    error_message = "github_repository must be owner/name."
  }
}
