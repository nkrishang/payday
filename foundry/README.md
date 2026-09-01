# Payday contracts

## Payment flow

[`PaymentFactory`](src/PaymentFactory.sol) is an ownerless, permissionless factory: `paymentAddress` predicts an invoice address and `execute` deploys its [`Payment`](src/Payment.sol), with the token, amount, receiver, expiration, recovery address, and caller-provided salt all committed into the deployment salt. A `Payment` constructor sweeps the full token balance to the receiver while the invoice is live (and reverts if underfunded), or sweeps any balance to recovery after expiry; its permissionless `recover` sends funds received after deployment to that same recovery address. [`BatchSweeper`](src/BatchSweeper.sol) processes independent invoices in one transaction, deploying fresh payments or recovering late funds from existing ones, and reports per-item failures without reverting successful siblings.

## Why CREATE3

`PaymentFactory` uses Solady's [`CREATE3`](lib/solady/src/utils/CREATE3.sol): `CREATE2` places a fixed proxy from the factory address and deployment salt, then that proxy creates `Payment`. The resulting payment address is therefore independent of `Payment` init code, so clients can derive and fund it before the payment contract exists. It is still specific to the factory address and every invoice parameter included in `deploymentSalt`; changing any of those produces another address.

## Rust/Solidity address parity

[`predict_payment_address`](../crates/gateway-core/src/deterministic_address.rs) reproduces the deployment-salt hash, Solady proxy `CREATE2` address, and proxy nonce-1 `CREATE` address in Rust. [`PaymentAddressTest.testFuzz_address_parity`](test/PaymentAddress.t.sol) fuzzes invoice parameters, obtains Solidity's `PaymentFactory.paymentAddress`, invokes the [`derive-address`](../crates/gateway-core/test-helpers/derive_address.rs) Rust binary through `vm.ffi`, and compares the results. From the repository root:

```sh
cargo build -p gateway-core --bin derive-address
forge test --match-contract PaymentAddressTest
```
