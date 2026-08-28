set shell := ["bash", "-euo", "pipefail", "-c"]

# Run PostgreSQL, Anvil, the contract bootstrap, API, and indexer locally.
dev:
    ./scripts/local-runner.sh dev

# Run the full Anvil end-to-end suite against an isolated container database.
e2e:
    ./scripts/local-runner.sh e2e

# Create/refresh the local development identity through gateway-cli.
seed:
    ./scripts/local-runner.sh seed
