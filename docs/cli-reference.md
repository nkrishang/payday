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

The CLI's interactive account commands (`login`, `keys`) exchange an Auth0
email code that the API no longer accepts: merchants sign in through Privy on
the dashboard, which is where API keys are generated and rolled. Give the CLI
a key from there as `PAYDAY_API_KEY`. HTTP is accepted only for
localhost/loopback; redirects are not followed.

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

## Invoices

### `payday create`

```text
payday create --amount AMOUNT --to ADDRESS --issuer NAME --bill-to NAME
  [--heading TEXT] [--reference TEXT]
  [--expires-in DURATION | --expires-at RFC3339]
  [--attachment FILE.pdf]
  [--idempotency-key KEY]

payday create --from-file invoice.json [--attachment FILE.pdf] [--idempotency-key KEY]
```

The quick path issues a `permissionless` invoice from flags: `--issuer` and
`--bill-to` become the parties' names. Anything richer — party emails and
details, notes, a verified payer policy, a customer reference, metadata — is
written as the API's create body in a JSON file and passed with `--from-file`,
which rejects unknown fields exactly as the API does. The file is the whole
body, so `--from-file` conflicts with every quick-path flag (`--amount`,
`--to`, `--issuer`, `--bill-to`, `--heading`, `--reference`, `--expires-in`,
`--expires-at`); only `--attachment` and `--idempotency-key` combine with it.

Amounts are positive USDC decimals with at most six fractional digits, used
directly; there are no line items. Exactly the amount settles to `--to`. There
is no recovery flag: overpayments, late transfers, and expired balances go to
the Payday recovery wallet (reported as `recovery_address` with `--json`) and
are returned by Payday after manual review. Expiry defaults to 24 hours and
must resolve between 10 minutes and 366 days ahead. The deployment supplies
chain and native-USDC defaults.

`--attachment` checks locally that the file starts with `%PDF-` and is at most
5 MiB, uploads it through the presigned upload, polls finalization with
backoff for up to two minutes while the malware scan runs (one status line on
stderr), and injects the resulting `attachment_id` into the request. A
rejected file, or a scan that has not reported in time
(`attachment_scan_timeout`), fails the command before any invoice is created.
A `--from-file` body that already carries `attachment_id` cannot also take
`--attachment`.

The CLI validates input locally, generates a UUIDv7 idempotency key when none is
supplied, and prints the invoice's payment instructions or the complete API
response with `--json`. Pin the key when a script may retry an uncertain
request; the key covers every immutable field, including the attachment's hash.

### `payday get REFERENCE [--watch] [--interval SECONDS] [--pdf FILE]`

`REFERENCE` is a complete `pay_…` ID or the payment address; partial IDs are
rejected locally with exit status 2. Output shows the document (heading,
issuer, bill-to, reference, attachment filename with its byte length and
SHA-256 prefix, attribution hash), the payer policy mode and whether
verification has completed, a likely-unsolicited marker when funds arrived
before verification, and the payment state. `--watch` waits for changes and
exits at `settled`, `returned`, or `needs_attention`; terminal output redraws
in place and plain or redirected output prints only changed frames.
`--interval` controls the long-poll refresh window and requires `--watch`.
`--pdf FILE` also saves the deterministic Payday-rendered invoice PDF
(`GET /v1/payments/{id}/invoice.pdf`); it conflicts with `--watch`.

### `payday list`

```text
payday list [--status STATUS] [--limit 1..100]
  [--starting-after PAY_ID]
```

Returns newest first; default limit is 20. Each row shows the payment ID,
status, amount, bill-to name, payer policy mode, and heading. Status is one of
`awaiting_payment`, `partially_paid`, `paid`, `settled`, `expired`, `returned`,
or `needs_attention`. Continue with the API's complete `next_cursor`.

### `payday cancel REFERENCE`

Records an advisory cancellation and tells clients to stop presenting the
invoice. It cannot disable the address or change on-chain payout, recovery, or
expiry terms. Because an issued invoice is immutable, cancel and reissue to
change its content or policy.

## Proof of Payment

```text
payday proof download REFERENCE --output proof.json
payday proof verify proof.json [--attachment FILE.pdf]
  [--trusted-attestor ADDRESS]... [--rpc-url URL]
```

`download` fetches the settled invoice's Proof of Payment and writes it to
`--output`; it fails with `payment_not_settled` before settlement. `verify`
needs no credentials and works offline: it recomputes the RFC 8785 canonical
hash of the issuance snapshot, derives the salt from the nonce, recomputes the
CREATE3 payment address, checks the attached PDF's length and SHA-256 when
`--attachment` is supplied, checks the Payday verification attestation's
signature and that it names this invoice's attribution hash, chain, and
payment address — so an attestation issued for another invoice cannot be
transplanted — and that its signer is one of `--trusted-attestor` when given
(without a trust list any self-consistent signer is accepted), confirms every
included transfer paid the payment address, and checks that the transfers sum
to at least the invoice amount. The proof's `settlement_transaction_hash` is
the fulfilment transaction that executed the `Payment` contract, not one of
the transfers; offline it can only be checked for shape. With `--rpc-url` it
additionally fetches each transfer's receipt with `eth_getTransactionReceipt`,
requiring a successful transaction carrying a USDC `Transfer` log from the
proof's sender to the payment address for that transfer's amount, and the
settlement receipt, requiring a successful transaction carrying a USDC
`Transfer` log from the payment address to the invoice's payout address for
exactly the invoice amount. The settlement receipt is reported as its own
named check.

Human output lists every check with `✓`, `✗`, or `·` (skipped: no file, no
RPC endpoint, or not reached after an earlier failure) and ends with the
verdict; `--json` emits `{"checks":[{"name","status","detail"}],"ok":bool}`.
Exit status 1 on any failed check.

## Customers

| Command | Result |
|---|---|
| `payday customers create --name NAME [--email EMAIL] [--details TEXT]` | Create a reusable counterparty record |
| `payday customers get ID` | Show one customer |
| `payday customers list [--limit 1..100] [--starting-after ID]` | List newest first |

`ID` is the customer's UUID. Reference a customer in a `--from-file` body with
`customer_id`; the invoice still stores its own immutable `bill_to` snapshot.
Editing a customer (`PATCH /v1/customers/{id}`) is available through the API
and SDK.

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
| `1` | API, authentication, or unexpected-response failure; a failed `proof verify` check; an attachment scan that did not report in time |
| `2` | Invalid input, usage, or configuration |
| `3` | Network, TLS, timeout, or other transport failure |
