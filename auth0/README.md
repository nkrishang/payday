# Auth0 configuration

Terraform owns Payday's passwordless email connection and branded OTP template.
It deliberately does not own the Resend credential: keep that key only in
Auth0's email-provider settings as described in
[`docs/authentication.md`](../docs/authentication.md).

## Bootstrap or update

Give a dedicated Auth0 deployment client only the connection read/update scopes
it needs, then expose its credentials through the provider's `AUTH0_DOMAIN`,
`AUTH0_CLIENT_ID`, and `AUTH0_CLIENT_SECRET` environment variables. Never put
them in a checked-in file or a Terraform command line.

Create `backend.hcl` (ignored by Git) for a separate encrypted S3 state key, for
example `production/payday-auth0.tfstate`, then initialize:

```bash
terraform -chdir=auth0 init -backend-config=backend.hcl
```

The production tenant already has an `email` connection. Import it before the
first plan so Terraform updates it in place instead of trying to create a
duplicate:

```bash
terraform -chdir=auth0 import auth0_connection.passwordless_email 'con_REPLACE_ME'
terraform -chdir=auth0 plan -out=auth0.tfplan
terraform -chdir=auth0 apply auth0.tfplan
```

Review every connection option in the first plan. Auth0 replaces the connection
options as a unit, so an omitted setting can reset a dashboard-only value. The
connection has destroy protection; removing it requires an explicit reviewed
code change.

Auth0's passwordless template API accepts HTML only and has no field for a
separate plain-text MIME part. Keep the HTML semantic and readable without CSS;
a true multipart alternative would require replacing Auth0's managed delivery
with a custom email-provider Action.

Run the local checks with:

```bash
node --test auth0/actions/*.test.js auth0/email/*.test.js
terraform -chdir=auth0 fmt -check -recursive
terraform -chdir=auth0 validate
```
