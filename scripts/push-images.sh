#!/usr/bin/env bash
set -euo pipefail

if (($# != 4)); then
  echo "usage: $0 <aws-region> <api-ecr-url> <indexer-ecr-url> <immutable-tag>" >&2
  exit 2
fi

region=$1
api_repository=$2
indexer_repository=$3
tag=$4
registry=${api_repository%%/*}
repo_root=$(git -C "$(dirname "${BASH_SOURCE[0]}")/.." rev-parse --show-toplevel)
cd "$repo_root"

expected_tag="git-$(git rev-parse HEAD)"
if [[ $tag != "$expected_tag" ]]; then
  echo "tag must exactly match the checked-out commit: $expected_tag" >&2
  exit 2
fi

if [[ -n $(git status --porcelain --untracked-files=normal) ]]; then
  echo "refusing to build from a dirty worktree; commit or remove every change first" >&2
  exit 2
fi

if [[ ${indexer_repository%%/*} != "$registry" ]]; then
  echo "API and indexer repositories must use the same ECR registry" >&2
  exit 2
fi

aws ecr get-login-password --region "$region" \
  | docker login --username AWS --password-stdin "$registry"

docker build --platform linux/amd64 --target gatewayd \
  --tag "$api_repository:$tag" .
docker build --platform linux/amd64 --target gateway-indexer \
  --tag "$indexer_repository:$tag" .

docker push "$api_repository:$tag"
docker push "$indexer_repository:$tag"

echo "pushed API and indexer images with tag $tag"
