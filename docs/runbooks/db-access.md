# Database access for operations

The RDS PostgreSQL instance is in a private subnet with no public accessibility.
You cannot connect to it directly from your laptop. Use a one-off Fargate task
with a Postgres image to run SQL inside the VPC.

## Prerequisites

You need the database URL from Secrets Manager:

```bash
DB_SECRET_ARN="$(terraform -chdir=infra output -raw database_url_secret_arn)"
DB_URL=$(aws secretsmanager get-secret-value \
  --secret-id "$DB_SECRET_ARN" \
  --query SecretString --output text --region "$AWS_REGION")
```

And the VPC networking details (subnets and security groups). Get them from
Terraform state:

```bash
cd infra
terraform show -json | python3 -c "
import sys, json
d = json.load(sys.stdin)
for r in d.get('values',{}).get('root_module',{}).get('resources',[]):
    addr = r['address']
    vals = r.get('values',{})
    if 'subnet.public' in addr:
        print(f'{addr}: {vals[\"id\"]}')
    if 'security_group.indexer' in addr:
        print(f'{addr}: {vals[\"id\"]}')
"
```

Use the **public subnets** (for Fargate image pull) and the **indexer security
group** (which has DB access).

## Method: One-off Fargate task with Postgres image

### Step 1: Register a minimal task definition (one-time)

```bash
cat > /tmp/db-access-task.json << 'ENDJSON'
{
  "family": "payday-db-access",
  "networkMode": "awsvpc",
  "requiresCompatibilities": ["FARGATE"],
  "cpu": "256",
  "memory": "512",
  "containerDefinitions": [
    {
      "name": "db-access",
      "image": "postgres:15-alpine",
      "essential": true
    }
  ]
}
ENDJSON

aws ecs register-task-definition \
  --cli-input-json file:///tmp/db-access-task.json \
  --region "$AWS_REGION"
```

### Step 2: Write the SQL command to a JSON overrides file

Use `decode()` instead of `\x` hex literals to avoid shell escaping issues:

```bash
# Example: read the indexer cursor
cat > /tmp/db-access-overrides.json << 'ENDJSON'
{
  "containerOverrides": [
    {
      "name": "db-access",
      "command": ["psql", "YOUR_DB_URL_WITH_SSLMODE_REQUIRE", "-c", "SELECT chain_id, last_block, encode(last_block_hash, 'hex') FROM indexer_cursor;"]
    }
  ]
}
ENDJSON
```

> **Note:** Replace `sslmode=verify-full` with `sslmode=require` and remove the
> `sslrootcert` parameter — the `postgres:15-alpine` image doesn't have the
> AWS RDS CA bundle.

### Step 3: Run the task

```bash
aws ecs run-task \
  --cluster payday \
  --launch-type FARGATE \
  --task-definition payday-db-access:1 \
  --count 1 \
  --region "$AWS_REGION" \
  --network-configuration "awsvpcConfiguration={subnets=[SUBNET_A,SUBNET_B],securityGroups=[INDEXER_SG],assignPublicIp=ENABLED}" \
  --overrides file:///tmp/db-access-overrides.json \
  --query 'tasks[0].{taskArn:taskArn,status:lastStatus}' --output json
```

### Step 4: Wait for it to complete

```bash
TASK_ID="<task ID from the ARN in step 3>"

# Wait ~30 seconds, then check:
aws ecs describe-tasks --cluster payday --tasks "$TASK_ID" \
  --region "$AWS_REGION" \
  --query 'tasks[0].{status:lastStatus,exitCode:containers[0].exitCode}' \
  --output json
```

Exit code `0` means the SQL executed successfully. The output goes to
CloudWatch logs:

```bash
aws logs tail /ecs/payday/db-access --since 5m --region "$AWS_REGION"
```

> If no log group exists for `payday-db-access`, the task output won't be
> captured. For important operations, create a log group first or check the
> task's `stoppedReason`.

## Alternative: Enable ECS Exec

If you need an interactive shell, enable ECS Exec on the indexer service:

```bash
aws ecs update-service --cluster payday --service indexer \
  --enable-execute-command --region "$AWS_REGION"
aws ecs update-service --cluster payday --service indexer \
  --force-new-deployment --region "$AWS_REGION"
```

Then exec into the running indexer container:

```bash
# Requires the Session Manager plugin installed locally:
# brew install --cask session-manager-plugin

TASK_ARN=$(aws ecs list-tasks --cluster payday --service-name indexer \
  --desired-status RUNNING --region "$AWS_REGION" --output text --query 'taskArns[0]')

aws ecs execute-command --cluster payday --task "$TASK_ARN" \
  --container indexer --command "sh" --interactive --region "$AWS_REGION"
```

The indexer image is minimal (no psql), but you can install it:

```sh
apk add --no-cache postgresql-client
psql "$DB_URL" -c "SELECT last_block FROM indexer_cursor WHERE chain_id = 143;"
```

Note: ECS Exec requires the Session Manager plugin. On macOS:
`brew install --cask session-manager-plugin` (requires sudo).

## Common SQL queries

### Check indexer cursor

```sql
SELECT chain_id, last_block, encode(last_block_hash, 'hex') AS last_block_hash
FROM indexer_cursor;
```

### Check deposit request statuses

```sql
SELECT id, status, amount_base_units, payment_address, created_at
FROM invoices
ORDER BY created_at DESC
LIMIT 10;
```

### Check deposit observations

```sql
SELECT block_number, sender, recipient, amount, status
FROM payment_observations
ORDER BY block_number DESC
LIMIT 10;
```

### Reset cursor (use with caution — see indexer-cursor-reset.md)

```sql
INSERT INTO indexer_cursor (chain_id, token_address, last_block, last_block_hash)
VALUES (143, decode('754704bc059f8c67012fed69bc8a327a5aafb603', 'hex'),
  <BLOCK_NUMBER>, decode('<BLOCK_HASH_NO_0X>', 'hex'))
ON CONFLICT (chain_id) DO UPDATE SET
  last_block = <BLOCK_NUMBER>,
  last_block_hash = decode('<BLOCK_HASH_NO_0X>', 'hex'),
  updated_at = now();
```
