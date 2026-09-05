#!/usr/bin/env bash
# Mint a local API key without a browser.
#
# Merchants sign in through Privy, which has no local stand-in: a real sign-in
# needs the dashboard, a real mailbox, and a real code. Scripts and the Anvil
# end-to-end suite only need *a* key for *an* account, so this writes one
# straight into the local database the runner owns — an account row with the
# key's SHA-256, and an identity row so the account looks exactly like one the
# API would have provisioned. Never point it at anything but a local database.
#
#   scripts/local-api-key.sh [EMAIL]      prints the plaintext key on stdout
#
# Reads DATABASE_URL, or finds the runner's PostgreSQL container when unset.
set -euo pipefail

email="${1:-dev@example.test}"
case "$email" in
  *@*) ;;
  *) echo "usage: $0 [EMAIL]" >&2; exit 2 ;;
esac

# The same shape gatewayd mints: the configured prefix and 32 random bytes,
# base64url without padding.
prefix="${PAYDAY_API_KEY_PREFIX:-payday_test_}"
case "$prefix" in
  payday_test_|payday_live_) ;;
  *) echo "PAYDAY_API_KEY_PREFIX must be payday_test_ or payday_live_" >&2; exit 2 ;;
esac
random="$(head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=\n')"
key="${prefix}${random}"
hash="$(printf '%s' "$key" | shasum -a 256 | cut -d' ' -f1)"
hint="…${key: -6}"
account_id="$(uuidgen | tr '[:upper:]' '[:lower:]')"
lowered="$(printf '%s' "$email" | tr '[:upper:]' '[:lower:]')"

sql="$(cat <<SQL
BEGIN;
INSERT INTO accounts (id, api_key_hash, api_key_hint, email)
VALUES ('${account_id}', decode('${hash}', 'hex'), '${hint}', '${lowered}');
INSERT INTO account_identities (issuer, subject, account_id)
VALUES ('local-seed', 'email|${lowered}', '${account_id}');
COMMIT;
SQL
)"

if [[ -n "${DATABASE_URL:-}" ]] && command -v psql >/dev/null 2>&1; then
  psql "$DATABASE_URL" --quiet --set ON_ERROR_STOP=1 --command "$sql" >/dev/null
else
  container="$(docker ps --filter 'name=payday-postgres-' --format '{{.Names}}' | head -n 1)"
  [[ -n "$container" ]] || {
    echo "no local database: set DATABASE_URL, or start the stack with 'just dev'" >&2
    exit 1
  }
  docker exec -i "$container" psql -U payday -d gateway --quiet --set ON_ERROR_STOP=1 \
    --command "$sql" >/dev/null
fi

echo "$key"
