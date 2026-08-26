# Secrets rotation

How to rotate the API key and other secrets stored in AWS Secrets Manager.

## Rotate the API key

### Step 1: Generate a new key

```bash
NEW_KEY=$(openssl rand -hex 32)
echo "New key generated (not shown here for security)"
```

### Step 2: Update Secrets Manager

```bash
API_KEY_SECRET_ARN="$(terraform -chdir=infra output -raw api_key_secret_arn)"

aws secretsmanager put-secret-value \
  --secret-id "$API_KEY_SECRET_ARN" \
  --secret-string "$NEW_KEY" \
  --region "$AWS_REGION"
```

### Step 3: Restart the API service

The API reads the secret at task startup. Restart to pick up the new value:

```bash
aws ecs update-service --cluster payday --service api \
  --force-new-deployment --region "$AWS_REGION"
```

### Step 4: Update your local environment

```bash
export GATEWAY_API_KEY="$NEW_KEY"
```

Any CLI clients or integrations using the old key will stop working
immediately. Distribute the new key through your secret management process
before rotating.

### Step 5: Verify

```bash
curl -sf -H "Authorization: Bearer $GATEWAY_API_KEY" \
  "$GATEWAY_API_URL/health" && echo " OK" || echo " FAIL"
```

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
- There is no overlapping-key rotation in this single-operator setup. Plan
  rotation during a low-traffic window.
