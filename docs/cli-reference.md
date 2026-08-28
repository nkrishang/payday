# CLI reference

`gateway-cli` is the command-line client for the payment gateway. Its syntax is:

```text
gateway-cli [GLOBAL OPTIONS] <invoice|account> <COMMAND> [OPTIONS]
```

## Global options and configuration

| Option | Environment variable | Default | Purpose |
|---|---|---|---|
| `--api-url <URL>` | `GATEWAY_API_URL` | `http://127.0.0.1:3000` | Gateway base URL |
| `--api-key <KEY>` | `GATEWAY_API_KEY` | none | Bearer API key for invoice commands |
| `--auth0-issuer <URL>` | `GATEWAY_AUTH0_ISSUER` | none | Identity issuer used by account commands |
| `--auth0-client-id <ID>` | `GATEWAY_AUTH0_CLIENT_ID` | none | Public Native application client ID used by account commands |
| `--auth0-audience <AUDIENCE>` | `GATEWAY_AUTH0_AUDIENCE` | none | Identity-token audience used by account commands |
| `--json` | none | off | Write the successful result as pretty-printed JSON |
| `-h`, `--help` | none | — | Show help |
| `-V`, `--version` | none | — | Show the version |

An explicit command-line option wins over its environment variable; the environment variable wins over the default. Global options may appear before or after subcommands.

The CLI accepts HTTPS gateway and identity URLs. Plain HTTP is accepted only when the host is `localhost` or a loopback IP address (for example `127.0.0.1` or `::1`). Requests do not follow redirects. Gateway requests time out after 30 seconds; identity/account-flow requests time out after 15 seconds.

Prefer `GATEWAY_API_KEY` to `--api-key`: a command-line secret can be retained in shell history and exposed in process listings. The CLI locally requires an API key of at least 32 visible ASCII characters.

## Invoice commands

Invoice commands require an API key and use it as `Authorization: Bearer <KEY>`.

### `invoice create`

```text
gateway-cli invoice create \
  --chain-id <U64> \
  --token <ADDRESS> \
  --beneficiary <ADDRESS> \
  --amount <DECIMAL> \
  --expiration-timestamp <U64> \
  --recovery <ADDRESS> \
  [--idempotency-key <KEY>]
```

All six main options are required. `--chain-id` selects the chain; `--token` is that chain's Circle-issued USDC contract; `--beneficiary` receives successful settlement; `--amount` is a human-readable USDC amount such as `100.5`; `--expiration-timestamp` is Unix seconds; and `--recovery` receives funds if execution occurs after expiration. Numeric options must fit an unsigned 64-bit integer. Token, beneficiary, recovery, and amount cannot be blank.

The server applies the additional validation documented in the [API reference](api-reference.md). If `--idempotency-key` is omitted, the CLI generates a new UUIDv7 for that invocation. It always writes `using idempotency key: <KEY>` to stderr. Save or explicitly supply this key when retrying an uncertain request; a new invocation otherwise gets a new key and can create another invoice.

```bash
GATEWAY_API_KEY='payday_live_…' gateway-cli \
  --api-url https://api.example.com \
  invoice create \
  --chain-id 8453 \
  --token 0x1111111111111111111111111111111111111111 \
  --beneficiary 0x2222222222222222222222222222222222222222 \
  --amount 100.5 \
  --expiration-timestamp "$(($(date +%s) + 3600))" \
  --recovery 0x3333333333333333333333333333333333333333 \
  --idempotency-key order-2026-1042
```

### `invoice get`

```text
gateway-cli invoice get <ID>
```

`ID` is an invoice UUID and cannot be blank.

```bash
gateway-cli --api-url https://api.example.com --json \
  invoice get 019539a0-7e00-7000-8000-000000000000
```

Without `--json`, both invoice commands print an aligned summary containing all invoice fields; unavailable optional values print as `-`. The payment address is labelled single-use and valid only while status is `created`. With `--json`, stdout is the exact invoice response object described in the API reference.

## Account commands

Account commands require all three identity settings (`GATEWAY_AUTH0_ISSUER`, `GATEWAY_AUTH0_CLIENT_ID`, and `GATEWAY_AUTH0_AUDIENCE`, or their option equivalents). They do **not** use the gateway API key. Instead, each invocation prompts on stderr for an email, sends a one-time code, prompts for that code, and uses the resulting short-lived Auth0 identity access token only for account/key management.

Prompts are `Email: ` and `One-time code: `. Input is read from stdin, trimmed, and must not be empty. The notice that a code was sent is also written to stderr. These commands are interactive even with `--json`.

### `account create`

```text
gateway-cli account create [-y|--yes]
```

Creates the account's first API key or replaces its one existing key. If a key exists, the CLI prints its hint and generation to stderr and asks `Proceed? [y/N]: `. Only `y` or `yes` (case-insensitive) confirms; every other answer cancels. `--yes` skips this replacement confirmation, but not email OTP authentication.

On cancellation, stdout is `API key replacement cancelled.` (including when `--json` is set). On success, the new key is printed to stdout. JSON has this shape:

```json
{
  "api_key": "payday_live_…",
  "generation": 2,
  "replaced_previous_key": true
}
```

The plaintext key is returned only when issued and cannot later be retrieved. Store it securely. Replacement invalidates the previous key immediately. The confirmed generation is sent with the replacement so a concurrent rotation fails rather than silently replacing an unexpected key. A fresh authentication event is required for another issuance attempt.

### `account get`

```text
gateway-cli account get
```

Authenticates by email OTP and prints non-secret metadata. JSON has this shape:

```json
{
  "hint": "payday_live_…abcd",
  "generation": 2,
  "created_at": "2026-08-27T12:00:00+00:00",
  "rotated_at": "2026-08-27T12:30:00+00:00"
}
```

`rotated_at` is `null` until replacement. Human output displays it as `never`.

## Output and exit status

Successful command results go to stdout with a trailing newline. Prompts, notices, replacement warnings, and the generated/selected invoice idempotency key go to stderr, keeping normal `--json` results parseable. Errors are written to stderr as `error: <message>`.

| Exit status | Meaning |
|---:|---|
| `0` | Success, cancellation, help, or version output |
| `1` | API error or an unexpected HTTP response |
| `2` | Invalid/missing CLI argument or other locally rejected input (including clap usage errors) |
| `3` | Network, TLS, timeout, or other transport failure |
