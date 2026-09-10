# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses semantic version numbers. The project is currently
pre-release software; the `0.1.0` version does not imply a stable public API.

### Added

- A staging environment (`docs/staging.md`): a second, isolated copy of the
  AWS stack at `api.staging.payday.sh`, on Monad mainnet with real USDC and
  its own contract generation, database, keys, bucket, and email identity,
  whose checkout origin is the web app on the operator's machine. `just
  web-staging` serves the local web app against it (`web/.env.staging`),
  and `just live-smoke` (`scripts/live-smoke.sh`) runs a real deposit end
  to end against any live API: issue, wallet attestation signed with
  `cast`, on-chain transfer, finalized settlement, Proof of Payment. The
  `Deploy staging` workflow builds and pushes images for every commit that
  reaches `main` and passes CI, then applies
  `infra/environments/staging.tfvars` with that tag, assuming a GitHub OIDC
  role that the staging Terraform creates when `github_repository` is set,
  and replaces staging's database whenever the migrations directory changed
  since the deployed commit. `checkout_base_url` accepts a loopback HTTP
  origin for this.

## [Unreleased]

### Changed

- The payer chooses the network. A deposit request no longer carries a
  chain: `POST /v1/deposit-requests` rejects `chain_id` and
  `token_address`, the request offers every supported network as
  `networks` (`[{chain, token}]`, Monad, Base, and Arbitrum One in
  production; their testnets in the sandbox), and `chain` and `token` are
  `null` until the payer picks one. The hosted checkout gains a mandatory
  network step before the wallet step; the choice rides in
  `POST …/wallet/challenge` as `chain_id` (`422 unsupported_chain` for a
  chain the request does not offer) and in the EIP-712 domain the wallet
  signs (`chainId` and that chain's factory as `verifyingContract`), so
  the wallet is switched to the chosen network before signing and the
  attestation binds the chain, its USDC, its factory, the salt, and the
  address together. The SDK's `wallet.challenge` takes the chain id;
  `WalletChallenge` reports `chain`. `GET /v1/status` answers
  `{chains: [...]}`, one entry per network. The merchant never picks a
  network; the dashboard shows the Payday wallet's USDC and gas balance on
  every network, and a deposit's network once the payer has chosen.
- A new contract generation: `Payment` takes the chain id, refuses to
  route anything when `block.chainid` differs (emitting `WrongChain`), and
  its permissionless `recover(address token)` names the token to return;
  `PaymentFactory.paymentAddress` and `execute` take the chain id, which
  is part of the deployment salt, and `BatchSweeper.Sweep` carries it. The
  deployment script requires a fresh deployer (nonce 0) so the generation
  lands at identical addresses on every chain, which is what makes a
  deposit sent on the wrong network returnable by hand
  (`docs/runbooks/wrong-network-deposit.md`).
- Canonical issuance snapshot `payday.invoice.v3` (`networks: [{chain_id,
  token_address, factory_address}]` sorted by chain id, in place of the
  top-level chain, token, and factory), attribution and salt domains v3,
  Proof of Payment `payday.proof.v3`: the proof's `chain_id`, `token_address`,
  and `factory_address` name the payer's choice, which `verify_proof`
  requires to be one of the snapshot's networks. Pre-release: no earlier
  proof or database is carried forward; migration `0004` makes the invoice
  chain columns part of the binding and adds the challenge's chain.
- Both services read one network registry, `PAYDAY_CHAINS` (a JSON array
  of `{chain_id, usdc, factory, batch_sweeper, factory_code_hash,
  batch_sweeper_code_hash, usdc_start_block, finality_source,
  finality_confirmations, block_time_ms, log_range_size,
  explorer_base_url}`), with one `PAYDAY_RPC_URL_<chain_id>` per chain
  (`PAYDAY_RPC_WS_URL_<chain_id>` to override or disable the signal), in
  place of `PAYDAY_CHAIN_ID`, `PAYDAY_FACTORY_ADDRESS`,
  `PAYDAY_BATCH_SWEEPER_ADDRESS`, the two code-hash variables,
  `PAYDAY_USDC_ADDRESS`, `PAYDAY_USDC_START_BLOCK`, `PAYDAY_RPC_URL`,
  `PAYDAY_RPC_WS_URL`, `PAYDAY_FINALITY_SOURCE`,
  `PAYDAY_FINALITY_CONFIRMATIONS`, `PAYDAY_LOG_RANGE_SIZE`, and
  `PAYDAY_EXPLORER_BASE_URL`. Terraform takes `chains` and a sensitive
  `rpc_urls` map (`TF_VAR_rpc_urls`), one Secrets Manager secret per
  chain; the staging workflow reads `STAGING_RPC_URLS`. The web app takes
  `NEXT_PUBLIC_CHAINS` in place of the five single-chain variables.
  Deployment verification runs on every chain at startup.
- The indexer runs one worker per chain in one process and scans on
  demand: a chain with nothing to watch fast-forwards its cursor every
  `PAYDAY_INDEXER_IDLE_INTERVAL_MS` (five minutes) without `eth_getLogs`
  and holds no WebSocket, so an idle chain costs about two calls every five
  minutes; three idle chains cost less than a fifth of the one
  always-scanning chain before. Every range's `eth_getLogs` is filtered to
  the addresses Payday is watching, 500 per call, on every chain, so RPC
  spend follows Payday's own activity and never a chain's USDC volume; the
  late-watch window that bounds the list (`PAYDAY_INDEXER_LATE_WATCH_DAYS`)
  defaults to a year, and a late transfer outside it is returned by hand.
  Base and Arbitrum One settle on `latest` minus a confirmation depth
  (their `finalized` tag is L1 finality); Monad keeps `finalized`. A wake
  on an L2 waits `block_time_ms` per block still ahead of the boundary.
  Unbound requests expire on any chain's clock. The local stack runs two
  Anvils (31337 `finalized`, 31338 `latest` plus confirmations) and
  the end-to-end script settles on the second chain, refuses an unoffered
  chain and a create with `chain_id`, checks the idle fast-forward, and
  rescues a wrong-chain deposit by hand.

### Added

- Customer documentation at `payday.sh/docs` (`web/app/docs`), in the web
  app's own design: an introduction, quickstart, concepts, payer
  verification, the dashboard, the hosted checkout, webhooks, Proof of
  Payment, recipes, the TypeScript SDK, environments, an architecture and
  security page written for due diligence, and a FAQ. The API reference is
  its own section, switched to from a tab row under the header: an
  introduction and the error table, then every route as a page of its own
  under its resource, listed in the sidebar with a method badge and a
  plain-English name. Routes are described once, as typed records in
  `web/components/docs/api`, and one template renders each page with the
  fields and answers on the left and the request and response samples
  beside them.
  Pages are static and carry inline diagrams (the deposit flow, the
  lifecycle, address derivation, the verification sequences, the indexer),
  a locked-and-unlocked checkout mock, code samples with server-side
  highlighting and copy, and curl/TypeScript tabs whose choice is
  remembered. The landing page's and dashboard's Docs links now point here
  instead of the repository.

### Changed

- Every id the API emits is now a UUID behind a prefix naming the resource,
  as deposit requests' `dr_` ids already were: `cus_` customer, `iss_`
  issuer identity, `pa_` payout address, `att_` attachment, `wh_` webhook
  endpoint, `whd_` webhook delivery, `evt_` webhook event (the envelope `id`
  and `Payday-Event-Id`), `va_` verification attempt, `rec_` recovery ledger
  entry, and `acct_` account. Only that canonical form is accepted back: a
  bare UUID or the wrong prefix in a path is `404 <resource>_not_found`, and
  in a body or query field `400 invalid_request` naming the form wanted. The
  database still stores UUIDs; attachment object keys and the Proof of
  Payment's `canonical_issuance_snapshot.attachment.id` (a hashed, frozen
  document) carry the raw UUID behind the `att_` id.
- An ergonomics pass over the merchant API, so every resource follows one
  set of conventions (documented under Conventions in
  `docs/api-reference.md`). A request body or query string that does not
  fit a route — malformed JSON, a missing or unknown field, a wrong type, a
  missing JSON content type — is now `400 invalid_request` with the
  deserializer's own message naming the field, where it was a `422` whose
  message said only "Unprocessable Entity". Webhooks catch up with the other
  resources: a missing, malformed, or foreign endpoint id is
  `404 webhook_not_found` (it was `400 invalid_request`), `DELETE
  /v1/webhooks/{id}` answers `204` and is idempotent, `GET /v1/webhooks`
  is enveloped as `{webhooks}`, `GET /v1/webhooks/{id}` reads one endpoint,
  `GET /v1/webhook-deliveries` pages with `limit` and `starting_after`,
  filters on `endpoint_id`, and answers `{deliveries, next_cursor}` with two
  queries instead of one per row, and registering an endpoint on a
  deployment with no encryption key is `503 webhooks_unavailable`, not a
  `500`. `GET /v1/deposit-requests/{id}/transfers` is enveloped as
  `{transfers}`. `POST /v1/deposit-requests/{id}/cancel` returns the
  deposit request itself rather than `{deposit_request, advisory}`. `PATCH
  /v1/customers/{id}` and `PATCH /v1/issuers/{id}` are partial: a field
  left out keeps its value and only an explicit `null` clears one, so
  renaming an issuer identity no longer requires resending (and risking)
  its proven contact address. `GET /v1/account/api-key`, a duplicate of
  `GET /v1/account`, is gone, and the key routes reject unknown body
  fields like every other route. Every timestamp the API and the webhook
  payloads emit is RFC 3339 in UTC to the second with a `Z` suffix; the
  previous mix of nanosecond `+00:00` and second `Z` forms is gone.
- Webhook payloads now carry a strict subset of the API's own deposit
  request object under the same names, units, and formats, so a handler can
  hand `data.deposit_request.id` straight to `GET /v1/deposit-requests/{id}`
  and compare amounts with the API's: the id is the `dr_`-prefixed form
  (it was a bare UUID), `amount` and `received` are the human decimal with
  `amount_base_units` and `received_base_units` beside them (`amount` was
  base units under the API's decimal name), `payer_wallet` and `address`
  are EIP-55 checksummed, `attention.code` matches the API's `attention`
  object (it was `reason_code`), `heading`, `customer_id`, `issuer_id`,
  `expires_at`, and `created_at` are included, and `recovery.block_number`
  is a decimal string like every other block number, with
  `recovery.amount` decimal beside `recovery.amount_base_units`.

### Added

- `POST /v1/deposit-requests` can take its parties and payout address from
  saved records instead of retyping them: with `issuer_id`, `issuer` and
  `payout_address` may be left out (the identity's name, contact address,
  details, and first saved payout address are snapshotted), and with
  `customer_id`, `payer` may be left out. An inline party still wins. The
  smallest valid request is `amount`, `issuer_id`, `customer_id`, and
  `payer_policy`.
- `DepositRequestSummary` carries `deposit_url`, `updated_at`, and
  `expires_at`, so a list renders and links each row without a second read.

- A payer named with an `email` on a deposit request is emailed their link
  to it as the request is issued, from the dashboard or the API alike. The
  message comes from `contact@payday.sh` through Resend
  (`PAYDAY_RESEND_API_KEY`, `PAYDAY_PAYER_EMAIL_FROM`; `resend_api_key` and
  `payer_email_from` in Terraform), in Payday's design, and names the
  issuer, the amount, the heading and reference, and the expiry, with a
  button to the `deposit_url` and `contact@payday.sh` for questions. It is
  queued in the issuing transaction, as a `payer` row of the notification
  outbox, and sent by the dispatcher that already delivers merchant
  attention email, so issuance never waits on the provider, transient
  failures retry on the outbox's backoff, and a rejection the provider
  will not change (`abandoned_at`) or a request that was cancelled, funded,
  or expired first is dropped rather than retried. Merchant-session
  requests and the onboarding walkthrough's reserved mailbox are never
  emailed. The dashboard's composer says so under the payer's email.

### Changed

- The public status page is gone: the `status.payday.sh` hostname, the
  separate ECS `status` service with its target group, listener rule, alarms,
  log group, and IAM role, `gatewayd`'s `PAYDAY_STATUS_ONLY` mode and its
  `/`, `/live`, and unauthenticated `/v1/status` routes, the
  `PAYDAY_STATUS_INDEXER_STALE_SECONDS` setting, the API heartbeat and the
  `api_status` table that existed only to feed the page, and the
  public-status-incident runbook. The authenticated merchant `GET /v1/status`
  and the SDK's `status()` are unchanged.
- The database schema is one baseline migration again.
  `crates/gateway-db/migrations/0001_initial_schema.sql` now holds the whole
  schema that the pre-release chain of 21 migrations ended with (same
  tables, columns, constraints, indexes, functions, and triggers; the one
  constraint name PostgreSQL had truncated at 63 characters is spelled out
  as `payment_observations_collected_at_tx_index_non_negative`). Payday is
  pre-release with no data to carry forward, so there is no upgrade path:
  a database that ran the old chain refuses the new set, and every existing
  environment is recreated empty. Until launch, schema changes keep editing
  the baseline; staging recreates its database when it deploys one.
- `pay.payday.sh` is gone. It was a second hostname on the API's load
  balancer whose only page, `GET /pay/{id}`, answered a `301` to the checkout
  on `payday.sh`; every `deposit_url` already points at the checkout, so the
  hostname, the `payment_domain_name` and `payment_route53_zone_id`
  variables, the certificate name, and the redirect route are removed, and
  `checkout_base_url` is required.
- Terraform no longer creates the SES DKIM records. They are names under
  `payday.sh`, whose DNS is hosted at Vercel, so they are published as the
  `notification_dkim_records` output and added in Vercel DNS by hand; the
  API's Route53 zone covers `api.payday.sh` alone.
- The deployment runbook (`docs/production-runbook.md`) is rewritten around
  the current shape of the product: the web app (landing, checkout,
  dashboard) on Vercel at `payday.sh`, the `api` and `indexer` services on
  AWS, Privy, Auth0 and Resend, and the contract generation on Monad. DNS
  stays as it is: `payday.sh` at Vercel with only `api.payday.sh` delegated
  to Route53. The sandbox tfvars example sends email from its own subdomain
  and no longer mentions the removed `recovery_address` variable.
- Deposits and deposit requests are the product's two primitives, and every
  surface now says so. The API resource `/v1/payments` is
  `/v1/deposit-requests` (payer routes `/v1/payer/deposit-requests/{id}`,
  the wallet challenge and attestation routes under it, the operator release
  route `/v1/admin/deposit-requests/{id}/release`, the rendered document
  `…/request.pdf`, the onboarding demo `…/onboarding-deposit`); IDs are
  `dr_…`; the request body's `bill_to` party is `payer`, list summaries carry
  `payer_name`, `payment_url` is `deposit_url`, the payer response's
  `payment_uri` is `deposit_uri` and its unlocked `invoice` block is
  `details`, and `paid_at`/`paid_at_block` are `deposited_at`/
  `deposited_at_block`. Public statuses are `awaiting_deposit`,
  `partially_deposited`, `deposited`, `settled`, `expired`, `returned`, and
  `needs_attention`. Webhook events are `deposit_request.deposited`,
  `.settled`, `.expired`, `.returned` (formerly `payment.refunded`),
  `.needs_attention`, `.likely_unsolicited`, `.recovered_funds`, and
  `.ready`, with `data.deposit_request` in place of `data.payment` and the
  envelope's status vocabulary now identical to the API's (migration `0021`,
  which also rewrites queued events). Error codes follow:
  `deposit_request_not_found`, `deposit_request_not_payable`,
  `deposit_request_not_settled`, `deposit_sender_mismatch`,
  `invalid_deposit_link`, `deposit_request_not_blocked`, and
  `onboarding_deposit_*`. `GET /v1/status` reports
  `deposit_indexing_and_settlement`. The SDK's `payments` namespace is
  `depositRequests` (`DepositRequest`, `CreateDepositRequest`,
  `DepositRequestStatus`, `PayerDepositRequest`, `requestPdf`,
  `onboardingDeposit`); the dashboard lists deposits at `/dashboard/deposits`
  and the composer, checkout, PDF, and emails speak of deposit requests,
  payers, and deposits. The Proof of Payment keeps its name and its
  versioned field names (`payment_id`, `payment_address`, the
  `payday.invoice` snapshot schema with `bill_to`), because those are
  hash-committed and signed formats that change only with a version bump.
- The payment address is created by the payer's wallet, not at issuance. A
  deposit request is issued without an address; once the payer's session
  satisfies its policy (at once for `permissionless`, after the mailbox code
  or the merchant's client secret otherwise), the hosted checkout has the
  payer sign an EIP-712 `PayerAttestation` from the wallet they will pay
  from (`POST /v1/payer/payments/{id}/wallet/challenge` then `/attest`). The
  CREATE3 salt is `keccak256("PAYDAY_SALT_V2" || attribution_hash ||
  attestation digest)` and the wallet is the address's recovery term, so
  `address`, `payer_wallet`, `recovery_address`, `wallet_bound_at`, and
  `self_settlement` are `null` until then and a `payment.ready` webhook
  reports the binding. The random attribution nonce is gone; the canonical
  issuance snapshot no longer carries `recovery_address`; `attribution.version`
  is 2. Only transfers from the attested wallet are the payer's: money from
  any other wallet still counts and settles but sets `likely_unsolicited_at`
  (its new meaning) and makes the proof unavailable
  (`409 payment_sender_mismatch`). `requirements` and the merchant's
  verification `facts` gain `wallet`; attempts gain the `wallet` kind. The
  `PaymentFactory` and `Payment` contracts are unchanged.
- The platform recovery wallet is gone. Overpayment remainders, expired
  balances, and late transfers return on-chain to the payer's attested
  wallet; Payday custodies nothing. `gatewayd` no longer reads
  `PAYDAY_RECOVERY_ADDRESS`, Terraform drops `recovery_address` and the
  API task's precondition on it, and the `recovery` KMS key stays only as a
  legacy resource until any balance it holds is returned. The
  `recovered_funds` ledger and `payment.recovered_funds` webhook now
  describe returns to the payer.
- Proof of Payment v2 (`payday.proof.v2`): the proof carries the payer's
  wallet attestation (the exact typed data signed, its digest, and the
  signature) and the recovery address, and `gateway_core::verify_proof`
  checks hash → attestation → salt → CREATE3 address, that every credited
  transfer came from the attested wallet, and a Payday attestation
  (`payday.attestation.v2`) that names the wallet, the challenge nonce, and
  the observed `facts` (`mailbox`, `merchant_session`, `wallet`) alongside
  the attribution hash, chain, and address.
- Migration `0018_merchant_session` is renumbered `0019_merchant_session`:
  it shared version 18 with `0018_account_wallet`, which sqlx refuses to
  apply. The payer wallet binding is `0020_payer_wallet_binding`. Both
  reset pre-release rows.
- Merchants sign in through Privy instead of Auth0. The landing page's
  "Start Building" dialog runs Privy's email code exchange; what the browser
  holds is Privy's identity token, and that token is the dashboard session
  the API accepts on every merchant route. Every account gets an embedded EVM
  wallet from Privy at its first sign-in (`accounts.wallet_address`, kept
  current from each session's identity token), and `GET /v1/account` now
  returns `email` and `wallet_address`. Auth0 stays for the flows that prove
  a mailbox without creating an account — payers on gated invoices and
  issuer contact addresses — which outnumber merchant sign-ups many times
  over. `gatewayd` takes `PAYDAY_PRIVY_APP_ID` (the app's public id; it
  verifies ES256 identity tokens against Privy's published JWKS, `iss`
  `privy.io`, `aud` the app) and no longer reads `PAYDAY_AUTH0_ISSUER`,
  `PAYDAY_AUTH0_AUDIENCE`, `PAYDAY_AUTH0_CLIENT_ID`, or
  `PAYDAY_DASHBOARD_AUTH0_CLIENT_ID`; the payer `PAYDAY_PAYER_AUTH0_*`
  settings are unchanged. Terraform takes `privy_app_id` and drops
  `auth0_audience`, `auth0_client_id`, and `dashboard_auth0_client_id`; the
  Auth0 `Payday Dashboard` application (`auth0/dashboard.tf`) is gone.
- API keys are managed from the dashboard session alone. `POST` and `DELETE
  /v1/account/api-key` take the same bearer the session reads with and refuse
  an API key (`identity_unauthorized`); the five-minute fresh-OTP step-up and
  the single-use authentication event are gone with the Auth0 merchant
  tokens. `expected_generation` is the generation `GET /v1/account` reports —
  a signed-in account exists at generation 1 before it holds a key — and the
  SDK's `account.issueApiKey(expectedGeneration)` and
  `account.revokeApiKey(expectedGeneration)` no longer take a separate token.
  The CLI's `payday login` is no longer accepted by the API.
- Deposit requests settle to the account's Payday wallet by default. The
  composer and the onboarding walkthrough preselect it; an identity's saved
  wallets are offered as alternatives when it has any, and setting up an
  identity no longer asks for one — a proven contact mailbox is all it needs.
  The dashboard gained an **Account** section: the signed-in mailbox, the
  wallet in full with copy and explorer links, and its USDC and gas balance
  on the deployment's chain read from the public RPC.
- Local development: `just seed` mints an account and API key straight into
  the runner's database (`scripts/local-api-key.sh`), since Privy has no local
  stand-in — the dashboard signs in against the real development app even
  locally — and `scripts/e2e-anvil.sh` does the same instead of driving the
  CLI's login. The browser suite swaps the Privy SDK for `web/test/privy-stub.tsx`
  at bundle time (`PAYDAY_PRIVY_STUB=1`).

### Removed

- Identity verification. The `verified_identity` and
  `verified_identity_unattributed` payer modes, the `expected_identity`
  assertion, the Didit provider and its `PAYDAY_DIDIT_*` settings, the
  hosted identity step in the checkout, `POST …/verify/identity/start`,
  `POST /v1/webhooks/identity`, the reconciler, credential reuse, manual
  review (`POST …/verification/review`,
  `POST /v1/admin/verifications/{id}/decision`), the `verification.declined`
  event, and the SDK's `startIdentity` and `requestVerificationReview` are
  gone. A payer policy is `permissionless` or `verified_email`;
  `requirements` carries `email` and `complete`. Migration `0017` drops the
  identity tables and columns, along with any pre-production rows only those
  modes could have produced.
- The `payday` command-line client (`crates/gateway-cli`), its installer,
  Homebrew formula generator, release workflow, and reference documentation.
  Payday is API-first with the dashboard for management: every command had an
  API route or a dashboard control behind it, and those remain. The offline
  Proof of Payment checks the CLI itemised live on in
  `gateway_core::verify_proof`; the CLI's optional live receipt checks over
  JSON-RPC have no replacement yet.
- The landing page's unused terminal demo and install components.

### Added

- The `merchant_session` payer mode, for applications that have already
  signed their user in. The policy names the payer by the merchant's own
  `payer_reference`; the `201` that issues the payment carries a single-use
  `client_secret` (fifteen minutes, returned once, stored hashed), and
  `POST /v1/payments/{id}/client-secret` mints another for a returning user.
  The merchant's server sends the user to `payment_url#cs=<secret>`; the
  hosted checkout reads the fragment, removes it from the address bar, and
  exchanges it at `POST /v1/payer/payments/{id}/session` for a payer session
  that already satisfies the policy. The exchange completes the payment's
  verification (raising `verification.approved`), records an approved
  `merchant_session` attempt, and needs no email provider. A second exchange
  answers `409 client_secret_used`; an unknown, expired, or foreign secret
  `401 client_secret_invalid`; email codes on such a payment, or a client
  secret on a `verified_email` one, `409 verification_method_not_applicable`.
  Webhook payment objects gain `payer_reference`, so `payment.paid` and
  `payment.settled` credit the right ledger without a lookup;
  `requirements` and the merchant's `facts` gain `merchant_session`.
  Dashboard: the mode is shown on requests that have it (payer reference,
  "Opened by your app" activity) but is not offered by the composer, since
  only an application with a signed-in user can hand over the secret. SDK:
  `payments.createClientSecret`, `verification.exchangeClientSecret`, and
  `checkoutUrl`. Migration `0018`.
- `GET /v1/payments/{id}/verification`: the merchant's verification view of
  an invoice — each fact the policy needs, and every attempt made against it
  with its status and times. Dashboard: verification activity on the request
  detail once the payer has made an attempt. SDK: `payments.verification`.

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
  same transaction.
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
  issuer, heading, masked mailbox, and requirements.
- SDK: `payer.verification.startEmail/confirmEmail/status` and
  `payer.payments.qr(id, payerSession)` returning a `Blob`. `qrUrl` is gone:
  a gated invoice's QR needs a session, which must never be in an image URL.
- The development identity provider serves a `payday-payer-local` client and
  audience next to the merchant ones; `just dev` and `just e2e` configure
  the payer settings against it.

- Invoice documents. `POST /v1/payments` now takes the document the payment
  fulfils: required `issuer` and `bill_to` parties (name, optional email and
  free-text details), a required `payer_policy` (`permissionless` or
  `verified_email` with the merchant's expected email), and optional
  `heading`, `notes`, `customer_id`, and `attachment_id`.
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
- Sign-up from the landing page: the hero's "Start Building" opens a dialog
  that runs the dashboard's own emailed-code exchange, and the API provisions
  an account on first sight of the identity, so there is no separate
  registration. A merchant already holding a session goes straight to the
  dashboard instead. While a code is live the dialog counts its window down;
  once the window closes it offers to send another, so two codes are never
  outstanding at once.
- Issuer identities and payout addresses: the merchant's own side of an
  invoice, saved once instead of retyped. `POST /v1/issuers` and its
  `GET`/`PATCH`/`DELETE`, `POST /v1/payout-addresses` and its `GET`/`DELETE`,
  and `PUT /v1/issuers/{id}/payout-addresses` to set which wallets an identity
  settles to. A wallet is stored EIP-55 checksummed and once per account —
  saving one twice returns the row you have — and the association is
  many-to-many, with composite foreign keys that make a cross-account link
  unwritable.
- The contact address on an identity is **proven with an emailed code** before
  an invoice can carry it: `POST /v1/issuers/{id}/verify/email/start` sends one
  to the stored address (never an address in the request, and at most one a
  minute per identity), and `.../confirm` records the proof. Moving the address
  clears it, because a different mailbox is a different claim. Payers are told
  to write to that address, so it gets the same treatment a payer's own mailbox
  gets rather than being taken on trust.
- Issuance is deliberately unchanged: `POST /v1/payments` still takes the party
  and the payout address inline and snapshots them, so an identity edited later
  cannot reach an invoice already issued. It additionally takes `issuer_id` and
  stores it immutably beside the document, returning it on `Payment` and
  `PaymentSummary`: the identity's id is the durable handle, so the requests
  issued under it stay identifiable after it is renamed, moved to another
  mailbox, or pointed at different wallets. Deleting an identity that requests
  were issued under is refused (`issuer_in_use`).
- `/dashboard` is the whole dashboard, and where signing in now lands: the
  deposit requests, the customers behind them, and the flow that issues a new
  one, on one page with no section tabs to click through first. With nothing
  issued yet it is an empty state that says what a deposit request is and
  offers the one action worth taking. That action runs in the page rather than
  in a modal or another route — amount and deadline, billing, payer policy,
  then a review — with a running preview beside it that doubles as the review
  and leads back to the step that set each value. `/dashboard/invoices` and
  `/dashboard/customers` redirect to it.
- A new account is not shown a description of the product: with neither an
  identity nor a request, the dashboard *is* the setup — name, contact address,
  wallet — and issuing follows straight out of it. The whole form is on the page
  at once: only the section being answered is enabled, the ones after it are
  visible but inert, and the ones before it keep their answers on screen with a
  control to go back and change them. The one action lives in a footer that
  belongs to the form rather than to any card, and sticks to the bottom of the
  viewport while the form runs past the fold, so it is never a scroll away from
  the section being answered; it names that section, and completing one scrolls
  the next into view. It resumes at whatever section is unfinished.
- `GET /v1/payments` filters on `customer_id`, `issuer_id`, and `verification`
  (`not_required`, `pending`, `verified`, `likely_unsolicited`). Verification is
  its own parameter because it is its own fact: a gated request can be funded
  before its payer has verified.
- The three dashboard sections — requests, identities, customers — share one
  shape: heading, a line of what the section holds, and the *New* control on
  that row, then the content. An account with nothing issued sees the requests
  table with its columns and an empty row rather than a placeholder frame.
- The dashboard's tables page five rows at a time and show Previous/Next only
  when there is another page; the requests table filters on status,
  verification, and customer through a styled control rather than the platform
  `<select>`, carries the issuer identity as a badge, shows amounts with the
  token's mark, and opens a request's detail when a row is clicked. Adding a
  deposit request, an identity, or a customer is now the same control in the
  same place above each table.
- The long invoice form at `/dashboard/invoices/new` is gone, and the composer
  asks everything `POST /v1/payments` takes. The billing step — renamed from
  "Billed to", since it now holds more than the party — carries the saved
  customer, the billed party, what the request is for, the reference, notes,
  and the PDF attachment. The deadline offers a moment of the merchant's own
  choosing beside the 24-hour, 7-day, and 30-day presets: a preset is sent as
  `expires_in`, a picked moment as `expires_at`, since converting it to a
  duration would re-anchor it to whenever the request arrived. A customer's own
  page links to `/dashboard?customer=`, which opens the composer on them.
- The expected payer email is required for every policy beyond permissionless,
  says so on its label, and arrives pre-filled from the billed party's address,
  following that field until the merchant types their own.
- A deposit request opens in its own row rather than on a page of its own. The
  list is where a merchant works, and reading one request no longer costs the
  filters and the page they were on. The open row leads with the two questions
  a list cannot answer, drawn rather than written — a bar for how much of the
  amount has arrived, and a three-point rail for issued, funded, and however it
  ended — then the payment, the verification and its activity, the payer's
  view, and the files, with nothing repeated from the row above. One row is
  open at a time, and a request is only fetched once its row has been opened.
  `/dashboard/invoices/{id}` redirects to the list, so links already sent still
  land somewhere useful. Addresses are shown in full and link to the explorer;
  a policy that only checks a mailbox no longer breaks that single fact out
  beside the verdict that already states it.
- The payer's view sits in the open row — the public payer projection, read
  with no payer session, so a gated request shows what an unverified visitor
  would see — with the shareable link ready to copy.
- An issuer identity's name is unique per account, case and surrounding space
  included: two identities with the same name are the same row to whoever reads
  a list or a picker. `POST /v1/issuers` and its `PATCH` answer
  `409 issuer_name_taken`, a unique index enforces it regardless of the caller,
  and both dashboard forms refuse the name before the request is made. Contact
  addresses may still repeat, since identities can share a support mailbox.
- Removing an identity's only wallet asks for its replacement rather than
  leaving it with nowhere to settle: the badge greys out, the wallet form
  opens, and the swap lands as one edit that can still be cancelled. An
  identity can no longer be stranded into the setup flow by an edit.
- An identity row takes the API's own answer as its new baseline when it saves,
  so a wallet that was just added stops reading as an unsaved edit and Save
  settles instead of staying lit.
- A billed party typed into the composer is saved as a customer, so it appears
  in the customers table and the next request can pick it rather than retype
  it.
- Every field is checked as it is typed rather than on submit, in the setup
  form, the composer, and the identity manager: a pasted address that is not one
  says so at once, and the button to the next step stays inert until the step is
  answerable. A payout-address label is bounded to 20 characters from a small
  printable set, in the browser, at the API, and in the column. Setting up gates issuing, never looking, so an account
  that issued from the CLI still sees what it has. The composer picks the
  identity and the wallet from what was saved, preselected when there is one of
  each, and no longer asks for either as free text.

### Changed

- `PAYDAY_AUTH0_CLIENT_ID` now names the `Payday Dashboard` single-page
  application, the one merchant client `gatewayd` accepts: its tokens are the
  session credential and, while fresh, the credential that issues an API key.
  `PAYDAY_DASHBOARD_AUTH0_CLIENT_ID`, the Terraform variable
  `dashboard_auth0_client_id`, and the Auth0 Action secret
  `PAYDAY_DASHBOARD_CLIENT_ID` are gone; the Action admits only
  `PAYDAY_CLIENT_ID`. Deployments must point `PAYDAY_AUTH0_CLIENT_ID` and
  `PAYDAY_CLIENT_ID` at the dashboard application and may delete the
  `Payday CLI` Native application from the tenant.
- `just seed` and the end-to-end suite create local accounts through
  `scripts/local-api-key.sh`, the same three-call email-OTP exchange the
  dashboard's API key section makes, and print the key once.
- The dashboard wears the landing page's dark palette and type in both colour
  schemes, by redefining the theme tokens its pages already read rather than
  restyling them, so arriving from "Start Building" no longer flips the ground
  to white. Its header is the landing page's too: the same wordmark at the same
  size, the same Docs and Pricing links on the same rule, plus Sign out.
- The dashboard login page is gone. The landing page's "Start Building" dialog
  is the only way in — it always was the same exchange — and anyone reaching a
  dashboard route without a live session, or signing out, lands on the landing
  page rather than on a second sign-in form.
- `payday-dev-identity` issues access tokens good for 24 hours rather than 5
  minutes, matching Auth0's default for a resource server. The dashboard
  session is exactly the token's lifetime because nothing refreshes it, so the
  old value signed a merchant out mid-invoice; the five-minute freshness that
  API-key issuance demands is a separate window and is unchanged.
- `PAYDAY_DEV_IDENTITY_OTP` fixes the code the local provider emails, so
  signing in during development does not mean reading it out of the runner's
  log.
- `issuer`, `bill_to`, and `payer_policy` are required on `POST /v1/payments`;
  a body without them is rejected. An issued invoice is immutable.
- An emailed one-time code is good for five minutes rather than three, in the
  Auth0 passwordless connection, in its email, and in `payday-dev-identity`,
  which previously kept codes until they were used and now refuses and
  discards an expired one.
- Cross-origin access to the merchant routes (payments, customers,
  issuers, attachments) is allowed from the configured web origin only
  (`PAYDAY_PUBLIC_BASE_URL`) for `GET`, `POST`, `PATCH`, `PUT`, and `DELETE`
  with the
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
