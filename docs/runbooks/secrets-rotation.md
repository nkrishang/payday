# Secrets rotation

How to rotate an account API key and infrastructure secrets.

## Rotate the API key

API-key replacement requires a fresh email OTP and invalidates the old key
immediately. Run:

```bash
export GATEWAY_AUTH0_ISSUER="https://<tenant>.auth0.com/"
export GATEWAY_AUTH0_AUDIENCE="https://api.payday.sh"
export GATEWAY_AUTH0_CLIENT_ID="<native-application-client-id>"
./target/release/gateway-cli account create
```

The CLI displays the existing key hint and generation, warns that replacement
is immediate, and asks `Proceed? [y/N]`. After confirmation, its success message
explicitly states that the old key is invalid. To capture JSON without printing
the secret in shared logs:

```bash
rotation_json="$(./target/release/gateway-cli --json account create --yes)"
export GATEWAY_API_KEY="$(jq -r .api_key <<<"$rotation_json")"
```

Store and distribute the new key through the account owner's approved secret
management process. The service does not retain recoverable plaintext and does
not need a restart. Verify the new key against an invoice owned by this account:

```bash
./target/release/gateway-cli invoice get <INVOICE_ID>
```

## Rotate the Resend API key

The Resend API key is held by Auth0, not by Payday's services or Terraform.
Create a replacement sending-only key in Resend, update **Branding → Email
Provider** in Auth0, and send a test email before revoking the old key. Then run
one complete staging email-OTP login. A failed rotation prevents login but does
not affect existing invoice API keys.

## Rotate the RPC URL

### Step 1: Update the secret

```bash
RPC_SECRET_ARN="$(terraform -chdir=infra output -raw rpc_url_secret_arn)"

aws secretsmanager put-secret-value \
  --secret-id "$RPC_SECRET_ARN" \
  --secret-string "https://your-new-quicknode-endpoint" \
  --region "$AWS_REGION"
```

### Step 2: Restart the indexer

```bash
aws ecs update-service --cluster payday --service indexer \
  --force-new-deployment --region "$AWS_REGION"
```

## Rotate the database password

Database password rotation requires updating the RDS instance and the
Secrets Manager secret, then restarting both services. This should be
planned carefully as it causes a brief downtime.

1. Update the RDS master password:

   ```bash
   NEW_PASSWORD=$(openssl rand -base64 32 | tr -d '/+=' | head -c 32)

   aws rds modify-db-instance \
     --db-instance-identifier payday \
     --master-user-password "$NEW_PASSWORD" \
     --apply-immediately \
     --region "$AWS_REGION"
   ```

2. Update the Secrets Manager secret with the new connection string:

   ```bash
   DB_SECRET_ARN="$(terraform -chdir=infra output -raw database_url_secret_arn)"

   # Build the new connection URL with the new password
   NEW_DB_URL="postgresql://gateway:${NEW_PASSWORD}@<RDS_ENDPOINT>:5432/gateway?sslmode=verify-full&sslrootcert=/usr/local/share/ca-certificates/aws-rds-global-bundle.pem"

   aws secretsmanager put-secret-value \
     --secret-id "$DB_SECRET_ARN" \
     --secret-string "$NEW_DB_URL" \
     --region "$AWS_REGION"
   ```

3. Wait for the RDS modification to complete, then restart both services:

   ```bash
   aws ecs update-service --cluster payday --service api \
     --force-new-deployment --region "$AWS_REGION"
   aws ecs update-service --cluster payday --service indexer \
     --force-new-deployment --region "$AWS_REGION"
   ```

## Notes

- Terraform state contains secret values in plaintext despite `sensitive`
  markings. Rotating a secret outside Terraform creates state drift — run
  `terraform plan` afterward and import/update the secret version as needed.
- The API key must be at least 32 bytes.
- There is no overlapping-key replacement. Plan it during a low-traffic window.
