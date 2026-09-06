# Payday

Payday is a stablecoin deposit gateway for merchants: create a deposit request through a small API, share its payer link, and let Payday bind the payer's attested wallet to a one-time USDC address, detect finalized transfers from that wallet, and settle exactly the requested amount to your wallet automatically. Partial deposits accumulate; overpayment remainders, expired balances, and late transfers go back to the payer's own wallet on-chain and are recorded per request; every observed transfer remains in an auditable PostgreSQL ledger; and every settled request yields an offline-verifiable Proof of Payment tying the document, the wallet, the address, and the transfers together.

## 60-second local quickstart

With Docker, Rust, Foundry (`anvil`, `cast`, and `forge`), `just`, `jq`, and the PostgreSQL client installed, run the repository's end-to-end flow. The recipe provisions an isolated PostgreSQL container, builds the workspace, starts Anvil and the Payday services, deploys local contracts, signs in through the development identity provider, exercises deposit and recovery flows, and cleans everything up:

```bash
just e2e
```

For a live local stack with multiplexed logs, run `just dev`, then sign in at the dashboard or run `just seed` in another shell for an API key. See [Local development](docs/local-development.md) for prerequisites and manual operation.

## API

`https://api.payday.sh/v1` is the product: create deposit requests, read and list deposits, manage customers and attachments, download proofs, and register webhooks with one bearer API key ([HTTP API reference](docs/api-reference.md)). TypeScript applications can use the zero-dependency client in [`sdk/typescript`](sdk/typescript/README.md). The key is minted in the dashboard, which signs in through Privy with an emailed one-time code and gives every account its own embedded wallet; see [Authentication and API keys](docs/authentication.md).

## Web

`payday.sh` — the landing page, the hosted checkout at `/pay/{id}`, and the
merchant dashboard at `/dashboard` — lives in [`web/`](web/). It is a Next.js
app built on `@payday/sdk`: the checkout consumes the public payer API and is
where every `deposit_url` points; the dashboard signs in through Privy with an
emailed code — every account gets its own embedded wallet, where deposits
settle by default — and uses the same merchant API as the SDK with Privy's
identity token, so no API key ever reaches the browser (see
[docs/dashboard.md](docs/dashboard.md)). Run it beside the local stack:

```bash
npm ci
just web
```

See [web/README.md](web/README.md).

## Documentation

Customer documentation is published at [payday.sh/docs](https://payday.sh/docs)
(`web/app/docs`): concepts, the dashboard, the hosted checkout, webhooks, the
SDK, the full API reference, and an architecture page for due diligence. The
Markdown below is the engineering record it draws on.

- [Customer quickstart](docs/quickstart.md)
- [Deposit concepts and lifecycle](docs/concepts.md)
- [HTTP API reference](docs/api-reference.md)
- [Customer FAQ](docs/faq.md)
- [Changelog](CHANGELOG.md)
- [Local development and configuration](docs/local-development.md)
- [Staging: main, live, driven from your machine](docs/staging.md)
- [Deposit requests API](docs/deposit-requests-api.md)
- [Authentication and API keys](docs/authentication.md)
- [Merchant dashboard](docs/dashboard.md)
- [USDC indexer architecture](docs/usdc-indexer-architecture.md)
- [Production deployment runbook](docs/production-runbook.md)
- [Operational runbook index](docs/runbooks/README.md)
- [End-to-end production smoke test](docs/runbooks/end-to-end-smoke-test.md)
- [Stuck deposit request diagnosis and recovery](docs/runbooks/stuck-deposit-request.md)
