#!/usr/bin/env bash
# Run `gum-server migrate` once as an ECS task and wait for it to exit 0.
#
# Usage: scripts/run-migrate-task.sh <environment-name>
#
# Reads the cluster, task definition family, subnets and security group
# from the Terraform outputs in infra/, so the task runs with exactly the
# credentials and network the api tasks have. Exits non-zero, with the
# task's log lines, if the migration fails; the deploy then stops before
# any service has rolled.
set -euo pipefail

if (($# != 1)); then
  echo "usage: $0 <environment-name>" >&2
  exit 2
fi
name=$1
repo_root=$(git -C "$(dirname "${BASH_SOURCE[0]}")/.." rev-parse --show-toplevel)

output() { terraform -chdir="$repo_root/infra" output -raw "$1"; }
family=$(output migrate_task_definition)
subnets=$(terraform -chdir="$repo_root/infra" output -json public_subnet_ids | jq -r 'join(",")')
security_group=$(output api_security_group_id)

echo "running $family on cluster $name"
task_arn=$(aws ecs run-task \
  --cluster "$name" \
  --task-definition "$family" \
  --launch-type FARGATE \
  --network-configuration "awsvpcConfiguration={subnets=[$subnets],securityGroups=[$security_group],assignPublicIp=ENABLED}" \
  --query 'tasks[0].taskArn' --output text)
echo "task $task_arn started; waiting"
aws ecs wait tasks-stopped --cluster "$name" --tasks "$task_arn"

exit_code=$(aws ecs describe-tasks --cluster "$name" --tasks "$task_arn" \
  --query 'tasks[0].containers[0].exitCode' --output text)
task_id=${task_arn##*/}
aws logs tail "/ecs/$name/api" --log-stream-names "migrate/migrate/$task_id" --since 30m || true
if [[ "$exit_code" != "0" ]]; then
  echo "migration task exited with $exit_code" >&2
  exit 1
fi
echo "migrations applied"
