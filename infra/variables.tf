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

variable "route53_zone_id" {
  description = "ID of an existing public Route53 hosted zone containing domain_name."
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
  type    = number
  default = 2
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
