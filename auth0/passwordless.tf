resource "auth0_connection" "passwordless_email" {
  name     = "email"
  strategy = "email"

  options {
    from                   = "Payday <login@auth.payday.sh>"
    subject                = "Your Payday code expires in 3 minutes"
    syntax                 = "liquid"
    template               = file("${path.module}/email/passwordless-code.liquid")
    disable_signup         = false
    brute_force_protection = true

    totp {
      time_step = 180
      length    = 6
    }
  }

  lifecycle {
    prevent_destroy = true
  }
}
