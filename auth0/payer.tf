# Payers verify the mailbox an invoice was issued to (product plan §6.1). They
# are anonymous link visitors, not tenant users worth an application each, so
# gatewayd drives the passwordless exchange for them against this one public
# application and a dedicated payer API audience. The merchant Post-Login
# Action never sees that audience, and the payer Action
# (`actions/payday-payer-email-otp.js`) admits only this client, so a payer
# token can never reach the merchant API and a merchant token never unlocks an
# invoice.
resource "auth0_client" "payer" {
  name            = "Payday Payer Verification"
  app_type        = "native"
  oidc_conformant = true

  # Only the embedded passwordless OTP exchange, performed server-side by
  # gatewayd; no redirect, password, or client-credentials grants.
  grant_types = ["http://auth0.com/oauth/grant-type/passwordless/otp"]

  jwt_configuration {
    alg = "RS256"
  }

  lifecycle {
    prevent_destroy = true
  }
}

resource "auth0_connection_client" "payer_passwordless_email" {
  connection_id = auth0_connection.passwordless_email.id
  client_id     = auth0_client.payer.id
}
