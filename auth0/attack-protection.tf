# Pin Auth0's documented defaults so a dashboard edit cannot silently alter the
# tenant's launch security posture. Empty allowlists are intentional.
resource "auth0_attack_protection" "payday" {
  brute_force_protection {
    allowlist    = []
    enabled      = true
    max_attempts = 10
    mode         = "count_per_identifier_and_ip"
    shields      = ["block", "user_notification"]
  }

  suspicious_ip_throttling {
    allowlist = []
    enabled   = true
    shields   = ["block", "admin_notification"]

    pre_login {
      max_attempts = 100
      rate         = 864000
    }

    pre_user_registration {
      max_attempts = 50
      rate         = 1728000
    }
  }

  # Payday has no password credentials: its only connection is passwordless
  # email OTP. Breached-password detection therefore has nothing to inspect.
  breached_password_detection {
    enabled = false
  }
}
