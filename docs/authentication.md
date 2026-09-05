# Authentication and API keys

Two identity providers, on purpose.

**Merchants sign in through [Privy](https://privy.io).** The dashboard asks for
a mailbox, Privy emails a code, and what the browser gets back is Privy's
identity token: a signed statement of the user's DID, the mailbox they proved,
and the embedded EVM wallet Privy created for them at their first sign-in.
That token is the dashboard session. The API verifies it on every merchant
route, provisions an account the first time it sees a DID, and records the
wallet as where the account's deposits settle by default. Payday never sees a
password, never generates a code, and never holds the wallet's key.

**Payers and issuer mailboxes are proven through Auth0.** A payer opening a
gated deposit request, or a merchant proving an issuer identity's contact address,
must show that a mailbox was just opened — but neither is a Payday customer,
and those checks run many times more often than a merchant signs up. Creating
a Privy user for each would be paying for accounts that exist for one code.
So they stay on Auth0's embedded passwordless OTP: Auth0 creates the code,
verifies it, and mints a token for a dedicated payer audience; Resend delivers
the email; `gatewayd` drives the exchange and only ever sees the code. Payday
stores neither the code nor the mailbox beyond the proof it needs.

Each Privy identity owns exactly one active Payday API key. Requesting another
key atomically replaces the stored hash, increments its generation, and makes
the prior key enter a 24-hour grace period. Plaintext keys are returned once;
PostgreSQL stores only a SHA-256 digest and a final-six-character hint.

## Credentials and where they belong

| Value | Where to obtain it | Where to place it |
|---|---|---|
| Privy app id | Privy **Configuration → App settings** | API service (`PAYDAY_PRIVY_APP_ID`), web (`NEXT_PUBLIC_PRIVY_APP_ID`), `infra/terraform.tfvars`; it is public |
| Auth0 issuer | Auth0 **Settings → Domain**, prefixed with `https://` and suffixed with `/` | API service, `infra/terraform.tfvars` |
| Auth0 payer audience | The `Payday Payer` API identifier, `https://api.payday.sh/payer` | API service, `infra/terraform.tfvars` |
| Auth0 payer client ID | `Payday Payer Verification` application **Settings → Client ID** | API service, Action secret, `infra/terraform.tfvars`; it is public |
| Resend API key | Resend **API Keys → Create API Key** | Auth0 **Branding → Email Provider** only |

There is no Privy app secret anywhere in Payday: verifying an identity token
needs only the app's published keys, which `gatewayd` fetches from
`https://auth.privy.io/api/v1/apps/<app id>/jwks.json`. The Auth0 payer
application has no client secret either. The separate Terraform deployment
identity is a confidential Management API application; keep that application's
secret only in the approved deployment secret store and environment. Never put
it or the Resend API key in this repository, ECS, or a developer's shell.

### Launch authorization limitations

At launch, a Privy identity maps to one Payday account with one active,
unnamed server-side API key. That key can both create deposits and read every
deposit owned by the account. Payday does not yet provide read-only or otherwise
scoped keys, multiple concurrent named keys, source-IP allowlists, or
team/organization membership. Share it only with principals that may exercise
the account's full authority; rotation eventually disrupts every integration
that does not adopt the replacement during its grace period. These limitations
are accepted for launch, not guarantees of the long-term authorization model.

## 1. Configure Privy

1. Create a Privy app (one per environment: production and sandbox accounts
   must never share an app). Record its **App ID**.
2. In **User management → Authentication**, enable **Email** and nothing
   else: the dashboard's dialog is email-only and never shows Privy's modal.
3. In **Wallets**, turn on **Automatically create embedded wallets on login**
   for Ethereum. The dashboard also asks for a wallet itself when a signed-in
   user has none, so an account never lacks one for long either way.
4. In **User management → Authentication → Advanced**, turn on **Return user
   data in an identity token**. The identity token is the session; without
   it the dashboard has nothing to send.
5. In **Configuration → App settings → Domains**, allow the dashboard's
   origins: `https://payday.sh` in production, `http://127.0.0.1:3002` for
   the local Next.js dev server (there is no local stand-in for Privy; local
   development signs in against the development app for real).
6. Optionally, in **Advanced**, enable **test accounts** on the development
   app, which gives a fixed mailbox and code for manual testing without a
   real inbox. They do not work once allowed domains are set on a production
   app.

## 2. Configure Resend

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
plan's daily sending limit. Merchant sign-in codes are Privy's, delivered by
Privy; Resend carries only payer and issuer-mailbox codes.

## 3. Configure Auth0 for payer verification

Auth0's job is one audience: the proof that a merchant-asserted mailbox was
just opened. It has no merchant audience and no merchant application any
more — a tenant that still carries the `Payday API` resource server, the
`Payday CLI` native application, the `Payday Dashboard` single-page
application, or the `Payday email OTP claims` Action from before can delete
them; nothing reads their tokens.

1. Create an Auth0 tenant in the desired production region. Record its domain as
   `PAYDAY_PAYER_AUTH0_ISSUER=https://<tenant-domain>/`.
2. Follow [`auth0/README.md`](../auth0/README.md) to import the existing Email
   connection and apply the tracked Terraform configuration. Terraform fixes
   the code at six digits and the expiry at five minutes, enables signups and
   brute-force protection, and installs the branded template. Auth0 supports
   HTML only for passwordless templates; it cannot attach a separate
   plain-text MIME part.
3. Open **Applications → APIs** and create an API named `Payday Payer` with
   identifier `https://api.payday.sh/payer` (RS256).
4. Let Terraform create the `Payday Payer Verification` application
   (`auth0/payer.tf`), or create a **Native** application by hand with only
   the passwordless OTP grant and the `email` connection enabled. In the
   connection's **Applications** tab, enable only this application.
5. Open **Actions → Library → Build Custom** and create a **Post Login**
   Action named `Payday payer email OTP claims` from
   `auth0/actions/payday-payer-email-otp.js` with two secrets:
   - `PAYDAY_PAYER_AUDIENCE` = `https://api.payday.sh/payer`
   - `PAYDAY_PAYER_CLIENT_ID` = the `Payday Payer Verification` Client ID
6. Deploy it and add it to the Post Login flow.

The payer Action is inert for every other audience and denies every other
client on its own. It sets exactly five claims: method, client ID,
authentication time, a random event ID, and the proven email, trimmed and
lowercased so `gatewayd` can compare it with the merchant's assertion.
`gatewayd` drives the exchange itself: the payer never names an email, and the
code goes to the address the merchant asserted. The same exchange proves an
issuer identity's contact address from the dashboard.

Auth0's passwordless endpoint rate limits apply by end-user IP. Apply and
verify the reviewable tenant controls and operational checklist in
[`auth0/README.md`](../auth0/README.md) before launch. That root enables
brute-force and suspicious-IP blocking and notifications; breached-password
detection is intentionally irrelevant to this passwordless-only tenant. Keep
the OTP email wording non-enumerating and free of deposit data.

## 4. Configure Payday

Configure `gatewayd` (or its untracked local `.env`) with:

```bash
# Merchant sign-in: the Privy app's public id. Unset, only API keys authenticate.
export PAYDAY_PRIVY_APP_ID="<Privy app id>"
# Payer and issuer-mailbox verification (all four together, or none).
export PAYDAY_PAYER_AUTH0_ISSUER="https://<tenant-domain>/"
export PAYDAY_PAYER_AUTH0_AUDIENCE="https://api.payday.sh/payer"
export PAYDAY_PAYER_AUTH0_CLIENT_ID="<Payday-Payer-Verification-client-id>"
# 32 random bytes, standard base64: derives the merchant-scoped payer references.
export PAYDAY_PAYER_REF_MASTER_KEY="$(openssl rand -base64 32)"
# The browser origin allowed to call the verification write routes; defaults
# to PAYDAY_PUBLIC_BASE_URL.
export PAYDAY_HOSTED_CHECKOUT_ORIGIN="https://payday.sh"
```

`gatewayd` fetches the Privy app's key set at startup and refuses to start if
it cannot; afterwards it refreshes the set every five minutes or on an unknown
key id, and fails closed once it has gone an hour without a successful
refresh. Without `PAYDAY_PRIVY_APP_ID` every dashboard session is refused and
only API keys authenticate. Without the `PAYDAY_PAYER_*` settings the API
still serves gated deposit requests, but their verification routes — and issuer
mailbox verification — answer `503 verification_unavailable`.

The web app is built with the same app id as `NEXT_PUBLIC_PRIVY_APP_ID`
(`web/.env.example`).

For AWS, set `privy_app_id`, `auth0_issuer`, `payer_auth0_audience`, and
`payer_auth0_client_id` in the untracked `infra/terraform.tfvars`; Terraform
passes them to the API task.

## 5. Sign in and create a first key

Open `payday.sh`, choose **Start Building**, enter a mailbox, and enter the
code Privy emails. The API provisions the account on that first request, with
the embedded wallet Privy created; the dashboard's **Account** section shows
the mailbox, the wallet in full, and its balance on the deployment's chain.

The **API key** section generates the key your own server calls the API
with. It is shown once. The session is the credential: no second code is
asked for, and an API key can never mint another — `POST /v1/account/api-key`
refuses anything but a dashboard session (`identity_unauthorized`). Give the
key to the SDK, `curl`, or the CI secret store.

## 6. Rotate or revoke a key

**Roll key** in the dashboard issues a new key against the account's current
generation. The previous key remains valid for 24 hours, allowing a safe
deployment overlap; concurrent rotations use generation checks and fail with
`api_key_generation_conflict` rather than replacing an unexpected key.

For compromise or decommissioning, **Revoke** immediately invalidates current
and grace-period keys. A later **Generate key** issues a new generation.

## 7. Dashboard sessions

The merchant dashboard at `payday.sh/dashboard` is a browser application, so it
never holds an API key. Its session is Privy's identity token: the landing
page's sign-up dialog, which is the only way in, runs Privy's email code
exchange through Privy's own SDK (`useLoginWithEmail`), and the SDK keeps the
resulting tokens in the browser and refreshes them while the merchant stays
signed in. The dashboard reads the current identity token from the SDK and
sends it as `Authorization: Bearer <token>` on every call; nothing of Payday's
stores a credential of its own.

`gatewayd` verifies the token's ES256 signature against the app's JWKS, its
issuer (`privy.io`), its audience (the app id), and its expiry, and reads the
`linked_accounts` claim — a JSON string, as Privy's own server SDK reads it —
for the mailbox (`type: email`) and the embedded wallet (`type: wallet`,
`chain_type: ethereum`, `wallet_client_type: privy`). The identity `(iss,
sub)` maps to the account, provisioning one without an API key on first
sight, and the wallet is recorded on that first sight and kept current on
every later one — so a session issued before Privy finished creating the
wallet is picked up as soon as one carries it. No key is issued or stored for
a dashboard login.

Signing out asks Privy to end the session, which clears its tokens; a
merchant who reaches a dashboard route without one is sent to the landing
page. A token Privy will not refresh — after sign-out elsewhere, or once its
refresh token has aged out — makes the next API call answer `401`, and the
dashboard signs out the same way.

Locally the dashboard signs in against the real development Privy app (there
is no local stand-in), with `http://127.0.0.1:3002` among its allowed domains
and `gatewayd` configured with the same app id. Scripts that need an account
without a browser — `just seed`, `scripts/e2e-anvil.sh` — write one straight
into the local database with `scripts/local-api-key.sh` instead.

## Clean pre-launch database

This release intentionally changes the initial schema rather than carrying
forward the global test key and pre-release deposit requests. Immediately before the
first deployment of this release, stop both services, delete the pre-release
database contents, recreate an empty `gateway` database/schema, and then start
the API and indexer so embedded migrations create the schema from scratch. Do
not deploy this build over the old test schema.

## Local and staging verification

Automated tests use ephemeral keys and PostgreSQL databases. They cover
Privy issuer, audience, signature, and subject enforcement; mailbox and wallet
extraction from the identity token, including a session that arrives before
the wallet exists; JWKS rotation and bounded outage behavior; account
provisioning and wallet adoption; key issuance, rotation, and revocation from
a session and their refusal to an API key; atomic key replacement; tenant
isolation; and the payer OTP exchange. `scripts/e2e-anvil.sh` exercises the
complete deposit request lifecycle and tenant isolation without contacting Privy or
Auth0.

Hosted Privy, hosted Auth0, and email delivery cannot be reproduced by local
unit tests. Before launch, additionally:

1. Sign in to staging with a real mailbox; confirm the account appears with
   its wallet, and that a second sign-in lands on the same account.
2. Generate, roll, and revoke a key from the dashboard; confirm the rolled
   key's predecessor keeps working through its grace window and fails
   immediately after revocation, and that an API key cannot call
   `POST /v1/account/api-key`.
3. Issue a gated deposit request and verify it as a payer with a real mailbox;
   confirm a reused/expired/wrong code fails and Auth0 does not reveal whether
   an address already exists.
4. Deliver real payer codes to Gmail and one other mailbox; inspect the raw
   received headers and require `spf=pass`, `dkim=pass`, and `dmarc=pass`
   with the Payday From domain aligned. Also confirm the preheader, mobile
   layout, code block, expiry copy, ignore copy, and support link render
   correctly.
5. Send a message to `support@payday.sh` and confirm it reaches the operator
   who owns authentication support.
6. Revoke a staging Resend key and confirm payer verification fails closed
   while merchant sign-in and already issued API keys continue to work.
