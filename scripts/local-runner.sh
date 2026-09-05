#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

mode="${1:-}"
case "$mode" in dev|e2e|seed) ;; *) echo "usage: $0 {dev|e2e|seed}" >&2; exit 2 ;; esac

container="payday-postgres-${mode}-${PPID}-$$"
pg_port="${PAYDAY_POSTGRES_PORT:-55432}"
postgres_image="${PAYDAY_POSTGRES_IMAGE:-postgres:16-alpine}"
postgres_password="${PAYDAY_POSTGRES_PASSWORD:-payday-local}"
minio_container="payday-minio-${mode}-${PPID}-$$"
minio_port="${PAYDAY_MINIO_PORT:-9000}"
minio_image="${PAYDAY_MINIO_IMAGE:-minio/minio}"
mc_image="${PAYDAY_MC_IMAGE:-minio/mc}"
# MinIO's root credentials double as the AWS credentials gatewayd signs with.
minio_credential="payday-local"
attachment_bucket="payday-attachments-local"
pids=()
postgres_started=false
minio_started=false

cleanup() {
  status=$?
  trap - EXIT INT TERM
  if ((${#pids[@]})); then
    kill "${pids[@]}" 2>/dev/null || true
    wait "${pids[@]}" 2>/dev/null || true
  fi
  if [[ "$postgres_started" == true ]]; then
    docker rm -f "$container" >/dev/null 2>&1 || true
  fi
  if [[ "$minio_started" == true ]]; then
    docker rm -f "$minio_container" >/dev/null 2>&1 || true
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

need() {
  command -v "$1" >/dev/null 2>&1 || { echo "required command not found: $1" >&2; exit 1; }
}

start_postgres() {
  need docker
  docker info >/dev/null 2>&1 || { echo "Docker daemon is not available" >&2; exit 1; }
  echo "[runner] starting PostgreSQL ($postgres_image) on port $pg_port"
  docker run -d --rm --name "$container" \
    -e POSTGRES_USER=payday -e POSTGRES_PASSWORD="$postgres_password" -e POSTGRES_DB=gateway \
    -p "127.0.0.1:${pg_port}:5432" "$postgres_image" >/dev/null
  postgres_started=true
  # Probe over TCP: the image's bootstrap runs a temporary server on the Unix
  # socket only, which a socket probe would mistake for the real one.
  for _ in {1..120}; do
    docker exec "$container" pg_isready -h 127.0.0.1 -U payday -d gateway >/dev/null 2>&1 && break
    sleep .25
  done
  docker exec "$container" pg_isready -h 127.0.0.1 -U payday -d gateway >/dev/null 2>&1 || {
    docker logs "$container" >&2
    echo "PostgreSQL did not become ready" >&2
    exit 1
  }
  export DATABASE_URL="postgresql://payday:${postgres_password}@127.0.0.1:${pg_port}/gateway"
}

# MinIO stands in for the S3 attachment bucket, and for GuardDuty: nothing
# scans local uploads, so the verdict tag finalize insists on is set by hand
# (docs/local-development.md). Called after .env is loaded so its exports win,
# like DATABASE_URL, because the runner owns this container.
start_minio() {
  need curl
  echo "[runner] starting MinIO ($minio_image) on port $minio_port"
  docker run -d --rm --name "$minio_container" \
    -e MINIO_ROOT_USER="$minio_credential" -e MINIO_ROOT_PASSWORD="$minio_credential" -e MINIO_BROWSER=off \
    -p "127.0.0.1:${minio_port}:9000" "$minio_image" server /data >/dev/null
  minio_started=true
  for _ in {1..100}; do
    curl -fsS "http://127.0.0.1:${minio_port}/minio/health/live" >/dev/null 2>&1 && break
    sleep .1
  done
  curl -fsS "http://127.0.0.1:${minio_port}/minio/health/live" >/dev/null 2>&1 || {
    docker logs "$minio_container" >&2
    echo "MinIO did not become ready" >&2
    exit 1
  }
  docker run --rm --network host \
    -e "MC_HOST_local=http://${minio_credential}:${minio_credential}@127.0.0.1:${minio_port}" \
    "$mc_image" mb --ignore-existing "local/${attachment_bucket}" >/dev/null
  export PAYDAY_ATTACHMENT_BUCKET="$attachment_bucket"
  export PAYDAY_ATTACHMENT_S3_ENDPOINT="http://127.0.0.1:${minio_port}"
  export PAYDAY_ATTACHMENT_S3_FORCE_PATH_STYLE=1
  export AWS_ACCESS_KEY_ID="$minio_credential"
  export AWS_SECRET_ACCESS_KEY="$minio_credential"
  export AWS_REGION=us-east-1
}

# Nothing scans local uploads (see start_minio, above), so this stands in for
# GuardDuty during an interactive `just dev` session too: a few seconds after
# a PDF lands, it stamps the clean verdict finalize is waiting on, so a manual
# upload through the browser does not have to sit out finalize's 2-minute poll
# timeout. It never overwrites a verdict already present, so stamping one by
# hand first (docs/local-development.md#attachments-and-the-scan-tag) still
# walks the rejection path; scripted rejection coverage lives in `just e2e`.
start_scan_stub() {
  # A plain space-delimited string, not an associative array: the macOS
  # system bash (3.2, still /usr/bin/env bash's target on a bare checkout)
  # has no declare -A.
  local seen=" "
  local mc_env="MC_HOST_local=http://${minio_credential}:${minio_credential}@127.0.0.1:${minio_port}"
  while :; do
    local listing object tags
    listing="$(docker run --rm --network host -e "$mc_env" "$mc_image" ls -r "local/${attachment_bucket}" 2>/dev/null)" || true
    while IFS= read -r object; do
      [[ -n "$object" ]] || continue
      case "$seen" in *" $object "*) continue ;; esac
      tags="$(docker run --rm --network host -e "$mc_env" "$mc_image" tag list "local/${attachment_bucket}/${object}" 2>/dev/null)" || true
      if [[ "$tags" != *GuardDutyMalwareScanStatus* ]]; then
        docker run --rm --network host -e "$mc_env" "$mc_image" \
          tag set "local/${attachment_bucket}/${object}" 'GuardDutyMalwareScanStatus=NO_THREATS_FOUND' >/dev/null 2>&1 || true
        echo "stamped a clean scan verdict on $object"
      fi
      seen="${seen}${object} "
    done < <(awk '{print $NF}' <<< "$listing")
    sleep 2
  done
}

prefix() {
  local name=$1
  shift
  "$@" > >(sed -u "s/^/[$name] /") 2> >(sed -u "s/^/[$name] /" >&2) &
  pids+=("$!")
}

load_local_env() {
  if [[ -f .env ]]; then
    # A .env that does not parse would abort the sourcing shell, and the EXIT
    # trap cannot see that failure, so the runner would report success having
    # started nothing. Refuse it explicitly instead.
    bash -n .env || { echo ".env does not parse as shell; fix it before running the local stack" >&2; exit 1; }
    set -a
    # shellcheck disable=SC1091
    source .env
    set +a
  fi
  export PAYDAY_CHAIN_ID="${PAYDAY_CHAIN_ID:-31337}"
  export PAYDAY_RPC_URL="${PAYDAY_RPC_URL:-http://127.0.0.1:8545}"
  export PAYDAY_API_URL="${PAYDAY_API_URL:-http://127.0.0.1:3000}"
  export PAYDAY_DEV_IDENTITY=1
  export PAYDAY_AUTH0_ISSUER="${PAYDAY_AUTH0_ISSUER:-http://127.0.0.1:3001}"
  export PAYDAY_AUTH0_CLIENT_ID="${PAYDAY_AUTH0_CLIENT_ID:-payday-dashboard-local}"
  export PAYDAY_AUTH0_AUDIENCE="${PAYDAY_AUTH0_AUDIENCE:-payday-api-local}"
  export PAYDAY_DEV_IDENTITY_ISSUER="$PAYDAY_AUTH0_ISSUER"
  export PAYDAY_FACTORY_ADDRESS="${PAYDAY_FACTORY_ADDRESS:-0x5FbDB2315678afecb367f032d93F642f64180aa3}"
  export PAYDAY_USDC_ADDRESS="${PAYDAY_USDC_ADDRESS:-0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512}"
  export PAYDAY_BATCH_SWEEPER_ADDRESS="${PAYDAY_BATCH_SWEEPER_ADDRESS:-0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0}"
  # Payday's local recovery wallet fixture: Anvil account #5, the same address
  # scripts/e2e-anvil.sh asserts recovered balances against.
  export PAYDAY_RECOVERY_ADDRESS="${PAYDAY_RECOVERY_ADDRESS:-0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc}"
  export PAYDAY_USDC_START_BLOCK="${PAYDAY_USDC_START_BLOCK:-0}"
  export PAYDAY_FINALITY_SOURCE="${PAYDAY_FINALITY_SOURCE:-finalized}"
  export PAYDAY_FINALITY_CONFIRMATIONS="${PAYDAY_FINALITY_CONFIRMATIONS:-0}"
  export PAYDAY_INDEXER_POLL_INTERVAL_MS="${PAYDAY_INDEXER_POLL_INTERVAL_MS:-1000}"
  export PAYDAY_SIGNER_KEY="${PAYDAY_SIGNER_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}"
  # The web dev server hosts both dashboard and checkout; gatewayd remains on
  # PAYDAY_API_URL and is called cross-origin by the browser.
  export PAYDAY_PUBLIC_BASE_URL="${PAYDAY_PUBLIC_BASE_URL:-http://127.0.0.1:3002}"
  export PAYDAY_API_KEY_PREFIX="${PAYDAY_API_KEY_PREFIX:-payday_test_}"
  export PAYDAY_ADMIN_BEARER_SECRET="${PAYDAY_ADMIN_BEARER_SECRET:-local-admin-bearer-secret-0123456789abcdef}"
  export PAYDAY_ADMIN_REVIEWER_ID="${PAYDAY_ADMIN_REVIEWER_ID:-local-operator}"
  # Proof of Payment attestations are signed with Anvil account #6
  # (0x976EA74026E726554dB657fA54763abd0C3a0aa9), the trusted attestor for
  # local proof verification; production signs with a KMS key instead.
  export PAYDAY_ATTESTATION_SIGNER_KEY="${PAYDAY_ATTESTATION_SIGNER_KEY:-0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e}"
  # Pays the onboarding walkthrough's one self-issued deposit request, so a
  # brand new merchant sees a real transfer settle before doing anything else.
  # Anvil account #1 (0x70997970C51812dc3A010C7d01b50e0d17dc79C8), already
  # minted 1,000,000 test USDC by Bootstrap.s.sol and otherwise unused
  # locally — a different account than PAYDAY_SIGNER_KEY so the two never
  # contend for a nonce.
  export PAYDAY_ONBOARDING_PAYER_KEY="${PAYDAY_ONBOARDING_PAYER_KEY:-0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d}"
  # Payer email verification against the same development provider, which
  # serves the payer client and audience next to the merchant ones.
  export PAYDAY_PAYER_AUTH0_ISSUER="${PAYDAY_PAYER_AUTH0_ISSUER:-$PAYDAY_AUTH0_ISSUER}"
  export PAYDAY_PAYER_AUTH0_AUDIENCE="${PAYDAY_PAYER_AUTH0_AUDIENCE:-payday-payer-local}"
  export PAYDAY_PAYER_AUTH0_CLIENT_ID="${PAYDAY_PAYER_AUTH0_CLIENT_ID:-payday-payer-local}"
  # A fixed local key: payer references derived here never leave the developer's database.
  export PAYDAY_PAYER_REF_MASTER_KEY="${PAYDAY_PAYER_REF_MASTER_KEY:-AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=}"
  # The Next.js dev server (`just web`) is the hosted checkout locally.
  export PAYDAY_HOSTED_CHECKOUT_ORIGIN="${PAYDAY_HOSTED_CHECKOUT_ORIGIN:-$PAYDAY_PUBLIC_BASE_URL}"
}

# Both services compare the deployed runtime bytecode with these hashes at
# startup and refuse to start on a mismatch. The running chain is the truth for
# the contract generation, so they are always computed from it here, overriding
# anything .env carries, once the bootstrap has deployed the fixtures.
pin_deployment_code_hashes() {
  local factory_code sweeper_code
  factory_code="$(cast code "$PAYDAY_FACTORY_ADDRESS" --rpc-url "$PAYDAY_RPC_URL")"
  sweeper_code="$(cast code "$PAYDAY_BATCH_SWEEPER_ADDRESS" --rpc-url "$PAYDAY_RPC_URL")"
  [[ -n "$factory_code" && "$factory_code" != 0x ]] || {
    echo "no code at PAYDAY_FACTORY_ADDRESS $PAYDAY_FACTORY_ADDRESS; the bootstrap did not deploy PaymentFactory" >&2
    exit 1
  }
  [[ -n "$sweeper_code" && "$sweeper_code" != 0x ]] || {
    echo "no code at PAYDAY_BATCH_SWEEPER_ADDRESS $PAYDAY_BATCH_SWEEPER_ADDRESS; the bootstrap did not deploy BatchSweeper" >&2
    exit 1
  }
  PAYDAY_FACTORY_CODE_HASH="$(cast keccak "$factory_code")"
  PAYDAY_BATCH_SWEEPER_CODE_HASH="$(cast keccak "$sweeper_code")"
  export PAYDAY_FACTORY_CODE_HASH PAYDAY_BATCH_SWEEPER_CODE_HASH
  echo "[bootstrap] factory code hash $PAYDAY_FACTORY_CODE_HASH"
  echo "[bootstrap] batch sweeper code hash $PAYDAY_BATCH_SWEEPER_CODE_HASH"
}

# Create a local account and print its API key once, through the same
# email-OTP exchange the dashboard makes; the code is in the [identity] log.
seed() {
  ./scripts/local-api-key.sh "$@"
}

if [[ "$mode" == seed ]]; then
  shift
  load_local_env
  seed "$@"
  exit
fi

start_postgres
load_local_env
# DATABASE_URL must always address the managed container, never a value in .env.
export DATABASE_URL="postgresql://payday:${postgres_password}@127.0.0.1:${pg_port}/gateway"

if [[ "$mode" == e2e ]]; then
  need cargo; need anvil; need cast; need forge; need jq; need psql
  cargo build --locked --workspace
  # The suite starts its own MinIO next to the processes it manages.
  ./scripts/e2e-anvil.sh
  exit
fi

need cargo; need anvil; need cast; need forge; need curl
cargo build --locked --workspace
start_minio
prefix postgres docker logs -f "$container"
prefix minio docker logs -f "$minio_container"
prefix scan-stub start_scan_stub
prefix anvil anvil --chain-id "$PAYDAY_CHAIN_ID" --slots-in-an-epoch 1 --mixed-mining --block-time 1
for _ in {1..100}; do
  cast chain-id --rpc-url "$PAYDAY_RPC_URL" >/dev/null 2>&1 && break
  sleep .1
done
cast chain-id --rpc-url "$PAYDAY_RPC_URL" >/dev/null 2>&1 || { echo "Anvil did not become ready" >&2; exit 1; }
echo "[bootstrap] deploying deterministic local fixtures"
forge script foundry/script/Bootstrap.s.sol:BootstrapScript \
  --rpc-url "$PAYDAY_RPC_URL" --private-key "$PAYDAY_SIGNER_KEY" --broadcast
pin_deployment_code_hashes
prefix identity ./target/debug/payday-dev-identity
for _ in {1..100}; do
  curl -fsS "$PAYDAY_AUTH0_ISSUER/.well-known/jwks.json" >/dev/null 2>&1 && break
  sleep .1
done
curl -fsS "$PAYDAY_AUTH0_ISSUER/.well-known/jwks.json" >/dev/null || {
  echo "development identity provider did not become ready" >&2
  exit 1
}
prefix gatewayd ./target/debug/gatewayd
for _ in {1..100}; do
  curl -fsS "$PAYDAY_API_URL/health" >/dev/null 2>&1 && break
  sleep .1
done
curl -fsS "$PAYDAY_API_URL/health" >/dev/null || { echo "gatewayd did not become ready" >&2; exit 1; }
prefix indexer ./target/debug/gateway-indexer
echo "[runner] ready: API $PAYDAY_API_URL; sign in at the dashboard, or run 'just seed' in another shell for an API key"
echo "[runner] Ctrl-C stops services and removes the local database and attachment store"
while :; do
  for pid in "${pids[@]}"; do
    kill -0 "$pid" 2>/dev/null || {
      wait "$pid" || true
      echo "[runner] a service exited; stopping the local stack" >&2
      exit 1
    }
  done
  sleep 1
done
