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
pids=()
postgres_started=false

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
  for _ in {1..60}; do
    docker exec "$container" pg_isready -U payday -d gateway >/dev/null 2>&1 && break
    sleep .25
  done
  docker exec "$container" pg_isready -U payday -d gateway >/dev/null 2>&1 || {
    docker logs "$container" >&2
    echo "PostgreSQL did not become ready" >&2
    exit 1
  }
  export DATABASE_URL="postgresql://payday:${postgres_password}@127.0.0.1:${pg_port}/gateway"
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
  export PAYDAY_AUTH0_CLIENT_ID="${PAYDAY_AUTH0_CLIENT_ID:-payday-cli-local}"
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
  export PAYDAY_PUBLIC_BASE_URL="${PAYDAY_PUBLIC_BASE_URL:-$PAYDAY_API_URL}"
  export PAYDAY_API_KEY_PREFIX="${PAYDAY_API_KEY_PREFIX:-payday_test_}"
  export PAYDAY_ADMIN_BEARER_SECRET="${PAYDAY_ADMIN_BEARER_SECRET:-local-admin-bearer-secret-0123456789abcdef}"
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

seed() {
  [[ -x target/debug/payday ]] || cargo build --locked -p gateway-cli
  # The local profile is the public identity/bootstrap boundary. Keep this
  # wrapper independent of its implementation (and permit early overrides).
  if [[ -n "${PAYDAY_LOCAL_SEED_COMMAND:-}" ]]; then
    bash -c "$PAYDAY_LOCAL_SEED_COMMAND"
  else
    target/debug/payday --profile local login --yes
  fi
}

if [[ "$mode" == seed ]]; then
  load_local_env
  seed
  exit
fi

start_postgres
load_local_env
# DATABASE_URL must always address the managed container, never a value in .env.
export DATABASE_URL="postgresql://payday:${postgres_password}@127.0.0.1:${pg_port}/gateway"

if [[ "$mode" == e2e ]]; then
  need cargo; need anvil; need cast; need forge; need jq; need psql
  cargo build --locked --workspace
  ./scripts/e2e-anvil.sh
  exit
fi

need cargo; need anvil; need cast; need forge; need curl
cargo build --locked --workspace
prefix postgres docker logs -f "$container"
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
echo "[runner] ready: API $PAYDAY_API_URL; run 'just seed' in another shell"
echo "[runner] Ctrl-C stops services and removes the local database"
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
