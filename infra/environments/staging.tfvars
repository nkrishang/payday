# Staging: a second, isolated instance of the production stack that tracks
# main (docs/staging.md). Same chain, same USDC, its own contracts, database,
# keys, bucket, and hostname. The checkout origin is the web app on the
# operator's machine, so every deposit_url staging mints opens locally.
#
# Values here are public identifiers only. The RPC URL comes from the
# STAGING_RPC_URL repository secret (TF_VAR_rpc_url); the image tag is set by
# the deploy workflow from the commit it deploys.

name           = "payday-staging"
api_key_prefix = "payday_test_"
aws_region     = "eu-north-1"
domain_name    = "api.staging.payday.sh"
# The Route53 hosted zone for api.staging.payday.sh, delegated from Vercel
# DNS with four NS records named api.staging (docs/staging.md, bootstrap).
route53_zone_id   = "Z_REPLACE_WITH_STAGING_ZONE_ID"
checkout_base_url = "http://127.0.0.1:3002"
explorer_base_url = "https://monadvision.com"
alarm_email       = "krishang@payday.sh"
admin_reviewer_id = "krishang@payday.sh"

# Merchant sign-in uses the development Privy app, which already allows the
# local dashboard origin; a staging account is still its own row in the
# staging database.
privy_app_id = "cmt9wxn7h011h0cjsma7fzytr"
# Payer and issuer-mailbox codes go through the production Auth0 tenant.
auth0_issuer = "https://dev-5ojfw164vnkjnk6m.us.auth0.com/"
# Set both once the Payday Payer API and the Payday Payer Verification
# application exist in that tenant (docs/authentication.md §3); until then
# gated deposit requests answer verification_unavailable on staging.
# payer_auth0_audience  = "https://api.payday.sh/payer"
# payer_auth0_client_id = "REPLACE_WITH_PAYER_VERIFICATION_CLIENT_ID"

# Monad mainnet and Circle native USDC: staging settles real cents so RPC,
# finality, and sweeping behave exactly as in production.
chain_id     = 143
usdc_address = "0x754704Bc059F8C67012fEd69BC8A327a5aafb603"
# Staging's own PaymentFactory and BatchSweeper generation, deployed with
# foundry/script/PaymentFactory.s.sol (docs/staging.md, bootstrap). Zero
# hashes pass validation but both services refuse to start on them.
factory_address         = "0x0000000000000000000000000000000000000001"
batch_sweeper_address   = "0x0000000000000000000000000000000000000002"
factory_code_hash       = "0x0000000000000000000000000000000000000000000000000000000000000000"
batch_sweeper_code_hash = "0x0000000000000000000000000000000000000000000000000000000000000000"
# The Monad block immediately before the first staging deployment.
usdc_start_block       = 0
finality_confirmations = 2

# Merchant email from staging's own subdomain; its DKIM records are the
# notification_dkim_records output, added in Vercel DNS.
notification_domain_name  = "staging.payday.sh"
notification_from_address = "alerts@staging.payday.sh"

# Overwritten by the deploy workflow with git-<sha> of the commit it ships.
image_tag = "git-0000000000000000000000000000000000000000"

# Cheap and disposable: one AZ, no deletion protection, no final snapshot,
# so a schema reset is `terraform apply -replace=aws_db_instance.this`.
db_multi_az            = false
db_deletion_protection = false
db_skip_final_snapshot = true

# main deploys here automatically after CI passes.
github_repository = "nkrishang/payday"
