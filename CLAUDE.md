# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Constitution

@AGENTS.md is the authoritative project constitution — mission, product flow, protocol
invariants, decision status, architecture, and reliability rules. Read it before making
non-trivial changes. It is imported above, so its full content is loaded every session.
Update it (not this file) when a durable decision changes.

## Layout

Cargo workspace + a **separate Foundry project rooted at `foundry/`** (not the repo root).

- `crates/gateway-core` — domain types, invariants, CREATE3 derivation.
- `crates/gatewayd` — Axum server + workers; SQLx/Alloy adapters live here as modules.
- `crates/gateway-cli` — HTTP client (must go through the API, never the DB).
- `foundry/` — Solidity contracts. `foundry.toml` redirects `src`/`test`/`out`/`cache` under
  `foundry/`, so run `forge` commands from the repo root, not from inside `foundry/`.
- Rust edition is **2024** (needs a current toolchain).

## Build & test

- Rust: `cargo build` / `cargo test` / `cargo clippy` / `cargo fmt` (workspace-wide).
- Contracts: `forge build` / `forge test` from the repo root.
- **Cross-language CREATE3 parity gotcha:** `foundry/test/PaymentAddress.t.sol` uses `vm.ffi`
  to shell out to the prebuilt `target/debug/derive-address` binary. Build it before running
  Foundry tests, or that test fails:
  `cargo build -p gateway-core --bin derive-address && forge test`.
  (`ffi = true` is enabled in `foundry.toml`.)

## Database

- Real PostgreSQL via SQLx — the DB is the durable source of truth, never process memory.
- There is **no `.sqlx` offline cache**, so SQLx's compile-time query macros need a live
  `DATABASE_URL`. It's set in `.env` (gitignored) alongside `GATEWAY_BIND_ADDR`, `RUST_LOG`,
  `GATEWAY_CHAIN_ID`, `GATEWAY_FACTORY_ADDRESS`.
- Migrations live in `crates/gatewayd/migrations/`.

## Non-negotiables (see AGENTS.md for the full list)

- Token amounts are integer base-unit `U256` — **never** `f32`/`f64`. API takes human-readable
  strings; convert once at the boundary.
- Every state transition must be **idempotent** (events/jobs are at-least-once).
- Typed `thiserror` domain errors; avoid `unwrap`/`expect` in runtime paths. Never log secrets.
- Keep API DTOs distinct from DB rows and domain models.
