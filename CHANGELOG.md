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

- Authenticated creation and account-scoped lookup of single-use USDC invoices,
  with idempotent creation keys and deterministic counterfactual payment
  addresses.
- Support for one configured EVM chain and its exact Circle native-USDC proxy
  per deployment, using six-decimal amounts and bounded invoice expirations.
- Finality-gated ERC-20 transfer indexing with cumulative partial payments,
  overpayments, expiry classification, and durable invoice settlement state.
- Batched on-chain sweeping to beneficiaries before expiry and recovery
  addresses after expiry, including collection of transfers sent after
  settlement.
- Invoice lifecycle reporting for created, funded, deploying, fulfilled,
  expired, recovered, and blocked states, with credited amounts and settlement
  transaction details.
- A CLI for email-OTP account provisioning, immediate single-key replacement,
  invoice creation, invoice lookup, and JSON output.

### Security

- API keys are returned once and stored as SHA-256 digests; invoice access is
  isolated by account.
- Production signing supports AWS KMS, while local development supports an
  explicit signer key.

### Known limitations

- This is an initial pre-release. The API has no invoice list, webhook, or
  refund endpoint.
- Each deployment supports one configured chain and one native-USDC contract.
