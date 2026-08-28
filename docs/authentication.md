# Email authentication and API keys

Payday uses Auth0's embedded passwordless email OTP flow. Auth0 creates the
identity and verifies the one-time code; Resend delivers the email. Payday never
receives a password, generates an OTP, or stores an email address.

Each Auth0 identity owns exactly one active Payday API key. Requesting another
key atomically replaces the stored hash, increments its generation, and makes
the prior key enter a 24-hour grace period. Plaintext keys are returned once; PostgreSQL stores
only a SHA-256 digest and a final-six-character hint.

## Credentials and where they belong

| Value | Where to obtain it | Where to place it |
|---|---|---|
| Auth0 issuer | Auth0 **Settings → Domain**, prefixed with `https://` and suffixed with `/` | API service, CLI, `infra/terraform.tfvars` |
| Auth0 API audience | The API **Identifier** created below; use `https://api.payday.sh` | API service, CLI, `infra/terraform.tfvars` |
| Auth0 client ID | Native application **Settings → Client ID** | API service, CLI, `infra/terraform.tfvars`; it is public |
| Resend API key | Resend **API Keys → Create API Key** | Auth0 **Branding → Email Provider** only |

There is no Auth0 client secret in this design. Never put the Resend API key in
this repository, Terraform, ECS, or a user's CLI environment.

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
3. Open **Applications → Applications → Create Application**:
   - Name: `Payday CLI`
   - Type: **Native**
   Record its public Client ID. `gatewayd` uses it as
   `GATEWAY_AUTH0_CLIENT_ID`; non-production CLI testing uses it as the hidden
   `PAYDAY_AUTH0_CLIENT_ID` override.
4. In that application's **Advanced Settings → Grant Types**, enable
   **Passwordless OTP**. Disable grants the CLI does not use, especially
   Password and Client Credentials. Do not create or distribute a client
   secret.
5. Follow [`auth0/README.md`](../auth0/README.md) to import the existing Email
   connection and apply the tracked Terraform configuration. The CLI selects
   Code on each passwordless request; Terraform fixes the code at six digits
   and the expiry at three minutes, enables signups and brute-force protection,
   and installs the branded template. This connection does not prohibit another
   authorized client from requesting a magic link. Auth0 supports HTML only for
   passwordless templates; it cannot attach a separate plain-text MIME part. In
   the connection's **Applications** tab, enable only `Payday CLI`.
6. In `Payday CLI`'s **Connections** tab, enable only passwordless **Email**.
   Disable every database, social, and enterprise connection.
7. In the Payday API's application access settings, authorize only
   `Payday CLI`.

Auth0's passwordless endpoint rate limits apply by end-user IP. Configure Auth0
attack protection before launch, and keep the OTP email wording
non-enumerating and free of invoice data.

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
   - `PAYDAY_CLIENT_ID` = the `Payday CLI` Client ID
4. Click **Deploy**.
5. Open **Actions → Triggers → Post Login**, drag the deployed Action into the
   flow, and click **Apply**.

The Action denies tokens for the Payday audience unless the client, connection,
email authentication method, and authentication age match. It adds a random,
signed authentication-event ID. The API accepts that event for one key issuance
only, so replaying the same access token cannot rotate a key twice.

## 4. Configure Payday

Configure `gatewayd` (or its untracked local `.env`) with:

```bash
export GATEWAY_AUTH0_ISSUER="https://<tenant-domain>/"
export GATEWAY_AUTH0_AUDIENCE="https://api.payday.sh"
export GATEWAY_AUTH0_CLIENT_ID="<Payday-CLI-client-id>"
```

For AWS, set `auth0_issuer`, `auth0_audience`, and `auth0_client_id` in the
untracked `infra/terraform.tfvars`; Terraform passes them to the API task.

The production CLI includes the public Payday Auth0 issuer, native client ID,
and `https://api.payday.sh` audience. A non-production `PAYDAY_API_URL` requires
all three hidden identity overrides `PAYDAY_AUTH0_ISSUER`,
`PAYDAY_AUTH0_CLIENT_ID`, and `PAYDAY_AUTH0_AUDIENCE`. This prevents production
identity tokens from being forwarded to another API.

## 5. Test signup and first key creation from the CLI

Build the CLI and sign in:

```bash
cargo build -p gateway-cli --bin payday
./target/debug/payday login
```

The interaction is terminal-native:

```text
Payday — stablecoin virtual accounts for every payment
Email  › user@example.com
✓ Code sent to u***@example.com · expires in 3 min · r to resend
Code   › 123456
✓ Signed in as user@example.com
✓ API key saved to ~/.config/payday/credentials (payday_live_…1234)
```

Entering the OTP calls Auth0 directly and does not open a browser. The masked key
is saved in an XDG-aware credentials file (`$XDG_CONFIG_HOME/payday/credentials`,
or `~/.config/payday/credentials`; Windows uses `%APPDATA%\\payday\\credentials`).
`PAYDAY_CONFIG_DIR` overrides the directory. Separate `default` and `local`
profiles are bound to their issuing API URL. Use `payday login --show` only when
the plaintext key must be copied to an approved secret manager. Payment commands
load the saved profile automatically:

```bash
./target/debug/payday create \
  --chain-id 143 \
  --token <USDC_ADDRESS> \
  --payout <PAYOUT_ADDRESS> \
  --amount 1.00 \
  --expires-in 3600 \
  --refund <REFUND_ADDRESS>
./target/debug/payday get <PAYMENT_ID>
```

`payday whoami` shows the account, key hint, generation, timestamps, and any
previous-key expiry. `payday logout` removes only the saved profile; it does not
revoke a server-side key.

## 6. Rotate or revoke a key

Rotation requires a fresh email OTP:

```bash
./target/debug/payday keys rotate
```

Confirm the prompt (or pass `-y`). The new key replaces the saved profile and is
masked unless `--show` is supplied. The previous key remains valid for 24 hours,
allowing a safe deployment overlap. Concurrent rotations use generation checks
and fail with a conflict rather than replacing an unexpected key.

For CI, inject a key from its secret store; do not perform routine interactive
OTP login. `PAYDAY_API_KEY` takes precedence over saved credentials:

```bash
export PAYDAY_API_KEY="<key-from-CI-secret-store>"
./target/debug/payday --json get <PAYMENT_ID>
```

For compromise or decommissioning, `payday keys revoke` (or `-y`) immediately
invalidates current and grace-period keys and removes the saved profile. A later
`payday login` issues a new generation.

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
   and fail immediately after `payday keys revoke`.
5. Deliver real OTPs to Gmail and one other mailbox; inspect the raw received
   headers and require `spf=pass`, `dkim=pass`, and `dmarc=pass` with the Payday
   From domain aligned. Also confirm the preheader, mobile layout, code block,
   expiry copy, ignore copy, and support link render correctly.
6. Send a message to `support@payday.sh` and confirm it reaches the operator who
   owns authentication support.
7. Revoke a staging Resend key and confirm login fails closed while already
   issued Payday API keys continue to work.
