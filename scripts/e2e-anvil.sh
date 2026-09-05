#!/usr/bin/env bash
# End-to-end run of the payment gateway against a local Anvil node.
#
# Anvil is started with one-slot epochs and interval mining so the node's
# `finalized` tag advances on its own (finalized = latest - 2, one block per
# second), which is the finality source the indexer uses in production.
# Transactions wait for the next interval block, exactly like a real chain.
# Every flow below is exercised through the public or operator API; PostgreSQL
# is consulted only for test-only ledger and queue invariants.
set -euo pipefail

RPC_URL="${PAYDAY_RPC_URL:-http://127.0.0.1:8545}"
API_URL="${PAYDAY_API_URL:-http://127.0.0.1:3000}"
FACTORY="${PAYDAY_FACTORY_ADDRESS:-0x5FbDB2315678afecb367f032d93F642f64180aa3}"
BATCH_SWEEPER="${PAYDAY_BATCH_SWEEPER_ADDRESS:-0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0}"
USDC="${PAYDAY_USDC_ADDRESS:-0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512}"
SIGNER_KEY="${PAYDAY_SIGNER_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}"
PAYER_KEY="0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
PAYER="0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
BENEFICIARY_EXACT="0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"
BENEFICIARY_PARTIAL="0x90F79bf6EB2c4f870365E785982E1f101E93b906"
BENEFICIARY_OVERPAYMENT="0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65"
BENEFICIARY_EXPIRED="0x976EA74026E726554dB657fA54763abd0C3a0aa9"
BENEFICIARY_THIRD_PARTY="0x14dC79964da2C08b23698B3D3cc7Ca32193d9955"
BENEFICIARY_PAUSED="0x23618e81E3f5cdF7f54C3d65f7FBc0aBf5B21E8f"
BENEFICIARY_BLACKLISTED="0xa0Ee7A142d267C1f36714E4a8F75612F20a79720"
# A wallet that is not the payer's: Anvil account #5. It pays one request to
# show that money from an unattested wallet is flagged and earns no proof.
STRANGER_KEY="0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba"
STRANGER="0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"
# Proof of Payment attestations are signed with Anvil account #6; its address,
# 0x976EA74026E726554dB657fA54763abd0C3a0aa9, is the trusted attestor here.
ATTESTATION_SIGNER_KEY="0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e"
MINIO_PORT="${PAYDAY_MINIO_PORT:-9000}"
MINIO_IMAGE="${PAYDAY_MINIO_IMAGE:-minio/minio}"
MC_IMAGE="${PAYDAY_MC_IMAGE:-minio/mc}"
# MinIO's root credentials double as the AWS credentials gatewayd signs with.
MINIO_CREDENTIAL="payday-local"
ATTACHMENT_BUCKET="payday-attachments-local"

export PAYDAY_RPC_URL="$RPC_URL"
export PAYDAY_API_URL="$API_URL"
export PAYDAY_FACTORY_ADDRESS="$FACTORY"
export PAYDAY_BATCH_SWEEPER_ADDRESS="$BATCH_SWEEPER"
export PAYDAY_USDC_ADDRESS="$USDC"
export PAYDAY_CHAIN_ID="${PAYDAY_CHAIN_ID:-31337}"
export PAYDAY_USDC_START_BLOCK="${PAYDAY_USDC_START_BLOCK:-0}"
export PAYDAY_FINALITY_SOURCE="${PAYDAY_FINALITY_SOURCE:-finalized}"
export PAYDAY_FINALITY_CONFIRMATIONS="${PAYDAY_FINALITY_CONFIRMATIONS:-0}"
export PAYDAY_INDEXER_POLL_INTERVAL_MS="${PAYDAY_INDEXER_POLL_INTERVAL_MS:-250}"
export PAYDAY_SIGNER_KEY="$SIGNER_KEY"
export PAYDAY_PUBLIC_BASE_URL="${PAYDAY_PUBLIC_BASE_URL:-$API_URL}"
export PAYDAY_ADMIN_BEARER_SECRET="${PAYDAY_ADMIN_BEARER_SECRET:-local-admin-bearer-secret-0123456789abcdef}"
export PAYDAY_ADMIN_SECRET="${PAYDAY_ADMIN_SECRET:-$PAYDAY_ADMIN_BEARER_SECRET}"
export PAYDAY_API_KEY_PREFIX="payday_test_"
export PAYDAY_DEV_IDENTITY=1
export PAYDAY_DEV_IDENTITY_OTP="${PAYDAY_DEV_IDENTITY_OTP:-123456}"
export PAYDAY_DEV_IDENTITY_BIND="127.0.0.1:3001"
export PAYDAY_DEV_IDENTITY_ISSUER="http://127.0.0.1:3001"
# No PAYDAY_PRIVY_APP_ID: this suite runs offline on API keys minted straight
# into its database, and gatewayd simply refuses dashboard sessions.
export PAYDAY_PAYER_AUTH0_ISSUER="$PAYDAY_DEV_IDENTITY_ISSUER"
export PAYDAY_PAYER_AUTH0_AUDIENCE="payday-payer-local"
export PAYDAY_PAYER_AUTH0_CLIENT_ID="payday-payer-local"
export PAYDAY_PAYER_REF_MASTER_KEY="AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8="
export PAYDAY_HOSTED_CHECKOUT_ORIGIN="$API_URL"
export PAYDAY_ATTESTATION_SIGNER_KEY="$ATTESTATION_SIGNER_KEY"
# MinIO stands in for the S3 attachment bucket; see start_minio below.
export PAYDAY_ATTACHMENT_BUCKET="$ATTACHMENT_BUCKET"
export PAYDAY_ATTACHMENT_S3_ENDPOINT="http://127.0.0.1:${MINIO_PORT}"
export PAYDAY_ATTACHMENT_S3_FORCE_PATH_STYLE=1
export AWS_ACCESS_KEY_ID="$MINIO_CREDENTIAL"
export AWS_SECRET_ACCESS_KEY="$MINIO_CREDENTIAL"
export AWS_REGION=us-east-1

logs="$(mktemp -d)"
pids=()
minio_container="payday-minio-e2e-$$"
minio_started=false

cleanup() {
  local status=$?
  trap - EXIT INT TERM

  if ((status != 0)); then
    for log in "$logs"/*.log; do
      [[ -e "$log" ]] || continue
      echo "--- $log ---" >&2
      tail -n 200 "$log" >&2
    done
    if [[ "$minio_started" == true ]]; then
      echo "--- minio ---" >&2
      docker logs --tail 50 "$minio_container" >&2 2>&1 || true
    fi
  fi

  if ((${#pids[@]})); then
    kill "${pids[@]}" 2>/dev/null || true
    wait "${pids[@]}" 2>/dev/null || true
  fi
  if [[ "$minio_started" == true ]]; then
    docker rm -f "$minio_container" >/dev/null 2>&1 || true
  fi
  rm -rf "$logs"
  exit "$status"
}
trap cleanup EXIT INT TERM

for command in anvil cast curl docker forge jq psql; do
  command -v "$command" >/dev/null || {
    echo "required command not found: $command" >&2
    exit 1
  }
done

[[ -n "${DATABASE_URL:-}" ]] || {
  echo "DATABASE_URL must be set" >&2
  exit 1
}

wait_for_rpc() {
  for _ in {1..100}; do
    if cast chain-id --rpc-url "$RPC_URL" >/dev/null 2>&1; then
      return
    fi
    sleep 0.1
  done
  echo "Anvil did not become ready" >&2
  return 1
}

wait_for_api() {
  for _ in {1..100}; do
    if curl --fail --silent --output /dev/null "$API_URL/health"; then
      return
    fi
    sleep 0.1
  done
  echo "gatewayd did not become ready" >&2
  return 1
}

# The MinIO client, aimed at the suite's MinIO. Host networking reaches the
# published loopback port on Linux and Docker Desktop alike.
minio_client() {
  docker run --rm --network host \
    -e "MC_HOST_local=http://${MINIO_CREDENTIAL}:${MINIO_CREDENTIAL}@127.0.0.1:${MINIO_PORT}" \
    "$MC_IMAGE" "$@"
}

# MinIO stands in for the S3 attachment bucket. Nothing scans local uploads,
# so the suite stamps the verdict tag itself with tag_object_scanned.
start_minio() {
  docker info >/dev/null 2>&1 || {
    echo "Docker daemon is not available" >&2
    return 1
  }
  docker run -d --rm --name "$minio_container" \
    -e MINIO_ROOT_USER="$MINIO_CREDENTIAL" -e MINIO_ROOT_PASSWORD="$MINIO_CREDENTIAL" -e MINIO_BROWSER=off \
    -p "127.0.0.1:${MINIO_PORT}:9000" "$MINIO_IMAGE" server /data >/dev/null
  minio_started=true
  for _ in {1..100}; do
    if curl --fail --silent --output /dev/null "http://127.0.0.1:${MINIO_PORT}/minio/health/live"; then
      minio_client mb --ignore-existing "local/${ATTACHMENT_BUCKET}" >/dev/null
      return
    fi
    sleep 0.1
  done
  echo "MinIO did not become ready" >&2
  return 1
}

# Sets the tag GuardDuty Malware Protection writes after a clean scan on one
# object of the attachment bucket, which is what finalize requires; every
# other value, or no tag at all, must keep the upload unattachable.
tag_object_scanned() {
  local object_key=$1
  minio_client tag set "local/${ATTACHMENT_BUCKET}/${object_key}" \
    'GuardDutyMalwareScanStatus=NO_THREATS_FOUND' >/dev/null
}

# gatewayd and the indexer refuse to start unless the deployed runtime bytecode
# hashes to these values, so they are computed from the freshly bootstrapped
# chain rather than trusted from the environment.
pin_deployment_code_hashes() {
  local factory_code sweeper_code
  factory_code="$(cast code "$FACTORY" --rpc-url "$RPC_URL")"
  sweeper_code="$(cast code "$BATCH_SWEEPER" --rpc-url "$RPC_URL")"
  [[ -n "$factory_code" && "$factory_code" != 0x ]] || {
    echo "no code at PaymentFactory $FACTORY after the bootstrap" >&2
    return 1
  }
  [[ -n "$sweeper_code" && "$sweeper_code" != 0x ]] || {
    echo "no code at BatchSweeper $BATCH_SWEEPER after the bootstrap" >&2
    return 1
  }
  PAYDAY_FACTORY_CODE_HASH="$(cast keccak "$factory_code")"
  PAYDAY_BATCH_SWEEPER_CODE_HASH="$(cast keccak "$sweeper_code")"
  export PAYDAY_FACTORY_CODE_HASH PAYDAY_BATCH_SWEEPER_CODE_HASH
}

# Issue a permissionless request and bind the payer's wallet to it, so the
# response carries the address the flows below pay. The merchant response is
# re-read after the binding, as an integration polling for `address` would.
create_invoice() {
  local amount=$1 beneficiary=$2 expires_in=$3 idempotency_key=$4 issued id
  issued="$(issue_invoice "$amount" "$beneficiary" "$expires_in" "$idempotency_key")"
  id="$(jq -er .id <<<"$issued")"
  if [[ "$(jq -r .address <<<"$issued")" == null ]]; then
    bind_payer_wallet "$id" >/dev/null
  fi
  get_invoice "$id"
}

# Issue only: no address until a payer binds a wallet.
issue_invoice() {
  local amount=$1 beneficiary=$2 expires_in=$3 idempotency_key=$4
  curl --fail --silent \
    --header "Authorization: Bearer $PAYDAY_API_KEY" \
    --header "Content-Type: application/json" \
    --header "Idempotency-Key: $idempotency_key" \
    --data "$(invoice_body "$amount" "$beneficiary" "$expires_in")" \
    "$API_URL/v1/payments"
}

# The payer's wallet step, as the hosted checkout performs it: a challenge
# for the payer's wallet, signed with cast exactly as a wallet signs EIP-712
# typed data, then attested. A session may be passed for a gated request
# (one that has verified its mailbox); a permissionless request gets one
# from the challenge. Prints the session the binding was made in.
bind_payer_wallet() {
  local id=$1 session=${2:-} challenge typed signature
  local -a session_header=()
  if [[ -n "$session" ]]; then
    session_header=(--header "Payday-Payer-Session: $session")
  fi
  challenge="$(curl --fail --silent --request POST \
    --header "Origin: $PAYDAY_HOSTED_CHECKOUT_ORIGIN" --header "Content-Type: application/json" \
    ${session_header[@]+"${session_header[@]}"} \
    --data "$(jq -cn --arg wallet "$PAYER" '{wallet: $wallet}')" \
    "$API_URL/v1/payer/payments/$id/wallet/challenge")"
  session="$(jq -er .payer_session <<<"$challenge")"
  typed="$logs/typed-${id#pay_}.json"
  jq -c .typed_data <<<"$challenge" >"$typed"
  signature="$(cast wallet sign --private-key "$PAYER_KEY" --data --from-file "$typed")"
  curl --fail --silent --output /dev/null --request POST \
    --header "Origin: $PAYDAY_HOSTED_CHECKOUT_ORIGIN" --header "Content-Type: application/json" \
    --header "Payday-Payer-Session: $session" \
    --data "$(jq -cn --arg wallet "$PAYER" --arg signature "$signature" '{wallet: $wallet, signature: $signature}')" \
    "$API_URL/v1/payer/payments/$id/wallet/attest"
  echo "$session"
}

get_invoice() {
  curl --fail --silent \
    --header "Authorization: Bearer $PAYDAY_API_KEY" \
    "$API_URL/v1/payments/$1"
}

# Poll at the documented per-account rate until a jq expression is true. A
# 100ms loop used to be harmless, but now tests the rate limiter instead of the
# payment transition and can starve the rest of this end-to-end suite.
wait_for_invoice() {
  local id=$1 expression=$2 description=$3 invoice
  for _ in {1..60}; do
    invoice="$(get_invoice "$id")"
    if jq -e "$expression" <<<"$invoice" >/dev/null; then
      return
    fi
    sleep 1
  done
  echo "invoice $id never satisfied '$description'; last state: $(jq -c . <<<"$invoice")" >&2
  return 1
}

wait_for_status() {
  wait_for_invoice "$1" ".status == \"$2\"" "status == $2"
}

wait_for_sql() {
  local expected=$1 query=$2 description=$3 actual
  for _ in {1..600}; do
    actual="$(psql "$DATABASE_URL" --tuples-only --no-align --command "$query")"
    if [[ "$actual" == "$expected" ]]; then
      return
    fi
    sleep 0.1
  done
  echo "$description: expected $expected, last value $actual" >&2
  return 1
}

token_balance() {
  cast call "$USDC" 'balanceOf(address)(uint256)' "$1" --rpc-url "$RPC_URL" | awk '{print $1}'
}

send_usdc() {
  cast send "$USDC" 'transfer(address,uint256)' "$1" "$2" \
    --private-key "$PAYER_KEY" --rpc-url "$RPC_URL" >/dev/null
}

set_paused() {
  cast send "$USDC" 'setPaused(bool)' "$1" --private-key "$PAYER_KEY" --rpc-url "$RPC_URL" >/dev/null
}

set_blacklisted() {
  cast send "$USDC" 'setBlacklisted(address,bool)' "$1" "$2" \
    --private-key "$PAYER_KEY" --rpc-url "$RPC_URL" >/dev/null
}

# Raw request body for POST /v1/payments; built with jq so no shell quoting
# is involved (bash 3.2 brace-expands nested quotes inside "$(...)"). Recovery
# is not a request field: the platform stamps its own wallet on every invoice.
# The document fields are the minimum an invoice carries: two parties and an
# open payer policy.
invoice_body() {
  local amount=$1 beneficiary=$2 expires_in=$3
  jq -cn --arg chain "$PAYDAY_CHAIN_ID" --arg token "$USDC" --arg beneficiary "$beneficiary" \
    --arg amount "$amount" --argjson expires_in "$expires_in" \
    '{chain_id: $chain, token_address: $token, payout_address: $beneficiary,
      amount: $amount, expires_in: $expires_in,
      issuer: {name: "Payday E2E Issuer"}, bill_to: {name: "Payday E2E Customer"},
      payer_policy: {mode: "permissionless"}}'
}

# A merchant API request with the primary account's key. The body, when
# given, is JSON; extra curl arguments follow it.
merchant_curl() {
  local method=$1 path=$2 body=$3
  shift 3
  if [[ -n "$body" ]]; then
    curl --silent --request "$method" --header "Authorization: Bearer $PAYDAY_API_KEY" \
      --header "Content-Type: application/json" --data "$body" "$@" "$API_URL$path"
  else
    curl --silent --request "$method" --header "Authorization: Bearer $PAYDAY_API_KEY" \
      "$@" "$API_URL$path"
  fi
}

# The response body of a request that must succeed. The body slot is
# positional, so pass "" before any extra curl arguments (e.g. --output).
api_json() {
  merchant_curl "$1" "$2" "${3:-}" --fail "${@:4}"
}

# Only the HTTP status of a request that may fail.
api_status() {
  merchant_curl "$1" "$2" "${3:-}" --output /dev/null --write-out '%{http_code}'
}

# Only the stable error code of a request that must fail.
api_error_code() {
  merchant_curl "$1" "$2" "${3:-}" | jq -r .error.code
}

# PUT a file to the presigned upload slot exactly as an SDK client would:
# every returned header is signed and must be sent verbatim.
upload_attachment() {
  local slot=$1 file=$2 header
  local args=(--fail --silent --output /dev/null --request PUT --upload-file "$file")
  while IFS= read -r header; do
    args+=(--header "$header")
  done < <(jq -r '.headers | to_entries[] | "\(.key): \(.value)"' <<<"$slot")
  curl "${args[@]}" "$(jq -er .upload_url <<<"$slot")"
}

# Reserve a slot, upload the file, and stamp the scan verdict. Prints the
# attachment id; finalization is the caller's, so it can assert on it.
stage_attachment() {
  local file=$1 filename=$2 verdict=$3 slot id
  slot="$(api_json POST /v1/attachments "$(jq -cn --arg filename "$filename" '{filename: $filename}')")"
  id="$(jq -er .id <<<"$slot")"
  upload_attachment "$slot" "$file"
  if [[ -n "$verdict" ]]; then
    minio_client tag set "local/${ATTACHMENT_BUCKET}/uploads/${account_id}/${id}.pdf" \
      "GuardDutyMalwareScanStatus=${verdict}" >/dev/null
  fi
  echo "$id"
}

sha256_of() {
  echo "0x$(shasum -a 256 "$1" | awk '{print $1}')"
}

lowercase() {
  tr '[:upper:]' '[:lower:]' <<<"$1"
}

# Query text for wait_for_sql: the recovery ledger rows an invoice should carry.
recovery_ledger_query() {
  local invoice_id=$1 reason=$2 amount=$3
  echo "SELECT count(*) FROM recovered_funds
    WHERE invoice_id = '${invoice_id#pay_}'::uuid AND reason = '$reason' AND amount = '$amount'"
}

api_status_code() {
  local body=$1
  curl --silent --output /dev/null --write-out '%{http_code}' \
    --header "Authorization: Bearer $PAYDAY_API_KEY" \
    --header "Content-Type: application/json" \
    --header "Idempotency-Key: validation-$RANDOM-$RANDOM" \
    --data "$body" "$API_URL/v1/payments"
}

assert_eq() {
  local expected=$1 actual=$2 message=$3
  if [[ "$actual" != "$expected" ]]; then
    echo "$message: expected $expected, got $actual" >&2
    return 1
  fi
}

assert_payment_deployed_and_empty() {
  local address=$1
  assert_eq 0 "$(token_balance "$address")" "payment address was not emptied"
  [[ "$(cast code "$address" --rpc-url "$RPC_URL")" != "0x" ]] || {
    echo "Payment was not deployed at $address" >&2
    return 1
  }
}

assert_process_alive() {
  local name=$1 pid=$2
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "$name exited before the end-to-end test completed" >&2
    return 1
  fi
}

echo "Starting Anvil (finalized = latest - 2, one block per second) and deploying local fixtures"
anvil --chain-id "$PAYDAY_CHAIN_ID" --slots-in-an-epoch 1 --block-time 1 --silent \
  >"$logs/anvil.log" 2>&1 &
anvil_pid=$!
pids+=("$anvil_pid")
wait_for_rpc
forge script foundry/script/Bootstrap.s.sol:BootstrapScript \
  --rpc-url "$RPC_URL" --private-key "$SIGNER_KEY" --broadcast >/dev/null
pin_deployment_code_hashes

echo "Starting MinIO as the local attachment store"
start_minio

echo "Starting gateway services"
./target/debug/payday-dev-identity >"$logs/dev-identity.log" 2>&1 &
identity_pid=$!
pids+=("$identity_pid")
for _ in {1..100}; do
  curl --fail --silent --output /dev/null "$PAYDAY_DEV_IDENTITY_ISSUER/.well-known/jwks.json" && break
  sleep 0.1
done
curl --fail --silent --output /dev/null "$PAYDAY_DEV_IDENTITY_ISSUER/.well-known/jwks.json" || {
  echo "development identity provider did not become ready" >&2
  exit 1
}
./target/debug/gatewayd >"$logs/gatewayd.log" 2>&1 &
gatewayd_pid=$!
pids+=("$gatewayd_pid")
wait_for_api
echo "Creating two accounts with keys minted straight into the database"
PAYDAY_API_KEY="$(./scripts/local-api-key.sh primary@example.test)"
export PAYDAY_API_KEY
SECOND_API_KEY="$(./scripts/local-api-key.sh secondary@example.test)"
./target/debug/gateway-indexer >"$logs/indexer.log" 2>&1 &
indexer_pid=$!
pids+=("$indexer_pid")

run_id="${GITHUB_RUN_ID:-local}-$(date +%s)-$$"

echo "Testing API validation"
too_soon="$(invoice_body 1 "$BENEFICIARY_EXACT" 60)"
assert_eq 400 "$(api_status_code "$too_soon")" "an expiration inside the minimum lead time was accepted"
negative="$(invoice_body -1 "$BENEFICIARY_EXACT" 3600)"
assert_eq 400 "$(api_status_code "$negative")" "a negative amount was accepted"
zero_beneficiary="$(invoice_body 1 0x0000000000000000000000000000000000000000 3600)"
assert_eq 400 "$(api_status_code "$zero_beneficiary")" "a zero beneficiary was accepted"
valid="$(invoice_body 1 "$BENEFICIARY_EXACT" 3600)"
assert_eq 201 "$(api_status_code "$valid")" "a valid request was rejected"
# Merchants do not choose where recovered funds go; the field is unknown to
# the API and must fail like any other unknown field. The exact code is the
# API's decision, so only the class is asserted.
with_refund_address="$(jq -c --arg recovery "$STRANGER" '. + {refund_address: $recovery}' <<<"$valid")"
refund_status="$(api_status_code "$with_refund_address")"
[[ "$refund_status" != 201 && "$refund_status" -ge 400 && "$refund_status" -le 499 ]] || {
  echo "a request carrying refund_address was not rejected: status $refund_status" >&2
  exit 1
}

echo "Testing exact payment, the payer's wallet binding, and API idempotency"
exact_issued="$(issue_invoice 1.5 "$BENEFICIARY_EXACT" 3600 "exact-payment-$run_id")"
exact_id="$(jq -er .id <<<"$exact_issued")"
assert_eq null "$(jq -r .address <<<"$exact_issued")" "a freshly issued request already has an address"
assert_eq null "$(jq -r .payer_wallet <<<"$exact_issued")" "a freshly issued request already names a payer wallet"
assert_eq 2 "$(jq -r .attribution.version <<<"$exact_issued")" "issued invoice has the wrong attribution version"
# The payer's wallet, not the merchant, is what turns the request into an address.
exact_session="$(bind_payer_wallet "$exact_id")"
exact="$(get_invoice "$exact_id")"
exact_replay="$(issue_invoice 1.5 "$BENEFICIARY_EXACT" 3600 "exact-payment-$run_id")"
assert_eq "$exact_id" "$(jq -r .id <<<"$exact_replay")" "idempotent replay created another invoice"
assert_eq "$(jq -r .address <<<"$exact")" "$(jq -r .address <<<"$exact_replay")" \
  "idempotent replay does not report the bound address"
assert_eq "$(lowercase "$PAYER")" "$(jq -r '.payer_wallet | ascii_downcase' <<<"$exact")" \
  "invoice does not name the attested payer wallet"
assert_eq "$(lowercase "$PAYER")" "$(jq -r '.recovery_address | ascii_downcase' <<<"$exact")" \
  "the recovery term is not the payer's wallet"
[[ "$(jq -r .wallet_bound_at <<<"$exact")" != null ]] || {
  echo "the bound request does not record when its wallet was bound" >&2
  exit 1
}
wait_for_sql 1 "SELECT count(*) FROM webhook_events
  WHERE invoice_id = '${exact_id#pay_}'::uuid AND event_type = 'payment.ready'" \
  "binding the wallet did not raise payment.ready"
# The address is final: another wallet cannot take the request.
assert_eq 409 "$(curl --silent --output /dev/null --write-out '%{http_code}' --request POST \
  --header "Origin: $PAYDAY_HOSTED_CHECKOUT_ORIGIN" --header "Content-Type: application/json" \
  --data "$(jq -cn --arg wallet "$STRANGER" '{wallet: $wallet}')" \
  "$API_URL/v1/payer/payments/$exact_id/wallet/challenge")" \
  "a second wallet was offered a challenge on a bound request"
# The payer page shows the address to anyone with the link, with the wallet it is for.
exact_payer="$(curl --fail --silent "$API_URL/v1/payer/payments/$exact_id")"
assert_eq "$(jq -r .address <<<"$exact")" "$(jq -r .address <<<"$exact_payer")" "the payer page shows a different address"
assert_eq "$(lowercase "$PAYER")" "$(jq -r '.payer_wallet | ascii_downcase' <<<"$exact_payer")" \
  "the payer page does not name the attested wallet"
assert_eq approved "$(jq -r .requirements.wallet <<<"$exact_payer")" "the payer page does not report the wallet fact"
[[ "$(jq -c 'has("refund_address")' <<<"$exact")" == false ]] || {
  echo "the merchant response still exposes refund_address" >&2
  exit 1
}
second_account_invoice="$(PAYDAY_API_KEY="$SECOND_API_KEY" create_invoice 1.5 "$BENEFICIARY_EXACT" 3600 "exact-payment-$run_id")"
[[ "$(jq -r .id <<<"$second_account_invoice")" != "$exact_id" ]] || {
  echo "account-scoped idempotency returned another account's invoice" >&2
  exit 1
}
cross_account_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
  --header "Authorization: Bearer $SECOND_API_KEY" "$API_URL/v1/payments/$exact_id")"
assert_eq 404 "$cross_account_status" "cross-account invoice lookup leaked an invoice"
exact_address="$(jq -r .address <<<"$exact")"
exact_before="$(token_balance "$BENEFICIARY_EXACT")"
send_usdc "$exact_address" 1500000
wait_for_status "$exact_id" settled
assert_eq "$((exact_before + 1500000))" "$(token_balance "$BENEFICIARY_EXACT")" \
  "exact payment beneficiary balance mismatch"
assert_payment_deployed_and_empty "$exact_address"
exact_final="$(get_invoice "$exact_id")"
assert_eq 1500000 "$(jq -r .received_base_units <<<"$exact_final")" "received amount not reported"
[[ "$(jq -r .settlement_tx_hash <<<"$exact_final")" == 0x* ]] || {
  echo "settlement transaction hash not reported" >&2
  exit 1
}
[[ "$(jq -r .settled_block <<<"$exact_final")" != "null" ]] || {
  echo "resolution block not reported" >&2
  exit 1
}
assert_eq true "$(cast call "$exact_address" 'settled()(bool)' --rpc-url "$RPC_URL")" \
  "deployed Payment must record settlement"

echo "Testing cumulative partial payments"
partial="$(create_invoice 1 "$BENEFICIARY_PARTIAL" 3600 "partial-payment-$run_id")"
partial_id="$(jq -r .id <<<"$partial")"
partial_address="$(jq -r .address <<<"$partial")"
partial_before="$(token_balance "$BENEFICIARY_PARTIAL")"
send_usdc "$partial_address" 400000
assert_eq 400000 "$(token_balance "$partial_address")" "first partial payment was not retained"
wait_for_invoice "$partial_id" '.received_base_units == "400000" and .status == "partially_paid"' "partial credit visible while still open"
send_usdc "$partial_address" 600000
wait_for_status "$partial_id" settled
assert_eq "$((partial_before + 1000000))" "$(token_balance "$BENEFICIARY_PARTIAL")" \
  "partial payment beneficiary balance mismatch"
assert_payment_deployed_and_empty "$partial_address"

echo "Testing exact settlement with the remainder returned to the payer"
overpayment="$(create_invoice 1 "$BENEFICIARY_OVERPAYMENT" 3600 "overpayment-$run_id")"
overpayment_id="$(jq -r .id <<<"$overpayment")"
overpayment_address="$(jq -r .address <<<"$overpayment")"
overpayment_before="$(token_balance "$BENEFICIARY_OVERPAYMENT")"
# The payer is both the sender and, being the attested wallet, where the
# remainder comes back to.
payer_before="$(token_balance "$PAYER")"
send_usdc "$overpayment_address" 1250000
wait_for_status "$overpayment_id" settled
assert_eq "$((overpayment_before + 1000000))" "$(token_balance "$BENEFICIARY_OVERPAYMENT")" \
  "beneficiary must receive exactly the invoice amount"
assert_eq "$((payer_before - 1000000))" "$(token_balance "$PAYER")" \
  "overpayment remainder did not return to the payer's wallet"
assert_payment_deployed_and_empty "$overpayment_address"
wait_for_sql 1 "$(recovery_ledger_query "$overpayment_id" overpayment 250000)" \
  "overpayment remainder was not recorded in the recovery ledger"

echo "Testing one-transaction batch sweeping"
batch_one="$(create_invoice 0.1 "$BENEFICIARY_EXACT" 3600 "batch-one-$run_id")"
batch_two="$(create_invoice 0.2 "$BENEFICIARY_PARTIAL" 3600 "batch-two-$run_id")"
batch_three="$(create_invoice 0.3 "$BENEFICIARY_OVERPAYMENT" 3600 "batch-three-$run_id")"
batch_one_id="$(jq -r .id <<<"$batch_one")"
batch_two_id="$(jq -r .id <<<"$batch_two")"
batch_three_id="$(jq -r .id <<<"$batch_three")"
batch_one_address="$(jq -r .address <<<"$batch_one")"
batch_two_address="$(jq -r .address <<<"$batch_two")"
batch_three_address="$(jq -r .address <<<"$batch_three")"

# Mine all three transfers in one block so one acquisition pass makes the
# invoices eligible together: pause interval mining, queue the transfers with
# explicit nonces, mine once, then resume the one-second cadence.
payer_nonce="$(cast nonce "$PAYER" --block pending --rpc-url "$RPC_URL")"
cast rpc --rpc-url "$RPC_URL" evm_setIntervalMining 0 >/dev/null
for payment in \
  "$batch_one_address:100000" \
  "$batch_two_address:200000" \
  "$batch_three_address:300000"; do
  payment_address="${payment%%:*}"
  payment_amount="${payment##*:}"
  cast send "$USDC" 'transfer(address,uint256)' "$payment_address" "$payment_amount" \
    --private-key "$PAYER_KEY" --nonce "$payer_nonce" --rpc-url "$RPC_URL" --async >/dev/null
  payer_nonce="$((payer_nonce + 1))"
done
cast rpc --rpc-url "$RPC_URL" anvil_mine 1 >/dev/null
cast rpc --rpc-url "$RPC_URL" evm_setIntervalMining 1 >/dev/null

wait_for_status "$batch_one_id" settled
wait_for_status "$batch_two_id" settled
wait_for_status "$batch_three_id" settled
batch_hashes="$(for id in "$batch_one_id" "$batch_two_id" "$batch_three_id"; do
  get_invoice "$id" | jq -r .settlement_tx_hash
done | sort -u | wc -l | tr -d ' ')"
assert_eq 1 "$batch_hashes" "eligible invoices did not share one batch transaction"
assert_payment_deployed_and_empty "$batch_one_address"
assert_payment_deployed_and_empty "$batch_two_address"
assert_payment_deployed_and_empty "$batch_three_address"

echo "Testing that a transfer after settlement is forwarded back to the payer"
payer_before="$(token_balance "$PAYER")"
send_usdc "$batch_one_address" 7
send_usdc "$batch_one_address" 0
wait_for_sql 1 "SELECT count(*) FROM payment_observations
  WHERE invoice_id = '${batch_one_id#pay_}'::uuid AND disposition = 'late' AND collected_at_block IS NOT NULL" \
  "late transfer was not collected"
assert_eq "$payer_before" "$(token_balance "$PAYER")" "late transfer did not come back to the payer's wallet"
wait_for_sql 1 "$(recovery_ledger_query "$batch_one_id" late_transfer 7)" \
  "late transfer was not recorded in the recovery ledger"
assert_eq 1 "$(psql "$DATABASE_URL" --tuples-only --no-align --command "
  SELECT count(*) FROM payment_observations
  WHERE invoice_id = '${batch_one_id#pay_}'::uuid AND disposition = 'error' AND disposition_reason = 'zero_amount'
")" "zero-value transfer was not marked erroneous"
batch_one_final="$(get_invoice "$batch_one_id")"
assert_eq settled "$(jq -r .status <<<"$batch_one_final")" "late collection changed the payment status"
assert_eq 100000 "$(jq -r .received_base_units <<<"$batch_one_final")" "late transfer was credited to the invoice"
assert_eq 0 "$(token_balance "$batch_one_address")" "late transfer stranded at the payment address"

echo "Testing an invoice executed by a third party before the worker"
third="$(create_invoice 2 "$BENEFICIARY_THIRD_PARTY" 3600 "third-party-$run_id")"
third_id="$(jq -r .id <<<"$third")"
third_address="$(jq -r .address <<<"$third")"
third_salt="$(jq -r .self_settlement.salt <<<"$third")"
third_expiration="$(jq -r '.expires_at | fromdateiso8601' <<<"$third")"
send_usdc "$third_address" 2000000
third_party_tx="$(cast send "$FACTORY" \
  'execute(address,uint256,address,uint64,address,bytes32)' \
  "$USDC" 2000000 "$BENEFICIARY_THIRD_PARTY" "$third_expiration" "$PAYER" "$third_salt" \
  --private-key "$PAYER_KEY" --rpc-url "$RPC_URL" --json | jq -r .transactionHash)"
wait_for_status "$third_id" settled
assert_eq "$third_party_tx" "$(get_invoice "$third_id" | jq -r .settlement_tx_hash)" \
  "third-party settlement transaction hash was not reported"
assert_eq 2000000 "$(token_balance "$BENEFICIARY_THIRD_PARTY")" "third-party settlement balance mismatch"

echo "Testing that a paused token is retried rather than blocked"
paused="$(create_invoice 0.5 "$BENEFICIARY_PAUSED" 3600 "paused-$run_id")"
paused_id="$(jq -r .id <<<"$paused")"
paused_address="$(jq -r .address <<<"$paused")"
send_usdc "$paused_address" 500000
set_paused true
wait_for_sql 1 "SELECT count(*) FROM invoices WHERE id = '${paused_id#pay_}'::uuid AND sweep_attempts >= 1 AND status = 'deploying'" \
  "paused token did not produce a retryable failure"
assert_eq null "$(get_invoice "$paused_id" | jq -r .attention)" "a paused token must not block the payment"
set_paused false
wait_for_status "$paused_id" settled
assert_eq 500000 "$(token_balance "$BENEFICIARY_PAUSED")" "post-pause settlement balance mismatch"

echo "Testing that a blacklisted beneficiary is blocked, then released by the operator"
set_blacklisted "$BENEFICIARY_BLACKLISTED" true
blacklisted="$(create_invoice 0.25 "$BENEFICIARY_BLACKLISTED" 3600 "blacklisted-$run_id")"
blacklisted_id="$(jq -r .id <<<"$blacklisted")"
blacklisted_address="$(jq -r .address <<<"$blacklisted")"
send_usdc "$blacklisted_address" 250000
wait_for_status "$blacklisted_id" needs_attention
assert_eq beneficiary_blacklisted "$(get_invoice "$blacklisted_id" | jq -r .attention.code)" "wrong attention code"
assert_eq 250000 "$(token_balance "$blacklisted_address")" "funds must stay at the address while blocked"
set_blacklisted "$BENEFICIARY_BLACKLISTED" false
# Audited operator procedure from docs/runbooks/stuck-invoice.md.
curl --fail --silent --output /dev/null --request POST \
  --header "Authorization: Bearer $PAYDAY_ADMIN_SECRET" \
  "$API_URL/v1/admin/payments/$blacklisted_id/release"
wait_for_status "$blacklisted_id" settled
assert_eq 250000 "$(token_balance "$BENEFICIARY_BLACKLISTED")" "released invoice was not settled"

echo "Testing automatic return of an expired partial payment and its late completion"
expired="$(create_invoice 1 "$BENEFICIARY_EXPIRED" 660 "expired-recovery-$run_id")"
expired_id="$(jq -r .id <<<"$expired")"
expired_address="$(jq -r .address <<<"$expired")"
payer_before="$(token_balance "$PAYER")"
send_usdc "$expired_address" 400000
wait_for_invoice "$expired_id" '.received_base_units == "400000" and .status == "partially_paid"' "partial payment credited"
# Chain time passes the deadline; wall-clock stays where it is.
cast rpc --rpc-url "$RPC_URL" evm_increaseTime 700 >/dev/null
wait_for_status "$expired_id" returned
assert_eq "$payer_before" "$(token_balance "$PAYER")" \
  "expired partial payment was not returned to the payer's wallet"
assert_payment_deployed_and_empty "$expired_address"
assert_eq false "$(cast call "$expired_address" 'settled()(bool)' --rpc-url "$RPC_URL")" \
  "deployed Payment must record recovery"
wait_for_sql 1 "$(recovery_ledger_query "$expired_id" expired 400000)" \
  "expired balance was not recorded in the recovery ledger"
send_usdc "$expired_address" 600000
wait_for_sql 1 "SELECT count(*) FROM payment_observations
  WHERE invoice_id = '${expired_id#pay_}'::uuid AND amount = '600000' AND disposition = 'late' AND collected_at_block IS NOT NULL" \
  "late completion was not collected"
assert_eq "$payer_before" "$(token_balance "$PAYER")" \
  "late completion was not returned to the payer's wallet"
assert_eq returned "$(get_invoice "$expired_id" | jq -r .status)" "late completion changed the payment status"
assert_eq 0 "$(token_balance "$expired_address")" "late completion stranded at the payment address"
wait_for_sql 1 "$(recovery_ledger_query "$expired_id" late_transfer 600000)" \
  "late completion was not recorded in the recovery ledger"

echo "Checking that every helper transaction batch resolved"
assert_eq 0 "$(psql "$DATABASE_URL" --tuples-only --no-align --command "
  SELECT count(*) FROM sweep_batches WHERE resolved_at IS NULL
")" "a sweep batch is still open"
assert_eq 0 "$(psql "$DATABASE_URL" --tuples-only --no-align --command "
  SELECT count(*) FROM invoices WHERE uncollected_count > 0 AND blocked_reason IS NULL
")" "collectable funds remain queued"

echo "Checking that every recovery ledger row raised a payment.recovered_funds event"
ledger_rows="$(psql "$DATABASE_URL" --tuples-only --no-align --command "
  SELECT count(*) FROM recovered_funds
")"
[[ "$ledger_rows" -ge 4 ]] || {
  echo "expected at least four recovery ledger rows (overpayment, late transfer, expired, late completion), found $ledger_rows" >&2
  exit 1
}
assert_eq "$ledger_rows" "$(psql "$DATABASE_URL" --tuples-only --no-align --command "
  SELECT count(*) FROM webhook_events WHERE event_type = 'payment.recovered_funds'
")" "recovery ledger rows and payment.recovered_funds events differ"

echo "Testing customers, PDF attachments, invoice documents, and Proof of Payment"
account_id="$(api_json GET /v1/account | jq -er .account_id)"
customer="$(api_json POST /v1/customers '{"name":"Globex Corporation","email":"ap@globex.example"}')"
customer_id="$(jq -er .id <<<"$customer")"
assert_eq "Globex Corporation" "$(api_json GET "/v1/customers/$customer_id" | jq -r .name)" \
  "customer lookup returned the wrong record"
assert_eq "$customer_id" "$(api_json GET "/v1/customers?limit=1" | jq -r '.customers[0].id')" \
  "customer listing does not start with the newest customer"
assert_eq 404 "$(curl --silent --output /dev/null --write-out '%{http_code}' \
  --header "Authorization: Bearer $SECOND_API_KEY" "$API_URL/v1/customers/$customer_id")" \
  "cross-account customer lookup leaked a customer"

pdf_file="$logs/attachment.pdf"
printf '%%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%%%EOF' >"$pdf_file"
pdf_sha256="$(sha256_of "$pdf_file")"
attachment_id="$(stage_attachment "$pdf_file" contract.pdf "")"
assert_eq 409 "$(api_status POST "/v1/attachments/$attachment_id/finalize")" \
  "an unscanned upload was finalized"
assert_eq attachment_scan_pending "$(api_error_code POST "/v1/attachments/$attachment_id/finalize")" \
  "unscanned upload reported the wrong error"
tag_object_scanned "uploads/${account_id}/${attachment_id}.pdf"
finalized="$(api_json POST "/v1/attachments/$attachment_id/finalize")"
assert_eq "$pdf_sha256" "$(jq -r .sha256 <<<"$finalized")" "finalized attachment hash mismatch"
assert_eq "$(wc -c <"$pdf_file" | tr -d ' ')" "$(jq -r .byte_length <<<"$finalized")" \
  "finalized attachment length mismatch"

document_body="$(jq -c --arg customer "$customer_id" --arg attachment "$attachment_id" \
  '. + {customer_id: $customer, attachment_id: $attachment, heading: "March retainer",
        reference: "INV-2026-03", notes: "Net 30", issuer: {name: "Acme Corp", email: "billing@acme.example"},
        bill_to: {name: "Globex Corporation"}}' <<<"$(invoice_body 3 "$BENEFICIARY_EXACT" 3600)")"
documented="$(curl --fail --silent \
  --header "Authorization: Bearer $PAYDAY_API_KEY" --header "Content-Type: application/json" \
  --header "Idempotency-Key: document-$run_id" --data "$document_body" "$API_URL/v1/payments")"
documented_id="$(jq -er .id <<<"$documented")"
assert_eq "$pdf_sha256" "$(jq -r .attachment.sha256 <<<"$documented")" \
  "issued invoice does not carry the attachment commitment"
assert_eq "$customer_id" "$(jq -r .customer_id <<<"$documented")" "issued invoice lost its customer"
assert_eq 2 "$(jq -r .attribution.version <<<"$documented")" "issued invoice lacks an attribution version"
descriptor="$(api_json GET "/v1/payments/$documented_id/attachment")"
downloaded="$logs/downloaded.pdf"
curl --fail --silent --output "$downloaded" "$(jq -er .download_url <<<"$descriptor")"
assert_eq "$pdf_sha256" "$(sha256_of "$downloaded")" "downloaded attachment bytes differ from the upload"
reuse_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
  --header "Authorization: Bearer $PAYDAY_API_KEY" --header "Content-Type: application/json" \
  --header "Idempotency-Key: document-reuse-$run_id" --data "$document_body" "$API_URL/v1/payments")"
assert_eq 409 "$reuse_status" "an attached PDF was attached to a second invoice"

gated_body="$(jq -c '. + {heading: "Gated retainer",
  payer_policy: {mode: "verified_email", expected_email: "alice@example.test"}}' \
  <<<"$(invoice_body 2 "$BENEFICIARY_EXACT" 3600)")"
gated="$(curl --fail --silent \
  --header "Authorization: Bearer $PAYDAY_API_KEY" --header "Content-Type: application/json" \
  --header "Idempotency-Key: gated-$run_id" --data "$gated_body" "$API_URL/v1/payments")"
gated_id="$(jq -er .id <<<"$gated")"
gated_payer="$(curl --fail --silent "$API_URL/v1/payer/payments/$gated_id")"
assert_eq false "$(jq -r .content_unlocked <<<"$gated_payer")" "gated invoice unlocked its content"
assert_eq null "$(jq -r .amount <<<"$gated_payer")" "gated invoice revealed its amount"
assert_eq null "$(jq -r .address <<<"$gated_payer")" "gated invoice revealed its payment address"
assert_eq "Gated retainer" "$(jq -r .heading <<<"$gated_payer")" "gated invoice hides its heading"
assert_eq 401 "$(curl --silent --output /dev/null --write-out '%{http_code}' \
  "$API_URL/v1/payer/payments/$gated_id/attachment")" "gated invoice served its attachment"

# Payer email verification against the development identity provider: the
# gateway sends the code to the merchant-asserted mailbox and exchanges it on
# the payer's behalf; the payer supplies nothing but the code.
verify_payer_email() {
  local id=$1 started session
  started="$(curl --fail --silent --request POST --header "Origin: $PAYDAY_HOSTED_CHECKOUT_ORIGIN" \
    "$API_URL/v1/payer/payments/$id/verify/email/start")"
  session="$(jq -er .payer_session <<<"$started")"
  assert_eq 401 "$(curl --silent --output /dev/null --write-out '%{http_code}' --request POST \
    --header "Content-Type: application/json" --header "Payday-Payer-Session: $session" \
    --data '{"otp":"000000"}' "$API_URL/v1/payer/payments/$id/verify/email/confirm")" \
    "a wrong code was accepted"
  assert_eq 200 "$(curl --silent --output /dev/null --write-out '%{http_code}' --request POST \
    --header "Content-Type: application/json" --header "Payday-Payer-Session: $session" \
    --data "$(jq -cn --arg otp "$PAYDAY_DEV_IDENTITY_OTP" '{otp: $otp}')" \
    "$API_URL/v1/payer/payments/$id/verify/email/confirm")" \
    "the right code was not accepted"
  echo "$session"
}

echo "Testing that a gated request takes the wallet step only from a verified session"
gated_before="$(token_balance "$BENEFICIARY_EXACT")"
# No session, no wallet step: the identity policy comes first.
assert_eq 401 "$(curl --silent --output /dev/null --write-out '%{http_code}' --request POST \
  --header "Origin: $PAYDAY_HOSTED_CHECKOUT_ORIGIN" --header "Content-Type: application/json" \
  --data "$(jq -cn --arg wallet "$PAYER" '{wallet: $wallet}')" \
  "$API_URL/v1/payer/payments/$gated_id/wallet/challenge")" \
  "a gated request offered a wallet challenge without a verified session"
gated_session="$(verify_payer_email "$gated_id")"
gated_unlocked="$(curl --fail --silent --header "Payday-Payer-Session: $gated_session" \
  "$API_URL/v1/payer/payments/$gated_id")"
assert_eq true "$(jq -r .content_unlocked <<<"$gated_unlocked")" "verified session did not unlock the invoice"
assert_eq "Payday E2E Customer" "$(jq -r .invoice.bill_to.name <<<"$gated_unlocked")" "verified session did not see the document"
assert_eq null "$(jq -r .address <<<"$gated_unlocked")" "an unlocked request had an address before the wallet step"
assert_eq pending "$(jq -r .requirements.wallet <<<"$gated_unlocked")" "the wallet fact is not pending before the wallet step"
[[ "$(get_invoice "$gated_id" | jq -r .verification_completed_at)" != null ]] || {
  echo "the verified request does not record its verification completion" >&2
  exit 1
}
# The verified session signs; the request now has its address.
bind_payer_wallet "$gated_id" "$gated_session" >/dev/null
gated_bound="$(curl --fail --silent --header "Payday-Payer-Session: $gated_session" \
  "$API_URL/v1/payer/payments/$gated_id")"
gated_address="$(jq -er .address <<<"$gated_bound")"
assert_eq "$gated_address" "$(get_invoice "$gated_id" | jq -r .address)" "merchant and payer disagree on the address"
assert_eq null "$(curl --fail --silent "$API_URL/v1/payer/payments/$gated_id" | jq -r .address)" \
  "the bare link unlocked after another session verified"
send_usdc "$gated_address" 2000000
wait_for_invoice "$gated_id" '.received_base_units == "2000000" and .status == "paid"' \
  "verified payment credited"
# The invoice is fully funded, so even the verified session gets no QR: the
# address must not be offered for a second payment. The bare link is refused
# earlier, for want of a session.
assert_eq 410 "$(curl --silent --output /dev/null --write-out '%{http_code}' \
  --header "Payday-Payer-Session: $gated_session" "$API_URL/v1/payer/payments/$gated_id/qr")" \
  "a funded invoice offered its QR to the verified session"
assert_eq 401 "$(curl --silent --output /dev/null --write-out '%{http_code}' \
  "$API_URL/v1/payer/payments/$gated_id/qr")" "the bare link fetched the QR"
wait_for_status "$gated_id" settled
assert_eq "$((gated_before + 2000000))" "$(token_balance "$BENEFICIARY_EXACT")" \
  "verified invoice did not settle to the beneficiary"
assert_payment_deployed_and_empty "$gated_address"
assert_eq null "$(get_invoice "$gated_id" | jq -r .likely_unsolicited_at)" \
  "the payer's own transfer was flagged as unsolicited"

echo "Testing that funds from a wallet other than the attested one are flagged and earn no proof"
stranger_paid="$(create_invoice 1 "$BENEFICIARY_EXPIRED" 3600 "stranger-$run_id")"
stranger_id="$(jq -er .id <<<"$stranger_paid")"
stranger_address="$(jq -er .address <<<"$stranger_paid")"
# Fund the stranger, then let it pay the request bound to the payer.
send_usdc "$STRANGER" 1000000
cast send "$USDC" 'transfer(address,uint256)' "$stranger_address" 1000000 \
  --private-key "$STRANGER_KEY" --rpc-url "$RPC_URL" >/dev/null
wait_for_status "$stranger_id" settled
stranger_final="$(get_invoice "$stranger_id")"
[[ "$(jq -r .likely_unsolicited_at <<<"$stranger_final")" != null ]] || {
  echo "a stranger's transfer was not flagged as likely unsolicited" >&2
  exit 1
}
assert_eq "$(lowercase "$STRANGER")" "$(jq -r '.transfers[0].sender | ascii_downcase' <<<"$stranger_final")" \
  "the transfer's sender is not the stranger"
wait_for_sql 1 "SELECT count(*) FROM webhook_events
  WHERE invoice_id = '${stranger_id#pay_}'::uuid AND event_type = 'payment.likely_unsolicited'" \
  "the stranger's transfer did not raise payment.likely_unsolicited"
assert_eq 409 "$(api_status GET "/v1/payments/$stranger_id/proof")" \
  "a proof was served for a request paid by another wallet"
assert_eq payment_sender_mismatch "$(api_error_code GET "/v1/payments/$stranger_id/proof")" \
  "the refused proof reported the wrong error"

invoice_pdf="$logs/invoice.pdf"
api_json GET "/v1/payments/$documented_id/invoice.pdf" "" --output "$invoice_pdf"
assert_eq "%PDF-" "$(head -c 5 "$invoice_pdf")" "invoice document is not a PDF"

proof="$(api_json GET "/v1/payments/$exact_id/proof")"
jq -e '.version == "payday.proof.v2" and .payment_address != null and .salt != null
  and .attribution_hash != null and .payer_wallet.signature != null
  and .payer_wallet.typed_data.primaryType == "PayerAttestation"
  and .recovery_address == .payer_wallet.address
  and .settlement_transaction_hash != null
  and (.transfers | length) > 0 and (.transfers | all(.sender == $payer))
  and .verification.signature != null
  and .verification.payload.payer_wallet == $payer
  and (.verification.payload.facts | map(.kind) | index("wallet") != null)
  and .verification.signer == "0x976EA74026E726554dB657fA54763abd0C3a0aa9"' \
  --arg payer "$PAYER" <<<"$proof" >/dev/null || {
  echo "Proof of Payment is incomplete: $(jq -c . <<<"$proof")" >&2
  exit 1
}
# The proof names the fulfilment transaction (the batch sweep), not a payer
# transfer, and its attestation is bound to the issuance commitment.
assert_eq "$(get_invoice "$exact_id" | jq -r .settlement_tx_hash)" \
  "$(jq -r .settlement_transaction_hash <<<"$proof")" \
  "proof settlement_transaction_hash is not the payment's settlement_tx_hash"
assert_eq "$(jq -r .attribution_hash <<<"$proof")" \
  "$(jq -r .verification.payload.attribution_hash <<<"$proof")" \
  "attestation is not bound to the proof's attribution hash"
assert_eq "$(jq -r .payment_address <<<"$proof")" \
  "$(jq -r .verification.payload.payment_address <<<"$proof")" \
  "attestation is not bound to the proof's payment address"
assert_eq 409 "$(api_status GET "/v1/payments/$documented_id/proof")" \
  "a proof was served for an unsettled invoice"

not_pdf="$logs/not-a-pdf.pdf"
printf 'this is plain text, not a PDF' >"$not_pdf"
rejected_id="$(stage_attachment "$not_pdf" notes.pdf NO_THREATS_FOUND)"
assert_eq 422 "$(api_status POST "/v1/attachments/$rejected_id/finalize")" \
  "non-PDF bytes were finalized"
assert_eq attachment_rejected "$(api_error_code POST "/v1/attachments/$rejected_id/finalize")" \
  "rejected upload reported the wrong error"

assert_process_alive Anvil "$anvil_pid"
assert_process_alive gatewayd "$gatewayd_pid"
assert_process_alive gateway-indexer "$indexer_pid"

echo "All Anvil end-to-end flows passed"
