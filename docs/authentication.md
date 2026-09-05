# Email authentication and API keys

Payday uses Auth0's embedded passwordless email OTP flow. Auth0 creates the
identity and verifies the one-time code; Resend delivers the email. Payday never
receives a password, generates an OTP, or stores an email address.

There is one way in: the merchant dashboard at `payday.sh/dashboard`. Its
sign-in is the identity, and its API key section is where a merchant mints the
key their own server calls the API with. Each Auth0 identity owns exactly one
active Payday API key. Requesting another key atomically replaces the stored
hash, increments its generation, and makes the prior key enter a 24-hour grace
period. Plaintext keys are shown once; PostgreSQL stores only a SHA-256 digest
and a final-six-character hint.

## Credentials and where they belong

| Value | Where to obtain it | Where to place it |
|---|---|---|
| Auth0 issuer | Auth0 **Settings → Domain**, prefixed with `https://` and suffixed with `/` | API service, web `NEXT_PUBLIC_AUTH0_DOMAIN`, `infra/terraform.tfvars` |
| Auth0 API audience | The API **Identifier** created below; use `https://api.payday.sh` | API service, web `NEXT_PUBLIC_AUTH0_AUDIENCE`, `infra/terraform.tfvars` |
| Auth0 client ID | `Payday Dashboard` SPA application created by `auth0/dashboard.tf` | API service, web `NEXT_PUBLIC_AUTH0_CLIENT_ID`, Action secret, `infra/terraform.tfvars`; it is public |
| Resend API key | Resend **API Keys → Create API Key** | Auth0 **Branding → Email Provider** only |

The dashboard application has no Auth0 client secret; it is a browser
application. The separate Terraform deployment identity is a confidential
Management API application; keep that application's secret only in the approved
deployment secret store and environment. Never put either it or the Resend API
key in this repository or ECS.

### Launch authorization limitations

At launch, an Auth0 identity maps to one Payday account with one active,
unnamed server-side API key. That key can both create payments and read every
payment owned by the account. Payday does not yet provide read-only or otherwise
scoped keys, multiple concurrent named keys, source-IP allowlists, or
team/organization membership. Share it only with principals that may exercise
the account's full authority; rotation eventually disrupts every user and
integration that does not adopt the replacement during its grace period. These
limitations are accepted for launch, not guarantees of the long-term
authorization model.

## 1. Configure Resend

1. Create a Resend account controlled by Payday.
2. In **Domains**, add a dedicated sending subdomain such as
   `auth.payday.sh`.
3. Add the DKIM, SPF, and return-path DNS records Resend displays. Wait for the
   domain to show **Verified**. Add a DMARC record in monitoring mode and tighten
   it after verifying alignment.
4. In **API Keys**, create a key named `Auth0 production`, grant **Sending
   access**, and restrict it to `auth.payday.sh`. Copy it now; Resend will not
   show it again.
5. In Auth0, open **Branding → Email Provider**, enable **Use my own email
   provider**, select **Resend**, set the sender to a verified address such as
   `Payday <login@auth.payday.sh>`, paste the Resend key, and save.
6. Click **Send Test Email**. Confirm delivery and check both Auth0
   **Monitoring → Logs** and Resend **Emails**.

The passwordless Email connection's **From** address must match the sender
configured for Resend. Monitor bounces, complaints, suppressions, and the Resend
plan's daily sending limit.

## 2. Configure Auth0

1. Create an Auth0 tenant in the desired production region. Record its domain as
   `PAYDAY_AUTH0_ISSUER=https://<tenant-domain>/`.
2. Open **Applications → APIs → Create API**:
   - Name: `Payday API`
   - Identifier: `https://api.payday.sh`
   - Signing algorithm: `RS256`
   - Token lifetime: 300 seconds
3. Follow [`auth0/README.md`](../auth0/README.md) to import the existing Email
   connection and apply the tracked Terraform configuration. That apply creates
   the `Payday Dashboard` single-page application (`auth0/dashboard.tf`) with
   only the passwordless OTP grant and enables the passwordless Email
   connection for it. Record its public Client ID: `gatewayd` uses it as
   `PAYDAY_AUTH0_CLIENT_ID`, the web app as `NEXT_PUBLIC_AUTH0_CLIENT_ID`.
4. The dashboard selects Code on each passwordless request; Terraform fixes the
   code at six digits and the expiry at five minutes, enables signups and
   brute-force protection, and installs the branded template. This connection
   does not prohibit another authorized client from requesting a magic link.
   Auth0 supports HTML only for passwordless templates; it cannot attach a
   separate plain-text MIME part. In the connection's **Applications** tab,
   enable only `Payday Dashboard` (and the payer application from § 3a).
5. In `Payday Dashboard`'s **Connections** tab, enable only passwordless
   **Email**. Disable every database, social, and enterprise connection. Do not
   create or distribute a client secret.

Auth0's passwordless endpoint rate limits apply by end-user IP. Apply and verify
the reviewable tenant controls and operational checklist in
[`auth0/README.md`](../auth0/README.md) before launch. That root enables
brute-force and suspicious-IP blocking and notifications; breached-password
detection is intentionally irrelevant to this passwordless-only tenant. Keep
the OTP email wording non-enumerating and free of payment data.

## 3. Install the token-enforcement Action

The API requires signed claims proving that Auth0 issued the token to the exact
Payday client after a fresh passwordless email authentication. Dashboard
connection settings alone are not this security boundary.

1. Open **Actions → Library → Build Custom** and create a **Post Login** Action
   named `Payday email OTP claims` on the latest supported Node runtime.
2. Paste the tracked source from
   `auth0/actions/payday-email-otp.js` into the editor.
3. Add two Action secrets:
   - `PAYDAY_API_AUDIENCE` = `https://api.payday.sh`
   - `PAYDAY_CLIENT_ID` = the `Payday Dashboard` Client ID
4. Click **Deploy**.
5. Open **Actions → Triggers → Post Login**, drag the deployed Action into the
   flow, and click **Apply**.

The Action denies tokens for the Payday audience unless the client is the
configured ID and the connection, email authentication method, and
authentication age match. It sets five claims — method, email, the client ID
that authenticated, the authentication time, and a random signed
authentication-event ID. The API accepts that event for one key issuance only,
so replaying the same access token cannot rotate a key twice. The verified
address is stored with the account for urgent payout-support notifications.

## 3a. Install the payer audience and Action

Payers prove ownership of the mailbox an invoice was issued to through a
dedicated audience, so that a payer token is useless against the merchant API
and a merchant token never unlocks an invoice. `gatewayd` drives the exchange
itself: the payer never names an email, and the code goes to the address the
merchant asserted.

1. Open **Applications → APIs** and create an API named `Payday Payer` with
   identifier `https://api.payday.sh/payer` (RS256).
2. Let Terraform create the `Payday Payer Verification` application
   (`auth0/payer.tf`), or create a **Native** application by hand with only
   the passwordless OTP grant and the `email` connection enabled.
3. Create a second **Post Login** Action named `Payday payer email OTP claims`
   from `auth0/actions/payday-payer-email-otp.js` with two secrets:
   - `PAYDAY_PAYER_AUDIENCE` = `https://api.payday.sh/payer`
   - `PAYDAY_PAYER_CLIENT_ID` = the `Payday Payer Verification` Client ID
4. Deploy it and add it to the Post Login flow next to the merchant Action.

The payer Action is inert for every other audience and denies every other
client on its own. It sets exactly five claims: method, client ID,
authentication time, a random event ID, and the proven email, trimmed and
lowercased so `gatewayd` can compare it with the merchant's assertion.

## 4. Configure Payday

Configure `gatewayd` (or its untracked local `.env`) with:

```bash
export PAYDAY_AUTH0_ISSUER="https://<tenant-domain>/"
export PAYDAY_AUTH0_AUDIENCE="https://api.payday.sh"
export PAYDAY_AUTH0_CLIENT_ID="<Payday-Dashboard-client-id>"
# Payer email verification (all four together, or none).
export PAYDAY_PAYER_AUTH0_ISSUER="https://<tenant-domain>/"
export PAYDAY_PAYER_AUTH0_AUDIENCE="https://api.payday.sh/payer"
export PAYDAY_PAYER_AUTH0_CLIENT_ID="<Payday-Payer-Verification-client-id>"
# 32 random bytes, standard base64: derives the merchant-scoped payer references.
export PAYDAY_PAYER_REF_MASTER_KEY="$(openssl rand -base64 32)"
# The browser origin allowed to call the verification write routes; defaults
# to PAYDAY_PUBLIC_BASE_URL.
export PAYDAY_HOSTED_CHECKOUT_ORIGIN="https://payday.sh"
```

Without the `PAYDAY_PAYER_*` settings the API still serves gated invoices, but
their verification routes answer `503 verification_unavailable`.

The web app needs the same issuer, audience, and client ID as
`NEXT_PUBLIC_AUTH0_DOMAIN`, `NEXT_PUBLIC_AUTH0_AUDIENCE`, and
`NEXT_PUBLIC_AUTH0_CLIENT_ID` (`web/.env.example`).

### Identity verification (Didit)

The two identity modes use Didit's hosted document, liveness, and face-match
session behind a thin provider boundary. Configure one workflow in the Didit
console that requires all three checks and declines expected-name mismatches,
create a webhook destination pointing at
`https://api.payday.sh/v1/webhooks/identity`, and set (all three together, or
none):

```bash
export PAYDAY_DIDIT_API_KEY="<Didit API key>"
export PAYDAY_DIDIT_WORKFLOW_ID="<pinned workflow id>"
export PAYDAY_DIDIT_WEBHOOK_SECRET="<the destination's shared secret>"
# Optional; defaults to https://verification.didit.me.
export PAYDAY_DIDIT_BASE_URL="https://verification.didit.me"
# Recorded as the reviewer on manual verification decisions; defaults to
# "operator".
export PAYDAY_ADMIN_REVIEWER_ID="reviewer@example.com"
```

Without them the identity modes can still be issued and email-verified, and
`identity/start` answers `503 verification_unavailable`. Payday sends Didit
the payer reference (never the mailbox), Payday's own attempt id as metadata,
the checkout URL to return to, and, for `verified_identity` only, the
expected first and last name; it keeps statuses, the session reference, and
risk categories, and never the extracted identity. Before enabling this in
production, complete the data-processing, retention, consent, appeal, and
human-review requirements in `features/product-plan.md` (Slice 0).

For AWS, set `auth0_issuer`, `auth0_audience`, and `auth0_client_id` in the
untracked `infra/terraform.tfvars`; Terraform passes them to the API task.

## 5. Sign in and mint the first key

Open the landing page and choose **Start Building**. The dialog asks for an
email address, sends the code through Auth0, and exchanges it for an access
token for the Payday API audience with the `Payday Dashboard` client ID. That
token is the dashboard session ([§ 7](#7-dashboard-sessions)); the API
provisions the account the first time it sees the identity, so the first code
creates the account and every later one signs into it.

The dashboard's **API key** section shows the account's key metadata — hint,
generation, timestamps, and any previous-key expiry — and mints a key. Minting
steps up: it asks for a fresh code first, because reading an account does not
imply permission to create a live credential for it. The raw key is shown
exactly once; copy it straight into the secret store your server reads
`PAYDAY_API_KEY` from. Nothing later, including the account routes, can return
it again.

Underneath, the dashboard calls three endpoints that a script can call too
(`scripts/local-api-key.sh` does exactly this against the local stack):
`POST {issuer}/passwordless/start` to send the code,
`POST {issuer}/oauth/token` with the
`http://auth0.com/oauth/grant-type/passwordless/otp` grant to exchange it, and
`POST /v1/account/api-key` with that token as the bearer. The last accepts only
a *fresh, single-use* authentication: completed within five minutes and not
already spent on another issue or revoke.

## 6. Rotate or revoke a key

Rotation is the same step-up in the dashboard's API key section: a fresh code,
then **Roll**. The new key replaces the old one at the next generation and is
shown once; the previous key remains valid for 24 hours, allowing a safe
deployment overlap. Concurrent rotations use generation checks
(`expected_generation`) and fail with `409 api_key_generation_conflict` rather
than replacing an unexpected key.

For CI and servers, inject the key from its secret store as `PAYDAY_API_KEY`;
do not perform routine interactive OTP login.

For compromise or decommissioning, **Revoke** immediately invalidates the
current and grace-period keys. A later mint issues a new generation. The
underlying routes are `POST` and `DELETE /v1/account/api-key`; see the
[API reference](api-reference.md#account-key-api).

## 7. Dashboard sessions

The merchant dashboard at `payday.sh/dashboard` is a browser application, so it
never stores an API key. It signs in with the email OTP: the landing page's
sign-up dialog, which is the only way in, calls the issuer's
`/passwordless/start` and `/oauth/token` endpoints directly with the
`Payday Dashboard` client ID (`auth0/dashboard.tf`) and the Payday API
audience, and receives an access token. That token is the session. It is kept
in memory and mirrored to the tab's `sessionStorage` only so a reload does not
demand a new code; it is never written to `localStorage`.

Nothing refreshes it: the session runs for the token's own lifetime, which the
issuer sets (Auth0's for the API audience; `ACCESS_TOKEN_TTL` in
`payday-dev-identity` locally, matching Auth0's 24-hour default). When it
lapses the browser stops sending it and the dashboard sends the merchant to the
landing page for a new code. That is a different window from the five-minute
freshness key issuance demands, above.

The Post-Login Action admits the dashboard client only when its ID is set as
the `PAYDAY_CLIENT_ID` secret; `gatewayd` learns the same ID from
`PAYDAY_AUTH0_CLIENT_ID`.

The API accepts a dashboard access token as a session credential on the
payment, customer, and attachment routes: `Authorization: Bearer <token>`
carries either an API key or an Auth0 token, and the identity `(iss, sub)`
maps to the account, provisioning one without an API key on first sight. No
key is issued or stored for a dashboard login. Key issuance and revocation
(`/v1/account/api-key`) keep their stricter rule: a fresh OTP within five
minutes, and each signed authentication event mutates key state once.

Locally, `payday-dev-identity` issues merchant tokens to
`payday-dashboard-local` for the `payday-api-local` audience and answers
browser preflights, so the web dev server (`web/.env.example`) signs in against
it with the code printed in the provider's log, and `just seed` mints an API
key through the same exchange.

## Clean pre-launch database

This release intentionally changes the initial schema rather than carrying
forward the global test key and pre-release invoices. Immediately before the
first deployment of this release, stop both services, delete the pre-release
database contents, recreate an empty `gateway` database/schema, and then start
the API and indexer so embedded migrations create the schema from scratch. Do
not deploy this build over the old test schema.

## Local and staging verification

Automated tests use ephemeral RSA keys and PostgreSQL databases. They cover
issuer, audience, client, email-method and freshness enforcement; JWKS rotation
and bounded outage behavior; one-time authentication-event consumption; atomic
key replacement; tenant isolation; and the embedded OTP HTTP exchange.
`scripts/e2e-anvil.sh` exercises the complete payment lifecycle and tenant
isolation without contacting Auth0.

Hosted Auth0 and email delivery cannot be reproduced by local unit tests. Before
launch, additionally:

1. Complete first-key and replacement flows against staging using a real email.
2. Confirm a reused/expired/wrong OTP fails and Auth0 does not reveal whether an
   address already exists.
3. Confirm the first access token cannot issue two keys.
4. Confirm old API keys remain valid during the 24-hour rotation grace period
   and fail immediately after a revoke.
5. Deliver real OTPs to Gmail and one other mailbox; inspect the raw received
   headers and require `spf=pass`, `dkim=pass`, and `dmarc=pass` with the Payday
   From domain aligned. Also confirm the preheader, mobile layout, code block,
   expiry copy, ignore copy, and support link render correctly.
6. Send a message to `support@payday.sh` and confirm it reaches the operator who
   owns authentication support.
7. Revoke a staging Resend key and confirm login fails closed while already
   issued Payday API keys continue to work.
