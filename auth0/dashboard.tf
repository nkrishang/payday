# The merchant dashboard is a browser application, so it holds no secret and
# authenticates with a passwordless email OTP exchanged directly against
# Auth0's passwordless endpoints (no redirect flow). It is the only merchant
# application: its access tokens are the API's session credential and, while
# fresh, the credential that issues an API key. The Post-Login Action admits
# them only when this client's ID is configured as the PAYDAY_CLIENT_ID secret.
resource "auth0_client" "dashboard" {
  name            = "Payday Dashboard"
  app_type        = "spa"
  oidc_conformant = true

  # Only the embedded passwordless OTP exchange; no redirect, password, or
  # client-credentials grants.
  grant_types = ["http://auth0.com/oauth/grant-type/passwordless/otp"]

  # Production and the local Next.js dev server (see web/.env.example).
  callbacks           = ["https://payday.sh/dashboard", "http://127.0.0.1:3002/dashboard"]
  allowed_logout_urls = ["https://payday.sh", "http://127.0.0.1:3002"]
  web_origins         = ["https://payday.sh", "http://127.0.0.1:3002"]
  # Origins allowed to call /passwordless/start and /oauth/token from a browser.
  allowed_origins = ["https://payday.sh", "http://127.0.0.1:3002"]

  jwt_configuration {
    alg = "RS256"
  }

  lifecycle {
    prevent_destroy = true
  }
}

# Enable the passwordless email connection for the dashboard client. Access to
# the Payday API audience is not a Terraform object for a browser client: the
# client requests the audience when exchanging the OTP, and the Post-Login
# Action decides whether to issue the token.
resource "auth0_connection_client" "dashboard_passwordless_email" {
  connection_id = auth0_connection.passwordless_email.id
  client_id     = auth0_client.dashboard.id
}
