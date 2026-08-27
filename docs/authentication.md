# Email authentication and API keys

Payday uses Auth0's embedded passwordless email OTP flow. Auth0 creates the
identity and verifies the one-time code; Resend delivers the email. Payday never
receives a password, generates an OTP, or stores an email address.

Each Auth0 identity owns exactly one active Payday API key. Requesting another
key atomically replaces the stored hash, increments its generation, and makes
the old key fail immediately. Plaintext keys are returned once; PostgreSQL stores
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
   Record its public Client ID as `PAYDAY_AUTH0_CLIENT_ID`.
4. In that application's **Advanced Settings → Grant Types**, enable
   **Passwordless OTP**. Disable grants the CLI does not use, especially
   Password and Client Credentials. Do not create or distribute a client
   secret.
5. Open **Authentication → Passwordless → Email**. Enable it, select **Code**
   rather than magic link, retain a short expiry (three minutes is the Auth0
   default), and enable signups. In its **Applications** tab, enable only
   `Payday CLI`.
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

For local shells, put the public Auth0 values in an untracked `.env` or export
them directly:

```bash
export PAYDAY_AUTH0_ISSUER="https://<tenant-domain>/"
export PAYDAY_AUTH0_AUDIENCE="https://api.payday.sh"
export PAYDAY_AUTH0_CLIENT_ID="<Payday-CLI-client-id>"
```

The same three values are required by `gatewayd`. For AWS, set
`auth0_issuer`, `auth0_audience`, and `auth0_client_id` in the untracked
`infra/terraform.tfvars`; Terraform passes them to the API task. The CLI reads
the three environment variables directly.

## 5. Test signup and first key creation from the CLI

Build the CLI, set the API URL, and run:

```bash
cargo build -p gateway-cli --bin payday
export PAYDAY_API_URL="https://api.payday.sh"
./target/debug/payday account create
```

The interaction is terminal-native:

```text
Email: user@example.com
A one-time code was sent to user@example.com. Check your email.
One-time code: 123456
API key created (generation 1).
API key: payday_live_...
Store this key securely; it cannot be retrieved later.
```

Entering the OTP calls Auth0 directly and does not open a browser. Store the key
in a password manager, then use it without putting it in shell history:

```bash
export PAYDAY_API_KEY="payday_live_..."
./target/debug/payday create \
  --chain-id 143 \
  --token <USDC_ADDRESS> \
  --payout <PAYOUT_ADDRESS> \
  --amount 1.00 \
  --expires-in 3600 \
  --refund <REFUND_ADDRESS>
./target/debug/payday get <PAYMENT_ID>
```

`payday account get` repeats email OTP authentication and returns only the
key hint, generation, and timestamps—not the key itself.

## 6. Replace a key

Run the same creation command again:

```bash
./target/debug/payday account create
```

After email OTP authentication, an existing account gets an explicit warning:

```text
WARNING: issuing a new API key immediately invalidates the existing key …abc123 (generation 1).
Proceed? [y/N]: y
API key replaced (generation 2).
API key: payday_live_...
Advisory: your previous API key is now invalid. Store this key securely; it cannot be retrieved later.
```

Enter `n` or press Enter to cancel. Automation may pass `--yes`; the warning is
still written to stderr. With `--json`, stdout is machine-readable and includes
`replaced_previous_key`; prompts and warnings remain on stderr:

```bash
key_json="$(./target/debug/payday --json account create --yes)"
export PAYDAY_API_KEY="$(jq -r .api_key <<<"$key_json")"
```

The server checks the generation shown in the warning inside the same database
transaction that replaces the key. If another command creates or replaces the
key first, this command fails with a conflict without changing either key; run
it again to review and confirm the new generation.

Verify the former key receives HTTP 401 and the replacement can still get an
invoice owned by the account.

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
4. Confirm old API keys immediately return 401 after replacement.
5. Inspect received headers for SPF, DKIM, and DMARC alignment.
6. Revoke a staging Resend key and confirm login fails closed while already
   issued Payday API keys continue to work.
