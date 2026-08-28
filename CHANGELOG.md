# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses semantic version numbers. The project is currently
pre-release software; the `0.1.0` version does not imply a stable public API.

## [Unreleased]

### Documentation

- Added a customer [quickstart](docs/quickstart.md),
  [concepts guide](docs/concepts.md), [CLI reference](docs/cli-reference.md),
  [HTTP API reference](docs/api-reference.md), and [FAQ](docs/faq.md).
- Added a customer-first repository landing page and documentation index.

## [0.1.0] - 2026-08-27

### Added

- Authenticated creation, listing, filtering, pagination, cancellation, and
  account-scoped lookup of single-use USDC payments by ID, prefix, or address.
- Merchant references and metadata, finalized transfer provenance, indexer
  freshness, explorer links, long polling, and signed hosted payer checkouts.
- Signed lifecycle webhooks with test events, delivery/attempt visibility,
  SSRF-resistant endpoints, and durable retries.
- Support for one configured EVM chain and its exact Circle native-USDC proxy
  per deployment, using six-decimal amounts and bounded invoice expirations.
- Finality-gated ERC-20 transfer indexing with cumulative partial payments,
  overpayments, expiry classification, and durable invoice settlement state.
- Batched on-chain sweeping to beneficiaries before expiry and recovery
  addresses after expiry, including collection of transfers sent after
  settlement.
- Customer lifecycle reporting for awaiting, partial, paid, settled, expired,
  returned, and attention states, with payout-support guidance.
- A `payday` CLI with email-OTP login, private endpoint-bound credentials,
  payment tracking/watch mode, key rotation/revocation, webhook management,
  JSON output, completions, built-in guides, and verified upgrades.
- OpenAPI 3.1 and interactive API references, a zero-runtime-dependency
  TypeScript client, checksum-verified release archives, install script,
  generated Homebrew formula, and isolated Monad testnet sandbox support.
- Public and authenticated service-status views with request IDs and
  per-account API rate limits.

### Security

- API keys are returned once and stored as SHA-256 digests; payment access is
  isolated by account. Rotation has a 24-hour deployment grace period and
  revocation immediately invalidates current and previous keys.
- Production signing supports AWS KMS, while local development supports an
  explicit signer key.

### Known limitations

- This is an initial pre-release; compatibility is not yet guaranteed.
- Payday does not initiate payer refunds. Cancellation is advisory and the
  merchant controls the payment's refund address.
- Accounts have one unscoped, unnamed key generation and no team/organization
  membership or source-IP restrictions.
- Each deployment supports one configured chain and one native-USDC contract.
