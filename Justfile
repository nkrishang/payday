set shell := ["bash", "-euo", "pipefail", "-c"]

# Run PostgreSQL, Anvil, the contract bootstrap, API, and indexer locally.
dev:
    ./scripts/local-runner.sh dev

# Run the full Anvil end-to-end suite against an isolated container database.
e2e:
    ./scripts/local-runner.sh e2e

# Mint a local account and API key straight into the running dev database
# (merchant sign-in itself is Privy, which has no local stand-in).
seed EMAIL="dev@example.test":
    SEED_EMAIL="{{EMAIL}}" ./scripts/local-runner.sh seed

# Serve payday.sh — the landing page and hosted checkout — on port 3002.
# Port 3001 belongs to the local development identity provider.
web:
    npm run dev --workspace @payday/web

# Serve the web app on port 3002 against the staging API (docs/staging.md):
# sign in, mint keys, issue and pay deposit requests on a live stack running
# main. The staging values win over web/.env.local.
web-staging:
    set -a; source web/.env.staging; set +a; npm run dev --workspace @payday/web

# A real deposit end to end against a live API, staging by default. Needs
# PAYDAY_API_KEY, PAYDAY_RPC_URL, and PAYER_KEY (docs/staging.md).
live-smoke:
    ./scripts/live-smoke.sh

# Type-check, lint, and unit-test the TypeScript SDK and web app.
web-check:
    npm run build --workspace @payday/sdk
    npm test --workspace @payday/sdk
    npm run typecheck --workspace @payday/web
    npm run lint --workspace @payday/web
    npm test --workspace @payday/web

# Run the browser suite against a stubbed payer API, on both servers.
web-e2e:
    npm run test:e2e --workspace @payday/web
    npm run test:e2e:dev --workspace @payday/web
