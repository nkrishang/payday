# Auth0 configuration

Terraform owns Payday's passwordless email connection, branded OTP template,
and tenant attack protection. It deliberately does not own the Resend
credential: keep that key only in Auth0's email-provider settings as described
in [`docs/authentication.md`](../docs/authentication.md).

## Bootstrap or update

Create a dedicated Auth0 Machine-to-Machine deployment client authorized only
for the Auth0 Management API scopes `read:connections`, `update:connections`,
`read:attack_protection`, and `update:attack_protection`. Do not authorize it
for the Payday API. Expose its credentials through the provider's
`AUTH0_DOMAIN`, `AUTH0_CLIENT_ID`, and `AUTH0_CLIENT_SECRET` environment
variables; never put them in a checked-in file or Terraform command line.

Create `backend.hcl` (ignored by Git) for an encrypted S3 state key separate
from the AWS stack, then initialize:

```hcl
bucket       = "company-payday-terraform-state"
key          = "production/payday-auth0.tfstate"
region       = "us-east-1"
encrypt      = true
use_lockfile = true
```

```bash
terraform -chdir=auth0 init -backend-config=backend.hcl
```

Auth0 creates both the production tenant's `email` connection and its
attack-protection singleton before Terraform manages them. Import both before
the first plan. Replace the connection ID; the attack-protection provider
accepts any UUID because its Management API object has no ID.

```bash
terraform -chdir=auth0 import auth0_connection.passwordless_email 'con_REPLACE_ME'
terraform -chdir=auth0 import auth0_attack_protection.payday \
  '24940d4b-4bd4-44e7-894e-f92e4de36a40'
terraform -chdir=auth0 plan -out=auth0.tfplan
terraform -chdir=auth0 apply auth0.tfplan
```

Review every connection and attack-protection option in the first plan. Auth0
replaces connection options as a unit, so an omitted setting can reset a
dashboard-only value. The connection has destroy protection; removing it
requires an explicit reviewed code change.

## Attack-protection policy

Terraform pins Auth0's documented defaults so dashboard edits cannot silently
weaken the launch posture:

* **Brute-force protection:** 10 failed attempts per identifier and IP, IP
  blocking, affected-user email notification, and no allowlisted IPs.
* **Suspicious-IP throttling:** blocking and administrator notification, with
  no allowlisted IPs. Each IP receives 100 failed login attempts per day
  (replenished every 864,000 ms) and 50 signup attempts per day (replenished
  every 1,728,000 ms).
* **Breached-password detection:** disabled because Payday's only connection is
  email OTP and has no password to inspect. Independently, Auth0 accepts only
  three failed submissions for one OTP before requiring a new code.

The provider has no recipient-address field for `admin_notification`; Auth0
sends suspicious-IP notices to tenant administrators. Before applying, verify
in **Dashboard → Settings → Tenant Members** that at least two current Payday
operators are administrators who receive Auth0 mail. Remove departed members
immediately and audit recipients quarterly.

## Launch and recovery checks

1. Apply a reviewed plan. In **Security → Attack Protection**, confirm
   Brute-force is Enabled with threshold 10, per-identifier-and-IP blocking and
   user notification. Confirm Suspicious IP Throttling is Enabled with 100/50
   daily login/signup limits, blocking and administrator notification. Neither
   feature may show Monitoring-only.
2. Confirm both allowlists are empty. Add only a documented fixed trusted
   egress range; do not automatically trust shared-user NATs.
3. In staging, verify that the fourth bad submission for one OTP requires a new
   code. Exercise high-volume behavior only from an approved test source, then
   inspect **Monitoring → Logs** for attack-protection/429 events and confirm an
   administrator receives the notification.
4. For a brute-force block, the affected user can use Auth0's notification
   unblock link. Otherwise the block lasts up to 30 days after the last failure.
   An operator may call `DELETE /user-blocks` or `DELETE /user-blocks/{id}` with
   a separate break-glass Management API credential granted `update:users`; do
   not grant that scope to the Terraform deployment client.
5. Suspicious-IP throttling is a separate token bucket. Confirm legitimate
   traffic recovers as attempts replenish; never bypass an active incident by
   allowlisting its source.

Bot Detection has a passwordless challenge policy, but availability and
behavior depend on the Auth0 subscription and embedded flow, and the CLI does
not currently implement a CAPTCHA exchange. Do not enable it without selecting
a fail-closed CAPTCHA, implementing the challenge flow, and testing it in
staging.

Auth0's passwordless template API accepts HTML only and has no field for a
separate plain-text MIME part. Keep the HTML semantic and readable without CSS;
a true multipart alternative would require replacing Auth0's managed delivery
with a custom email-provider Action.

## Verification

```bash
node --test auth0/actions/*.test.js auth0/email/*.test.js
terraform -chdir=auth0 init -backend=false
terraform -chdir=auth0 fmt -check -recursive
terraform -chdir=auth0 validate
```

Authoritative references, checked 2026-08-28:

* [Provider 1.56 `auth0_attack_protection` schema](https://registry.terraform.io/providers/auth0/auth0/1.56.0/docs/resources/attack_protection)
* [Auth0 brute-force protection](https://auth0.com/docs/secure/attack-protection/brute-force-protection)
* [Auth0 suspicious-IP throttling](https://auth0.com/docs/secure/attack-protection/suspicious-ip-throttling)
* [Auth0 suspicious-IP throttling API defaults](https://auth0.com/docs/api/management/v2/attack-protection/get-suspicious-ip-throttling)
* [Auth0 passwordless email OTP behavior](https://auth0.com/docs/authenticate/passwordless/authentication-methods/email-otp)
