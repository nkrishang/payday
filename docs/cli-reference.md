# CLI reference

The executable is `payday`. Install it using [the distribution guide](install.md).

```text
payday [GLOBAL OPTIONS] <COMMAND> [OPTIONS]
```

## Connection and credentials

| Option or variable | Behavior |
|---|---|
| default | `https://api.payday.sh` and the `default` saved credential profile |
| `--sandbox` | `https://api.sandbox.payday.sh`; overridden by an explicit API URL |
| `--profile production` | Production endpoint defaults |
| `--profile local` | `http://127.0.0.1:3000`, local identity service, and `local` profile |
| `--api-url URL` / `PAYDAY_API_URL` | Explicit endpoint; the option wins over the environment |
| `PAYDAY_API_KEY` | Bearer-key override for scripts; wins over saved credentials |
| `PAYDAY_CONFIG_DIR` | Override the credentials directory |

The production CLI includes its public Auth0 configuration. Custom APIs need
the deployment's `PAYDAY_AUTH0_ISSUER`, `PAYDAY_AUTH0_CLIENT_ID`, and
`PAYDAY_AUTH0_AUDIENCE` for interactive account commands. HTTP is accepted only
for localhost/loopback; redirects are not followed.

Credentials are stored in an XDG-aware private file, normally
`~/.config/payday/credentials` (`%APPDATA%\payday\credentials` on Windows).
Profiles are bound to their issuing API URL so a key cannot silently be sent to
another host.

## Global output options

| Option | Behavior |
|---|---|
| `--json` | Machine-readable success output and structured errors |
| `--verbose` | Additional operational detail and authentication diagnostics |
| `--plain` | Disable colors, terminal redraws, and styling |
| `--color auto|always|never` | Color policy; default `auto` |

`NO_COLOR` also disables color. Global options can appear before or after a
subcommand.

## Payments

### `payday create`

```text
payday create --amount AMOUNT --to ADDRESS
  [--refund-to ADDRESS]
  [--expires-in DURATION | --expires-at RFC3339]
  [--memo TEXT]
  [--idempotency-key KEY]
```

Amounts are positive USDC decimals with at most six fractional digits.
`--refund-to` defaults to `--to`; expiry defaults to 24 hours and must resolve
between 10 minutes and 366 days ahead. `--memo` is the merchant-facing order
reference. The deployment supplies chain and native-USDC defaults.

The CLI validates input locally, generates a UUIDv7 idempotency key when none is
supplied, and prints human payment instructions or the complete API response
with `--json`. Pin the key when a script may retry an uncertain request.

### `payday get REFERENCE [--watch] [--interval SECONDS]`

`REFERENCE` may be a complete `pay_…` ID, an unambiguous canonical prefix, or
the payment address. `--watch` waits for changes and exits at `settled`,
`returned`, or `needs_attention`; terminal output redraws in place and plain or
redirected output prints only changed frames. `--interval` controls the
long-poll refresh window and requires `--watch`.

### `payday list`

```text
payday list [--status STATUS] [--limit 1..100]
  [--starting-after PAY_ID]
```

Returns newest first; default limit is 20. Status is one of
`awaiting_payment`, `partially_paid`, `paid`, `settled`, `expired`, `returned`,
or `needs_attention`. Continue with the API's complete `next_cursor`.

### `payday cancel REFERENCE`

Records an advisory cancellation and tells clients to stop presenting the
payment. It cannot disable the address or change on-chain payout, refund, or
expiry terms.

## Sign-in and keys

- `payday login [--show] [-y|--yes]` — email OTP sign-in and first key issuance.
  If an account already has a key, confirmation rotates it. The plaintext is
  masked unless `--show`; the saved profile always receives the full key.
- `payday logout` — remove only the local profile; server keys remain valid.
- `payday whoami` — show account ID, key hint, generation, creation/rotation,
  previous-key expiry, and revocation metadata.
- `payday keys rotate [--show] [-y|--yes]` — issue and save a replacement after
  fresh email OTP. The previous key remains valid for 24 hours.
- `payday keys revoke [-y|--yes]` — immediately revoke current and grace-period
  keys and remove the local profile.

OTP input is hidden on a terminal. Enter `r` instead of a code to resend; three
wrong six-digit codes end the command. Confirmation defaults to no.

## Webhooks

| Command | Result |
|---|---|
| `payday webhooks add HTTPS_URL` | Register an endpoint; signing secret shown once |
| `payday webhooks list` | List active endpoints without secrets |
| `payday webhooks remove UUID` | Disable an endpoint |
| `payday webhooks test UUID` | Queue a test delivery |
| `payday webhooks deliveries` | List deliveries and immutable attempt history |

Endpoint URLs must use HTTPS and pass the server's public-destination checks.
See [Webhooks](webhooks.md) for signature verification and retries.

## Utilities

- `payday completions bash|elvish|fish|powershell|zsh` writes completion code.
- `payday docs [getting-started|authentication|environment]` prints built-in
  offline guidance.
- `payday upgrade` verifies the latest release checksum and replaces the
  current executable on supported Unix platforms. Windows upgrades use a new
  release archive.

## Output and exit status

Successful results go to stdout. Prompts and progress go to stderr. Interactive
human output adds headings and next-step hints; redirected output stays clean.
With `--json`, errors are emitted on stderr as
`{"error":{"code":"…","message":"…"}}`.

| Status | Meaning |
|---:|---|
| `0` | Success, help/version output, or a declined confirmation |
| `1` | API, authentication, or unexpected-response failure |
| `2` | Invalid input, usage, or configuration |
| `3` | Network, TLS, timeout, or other transport failure |
