#!/usr/bin/env bash
# A real deposit, end to end, against a live Payday API: issue a deposit
# request, bind the payer's wallet exactly as the hosted checkout does, pay
# it on-chain, wait for finalized settlement, and check the Proof of Payment.
# Staging by default (docs/staging.md); production with PAYDAY_API_URL and a
# live key, as the launch check in docs/production-runbook.md.
#
# The payout address defaults to the paying wallet, so the USDC comes straight
# back and the run costs only gas. Required:
#   PAYDAY_API_KEY   a merchant API key minted in the dashboard
#   PAYDAY_RPC_URL   an HTTPS Monad RPC the payer wallet sends through
#   PAYER_KEY        the private key of a wallet holding USDC and MON
set -euo pipefail

API_URL="${PAYDAY_API_URL:-https://api.staging.payday.sh}"
RPC_URL="${PAYDAY_RPC_URL:?set PAYDAY_RPC_URL to an HTTPS Monad RPC endpoint}"
: "${PAYDAY_API_KEY:?set PAYDAY_API_KEY to a merchant key minted in the dashboard}"
PAYER_KEY="${PAYER_KEY:?set PAYER_KEY to the private key of a wallet holding USDC and MON}"
USDC="${PAYDAY_USDC_ADDRESS:-0x754704Bc059F8C67012fEd69BC8A327a5aafb603}"
CHAIN_ID="${PAYDAY_CHAIN_ID:-143}"
# The origin the hosted checkout runs on; the wallet routes answer only it.
CHECKOUT_ORIGIN="${PAYDAY_HOSTED_CHECKOUT_ORIGIN:-http://127.0.0.1:3002}"
AMOUNT="${AMOUNT:-0.01}"
TIMEOUT_SECS="${TIMEOUT_SECS:-900}"
# The attestor address published for this environment; checked when set.
ATTESTOR="${PAYDAY_ATTESTOR:-}"
# With LATE_TRANSFER=1, a second transfer after settlement must come back.
LATE_TRANSFER="${LATE_TRANSFER:-0}"

for tool in cast curl jq; do
  command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 2; }
done

PAYER="$(cast wallet address --private-key "$PAYER_KEY")"
PAYOUT_ADDRESS="${PAYOUT_ADDRESS:-$PAYER}"
logs="$(mktemp -d "${TMPDIR:-/tmp}/payday-live-smoke.XXXXXX")"
trap 'rm -rf "$logs"' EXIT

merchant() {
  local method=$1 path=$2 body=${3:-}
  shift $(($# < 3 ? $# : 3))
  if [[ -n "$body" ]]; then
    curl --fail --silent --show-error --request "$method" \
      --header "Authorization: Bearer $PAYDAY_API_KEY" \
      --header "Content-Type: application/json" --data "$body" "$@" "$API_URL$path"
  else
    curl --fail --silent --show-error --request "$method" \
      --header "Authorization: Bearer $PAYDAY_API_KEY" "$@" "$API_URL$path"
  fi
}

payer_post() {
  local path=$1 body=$2 session=${3:-}
  local -a session_header=()
  if [[ -n "$session" ]]; then
    session_header=(--header "Payday-Payer-Session: $session")
  fi
  curl --fail --silent --show-error --request POST \
    --header "Origin: $CHECKOUT_ORIGIN" --header "Content-Type: application/json" \
    ${session_header[@]+"${session_header[@]}"} \
    --data "$body" "$API_URL$path"
}

token_balance() {
  cast call "$USDC" 'balanceOf(address)(uint256)' "$1" --rpc-url "$RPC_URL" | awk '{print $1}'
}

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

lower() {
  tr '[:upper:]' '[:lower:]' <<<"$1"
}

same_address() {
  [[ "$(lower "$1")" == "$(lower "$2")" ]]
}

# Poll the merchant view until a jq expression holds, printing each change
# of status, at the documented per-account rate.
wait_for() {
  local id=$1 expression=$2 description=$3 deadline last="" view status
  deadline=$((SECONDS + TIMEOUT_SECS))
  while ((SECONDS < deadline)); do
    view="$(merchant GET "/v1/deposit-requests/$id")"
    status="$(jq -r .status <<<"$view")"
    if [[ "$status" != "$last" ]]; then
      echo "  status: $status" >&2
      last="$status"
    fi
    if jq -e "$expression" <<<"$view" >/dev/null; then
      echo "$view"
      return
    fi
    sleep 5
  done
  fail "$id never reached '$description' within ${TIMEOUT_SECS}s; last: $(jq -c . <<<"$view")"
}

echo "Payday live smoke test"
echo "  API:     $API_URL"
echo "  chain:   $CHAIN_ID, USDC $USDC"
echo "  payer:   $PAYER"
echo "  payout:  $PAYOUT_ADDRESS"
echo "  amount:  $AMOUNT USDC"

# 0. The API is up, the key works, and the wallet can pay.
merchant GET /health >/dev/null
account="$(merchant GET /v1/account)"
echo "  account: $(jq -r .id <<<"$account")"
amount_base="$(cast to-wei "$AMOUNT" mwei)"
needed="$amount_base"
if [[ "$LATE_TRANSFER" == 1 ]]; then
  needed=$((amount_base + 1))
fi
usdc_before="$(token_balance "$PAYER")"
mon_before="$(cast balance "$PAYER" --rpc-url "$RPC_URL")"
((usdc_before >= needed)) || fail "payer holds $usdc_before base units of USDC, needs $needed"
[[ "$mon_before" != 0 ]] || fail "payer holds no MON for gas"

# 1. Issue: no address until the payer binds a wallet.
run_id="live-smoke-$(date +%s)-$RANDOM"
body="$(jq -cn --arg payout "$PAYOUT_ADDRESS" --arg amount "$AMOUNT" --arg ref "$run_id" \
  '{amount: $amount, payout_address: $payout, expires_in: 3600, reference: $ref,
    issuer: {name: "Payday"}, payer: {name: "Live smoke test"},
    payer_policy: {mode: "permissionless"}}')"
created="$(curl --fail --silent --show-error --request POST \
  --header "Authorization: Bearer $PAYDAY_API_KEY" \
  --header "Content-Type: application/json" \
  --header "Idempotency-Key: $run_id" \
  --data "$body" "$API_URL/v1/deposit-requests")"
id="$(jq -er .id <<<"$created")"
echo "issued $id -> $(jq -r .deposit_url <<<"$created")"
[[ "$(jq -r .address <<<"$created")" == null ]] || fail "a fresh request already had an address"

# 2. Bind the payer wallet: challenge, sign the EIP-712 typed data with cast
#    as a wallet would, attest.
challenge="$(payer_post "/v1/payer/deposit-requests/$id/wallet/challenge" \
  "$(jq -cn --arg wallet "$PAYER" --arg chain "$CHAIN_ID" '{wallet: $wallet, chain_id: $chain}')")"
session="$(jq -er .payer_session <<<"$challenge")"
typed="$logs/typed.json"
jq -c .typed_data <<<"$challenge" >"$typed"
[[ "$(jq -r .domain.chainId "$typed")" == "$CHAIN_ID" ]] \
  || fail "the challenge is for chain $(jq -r .domain.chainId "$typed"), expected $CHAIN_ID"
signature="$(cast wallet sign --private-key "$PAYER_KEY" --data --from-file "$typed")"
bound="$(payer_post "/v1/payer/deposit-requests/$id/wallet/attest" \
  "$(jq -cn --arg wallet "$PAYER" --arg signature "$signature" '{wallet: $wallet, signature: $signature}')" \
  "$session")"
address="$(jq -er .address <<<"$bound")"
echo "bound $PAYER -> deposit address $address"
view="$(merchant GET "/v1/deposit-requests/$id")"
same_address "$(jq -r .address <<<"$view")" "$address" || fail "merchant view does not carry the bound address"
same_address "$(jq -r .payer_wallet <<<"$view")" "$PAYER" || fail "merchant view does not name the payer wallet"
same_address "$(jq -r .recovery_address <<<"$view")" "$PAYER" || fail "the recovery address is not the payer wallet"

# 3. Pay from the attested wallet.
payout_before="$(token_balance "$PAYOUT_ADDRESS")"
tx="$(cast send "$USDC" 'transfer(address,uint256)' "$address" "$amount_base" \
  --private-key "$PAYER_KEY" --rpc-url "$RPC_URL" --json | jq -r .transactionHash)"
echo "sent $AMOUNT USDC in $tx; waiting for finality and settlement (up to ${TIMEOUT_SECS}s)"

# 4. Settlement: exactly the requested amount reaches the payout address and
#    the deposit address is left empty.
settled="$(wait_for "$id" '.status == "settled"' "settled")"
[[ "$(jq -r .received_base_units <<<"$settled")" == "$amount_base" ]] \
  || fail "received $(jq -r .received_base_units <<<"$settled") base units, expected $amount_base"
[[ "$(jq -r .settlement_tx_hash <<<"$settled")" != null ]] || fail "no settlement transaction recorded"
[[ "$(jq -r .likely_unsolicited_at <<<"$settled")" == null ]] || fail "the attested wallet's own transfer was flagged as unsolicited"
[[ "$(token_balance "$address")" == 0 ]] || fail "the deposit address still holds USDC"
if [[ "$PAYOUT_ADDRESS" == "$PAYER" ]]; then
  [[ "$(token_balance "$PAYER")" == "$usdc_before" ]] || fail "the payer did not get exactly $AMOUNT USDC back"
else
  [[ "$(token_balance "$PAYOUT_ADDRESS")" == $((payout_before + amount_base)) ]] \
    || fail "the payout address did not receive exactly $AMOUNT USDC"
fi
echo "settled in $(jq -r .settlement_tx_hash <<<"$settled")"

# 5. The Proof of Payment names this wallet, this address, and the settlement.
proof="$(merchant GET "/v1/deposit-requests/$id/proof")"
jq -e --arg payer "$(lower "$PAYER")" --arg address "$(lower "$address")" \
  '.version == "payday.proof.v3"
   and (.payment_address | ascii_downcase) == $address
   and (.payer_wallet.address | ascii_downcase) == $payer
   and .payer_wallet.typed_data.primaryType == "PayerAttestation"
   and (.recovery_address | ascii_downcase) == $payer
   and .settlement_transaction_hash != null
   and (.transfers | length) > 0 and (.transfers | all((.sender | ascii_downcase) == $payer))
   and .verification.signature != null
   and (.verification.payload.payer_wallet | ascii_downcase) == $payer
   and (.verification.payload.payment_address | ascii_downcase) == $address
   and .verification.payload.attribution_hash == .attribution_hash' <<<"$proof" >/dev/null \
  || fail "the proof is incomplete: $(jq -c . <<<"$proof")"
signer="$(jq -r .verification.signer <<<"$proof")"
[[ "$(jq -r .settlement_transaction_hash <<<"$proof")" == "$(jq -r .settlement_tx_hash <<<"$settled")" ]] \
  || fail "the proof's settlement transaction differs from the request's"
if [[ -n "$ATTESTOR" ]]; then
  same_address "$signer" "$ATTESTOR" \
    || fail "the proof is attested by $signer, not the published attestor $ATTESTOR"
fi
echo "proof attested by $signer"

# 6. Optionally, money sent after settlement goes back to the payer.
if [[ "$LATE_TRANSFER" == 1 ]]; then
  late_before="$(token_balance "$PAYER")"
  cast send "$USDC" 'transfer(address,uint256)' "$address" 1 \
    --private-key "$PAYER_KEY" --rpc-url "$RPC_URL" >/dev/null
  echo "sent a late transfer of 1 base unit; waiting for it to come back"
  deadline=$((SECONDS + TIMEOUT_SECS))
  while ((SECONDS < deadline)); do
    if [[ "$(token_balance "$PAYER")" == "$late_before" && "$(token_balance "$address")" == 0 ]]; then
      break
    fi
    sleep 5
  done
  [[ "$(token_balance "$PAYER")" == "$late_before" ]] || fail "the late transfer was not returned"
  [[ "$(merchant GET "/v1/deposit-requests/$id" | jq -r .status)" == settled ]] \
    || fail "a late transfer changed the request's status"
  echo "late transfer returned to $PAYER"
fi

mon_after="$(cast balance "$PAYER" --rpc-url "$RPC_URL")"
echo "OK: $id settled; payer spent $(cast from-wei $((mon_before - mon_after))) MON in gas"
