# Payday contracts

## Payment flow

[`PaymentFactory`](src/PaymentFactory.sol) is an ownerless, permissionless factory: `paymentAddress` predicts an invoice address and `execute` deploys its [`Payment`](src/Payment.sol), with the token, amount, receiver, expiration, recovery address, and caller-provided salt all committed into the deployment salt. The recovery address is the Payday recovery wallet, configured per environment by the platform; merchants do not choose it.

A `Payment` constructor routes whatever balance is present when it is deployed:

- **Live and fully funded.** It transfers exactly the requested amount to the receiver and emits `Settled(receiver, amount)`, where `amount` is always the invoice amount, never the balance that happened to be present. Anything above the invoice amount is an overpayment: it is transferred to the Payday recovery wallet in the same transaction and reported as `Recovered(recovery, remainder)`. An exact payment emits no `Recovered` event. If the recovery leg fails (for example, the token blacklists the recovery wallet) the whole deployment reverts, the receiver is not paid, and the balance stays at the address for a later attempt.
- **Live but underfunded.** It reverts with `InsufficientTokenBalance` and leaves no code, so the same address stays usable for the deposit request once the payment is completed.
- **Expired.** It transfers the whole balance, however partial, to the Payday recovery wallet and emits `Recovered`.

Deployment is a one-way door. Funds that arrive afterwards (a duplicate payment, or the late completion of a deposit request that already expired) can be forwarded to the recovery wallet by anyone through the permissionless `recover`, which emits `Recovered` for the amount moved. [`BatchSweeper`](src/BatchSweeper.sol) processes independent invoices in one transaction, deploying fresh payments or recovering late funds from existing ones, and reports per-item failures without reverting successful siblings; an overpaid item whose recovery leg fails is reported like any other failed deployment and its siblings still settle.

The intended requested amount therefore always moves directly from the payment address to the merchant's receiver; Payday never intermediates it. Overpayment remainders, expired balances, and late transfers are the only funds Payday takes custody of. They are held in the recovery wallet, recorded per invoice by the indexer, reviewed manually, and returned by the operator.

## Contract generations and code hashes

`PaymentFactory` embeds `Payment.creationCode`, and `BatchSweeper` holds its factory in an immutable. Any change to `Payment` is therefore a new generation of both contracts: deploy a new factory and a new sweeper bound to it together, and never point a new factory at an old sweeper or an old factory at a new sweeper. A new factory address also means new payment addresses for the same invoice parameters (see below). Because Payday is not in production, every environment moves to the new generation rather than supporting a mix.

`gatewayd` and `gateway-indexer` pin the generation they were configured for. Both read `PAYDAY_FACTORY_CODE_HASH` and `PAYDAY_BATCH_SWEEPER_CODE_HASH`, compare them at startup with the runtime bytecode deployed at `PAYDAY_FACTORY_ADDRESS` and `PAYDAY_BATCH_SWEEPER_ADDRESS`, check that `BatchSweeper.factory()` is the configured factory, and refuse to start on any mismatch. Each hash is the `keccak256` of the runtime bytecode returned by `eth_getCode`:

```sh
cast keccak "$(cast code "$PAYDAY_FACTORY_ADDRESS" --rpc-url "$RPC_URL")"
cast keccak "$(cast code "$PAYDAY_BATCH_SWEEPER_ADDRESS" --rpc-url "$RPC_URL")"
```

[`PaymentFactory.s.sol`](script/PaymentFactory.s.sol) prints both addresses and both hashes after deploying a generation, and [`Bootstrap.s.sol`](script/Bootstrap.s.sol) prints the hashes of the local fixtures on every run. Hashes are never hardcoded; scripts compute them from the running chain. The local fixture addresses are Anvil account #0's first CREATE addresses, so a long-running Anvil keeps whichever generation it saw first: the bootstrap refuses to run against a stale factory, a sweeper bound to another factory, or a sweeper whose runtime bytecode differs from this build, and the fix is to restart Anvil so the new generation deploys.

## Why CREATE3

`PaymentFactory` uses Solady's [`CREATE3`](lib/solady/src/utils/CREATE3.sol): `CREATE2` places a fixed proxy from the factory address and deployment salt, then that proxy creates `Payment`. The resulting payment address is therefore independent of `Payment` init code, so clients can derive and fund it before the payment contract exists. It is still specific to the factory address and every invoice parameter included in `deploymentSalt`; changing any of those produces another address.

## Rust/Solidity address parity

[`predict_payment_address`](../crates/gateway-core/src/deterministic_address.rs) reproduces the deployment-salt hash, Solady proxy `CREATE2` address, and proxy nonce-1 `CREATE` address in Rust. [`PaymentAddressTest.testFuzz_address_parity`](test/PaymentAddress.t.sol) fuzzes invoice parameters, obtains Solidity's `PaymentFactory.paymentAddress`, invokes the [`derive-address`](../crates/gateway-core/test-helpers/derive_address.rs) Rust binary through `vm.ffi`, and compares the results. From the repository root:

```sh
cargo build -p gateway-core --bin derive-address
forge test --match-contract PaymentAddressTest
```
