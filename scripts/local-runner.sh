#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

mode="${1:-}"
case "$mode" in dev|e2e|seed) ;; *) echo "usage: $0 {dev|e2e|seed}" >&2; exit 2 ;; esac

container="gum-postgres-${mode}-${PPID}-$$"
pg_port="${GUM_POSTGRES_PORT:-55432}"
postgres_image="${GUM_POSTGRES_IMAGE:-postgres:16-alpine}"
postgres_password="${GUM_POSTGRES_PASSWORD:-gum-local}"
minio_container="gum-minio-${mode}-${PPID}-$$"
minio_port="${GUM_MINIO_PORT:-9000}"
# MinIO removed its Docker Hub images; quay.io is the official registry now.
minio_image="${GUM_MINIO_IMAGE:-quay.io/minio/minio}"
mc_image="${GUM_MC_IMAGE:-quay.io/minio/mc}"
# MinIO's root credentials double as the AWS credentials gum-server signs with.
minio_credential="gum-local"
attachment_bucket="gum-attachments-local"
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
    -e POSTGRES_USER=gum -e POSTGRES_PASSWORD="$postgres_password" -e POSTGRES_DB=gateway \
    -p "127.0.0.1:${pg_port}:5432" "$postgres_image" >/dev/null
  postgres_started=true
  # Probe over TCP: the image's bootstrap runs a temporary server on the Unix
  # socket only, which a socket probe would mistake for the real one.
  for _ in {1..120}; do
    docker exec "$container" pg_isready -h 127.0.0.1 -U gum -d gateway >/dev/null 2>&1 && break
    sleep .25
  done
  docker exec "$container" pg_isready -h 127.0.0.1 -U gum -d gateway >/dev/null 2>&1 || {
    docker logs "$container" >&2
    echo "PostgreSQL did not become ready" >&2
    exit 1
  }
  export DATABASE_URL="postgresql://gum:${postgres_password}@127.0.0.1:${pg_port}/gateway"
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
  export GUM_ATTACHMENT_BUCKET="$attachment_bucket"
  export GUM_ATTACHMENT_S3_ENDPOINT="http://127.0.0.1:${minio_port}"
  export GUM_ATTACHMENT_S3_FORCE_PATH_STYLE=1
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
  # Two local chains, so the checkout's network step has a real choice and
  # the indexer runs one worker per chain: Anvil on 8545 (chain 31337, the
  # Monad-shaped `finalized` path) and Anvil on 8546 (chain 31338, the
  # L2-shaped `latest` plus confirmations path). The
  # services read GUM_CHAINS and GUM_RPC_URL_<chain_id>; GUM_RPC_URL
  # names the first chain for the scripts' own cast calls.
  export GUM_CHAIN_ID="${GUM_CHAIN_ID:-31337}"
  export GUM_RPC_URL="${GUM_RPC_URL:-http://127.0.0.1:8545}"
  export GUM_SECOND_CHAIN_ID="${GUM_SECOND_CHAIN_ID:-31338}"
  export GUM_SECOND_RPC_URL="${GUM_SECOND_RPC_URL:-http://127.0.0.1:8546}"
  export "GUM_RPC_URL_${GUM_CHAIN_ID}=$GUM_RPC_URL"
  export "GUM_RPC_URL_${GUM_SECOND_CHAIN_ID}=$GUM_SECOND_RPC_URL"
  export GUM_API_URL="${GUM_API_URL:-http://127.0.0.1:3000}"
  # The three services. gum-server's internal listener serves the indexer's
  # RPC and its own health; the indexer and the signers each serve health
  # only. The shared bearer token is what makes the indexer's reports
  # trusted; any value works locally.
  export GUM_INTERNAL_BIND_ADDR="${GUM_INTERNAL_BIND_ADDR:-127.0.0.1:3010}"
  export GUM_SERVER_INTERNAL_URL="${GUM_SERVER_INTERNAL_URL:-http://$GUM_INTERNAL_BIND_ADDR}"
  export GUM_INTERNAL_TOKEN="${GUM_INTERNAL_TOKEN:-local-internal-token-0123456789abcdef}"
  export GUM_INDEXER_LISTEN_ADDR="${GUM_INDEXER_LISTEN_ADDR:-127.0.0.1:3011}"
  export GUM_SIGNERS_LISTEN_ADDR="${GUM_SIGNERS_LISTEN_ADDR:-127.0.0.1:3012}"
  # Merchants sign in through Privy, for real, even locally: the dashboard
  # (`just web`) uses the same app id, and gum-server verifies its identity
  # tokens against Privy's published keys. Payer and issuer-mailbox codes
  # come from the loopback development identity provider instead.
  export GUM_PRIVY_APP_ID="${GUM_PRIVY_APP_ID:-cmt9wxn7h011h0cjsma7fzytr}"
  export GUM_DEV_IDENTITY=1
  export GUM_DEV_IDENTITY_ISSUER="${GUM_DEV_IDENTITY_ISSUER:-http://127.0.0.1:3001}"
  # The fixture addresses (Bootstrap.s.sol) are the same on both chains.
  FACTORY="${GUM_FACTORY_ADDRESS:-0x5FbDB2315678afecb367f032d93F642f64180aa3}"
  USDC="${GUM_USDC_ADDRESS:-0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512}"
USDT="${GUM_USDT_ADDRESS:-0xCf7Ed3AccA5a467e9e704C703E8D87F634fB0Fc9}"
  BATCH_SWEEPER="${GUM_BATCH_SWEEPER_ADDRESS:-0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0}"
  export GUM_INDEXER_POLL_INTERVAL_MS="${GUM_INDEXER_POLL_INTERVAL_MS:-1000}"
  # The transfer signal derives ws://127.0.0.1:8545 from the RPC URL; Anvil
  # serves subscriptions on the same port and the signal falls back to the
  # standard `logs` subscription there. The timer backstop stays quick
  # locally so a test never waits a minute on a missed wake.
  export GUM_INDEXER_RECONCILE_INTERVAL_MS="${GUM_INDEXER_RECONCILE_INTERVAL_MS:-1000}"
  # An idle local chain still keeps its clock moving every few seconds so
  # expiry flows do not wait five minutes.
  export GUM_INDEXER_IDLE_INTERVAL_MS="${GUM_INDEXER_IDLE_INTERVAL_MS:-2000}"
  # Anvil has no request budget to trip, so the indexer paces nothing locally.
  export GUM_INDEXER_RPC_MAX_RPS="${GUM_INDEXER_RPC_MAX_RPS:-0}"
  # Anvil account #0 deploys the local fixtures (Bootstrap.s.sol) and is the
  # first sweep signer; mnemonic accounts #10 and #11 (funded because Anvil
  # starts with twelve accounts below) complete a three-key signer pool so
  # several helper transactions can be in flight at once, as in production.
  BOOTSTRAP_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
  export GUM_SIGNER_KEYS="${GUM_SIGNER_KEYS:-$BOOTSTRAP_KEY,0xf214f2b2cd398c806f84e317254e0f0b801d0643303237d97a22a48e01628897,0x701b615bbdfb9de65240bc28bd21bbc0d996645a3dd57e7b12bc2bdf6f192c82}"
  # The web dev server hosts both dashboard and checkout; gum-server remains on
  # GUM_API_URL and is called cross-origin by the browser.
  export GUM_PUBLIC_BASE_URL="${GUM_PUBLIC_BASE_URL:-http://127.0.0.1:3002}"
  export GUM_API_KEY_PREFIX="${GUM_API_KEY_PREFIX:-gum_test_}"
  export GUM_ADMIN_BEARER_SECRET="${GUM_ADMIN_BEARER_SECRET:-local-admin-bearer-secret-0123456789abcdef}"
  export GUM_ADMIN_REVIEWER_ID="${GUM_ADMIN_REVIEWER_ID:-local-operator}"
  # Proof of Payment attestations are signed with Anvil account #6
  # (0x976EA74026E726554dB657fA54763abd0C3a0aa9), the trusted attestor for
  # local proof verification; production signs with a KMS key instead.
  export GUM_ATTESTATION_SIGNER_KEY="${GUM_ATTESTATION_SIGNER_KEY:-0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e}"
  # Pays the onboarding walkthrough's one self-issued deposit request, so a
  # brand new merchant sees a real transfer settle before doing anything else.
  # Anvil account #1 (0x70997970C51812dc3A010C7d01b50e0d17dc79C8), already
  # minted 1,000,000 test USDC by Bootstrap.s.sol and otherwise unused
  # locally — an account outside GUM_SIGNER_KEYS so it never contends
  # with a sweep signer for a nonce.
  export GUM_ONBOARDING_PAYER_KEY="${GUM_ONBOARDING_PAYER_KEY:-0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d}"
  # Payer email verification against the development provider's payer client
  # and audience.
  export GUM_PAYER_AUTH0_ISSUER="${GUM_PAYER_AUTH0_ISSUER:-$GUM_DEV_IDENTITY_ISSUER}"
  export GUM_PAYER_AUTH0_AUDIENCE="${GUM_PAYER_AUTH0_AUDIENCE:-gum-payer-local}"
  export GUM_PAYER_AUTH0_CLIENT_ID="${GUM_PAYER_AUTH0_CLIENT_ID:-gum-payer-local}"
  # A fixed local key: payer references derived here never leave the developer's database.
  export GUM_PAYER_REF_MASTER_KEY="${GUM_PAYER_REF_MASTER_KEY:-AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=}"
  # The Next.js dev server (`just web`) is the hosted checkout locally.
  export GUM_HOSTED_CHECKOUT_ORIGIN="${GUM_HOSTED_CHECKOUT_ORIGIN:-$GUM_PUBLIC_BASE_URL}"
  # Relay is a stand-in locally (scripts/relay-stub.mjs): it quotes a USDC
  # transfer to its solver on the second chain and fills on the first.
  export GUM_RELAY_URL="${GUM_RELAY_URL:-http://127.0.0.1:4020}"
  export GUM_RELAY_API_KEY="${GUM_RELAY_API_KEY:-local}"
}


# One GUM_CHAINS entry for a bootstrapped Anvil: the fixture addresses are
# the same on every Anvil (account #0's first CREATE addresses), and both
# services compare the deployed runtime bytecode with the hashes at startup
# and refuse to start on a mismatch, so the hashes are always read from the
# running chain. Finality is per chain so the second local chain exercises
# the L2-shaped path (`latest` plus confirmations).
chain_entry() {
  local chain_id=$1 rpc_url=$2 finality_source=$3 confirmations=$4 tokens=$5 factory_code sweeper_code
  factory_code="$(cast code "$FACTORY" --rpc-url "$rpc_url")"
  sweeper_code="$(cast code "$BATCH_SWEEPER" --rpc-url "$rpc_url")"
  [[ -n "$factory_code" && "$factory_code" != 0x ]] || {
    echo "no code at PaymentFactory $FACTORY on chain $chain_id; the bootstrap did not deploy it" >&2
    return 1
  }
  [[ -n "$sweeper_code" && "$sweeper_code" != 0x ]] || {
    echo "no code at BatchSweeper $BATCH_SWEEPER on chain $chain_id; the bootstrap did not deploy it" >&2
    return 1
  }
  jq -cn --argjson chain_id "$chain_id" --argjson tokens "$tokens" --arg factory "$FACTORY" \
    --arg sweeper "$BATCH_SWEEPER" --arg factory_hash "$(cast keccak "$factory_code")" \
    --arg sweeper_hash "$(cast keccak "$sweeper_code")" --arg finality "$finality_source" \
    --argjson confirmations "$confirmations" \
    '{chain_id: $chain_id, tokens: $tokens, factory: $factory, batch_sweeper: $sweeper,
      factory_code_hash: $factory_hash, batch_sweeper_code_hash: $sweeper_hash,
      start_block: 0, finality_source: $finality, finality_confirmations: $confirmations,
      block_time_ms: 1000, log_range_size: 100}'
}

# The registry both services read, built from the two bootstrapped chains.
build_chain_registry() {
  local first second
  # The first chain serves USDC and USDT, the second USDC alone, so a USDT
  # request pins the first and the second stands for a chain without it.
  first="$(chain_entry "$GUM_CHAIN_ID" "$GUM_RPC_URL" finalized 0 \
    "$(jq -cn --arg usdc "$USDC" --arg usdt "$USDT" '[{currency: "USDC", address: $usdc}, {currency: "USDT", address: $usdt}]')")"
  second="$(chain_entry "$GUM_SECOND_CHAIN_ID" "$GUM_SECOND_RPC_URL" latest 2 \
    "$(jq -cn --arg usdc "$USDC" '[{currency: "USDC", address: $usdc}]')")"
  GUM_CHAINS="$(jq -cn --argjson first "$first" --argjson second "$second" '[$first, $second]')"
  export GUM_CHAINS
  echo "[bootstrap] GUM_CHAINS=$GUM_CHAINS"
}

wait_for_anvil() {
  local rpc_url=$1
  for _ in {1..100}; do
    cast chain-id --rpc-url "$rpc_url" >/dev/null 2>&1 && return
    sleep .1
  done
  echo "Anvil at $rpc_url did not become ready" >&2
  exit 1
}

# A local account with a key, written straight into the runner's database:
# there is no local Privy to sign in through from a script.
seed() {
  local key
  key="$(./scripts/local-api-key.sh "${SEED_EMAIL:-dev@example.test}")"
  echo "[seed] account ${SEED_EMAIL:-dev@example.test}"
  echo "[seed] GUM_API_KEY=$key"
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
export DATABASE_URL="postgresql://gum:${postgres_password}@127.0.0.1:${pg_port}/gateway"

if [[ "$mode" == e2e ]]; then
  need cargo; need anvil; need cast; need forge; need jq; need psql; need node
  cargo build --locked --workspace
  # The suite starts its own MinIO next to the processes it manages.
  ./scripts/e2e-anvil.sh
  exit
fi

need cargo; need anvil; need cast; need forge; need curl; need jq; need node
cargo build --locked --workspace
start_minio
prefix postgres docker logs -f "$container"
prefix minio docker logs -f "$minio_container"
prefix scan-stub start_scan_stub
prefix anvil anvil --chain-id "$GUM_CHAIN_ID" --port "${GUM_RPC_URL##*:}" --accounts 12 --slots-in-an-epoch 1 --mixed-mining --block-time 1
prefix anvil2 anvil --chain-id "$GUM_SECOND_CHAIN_ID" --port "${GUM_SECOND_RPC_URL##*:}" --accounts 12 --slots-in-an-epoch 1 --mixed-mining --block-time 1
wait_for_anvil "$GUM_RPC_URL"
wait_for_anvil "$GUM_SECOND_RPC_URL"
echo "[bootstrap] deploying deterministic local fixtures on both chains"
for rpc_url in "$GUM_RPC_URL" "$GUM_SECOND_RPC_URL"; do
  forge script foundry/script/Bootstrap.s.sol:BootstrapScript \
    --rpc-url "$rpc_url" --private-key "$BOOTSTRAP_KEY" --broadcast
done
build_chain_registry
# The Relay stand-in's solver (a fixed key outside Anvil's ten accounts) fills from its own USDC on
# the first chain; the deployer's Bootstrap mint funds it.
RELAY_SOLVER="$(cast wallet address --private-key 0x1111111111111111111111111111111111111111111111111111111111111111)"
cast send "$USDC" 'transfer(address,uint256)' "$RELAY_SOLVER" 100000000 \
  --private-key "$GUM_ONBOARDING_PAYER_KEY" --rpc-url "$GUM_RPC_URL" >/dev/null
prefix relay-stub env RELAY_STUB_USDC="$USDC" RELAY_STUB_USDT="$USDT" RELAY_STUB_PORT="${GUM_RELAY_URL##*:}" \
  RELAY_STUB_API_KEY="$GUM_RELAY_API_KEY" node scripts/relay-stub.mjs
prefix identity ./target/debug/gum-dev-identity
for _ in {1..100}; do
  curl -fsS "$GUM_DEV_IDENTITY_ISSUER/.well-known/jwks.json" >/dev/null 2>&1 && break
  sleep .1
done
curl -fsS "$GUM_DEV_IDENTITY_ISSUER/.well-known/jwks.json" >/dev/null || {
  echo "development identity provider did not become ready" >&2
  exit 1
}
# Migrations are an explicit step, never a side effect of a service
# starting: every service's readiness refuses until the schema is current.
echo "[migrate] applying schema migrations"
./target/debug/gum-server migrate
prefix gum-server ./target/debug/gum-server
for _ in {1..100}; do
  curl -fsS "$GUM_SERVER_INTERNAL_URL/health/ready" >/dev/null 2>&1 && break
  sleep .1
done
curl -fsS "$GUM_SERVER_INTERNAL_URL/health/ready" >/dev/null || { echo "gum-server did not become ready" >&2; exit 1; }
prefix indexer ./target/debug/gum-indexer
prefix signers ./target/debug/gum-signers
echo "[runner] ready: API $GUM_API_URL; 'just web' serves the dashboard, 'just seed' mints an API key"
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
