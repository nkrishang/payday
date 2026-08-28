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
RECOVERY="0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"

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
export PAYDAY_PAYER_TOKEN_SECRET="${PAYDAY_PAYER_TOKEN_SECRET:-local-payer-token-secret-0123456789abcdef}"
export PAYDAY_ADMIN_BEARER_SECRET="${PAYDAY_ADMIN_BEARER_SECRET:-local-admin-bearer-secret-0123456789abcdef}"
export PAYDAY_ADMIN_SECRET="${PAYDAY_ADMIN_SECRET:-$PAYDAY_ADMIN_BEARER_SECRET}"
export PAYDAY_API_KEY_PREFIX="payday_test_"
export PAYDAY_DEV_IDENTITY=1
export PAYDAY_DEV_IDENTITY_OTP="${PAYDAY_DEV_IDENTITY_OTP:-123456}"
export PAYDAY_DEV_IDENTITY_BIND="127.0.0.1:3001"
export PAYDAY_DEV_IDENTITY_ISSUER="http://127.0.0.1:3001"
export PAYDAY_AUTH0_ISSUER="$PAYDAY_DEV_IDENTITY_ISSUER"
export PAYDAY_AUTH0_CLIENT_ID="payday-cli-local"
export PAYDAY_AUTH0_AUDIENCE="payday-api-local"

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
  local amount=$1 beneficiary=$2 expires_in=$3 idempotency_key=$4
  curl --fail --silent \
    --header "Authorization: Bearer $PAYDAY_API_KEY" \
    --header "Content-Type: application/json" \
    --header "Idempotency-Key: $idempotency_key" \
    --data "$(invoice_body "$amount" "$beneficiary" "$expires_in")" \
    "$API_URL/v1/payments"
}

get_invoice() {
  ./target/debug/payday --json get "$1"
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
# is involved (bash 3.2 brace-expands nested quotes inside "$(...)").
invoice_body() {
  local amount=$1 beneficiary=$2 expires_in=$3
  jq -cn --arg chain "$PAYDAY_CHAIN_ID" --arg token "$USDC" --arg beneficiary "$beneficiary" \
    --arg amount "$amount" --argjson expires_in "$expires_in" --arg recovery "$RECOVERY" \
    '{chain_id: $chain, token_address: $token, payout_address: $beneficiary,
      amount: $amount, expires_in: $expires_in, refund_address: $recovery}'
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

echo "Starting gateway services"
./target/debug/payday-dev-identity >"$logs/dev-identity.log" 2>&1 &
identity_pid=$!
pids+=("$identity_pid")
for _ in {1..100}; do
  curl --fail --silent --output /dev/null "$PAYDAY_AUTH0_ISSUER/.well-known/jwks.json" && break
  sleep 0.1
done
curl --fail --silent --output /dev/null "$PAYDAY_AUTH0_ISSUER/.well-known/jwks.json" || {
  echo "development identity provider did not become ready" >&2
  exit 1
}
./target/debug/gatewayd >"$logs/gatewayd.log" 2>&1 &
gatewayd_pid=$!
pids+=("$gatewayd_pid")
wait_for_api
echo "Creating accounts through the real local email-OTP CLI flow"
export PAYDAY_CONFIG_DIR="$logs/payday-config"
primary_login="$(printf 'primary@example.test\n%s\n' "$PAYDAY_DEV_IDENTITY_OTP" | ./target/debug/payday --profile local --json login --yes --show)"
PAYDAY_API_KEY="$(jq -er .api_key <<<"$primary_login")"
export PAYDAY_API_KEY
second_login="$(printf 'secondary@example.test\n%s\n' "$PAYDAY_DEV_IDENTITY_OTP" | ./target/debug/payday --profile local --json login --yes --show)"
SECOND_API_KEY="$(jq -er .api_key <<<"$second_login")"
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

echo "Testing exact payment and API idempotency"
exact="$(create_invoice 1.5 "$BENEFICIARY_EXACT" 3600 "exact-payment-$run_id")"
exact_replay="$(create_invoice 1.5 "$BENEFICIARY_EXACT" 3600 "exact-payment-$run_id")"
exact_id="$(jq -r .id <<<"$exact")"
assert_eq "$exact_id" "$(jq -r .id <<<"$exact_replay")" "idempotent replay created another invoice"
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

echo "Testing overpayment full-balance sweep"
overpayment="$(create_invoice 1 "$BENEFICIARY_OVERPAYMENT" 3600 "overpayment-$run_id")"
overpayment_id="$(jq -r .id <<<"$overpayment")"
overpayment_address="$(jq -r .address <<<"$overpayment")"
overpayment_before="$(token_balance "$BENEFICIARY_OVERPAYMENT")"
send_usdc "$overpayment_address" 1250000
wait_for_status "$overpayment_id" settled
assert_eq "$((overpayment_before + 1250000))" "$(token_balance "$BENEFICIARY_OVERPAYMENT")" \
  "overpayment beneficiary balance mismatch"
assert_payment_deployed_and_empty "$overpayment_address"

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

echo "Testing that a transfer after settlement is forwarded to the recovery address"
recovery_before="$(token_balance "$RECOVERY")"
send_usdc "$batch_one_address" 7
send_usdc "$batch_one_address" 0
wait_for_sql 1 "SELECT count(*) FROM payment_observations
  WHERE invoice_id = '${batch_one_id#pay_}'::uuid AND disposition = 'late' AND collected_at_block IS NOT NULL" \
  "late transfer was not collected"
assert_eq "$((recovery_before + 7))" "$(token_balance "$RECOVERY")" "late transfer did not reach the recovery address"
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
  "$USDC" 2000000 "$BENEFICIARY_THIRD_PARTY" "$third_expiration" "$RECOVERY" "$third_salt" \
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
./target/debug/payday --json ops release "$blacklisted_id" >/dev/null
wait_for_status "$blacklisted_id" settled
assert_eq 250000 "$(token_balance "$BENEFICIARY_BLACKLISTED")" "released invoice was not settled"

echo "Testing automatic recovery of an expired partial payment and its late completion"
expired="$(create_invoice 1 "$BENEFICIARY_EXPIRED" 660 "expired-recovery-$run_id")"
expired_id="$(jq -r .id <<<"$expired")"
expired_address="$(jq -r .address <<<"$expired")"
recovery_before="$(token_balance "$RECOVERY")"
send_usdc "$expired_address" 400000
wait_for_invoice "$expired_id" '.received_base_units == "400000" and .status == "partially_paid"' "partial payment credited"
# Chain time passes the deadline; wall-clock stays where it is.
cast rpc --rpc-url "$RPC_URL" evm_increaseTime 700 >/dev/null
wait_for_status "$expired_id" returned
assert_eq "$((recovery_before + 400000))" "$(token_balance "$RECOVERY")" \
  "expired partial payment was not sent to recovery"
assert_payment_deployed_and_empty "$expired_address"
assert_eq false "$(cast call "$expired_address" 'settled()(bool)' --rpc-url "$RPC_URL")" \
  "deployed Payment must record recovery"
send_usdc "$expired_address" 600000
wait_for_sql 1 "SELECT count(*) FROM payment_observations
  WHERE invoice_id = '${expired_id#pay_}'::uuid AND amount = '600000' AND disposition = 'late' AND collected_at_block IS NOT NULL" \
  "late completion was not collected"
assert_eq "$((recovery_before + 1000000))" "$(token_balance "$RECOVERY")" \
  "late completion was not forwarded to recovery"
assert_eq returned "$(get_invoice "$expired_id" | jq -r .status)" "late completion changed the payment status"
assert_eq 0 "$(token_balance "$expired_address")" "late completion stranded at the payment address"

echo "Checking that every helper transaction batch resolved"
assert_eq 0 "$(psql "$DATABASE_URL" --tuples-only --no-align --command "
  SELECT count(*) FROM sweep_batches WHERE resolved_at IS NULL
")" "a sweep batch is still open"
assert_eq 0 "$(psql "$DATABASE_URL" --tuples-only --no-align --command "
  SELECT count(*) FROM invoices WHERE uncollected_count > 0 AND blocked_reason IS NULL
")" "collectable funds remain queued"

assert_process_alive Anvil "$anvil_pid"
assert_process_alive gatewayd "$gatewayd_pid"
assert_process_alive gateway-indexer "$indexer_pid"

echo "All Anvil end-to-end flows passed"
