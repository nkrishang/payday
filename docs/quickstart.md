# Payday quickstart

Payday creates a new, one-time USDC address for each invoice. Send the requested
USDC to that address; after the payment is finalized, Payday settles the address
to your beneficiary.

> **Deployment values:** The API URL, Auth0 settings, chain ID, and USDC contract
> below are placeholders. Get the values for your Payday deployment before
> continuing. Do not substitute a similarly named or bridged token.

## 1. Build the CLI

The CLI is currently distributed from this source repository, not through a
package manager. From the repository root, with a Rust toolchain installed:

```sh
cargo build --release -p gateway-cli
```

The executable is `./target/release/gateway-cli`.

## 2. Configure access

Set the deployment-provided values:

```sh
export GATEWAY_API_URL="https://<PAYDAY-API-HOST>"
export GATEWAY_AUTH0_ISSUER="https://<AUTH0-TENANT>/"
export GATEWAY_AUTH0_CLIENT_ID="<AUTH0-NATIVE-APPLICATION-CLIENT-ID>"
export GATEWAY_AUTH0_AUDIENCE="<AUTH0-API-AUDIENCE>"
```

The CLI requires HTTPS except when connecting to localhost.

Create an API key with email OTP authentication:

```sh
./target/release/gateway-cli account create
```

Enter your email address, then the one-time code sent to it. The API key is
shown only when it is issued, so store it securely and export it:

```sh
export GATEWAY_API_KEY="<NEW-API-KEY>"
```

Running `account create` again replaces and immediately invalidates the current
key after confirmation.

## 3. Create an invoice

Choose an expiration at least 10 minutes and at most 366 days in the future.
Timestamps are Unix **seconds**. Amounts are human-readable USDC with up to six
decimal places.

```sh
export PAYDAY_CHAIN_ID="<DEPLOYMENT-CHAIN-ID>"
export PAYDAY_USDC_ADDRESS="<DEPLOYMENT-NATIVE-USDC-CONTRACT>"
export BENEFICIARY_ADDRESS="<EVM-ADDRESS-THAT-RECEIVES-SUCCESSFUL-PAYMENTS>"
export RECOVERY_ADDRESS="<EVM-ADDRESS-THAT-RECEIVES-LATE-OR-EXPIRED-FUNDS>"
export EXPIRATION_TIMESTAMP="$(($(date +%s) + 3600))"

./target/release/gateway-cli invoice create \
  --chain-id "$PAYDAY_CHAIN_ID" \
  --token "$PAYDAY_USDC_ADDRESS" \
  --beneficiary "$BENEFICIARY_ADDRESS" \
  --amount "25.00" \
  --expiration-timestamp "$EXPIRATION_TIMESTAMP" \
  --recovery "$RECOVERY_ADDRESS" \
  --idempotency-key "checkout-<YOUR-UNIQUE-ORDER-ID>"
```

Reuse an idempotency key only to retry the same request. Omitting the option
makes the CLI generate a new UUID for that invocation.

The response includes the invoice `id`, `status`, `payment address`, requested
amount, and token details. Save the `id` and use the returned payment address
only for this invoice.

## 4. Pay and observe settlement

From a wallet on the configured chain, make an ERC-20 transfer of **only the
native USDC contract shown in the invoice's `token` field** to `payment_address`.
Do not send the chain's native gas currency, another stablecoin, bridged USDC,
or USDC on another network.

Fetch the invoice by its UUID:

```sh
./target/release/gateway-cli invoice get <INVOICE-ID>
```

For machine-readable output:

```sh
./target/release/gateway-cli --json invoice get <INVOICE-ID>
```

A normal first payment progresses through `created` → `funded` → `deploying` →
`fulfilled`. Only finalized transfers appear in `received`; detection is not
instant. `fulfilled` means the complete balance present at execution was sent
to the beneficiary. Stop presenting the payment address after the invoice
leaves `created`.

See [Concepts](concepts.md) for finality, partial and excess payments, expiry,
recovery, and every invoice status.
