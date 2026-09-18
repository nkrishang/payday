#!/usr/bin/env bash
set -euo pipefail

if (($# != 5)); then
  echo "usage: $0 <aws-region> <server-ecr-url> <indexer-ecr-url> <signers-ecr-url> <immutable-tag>" >&2
  exit 2
fi

region=$1
server_repository=$2
indexer_repository=$3
signers_repository=$4
tag=$5
registry=${server_repository%%/*}
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

for repository in "$indexer_repository" "$signers_repository"; do
  if [[ ${repository%%/*} != "$registry" ]]; then
    echo "all three repositories must use the same ECR registry" >&2
    exit 2
  fi
done

aws ecr get-login-password --region "$region" \
  | docker login --username AWS --password-stdin "$registry"

# One build context, three targets: the builder stage is shared, so the
# three images carry binaries from the same compilation.
docker build --platform linux/amd64 --target gum-server \
  --tag "$server_repository:$tag" .
docker build --platform linux/amd64 --target gum-indexer \
  --tag "$indexer_repository:$tag" .
docker build --platform linux/amd64 --target gum-signers \
  --tag "$signers_repository:$tag" .

docker push "$server_repository:$tag"
docker push "$indexer_repository:$tag"
docker push "$signers_repository:$tag"

echo "pushed server, indexer and signers images with tag $tag"
