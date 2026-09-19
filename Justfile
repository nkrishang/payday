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

# Serve gum.money — the landing page and hosted checkout — on port 3002.
# Port 3001 belongs to the local development identity provider.
web:
    npm run dev --workspace @gum/web

# Serve the web app on port 3002 against the staging API (docs/staging.md):
# sign in, mint keys, issue and pay deposit requests on a live stack running
# main. The staging values win over web/.env.local.
web-staging:
    set -a; source web/.env.staging; set +a; npm run dev --workspace @gum/web

# A real deposit end to end against a live API, staging by default. Needs
# GUM_API_KEY, GUM_RPC_URL, and PAYER_KEY (docs/staging.md).
live-smoke:
    ./scripts/live-smoke.sh

# Type-check, lint, and unit-test the TypeScript SDK and web app.
web-check:
    npm run build --workspace @gum/sdk
    npm test --workspace @gum/sdk
    npm run typecheck --workspace @gum/web
    npm run lint --workspace @gum/web
    npm test --workspace @gum/web

# Run the browser suite against a stubbed payer API, on both servers.
web-e2e:
    npm run test:e2e --workspace @gum/web
    npm run test:e2e:dev --workspace @gum/web

# Run the WithdrawalForwarder against the real USDC and CCTP V2 contracts on
# forks of Monad, Base and Arbitrum, and the EIP-3009 authorization a USDT
# withdrawal relays against the real USDT0 on Monad and Arbitrum. Public RPCs
# by default; override with GUM_FORK_RPC_URL_<chain id> for a paid endpoint.
forge-fork:
    GUM_FORK_TESTS=1 forge test --match-contract "ForwarderForkTest|Usdt0ForkTest" -vv
