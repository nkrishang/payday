# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses semantic version numbers. The project is currently
pre-release software; the `0.1.0` version does not imply a stable public API.

## [Unreleased]

### Added

- Payer identity verification for `verified_identity` and
  `verified_identity_unattributed`, through Didit's hosted document, liveness,
  and face-match session behind a thin `PayerIdentityProvider` boundary.
  `POST /v1/payer/payments/{id}/verify/identity/start` (session with a proven
  mailbox required) answers `{outcome: reused | redirect {url}}`: a hosted
  session is created lazily, with the merchant-scoped payer reference as
  vendor data, Payday's attempt id as metadata, and, for `verified_identity`
  only, the expected first and last name as the details to match; a mismatch
  declines. Only statuses, the provider's session reference, the document's
  issuing country, and allowlisted risk categories are kept; the decision
  types cannot carry names, document numbers, dates of birth, or images, and
  nothing logs a callback body. Approval sets the document, liveness, and
  (matched) identity-match facts on the verifying session only, mints a
  merchant-scoped `document_liveness` credential and, for matched mode, a
  `matched_identity` credential bound to the expected-identity hash
  (`keccak256("PAYDAY_EXPECTED_IDENTITY_V1" || JCS(expected_identity))`),
  and completes the invoice's verification only while it is still live.
  Credentials (180-day default lifetime) are reused within the merchant on
  later invoices whose mailbox is proven again — generic ones for
  unattributed mode, hash-bound ones for the same asserted name — and never
  across merchants or across different names.
- Callback and reconciliation: `POST /v1/webhooks/identity` verifies Didit's
  `X-Signature-V2` (HMAC-SHA256 over Didit's canonical JSON: sorted keys,
  compact separators, unescaped Unicode, whole floats as integers) and
  `X-Timestamp` (±300 s), records the event id once, answers `202`, and only
  brings the attempt's poll forward; a reconciler worker in gatewayd claims
  open attempts with `FOR UPDATE SKIP LOCKED` and a lease, polls pending
  sessions every 15 seconds and in-review ones with increasing backoff, stops
  on a settled status, and defers provider failures with backoff rather than
  declining the payer. `verification.declined` is now emitted, once per
  payment, when a decline is recorded.
- Retry and human review: one automated resubmission after a decline; a
  second decline sets `review_required` and further starts answer
  `409 review_required`. `GET /v1/payments/{id}/verification` gives the
  merchant every attempt with each fact reported separately, provider
  reference, risk categories, and reviews; `POST …/verification/review`
  asks a person to look at a declined attempt (automation stops for that
  payer); `POST /v1/admin/verifications/{id}/decision` records the
  reviewer (`PAYDAY_ADMIN_REVIEWER_ID`, default `operator`), decision, note,
  and time, and a manual approval binds the same expected-identity hash.
  Configured by `PAYDAY_DIDIT_API_KEY`, `PAYDAY_DIDIT_WORKFLOW_ID`,
  `PAYDAY_DIDIT_WEBHOOK_SECRET` (optional `PAYDAY_DIDIT_BASE_URL`), all
  together or none; without them identity start answers
  `503 verification_unavailable`.
- Checkout: explicit consent before the redirect (matched and unattributed
  copy, both privacy notices, the wallet-ownership caveat), the session
  restored from this tab's storage after the provider returns, the return
  URL's query ignored in favour of polling `GET …/verify`, and, after a
  decline, one retry and the appeal contact
  (`NEXT_PUBLIC_PAYER_APPEAL_EMAIL`). Dashboard: verification activity on the
  invoice page with each fact on its own, every attempt's provider reference
  and risk categories, the reviewer's outcome, retry availability, and a
  request-review action. SDK: `payer.verification.startIdentity`,
  `identity` on the verification status, `payments.verification`, and
  `payments.requestVerificationReview`.

- Payer email verification. A gated invoice's payer proves ownership of the
  mailbox the merchant asserted through
  `POST /v1/payer/payments/{id}/verify/email/start` (the gateway sends the
  code; the request names no email) and `…/verify/email/confirm` (the code),
  exchanged against a dedicated Auth0 payer audience
  (`auth0/actions/payday-payer-email-otp.js`). `start` mints an opaque payer
  session, stored hashed and valid for 24 hours, that travels in
  `Payday-Payer-Session` on every payer read and unlocks exactly that
  invoice's content, QR, and attachment for that session; `GET …/verify`
  reports the session's facts. One code per invoice per minute
  (`429 otp_resend_cooldown` with `Retry-After`); permissionless invoices
  answer `409 verification_not_required`, closed ones `410`. For
  `verified_email` the invoice's `verification_completed_at` is set in the
  same transaction; the identity modes record the mailbox and stay locked.
  The write routes answer cross-origin requests from the hosted checkout only
  (`PAYDAY_HOSTED_CHECKOUT_ORIGIN`); the reads keep `*` and now admit the
  session header. Configured by `PAYDAY_PAYER_AUTH0_ISSUER`,
  `PAYDAY_PAYER_AUTH0_AUDIENCE`, `PAYDAY_PAYER_AUTH0_CLIENT_ID`, and
  `PAYDAY_PAYER_REF_MASTER_KEY` (the merchant-scoped payer reference key).
- Verification gate on settlement: the sweep claim itself, in SQL, admits a
  live `funded` invoice only when its policy is permissionless or its
  verification has completed and the finalized chain clock has not passed
  its deadline; expired, fulfilled, and recovered invoices are always
  claimable so their balance reaches recovery. Funds that arrive before a
  gated invoice's verification are credited normally, stay on the same
  address, and stamp `likely_unsolicited_at` once; verification before the
  deadline lets that address settle, expiry without it moves the whole
  balance to recovery, and verification after expiry cannot revive it.
- Hosted checkout verification: the gate offers to send the code, takes it,
  keeps the session in this tab's `sessionStorage` (never in server output,
  local storage, or a URL), resumes it across reloads, and fetches the QR as
  a blob with the session in a header. Locked invoices render nothing but the
  issuer, heading, masked mailbox, and requirements; identity modes move on
  to an identity step after the email.
- SDK: `payer.verification.startEmail/confirmEmail/status` and
  `payer.payments.qr(id, payerSession)` returning a `Blob`. `qrUrl` is gone:
  a gated invoice's QR needs a session, which must never be in an image URL.
- The development identity provider serves a `payday-payer-local` client and
  audience next to the merchant ones; `just dev` and `just e2e` configure
  the payer settings against it.

- Invoice documents. `POST /v1/payments` now takes the document the payment
  fulfils: required `issuer` and `bill_to` parties (name, optional email and
  free-text details), a required `payer_policy` (`permissionless`,
  `verified_email`, `verified_identity`, or `verified_identity_unattributed`
  with the merchant's expected email and, for `verified_identity`, expected
  name), and optional `heading`, `notes`, `customer_id`, and `attachment_id`.
  The merchant `Payment` object returns them, the list summary carries
  `heading`, `bill_to_name`, `payer_policy_mode`, `customer_id`,
  `has_attachment`, `verification_completed_at`, and `likely_unsolicited_at`,
  and control characters in any text field are rejected as `invalid_request`.
- Attribution: at issuance the invoice is canonicalized (RFC 8785), hashed
  with a random nonce, and the CREATE3 salt is derived from that hash, so the
  payment address commits to the exact document. The response reports it as
  `attribution {version, hash}`, and the idempotency check covers every
  committed field, including the chain, token, factory, and recovery wallet.
- Customers: `POST`/`GET`/`PATCH /v1/customers`, a reusable counterparty an
  invoice can reference while keeping its own `bill_to` snapshot.
- PDF attachments: `POST /v1/attachments` returns a presigned upload (one PDF
  per invoice, at most 5 MiB, enforced at finalize), the client `PUT`s the
  bytes with the returned headers, and `POST /v1/attachments/{id}/finalize`
  answers `attachment_scan_pending` until the malware scan reports and
  `attachment_rejected` for anything that is not a clean PDF. Keys are
  write-once (`If-None-Match: *`, so a replayed upload fails with 412) and the
  finalized object version is pinned for every later read; rejected uploads
  are deleted, unattached ones expire after seven days, and an upload that
  expired unused is refused at issuance with `attachment_not_ready`.
- `GET /v1/payments/{id}/attachment` (a short-lived signed download),
  `GET /v1/payments/{id}/invoice.pdf` (a deterministic Payday-rendered
  summary), and `GET /v1/payments/{id}/proof`.
- Proof of Payment: for a settled invoice, the canonical issuance snapshot,
  nonce, attribution hash, salt, chain, factory, token and payment addresses,
  every credited transfer, the fulfilment transaction as
  `settlement_transaction_hash`, and a Payday-signed verification attestation
  whose payload names the invoice's `attribution_hash`, `chain_id`, and
  `payment_address`, so it cannot be transplanted onto another proof.
  Verifiers recompute hash → salt → address, check the attachment and the
  attestation, and require the transfers to sum to at least the invoice
  amount; `payday proof verify --rpc-url` also checks each transfer's receipt
  and the settlement receipt on chain.
- CLI: `payday create` takes `--issuer`, `--bill-to`, `--heading`, and
  `--reference`, or the whole API body with `--from-file`, and uploads a PDF
  with `--attachment` (waiting for the scan before issuing); `payday get
  --pdf` saves the invoice PDF; `payday customers create|get|list`; and
  `payday proof download|verify`.
- The merchant dashboard at `payday.sh/dashboard`: invoices, customers, and
  attachment upload with scan progress. It signs in with the same emailed
  code as the CLI, and the API accepts the resulting short-lived identity
  token as a session credential on the payment, customer, and attachment
  routes, so no API key ever reaches a browser. `gatewayd` reads
  `PAYDAY_DASHBOARD_AUTH0_CLIENT_ID` to admit it.
- Gated payer responses: for the verified policies the payer route returns
  only the issuer name, heading, requirements, and a masked expected mailbox
  until verification completes; the amount, address, invoice content, and the
  settlement transaction hash and explorer link stay `null` while locked.
- Environment: `PAYDAY_ATTACHMENT_BUCKET`, `PAYDAY_ATTACHMENT_S3_ENDPOINT`,
  `PAYDAY_ATTACHMENT_S3_FORCE_PATH_STYLE`, and
  `PAYDAY_ATTACHMENT_DOWNLOAD_TTL_SECS` for the attachment store; exactly one
  of `PAYDAY_ATTESTATION_SIGNER_KEY` or `PAYDAY_ATTESTATION_KMS_KEY_ID` for
  the attestation signer; `PAYDAY_DASHBOARD_AUTH0_CLIENT_ID`; and, for the
  web app, `NEXT_PUBLIC_AUTH0_DOMAIN`, `NEXT_PUBLIC_AUTH0_CLIENT_ID`,
  `NEXT_PUBLIC_AUTH0_AUDIENCE`, and `NEXT_PUBLIC_ATTACHMENT_UPLOAD_ORIGIN`.
- `recovery_address` on the merchant payment response: the Payday recovery
  wallet each payment is committed to. The payer response does not carry it.
- The `recovered_funds` ledger also records recoveries performed by executions
  Payday did not submit: when a third party settles or recovers a payment
  directly on-chain, the indexer takes what the contract routed to the
  recovery wallet from the payment's own events and writes the same ledger row
  and `payment.recovered_funds` event it would for its own sweep.
- A `recovered_funds` ledger with one row per nonzero amount the recovery
  wallet received on a payment's behalf (`overpayment`, `expired`, or
  `late_transfer`), written in the transaction that finalizes the sweep, and a
  `payment.recovered_funds` webhook per row. A payment can raise it more than
  once. Payment webhook payloads now also carry `payer_policy_mode`,
  `verification_completed_at`, and `likely_unsolicited_at`; the event
  vocabulary gains `verification.approved`, `verification.declined`, and
  `payment.likely_unsolicited`.
- Deployment code-hash pinning: `gatewayd` and the indexer require
  `PAYDAY_FACTORY_CODE_HASH` and `PAYDAY_BATCH_SWEEPER_CODE_HASH` (keccak256 of
  the deployed runtime bytecode), compare them with the chain at startup,
  verify `BatchSweeper.factory()`, and refuse to start on a mismatch.
  `gatewayd` therefore also reads `PAYDAY_RPC_URL` and
  `PAYDAY_BATCH_SWEEPER_ADDRESS`; a status-only instance skips all of this.
  `just dev` and `just e2e` compute the hashes from the running chain, and
  Terraform gains `factory_code_hash`, `batch_sweeper_code_hash`, and
  `recovery_address` plus a recovery KMS key that no task role can sign with.
- The consolidated `0011` schema: customers, invoice document and payer-policy
  columns, attachments, payer sessions, verifications and credentials,
  verification reviews, identity webhook receipts, and the attribution columns,
  landing ahead of the features that use them, along with an
  immutable-issuance trigger on invoices.
- A `payday.sh` web project (`web/`): a landing page and the hosted checkout at
  `/pay/{id}`, built with Next.js, Tailwind, and wagmi. The checkout is
  server-rendered, so it arrives complete, and a payer can pay from a connected
  wallet, a scanned QR, or a copied address.
- `PaydayPayerClient` in the TypeScript SDK — a keyless client for the public
  payer routes, so a merchant can build a checkout of their own.
- `Access-Control-Allow-Origin: *` on `GET /v1/payer/payments/{id}` and its
  `/qr`. No other route allows cross-origin reads.
- A `checkout_base_url` Terraform variable naming the origin that serves the
  checkout, which is where every `payment_url` points.

### Changed

- `issuer`, `bill_to`, and `payer_policy` are required on `POST /v1/payments`;
  a body without them is rejected. An issued invoice is immutable.
- Cross-origin access to the merchant routes (payments, customers,
  attachments) is allowed from the configured web origin only
  (`PAYDAY_PUBLIC_BASE_URL`) for `GET`, `POST`, and `PATCH` with the
  `Authorization`, `Content-Type`, `Idempotency-Key`, and `Accept` headers;
  the payer `GET` routes remain open to any origin.
- Settlement is exact: a live `Payment` deployment transfers exactly the
  invoice amount to the payout address and any remainder to the Payday
  recovery wallet (`Settled` then `Recovered`), reverts when underfunded, and
  still sends the whole balance to recovery once expired. `PaymentFactory` and
  `BatchSweeper` are redeployed together as a new generation; their interfaces
  and the address formula are unchanged.
- Recovery is platform-controlled. `gatewayd` reads `PAYDAY_RECOVERY_ADDRESS`
  and stamps it on every payment; merchants can no longer choose where
  overpayments, expired balances, or late transfers go. Recovered funds are
  held by Payday, reviewed manually, and returned by the operator. Payday takes
  custody of recovered amounts only; the intended invoice amount still moves
  directly to the merchant, and user-facing copy now says so.
- The payer link is tokenless and unauthenticated. Anyone holding it may read
  the payment and fulfil it, which is what makes it shareable; it exposes no
  payout address, recovery address, memo, reference, or metadata. Documentation
  that still described a signed token has been corrected.
- `GET /pay/{id}` on the gateway now answers `301` to the hosted checkout, so
  links shared before it moved keep working.
- Payments are looked up by their complete ID or payment address; ID prefixes
  are no longer accepted.
- The checkout's receipt says exactly the invoice amount reached the merchant,
  and a settled overpayment tells the payer the remainder went to the Payday
  recovery wallet; the received state says the invoice amount, not the
  balance, is being settled. Terraform's `recovery_address` fails closed: it
  defaults to null and the API task definition refuses to plan until it is set
  to the recovery key's address, so no placeholder can reach a deployment.

### Removed

- `memo` from `POST /v1/payments`, the payment responses, the TypeScript SDK,
  and the CLI (`--memo` survives only as a hidden alias of `--reference`).
  Use `reference` for the invoice number and `notes` for free text.
- `refund_address` from `POST /v1/payments` (a body carrying it is rejected as
  an unknown field), from `CreatePayment` in the TypeScript SDK, and
  `--refund-to` from `payday create`. The merchant response field
  `refund_address` is renamed to `recovery_address`.
- The placeholder checkout that was compiled into the gateway binary, along with
  its `/assets/payer.*` routes.

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
  freshness, explorer links, long polling, and hosted payer checkouts.
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
