set shell := ["bash", "-euo", "pipefail", "-c"]

# Run PostgreSQL, Anvil, the contract bootstrap, API, and indexer locally.
dev:
    ./scripts/local-runner.sh dev

# Run the full Anvil end-to-end suite against an isolated container database.
e2e:
    ./scripts/local-runner.sh e2e

# Create a local account through the email-OTP flow and print its API key once.
seed *ARGS:
    ./scripts/local-runner.sh seed {{ARGS}}

# Serve payday.sh — the landing page and hosted checkout — on port 3002.
# Port 3001 belongs to the local development identity provider.
web:
    npm run dev --workspace @payday/web

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
