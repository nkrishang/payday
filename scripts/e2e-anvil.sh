#!/usr/bin/env bash
set -euo pipefail

RPC_URL="${GATEWAY_RPC_URL:-http://127.0.0.1:8545}"
API_URL="${GATEWAY_API_URL:-http://127.0.0.1:3000}"
FACTORY="${GATEWAY_FACTORY_ADDRESS:-0x5FbDB2315678afecb367f032d93F642f64180aa3}"
USDC="${GATEWAY_USDC_ADDRESS:-0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512}"
SIGNER_KEY="${GATEWAY_SIGNER_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}"
PAYER_KEY="0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
BENEFICIARY_EXACT="0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"
BENEFICIARY_PARTIAL="0x90F79bf6EB2c4f870365E785982E1f101E93b906"
BENEFICIARY_OVERPAYMENT="0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65"
BENEFICIARY_EXPIRED="0x976EA74026E726554dB657fA54763abd0C3a0aa9"
RECOVERY="0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"

export GATEWAY_RPC_URL="$RPC_URL"
export GATEWAY_API_URL="$API_URL"
export GATEWAY_FACTORY_ADDRESS="$FACTORY"
export GATEWAY_USDC_ADDRESS="$USDC"
export GATEWAY_CHAIN_ID="${GATEWAY_CHAIN_ID:-31337}"
export GATEWAY_USDC_START_BLOCK="${GATEWAY_USDC_START_BLOCK:-0}"
export GATEWAY_FINALITY_CONFIRMATIONS="${GATEWAY_FINALITY_CONFIRMATIONS:-0}"
export GATEWAY_INDEXER_POLL_INTERVAL_MS="${GATEWAY_INDEXER_POLL_INTERVAL_MS:-100}"
export GATEWAY_SIGNER_KEY="$SIGNER_KEY"
export GATEWAY_API_KEY="${GATEWAY_API_KEY:-0123456789abcdef0123456789abcdef}"

logs="$(mktemp -d)"
pids=()

cleanup() {
  local status=$?
  trap - EXIT INT TERM

  if ((status != 0)); then
    for log in "$logs"/*.log; do
      [[ -e "$log" ]] || continue
      echo "--- $log ---" >&2
      tail -n 200 "$log" >&2
    done
  fi

  if ((${#pids[@]})); then
    kill "${pids[@]}" 2>/dev/null || true
    wait "${pids[@]}" 2>/dev/null || true
  fi
  rm -rf "$logs"
  exit "$status"
}
trap cleanup EXIT INT TERM

for command in anvil cast curl forge jq psql; do
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

create_invoice() {
  local amount=$1 beneficiary=$2 expiration=$3 idempotency_key=$4
  ./target/debug/gateway-cli --json invoice create \
    --chain-id "$GATEWAY_CHAIN_ID" \
    --token "$USDC" \
    --beneficiary "$beneficiary" \
    --expiration-timestamp "$expiration" \
    --recovery "$RECOVERY" \
    --amount "$amount" \
    --idempotency-key "$idempotency_key"
}

get_invoice() {
  ./target/debug/gateway-cli --json invoice get "$1"
}

wait_for_status() {
  local id=$1 expected=$2 status
  for _ in {1..200}; do
    status="$(get_invoice "$id" | jq -r .status)"
    if [[ "$status" == "$expected" ]]; then
      return
    fi
    sleep 0.1
  done
  echo "invoice $id did not reach $expected (last status: $status)" >&2
  return 1
}

wait_for_received() {
  local id=$1 expected=$2 received
  for _ in {1..200}; do
    received="$(psql "$DATABASE_URL" --tuples-only --no-align \
      --command "SELECT confirmed_received FROM invoices WHERE id = '$id'::uuid")"
    if [[ "$received" == "$expected" ]]; then
      return
    fi
    sleep 0.1
  done
  echo "invoice $id did not record $expected base units (last value: $received)" >&2
  return 1
}

token_balance() {
  cast call "$USDC" 'balanceOf(address)(uint256)' "$1" --rpc-url "$RPC_URL" | awk '{print $1}'
}

send_usdc() {
  cast send "$USDC" 'transfer(address,uint256)' "$1" "$2" \
    --private-key "$PAYER_KEY" --rpc-url "$RPC_URL" >/dev/null
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

echo "Starting Anvil and deploying local fixtures"
anvil --chain-id "$GATEWAY_CHAIN_ID" --silent >"$logs/anvil.log" 2>&1 &
anvil_pid=$!
pids+=("$anvil_pid")
wait_for_rpc
forge script foundry/script/Bootstrap.s.sol:BootstrapScript \
  --rpc-url "$RPC_URL" --private-key "$SIGNER_KEY" --broadcast >/dev/null

echo "Starting gateway services"
./target/debug/gatewayd >"$logs/gatewayd.log" 2>&1 &
gatewayd_pid=$!
pids+=("$gatewayd_pid")
wait_for_api
./target/debug/gateway-indexer >"$logs/indexer.log" 2>&1 &
indexer_pid=$!
pids+=("$indexer_pid")

expiration="$(( $(date +%s) + 3600 ))"
run_id="${GITHUB_RUN_ID:-local}-$(date +%s)-$$"

echo "Testing exact payment and API idempotency"
exact="$(create_invoice 1.5 "$BENEFICIARY_EXACT" "$expiration" "exact-payment-$run_id")"
exact_replay="$(create_invoice 1.5 "$BENEFICIARY_EXACT" "$expiration" "exact-payment-$run_id")"
exact_id="$(jq -r .id <<<"$exact")"
assert_eq "$exact_id" "$(jq -r .id <<<"$exact_replay")" "idempotent replay created another invoice"
exact_address="$(jq -r .payment_address <<<"$exact")"
exact_before="$(token_balance "$BENEFICIARY_EXACT")"
send_usdc "$exact_address" 1500000
wait_for_status "$exact_id" fulfilled
assert_eq "$((exact_before + 1500000))" "$(token_balance "$BENEFICIARY_EXACT")" \
  "exact payment beneficiary balance mismatch"
assert_payment_deployed_and_empty "$exact_address"

echo "Testing cumulative partial payments"
partial="$(create_invoice 1 "$BENEFICIARY_PARTIAL" "$expiration" "partial-payment-$run_id")"
partial_id="$(jq -r .id <<<"$partial")"
partial_address="$(jq -r .payment_address <<<"$partial")"
partial_before="$(token_balance "$BENEFICIARY_PARTIAL")"
send_usdc "$partial_address" 400000
assert_eq 400000 "$(token_balance "$partial_address")" "first partial payment was not retained"
wait_for_received "$partial_id" 400000
assert_eq created "$(get_invoice "$partial_id" | jq -r .status)" \
  "underfunded invoice advanced prematurely"
send_usdc "$partial_address" 600000
wait_for_status "$partial_id" fulfilled
assert_eq "$((partial_before + 1000000))" "$(token_balance "$BENEFICIARY_PARTIAL")" \
  "partial payment beneficiary balance mismatch"
assert_payment_deployed_and_empty "$partial_address"

echo "Testing overpayment full-balance sweep"
overpayment="$(create_invoice 1 "$BENEFICIARY_OVERPAYMENT" "$expiration" "overpayment-$run_id")"
overpayment_id="$(jq -r .id <<<"$overpayment")"
overpayment_address="$(jq -r .payment_address <<<"$overpayment")"
overpayment_before="$(token_balance "$BENEFICIARY_OVERPAYMENT")"
send_usdc "$overpayment_address" 1250000
wait_for_status "$overpayment_id" fulfilled
assert_eq "$((overpayment_before + 1250000))" "$(token_balance "$BENEFICIARY_OVERPAYMENT")" \
  "overpayment beneficiary balance mismatch"
assert_payment_deployed_and_empty "$overpayment_address"

echo "Testing permissionless recovery of an expired partial payment"
expired_at="$(( $(date +%s) + 60 ))"
expired="$(create_invoice 1 "$BENEFICIARY_EXPIRED" "$expired_at" "expired-recovery-$run_id")"
expired_id="$(jq -r .id <<<"$expired")"
expired_address="$(jq -r .payment_address <<<"$expired")"
expired_salt="$(jq -r .salt <<<"$expired")"
recovery_before="$(token_balance "$RECOVERY")"
send_usdc "$expired_address" 400000
wait_for_received "$expired_id" 400000
assert_eq created "$(get_invoice "$expired_id" | jq -r .status)" \
  "expired test invoice should remain underfunded"
cast rpc --rpc-url "$RPC_URL" evm_setNextBlockTimestamp "$((expired_at + 1))" >/dev/null
cast rpc --rpc-url "$RPC_URL" evm_mine >/dev/null
cast send "$FACTORY" \
  'execute(address,uint256,address,uint64,address,bytes32)' \
  "$USDC" 1000000 "$BENEFICIARY_EXPIRED" "$expired_at" "$RECOVERY" "$expired_salt" \
  --private-key "$PAYER_KEY" --rpc-url "$RPC_URL" >/dev/null
assert_eq "$((recovery_before + 400000))" "$(token_balance "$RECOVERY")" \
  "expired payment was not sent to recovery"
assert_payment_deployed_and_empty "$expired_address"

assert_process_alive Anvil "$anvil_pid"
assert_process_alive gatewayd "$gatewayd_pid"
assert_process_alive gateway-indexer "$indexer_pid"

echo "All Anvil end-to-end flows passed"
