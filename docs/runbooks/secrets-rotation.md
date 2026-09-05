# Secrets rotation

How to rotate an account API key and infrastructure secrets.

## Rotate the API key

API keys are rolled from the dashboard: sign in at `payday.sh`, open the
**API key** section, and choose **Roll key**. The session is the credential —
an API key cannot roll itself, and the CLI's Auth0 login is no longer accepted
by the API.

The CLI asks for confirmation (`-y` skips it), saves the replacement securely,
and masks it by default. Use `--show` only to transfer it directly to an approved
secret manager. The previous key remains valid for 24 hours:

```bash
./target/release/payday keys rotate --show -y
```

Store and distribute the new key through the account owner's approved secret
management process. The service does not retain recoverable plaintext and does
not need a restart. Verify the new key against a payment owned by this account:

```bash
./target/release/payday get <PAYMENT_ID>
```

CI should receive `PAYDAY_API_KEY` from its secret store rather than performing
an OTP login. Once consumers migrate, the prior key expires after 24 hours.

## Revoke API keys

For compromise or decommissioning, `payday keys revoke` immediately invalidates
the current and grace-period keys and removes the saved credential. Use `-y` to
skip confirmation. `payday logout` only removes the local profile.

## Rotate the Resend API key

The Resend API key is held by Auth0, not by Payday's services or Terraform.
Create a replacement sending-only key in Resend, update **Branding → Email
Provider** in Auth0, and send a test email before revoking the old key. Then run
one complete staging email-OTP login. A failed rotation prevents login but does
not affect existing Payday API keys.

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
