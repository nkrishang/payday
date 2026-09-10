# Payday contracts

## Payment flow

[`PaymentFactory`](src/PaymentFactory.sol) is an ownerless, permissionless factory: `paymentAddress` predicts an invoice address and `execute` deploys its [`Payment`](src/Payment.sol), with the token, amount, receiver, expiration, recovery address, caller-provided salt, and the chain id all committed into the deployment salt. The recovery address is the payer's attested wallet; merchants do not choose it. The chain id is the network the payer chose to pay on, so the same terms produce a different address on every chain.

A `Payment` constructor first checks that it is running on the chain it was committed to. On any other chain it stores nothing, touches no token (the committed token may not even exist there), emits `WrongChain(expected, actual)`, and returns, leaving an inert contract whose only use is the permissionless `recover(token)` below. On the right chain it routes whatever balance is present when it is deployed:

- **Live and fully funded.** It transfers exactly the requested amount to the receiver and emits `Settled(receiver, amount)`, where `amount` is always the invoice amount, never the balance that happened to be present. Anything above the invoice amount is an overpayment: it is transferred to the Payday recovery wallet in the same transaction and reported as `Recovered(recovery, remainder)`. An exact payment emits no `Recovered` event. If the recovery leg fails (for example, the token blacklists the recovery wallet) the whole deployment reverts, the receiver is not paid, and the balance stays at the address for a later attempt.
- **Live but underfunded.** It reverts with `InsufficientTokenBalance` and leaves no code, so the same address stays usable for the deposit request once the payment is completed.
- **Expired.** It transfers the whole balance, however partial, to the Payday recovery wallet and emits `Recovered`.

Deployment is a one-way door. Funds that arrive afterwards (a duplicate payment, the late completion of a deposit request that already expired, or USDC sent to the address on the wrong chain) can be forwarded to the recovery wallet by anyone through the permissionless `recover(address token)`, which names the token to move (the committed one, or whatever a wrong-chain deposit arrived in) and emits `Recovered(recovery, token, amount)`. [`BatchSweeper`](src/BatchSweeper.sol) processes independent invoices in one transaction, deploying fresh payments or recovering late funds from existing ones, and reports per-item failures without reverting successful siblings; an overpaid item whose recovery leg fails is reported like any other failed deployment and its siblings still settle.

The intended requested amount therefore always moves directly from the payment address to the merchant's receiver; Payday never intermediates it. Overpayment remainders, expired balances, and late transfers go straight back to the payer's wallet on-chain; Payday takes custody of nothing.

## One generation, every chain, the same addresses

Both contracts are deployed at the same addresses on every supported network. [`PaymentFactory.s.sol`](script/PaymentFactory.s.sol) enforces it: the deployer key must have nonce 0 on the target chain, so one fresh key run against each chain in turn produces identical `PaymentFactory` (nonce 0) and `BatchSweeper` (nonce 1) addresses. This is what makes a wrong-chain deposit recoverable at all: a payment address bound to chain A can be deployed on chain B only if chain B's factory sits at the same address and reproduces the same CREATE3 derivation, at which point the wrong-chain branch above plus `recover(token)` return the funds to the payer (`docs/runbooks/wrong-network-deposit.md`).

## Contract generations and code hashes

`PaymentFactory` embeds `Payment.creationCode`, and `BatchSweeper` holds its factory in an immutable. Any change to `Payment` is therefore a new generation of both contracts: deploy a new factory and a new sweeper bound to it together, and never point a new factory at an old sweeper or an old factory at a new sweeper. A new factory address also means new payment addresses for the same invoice parameters (see below). Because Payday is not in production, every environment moves to the new generation rather than supporting a mix.

`gatewayd` and `gateway-indexer` pin the generation they were configured for, on every chain. Each entry of `PAYDAY_CHAINS` carries `factory`, `batch_sweeper`, `factory_code_hash`, and `batch_sweeper_code_hash`; at startup both services compare the hashes with the runtime bytecode deployed at those addresses on that chain, check that `BatchSweeper.factory()` is the configured factory, and refuse to start on any mismatch. Each hash is the `keccak256` of the runtime bytecode returned by `eth_getCode`:

```sh
cast keccak "$(cast code "$FACTORY_ADDRESS" --rpc-url "$RPC_URL")"
cast keccak "$(cast code "$BATCH_SWEEPER_ADDRESS" --rpc-url "$RPC_URL")"
```

[`PaymentFactory.s.sol`](script/PaymentFactory.s.sol) prints both addresses and both hashes after deploying a generation, and [`Bootstrap.s.sol`](script/Bootstrap.s.sol) prints the hashes of the local fixtures on every run. Hashes are never hardcoded; scripts compute them from the running chain. The local fixture addresses are Anvil account #0's first CREATE addresses, so a long-running Anvil keeps whichever generation it saw first: the bootstrap refuses to run against a stale factory, a sweeper bound to another factory, or a sweeper whose runtime bytecode differs from this build, and the fix is to restart Anvil so the new generation deploys.

## Why CREATE3

`PaymentFactory` uses Solady's [`CREATE3`](lib/solady/src/utils/CREATE3.sol): `CREATE2` places a fixed proxy from the factory address and deployment salt, then that proxy creates `Payment`. The resulting payment address is therefore independent of `Payment` init code, so clients can derive and fund it before the payment contract exists. It is still specific to the factory address and every invoice parameter included in `deploymentSalt`, the chain id among them; changing any of those produces another address.

## Rust/Solidity address parity

[`predict_payment_address`](../crates/gateway-core/src/deterministic_address.rs) reproduces the deployment-salt hash, Solady proxy `CREATE2` address, and proxy nonce-1 `CREATE` address in Rust. [`PaymentAddressTest.testFuzz_address_parity`](test/PaymentAddress.t.sol) fuzzes invoice parameters, obtains Solidity's `PaymentFactory.paymentAddress`, invokes the [`derive-address`](../crates/gateway-core/test-helpers/derive_address.rs) Rust binary through `vm.ffi`, and compares the results. From the repository root:

```sh
cargo build -p gateway-core --bin derive-address
forge test --match-contract PaymentAddressTest
```
