# Service restart

Restart the API or indexer ECS service. This forces a new task deployment with
the current task definition. Use this when a container is unhealthy, crashed,
or needs to pick up new environment configuration.

## Restart the indexer

```bash
aws ecs update-service --cluster payday --service indexer \
  --force-new-deployment --region "$AWS_REGION"
```

The old task receives a SIGTERM and drains. The new task starts after the old
one stops, ensuring only one indexer holds the database advisory lock at a time.

Verify the new task is running:

```bash
# Wait ~30 seconds, then check:
aws ecs describe-services --cluster payday --service indexer \
  --region "$AWS_REGION" \
  --query 'services[0].{Running:runningCount,Desired:desiredCount,Events:events[:3]}' \
  --output json
```

Check startup logs:

```bash
aws logs tail /ecs/payday/indexer --since 2m --region "$AWS_REGION"
```

You should see:
- `database migrations applied`
- `exclusive indexer database lock acquired`
- `configured sweep signer address=0x... signer="aws-kms"`
- `block indexer started`
- `sweep worker started`

## Restart the API

```bash
aws ecs update-service --cluster payday --service api \
  --force-new-deployment --region "$AWS_REGION"
```

Verify:

```bash
# Wait ~30 seconds:
curl -sf "$GATEWAY_API_URL/health" && echo " OK" || echo " FAIL"
```

## Restart with a new task definition

If you updated the task definition (e.g. changed an environment variable via
Terraform), you must update the service to use the new revision:

```bash
# Find the latest revision
aws ecs list-task-definitions --family-prefix payday-indexer \
  --region "$AWS_REGION" --sort DESC --max-items 1 --output text

# Update the service to use it (replace :N with the revision number)
aws ecs update-service --cluster payday --service indexer \
  --task-definition payday-indexer:N --region "$AWS_REGION"
```

A `--force-new-deployment` alone only restarts with the *same* task definition
revision. To pick up task definition changes, you must specify the new revision.
