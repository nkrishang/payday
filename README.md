# Payday

Payday is a stablecoin payment gateway for merchants: create a payment through a small API or CLI, share its one-time USDC address or payer link, and let Payday detect finalized transfers and settle the full balance automatically. Partial payments accumulate, overpayments are forwarded, expired funds go to the refund address, and every observed transfer remains in an auditable PostgreSQL ledger.

## 60-second local quickstart

With Docker, Rust, Foundry (`anvil`, `cast`, and `forge`), `just`, `jq`, and the PostgreSQL client installed, run the repository's end-to-end flow. The recipe provisions an isolated PostgreSQL container, builds the workspace, starts Anvil and the Payday services, deploys local contracts, signs in through the development identity provider, exercises payment and recovery flows, and cleans everything up:

```bash
just e2e
```

For a live local stack with multiplexed logs, run `just dev`, then `just seed` in another shell. See [Local development](docs/local-development.md) for prerequisites and manual operation.

## CLI

Install the repository's `payday` binary with Cargo:

```bash
cargo install --path crates/gateway-cli --locked
```

The CLI provides polished login, payment creation and tracking, payer links, account-key management, and webhook operations. Local credentials are saved under a separate profile; production login uses Auth0. See [Install](docs/install.md) and [Authentication and API keys](docs/authentication.md).

## Documentation

- [Customer quickstart](docs/quickstart.md)
- [Payment concepts and lifecycle](docs/concepts.md)
- [CLI reference](docs/cli-reference.md)
- [HTTP API reference](docs/api-reference.md)
- [Customer FAQ](docs/faq.md)
- [Changelog](CHANGELOG.md)
- [Local development and configuration](docs/local-development.md)
- [CLI installation](docs/install.md)
- [Payments API](docs/payments-api.md)
- [Authentication and API keys](docs/authentication.md)
- [USDC indexer architecture](docs/usdc-indexer-architecture.md)
- [Production deployment runbook](docs/production-runbook.md)
- [Operational runbook index](docs/runbooks/README.md)
- [End-to-end production smoke test](docs/runbooks/end-to-end-smoke-test.md)
- [Stuck invoice diagnosis and recovery](docs/runbooks/stuck-invoice.md)
