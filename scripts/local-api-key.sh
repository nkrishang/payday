#!/usr/bin/env bash
# Issue an API key for a local account through the real email-OTP flow, the
# same three calls the dashboard's API key management makes: ask the
# development identity provider for a code, exchange it for a merchant token,
# and spend that fresh token on POST /v1/account/api-key. The account is
# provisioned on first sight, so this also creates it.
#
#   scripts/local-api-key.sh [EMAIL]
#
# The one-time code is read from PAYDAY_DEV_IDENTITY_OTP when set (as `just e2e`
# sets it) and otherwise prompted for; the provider prints it in its log. The
# raw key is printed once as JSON on stdout and never stored anywhere.
set -euo pipefail

issuer="${PAYDAY_AUTH0_ISSUER:-http://127.0.0.1:3001}"
api_url="${PAYDAY_API_URL:-http://127.0.0.1:3000}"
client_id="${PAYDAY_AUTH0_CLIENT_ID:-payday-dashboard-local}"
audience="${PAYDAY_AUTH0_AUDIENCE:-payday-api-local}"

need() {
  command -v "$1" >/dev/null 2>&1 || { echo "required command not found: $1" >&2; exit 1; }
}
need curl
need jq

email="${1:-}"
if [[ -z "$email" ]]; then
  read -r -p "Email  › " email
fi
[[ "$email" == *@* ]] || { echo "an email address is required" >&2; exit 2; }

curl --fail --silent --show-error \
  --header "Content-Type: application/json" \
  --data "$(jq -cn --arg client_id "$client_id" --arg email "$email" \
    '{client_id: $client_id, connection: "email", email: $email, send: "code"}')" \
  "$issuer/passwordless/start" >/dev/null

otp="${PAYDAY_DEV_IDENTITY_OTP:-}"
if [[ -z "$otp" ]]; then
  read -r -p "Code (printed in the identity provider's log)  › " otp
fi

token="$(curl --fail --silent --show-error \
  --header "Content-Type: application/json" \
  --data "$(jq -cn --arg client_id "$client_id" --arg email "$email" --arg otp "$otp" --arg audience "$audience" \
    '{grant_type: "http://auth0.com/oauth/grant-type/passwordless/otp",
      client_id: $client_id, username: $email, otp: $otp, realm: "email", audience: $audience}')" \
  "$issuer/oauth/token" | jq -er .access_token)"

# The key issuance route wants the account's current generation, or none for
# an identity that has never held a key.
generation="$(curl --silent --show-error \
  --header "Authorization: Bearer $token" \
  "$api_url/v1/account/api-key" | jq -c '.generation // empty')"
body='{}'
if [[ -n "$generation" ]]; then
  body="$(jq -cn --argjson generation "$generation" '{expected_generation: $generation}')"
fi

curl --fail --silent --show-error \
  --header "Authorization: Bearer $token" \
  --header "Content-Type: application/json" \
  --data "$body" \
  "$api_url/v1/account/api-key"
echo
