//! OpenAPI is generated from typed documentation models and operation
//! annotations. The models intentionally live at the HTTP boundary so the
//! persistence/domain types do not acquire documentation-only dependencies.
#![allow(dead_code)]

use axum::response::Html;
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi, ToSchema};

#[derive(Serialize, ToSchema)]
struct ErrorDetail {
    code: String,
    message: String,
}
#[derive(Serialize, ToSchema)]
struct ErrorResponse {
    error: ErrorDetail,
    request_id: String,
}
#[derive(Serialize, ToSchema)]
struct Chain {
    id: String,
    name: String,
    /// The gas token's symbol on this chain (`MON`, `ETH`).
    native_symbol: String,
}
#[derive(Serialize, ToSchema)]
struct Token {
    symbol: String,
    address: String,
    decimals: u8,
}
/// One network a deposit request can be paid on: the chain and the exact
/// native USDC contract there.
#[derive(Serialize, ToSchema)]
struct Network {
    chain: Chain,
    token: Token,
}
#[derive(Serialize, ToSchema)]
struct AsOf {
    block: String,
    at: String,
}
#[derive(Serialize, ToSchema)]
struct SelfSettlement {
    /// The chain the payer chose; `factory` is the PaymentFactory there.
    chain_id: String,
    factory: String,
    salt: String,
}
#[derive(Serialize, ToSchema)]
struct Attention {
    code: String,
    message: String,
    action: String,
}
#[derive(Serialize, ToSchema)]
struct IndexerFreshness {
    last_indexed_block: Option<String>,
    last_finalized_block: Option<String>,
    cursor_updated_at: Option<String>,
}
/// One side of the deposit request: bounded free text rendered verbatim.
#[derive(Serialize, Deserialize, ToSchema)]
struct Party {
    /// 1–255 bytes.
    name: String,
    /// 3–254 bytes.
    email: Option<String>,
    /// At most 4000 bytes.
    details: Option<String>,
}
/// The verification add-ons attached to a deposit request. Omitted or empty
/// means none: the permissionless default, which accepts any deposit. Any
/// combination may be attached.
#[derive(Serialize, Deserialize, ToSchema)]
#[schema(example = json!({"email":{"expected_email":"alice@example.com"},"wallet_attestation":true}))]
struct PayerVerification {
    /// Prove ownership of exactly this mailbox with a one-time code. The
    /// payer sees only a masked hint.
    email: Option<EmailVerification>,
    /// The merchant's own authentication: its application signed the payer
    /// in and released the single-use client secret that opens the checkout.
    merchant_auth: Option<MerchantAuth>,
    /// Require the payer to sign an EIP-712 attestation from the wallet
    /// they will pay from; only transfers from it count toward the request.
    #[serde(default)]
    wallet_attestation: bool,
}
/// Prove ownership of exactly this mailbox with a one-time code.
#[derive(Serialize, Deserialize, ToSchema)]
struct EmailVerification {
    /// Trimmed and lowercased; the payer sees only a masked hint.
    expected_email: String,
}
/// The merchant's own authentication for the payer it let in.
#[derive(Serialize, Deserialize, ToSchema)]
struct MerchantAuth {
    /// The merchant application's own identifier for the payer it
    /// authenticated: 1–128 bytes, one printable token, case preserved.
    /// Returned on every webhook for the deposit; never shown to the payer.
    payer_reference: String,
}
/// A single-use secret that opens the hosted checkout for the payer the
/// merchant authenticated. Returned once; the API stores only its hash.
#[derive(Serialize, ToSchema)]
struct ClientSecret {
    /// `cs_` plus 43 URL-safe characters. Hand it to the payer in the
    /// checkout URL's fragment (`/pay/{id}#cs=…`), never in a path or query.
    client_secret: String,
    /// Fifteen minutes after minting; the secret must be exchanged by then.
    expires_at: String,
}
#[derive(Serialize, ToSchema)]
struct AttachmentDescriptor {
    id: String,
    filename: String,
    /// Always `application/pdf`.
    mime_type: String,
    byte_length: String,
    /// `0x`-prefixed SHA-256 of the PDF bytes.
    sha256: String,
    /// Short-lived signed link, present on attachment routes only.
    download_url: Option<String>,
}
/// The commitment the deposit address was derived from (product plan §5).
#[derive(Serialize, ToSchema)]
struct Attribution {
    version: u16,
    hash: String,
}
#[derive(Deserialize, ToSchema)]
#[schema(example = json!({"amount":"10.50","payout_address":"0x1111111111111111111111111111111111111111","issuer":{"name":"Acme Corp"},"payer":{"name":"Globex"},"expires_in":3600,"reference":"INV-42","customer_id":"cus_0198f80c-1111-7dc1-a369-90556a64f700","metadata":{"po":"PO-77"}}))]
struct CreateDepositRequest {
    amount: String,
    /// `USDC` (the default) or `USDT`: the one currency the request is
    /// denominated in. `USDT` requires `chain_id`: it settles on the pinned
    /// chain and does not bridge.
    currency: Option<String>,
    /// Where exactly `amount` settles. May be left out when `issuer_id`
    /// names an identity with a saved payout address; its first one is used.
    payout_address: Option<String>,
    /// Pin the network the payer must pay on: one of the deployment's chain
    /// ids, as a decimal string. Left out, the payer chooses among all of
    /// them when they sign. A chain this deployment does not serve is
    /// `422 unsupported_chain`.
    chain_id: Option<String>,
    /// The issuing party as the document will carry it. May be left out
    /// when `issuer_id` is given: the identity's name, contact address, and
    /// details are snapshotted in its place. An inline party always wins.
    issuer: Option<Party>,
    /// The paying party. May be left out when `customer_id` is given: the
    /// customer is snapshotted in its place. An inline party always wins.
    payer: Option<Party>,
    /// The verification add-ons attached; omitted means none, the
    /// permissionless default.
    verification: Option<PayerVerification>,
    /// A `cus_` id of one of your customers.
    customer_id: Option<String>,
    /// The `iss_` id of the saved issuer identity this is issued under.
    /// `issuer` above is still the snapshot the document carries; this
    /// records which identity it came from and survives that identity being
    /// renamed.
    issuer_id: Option<String>,
    /// At most 4000 bytes.
    notes: Option<String>,
    /// At most 200 bytes; shown to the payer before verification.
    heading: Option<String>,
    reference: Option<String>,
    #[schema(value_type = Object)]
    metadata: Option<serde_json::Value>,
    /// The `att_` id of a finalized PDF upload; at most one per deposit request.
    attachment_id: Option<String>,
    expires_in: Option<u64>,
    expires_at: Option<String>,
}
#[derive(Serialize, ToSchema)]
struct DepositRequest {
    id: String,
    deposit_url: String,
    status: DepositRequestStatus,
    /// Every network the payer may pay on; the request commits to all of
    /// them and the payer picks one when they sign.
    networks: Vec<Network>,
    /// The network the payer chose; null until a wallet is bound.
    chain: Option<Chain>,
    currency: String,
    token: Option<Token>,
    /// The one-time deposit address; null until the payer attests the
    /// wallet they will pay from, which the address commits to.
    address: Option<String>,
    address_explorer_url: Option<String>,
    payout_address: String,
    /// The wallet the payer attested, once they have. Only transfers from
    /// it are the payer's; overpayments, expired balances, and late
    /// transfers return to it.
    payer_wallet: Option<String>,
    /// Always equal to `payer_wallet`: the address's recovery term.
    recovery_address: Option<String>,
    /// When the attestation was accepted and the address derived.
    wallet_bound_at: Option<String>,
    expires_in: Option<u64>,
    amount: String,
    amount_base_units: String,
    received: String,
    received_base_units: String,
    remaining: String,
    remaining_base_units: String,
    fee_amount: String,
    fee_amount_base_units: String,
    net_amount: String,
    net_amount_base_units: String,
    issuer: Party,
    payer: Party,
    notes: Option<String>,
    heading: Option<String>,
    reference: Option<String>,
    customer_id: Option<String>,
    issuer_id: Option<String>,
    /// The verification add-ons attached, including the merchant's
    /// assertions; omitted when none are attached. Never shown to payers
    /// beyond what the verification views disclose.
    verification: Option<PayerVerification>,
    attachment: Option<AttachmentDescriptor>,
    verification_completed_at: Option<String>,
    likely_unsolicited_at: Option<String>,
    /// `merchant_session` only, and only in the `201` that issued the
    /// deposit: the first client secret. Absent on every later read and
    /// on idempotent replays; mint another with `POST …/client-secret`.
    client_secret: Option<String>,
    client_secret_expires_at: Option<String>,
    attribution: Attribution,
    #[schema(value_type = Object)]
    metadata: serde_json::Value,
    created_at: String,
    updated_at: String,
    expires_at: String,
    deposited_at: Option<String>,
    deposited_at_block: Option<String>,
    settled_at: Option<String>,
    settled_block: Option<String>,
    expired_at: Option<String>,
    cancellation_requested_at: Option<String>,
    settlement_tx_hash: Option<String>,
    settlement_explorer_url: Option<String>,
    as_of: Option<AsOf>,
    /// Null until the address exists.
    self_settlement: Option<SelfSettlement>,
    attention: Option<Attention>,
    transfers: Vec<Transfer>,
    indexer_freshness: IndexerFreshness,
}
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum DepositRequestStatus {
    AwaitingDeposit,
    PartiallyDeposited,
    Deposited,
    Settled,
    Expired,
    Returned,
    NeedsAttention,
}
#[derive(Serialize, ToSchema)]
struct DepositRequestPage {
    deposit_requests: Vec<DepositRequestSummary>,
    next_cursor: Option<String>,
}
#[derive(Serialize, ToSchema)]
struct DepositRequestSummary {
    id: String,
    deposit_url: String,
    heading: Option<String>,
    payer_name: String,
    reference: Option<String>,
    #[schema(value_type = Object)]
    metadata: serde_json::Value,
    created_at: String,
    updated_at: String,
    expires_at: String,
    status: DepositRequestStatus,
    amount: String,
    received: String,
    /// The verification add-ons attached; omitted when none are attached.
    verification: Option<PayerVerification>,
    customer_id: Option<String>,
    issuer_id: Option<String>,
    has_attachment: bool,
    verification_completed_at: Option<String>,
    likely_unsolicited_at: Option<String>,
    cancellation_requested_at: Option<String>,
}
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum VerificationFactStatus {
    NotRequired,
    Pending,
    Approved,
    Declined,
}
/// Each fact the request's add-ons need, on its own. `wallet` is
/// `not_required` unless the wallet-attestation add-on is attached: only
/// then does the payer sign before an address exists. `complete` covers
/// the identity add-ons alone.
#[derive(Serialize, ToSchema)]
struct VerificationRequirements {
    email: VerificationFactStatus,
    wallet: VerificationFactStatus,
    /// The merchant's application opened the checkout by exchanging a
    /// client secret.
    merchant_session: VerificationFactStatus,
    complete: bool,
}
/// One attempt: what was attempted and where it stands.
#[derive(Serialize, ToSchema)]
struct VerificationAttempt {
    id: String,
    /// email, wallet, or merchant_session.
    kind: String,
    /// pending, approved, or abandoned. A merchant_session attempt is
    /// recorded approved: the exchange is the proof.
    status: String,
    verified_at: Option<String>,
    created_at: String,
}
#[derive(Serialize, ToSchema)]
struct VerificationDetail {
    /// The add-ons the request attached; omitted when none are attached.
    verification: Option<PayerVerification>,
    verification_completed_at: Option<String>,
    likely_unsolicited_at: Option<String>,
    facts: VerificationRequirements,
    attempts: Vec<VerificationAttempt>,
}
#[derive(Serialize, ToSchema)]
struct Transfer {
    disposition: String,
    transaction_hash: String,
    explorer_url: Option<String>,
    sender: String,
    amount: String,
    amount_base_units: String,
    block: String,
    timestamp: String,
    collected: bool,
    /// Present when Relay's solver sent the transfer for a cross-chain
    /// payment the attested wallet made from another network.
    relay: Option<TransferRelay>,
}
#[derive(Serialize, ToSchema)]
struct TransferRelay {
    /// Relay's request id, `0x` hex, 32 bytes.
    request_id: String,
    /// Decimal chain id the wallet paid on.
    origin_chain_id: String,
    /// The transaction the wallet sent there.
    origin_transaction_hash: Option<String>,
}
#[derive(Serialize, ToSchema)]
struct TransferList {
    transfers: Vec<Transfer>,
}
#[derive(Serialize, ToSchema)]
struct IssuedApiKey {
    /// Shown once; nothing later can return it again.
    api_key: String,
    generation: i64,
    replaced_previous_key: bool,
}
#[derive(Serialize, ToSchema)]
struct Account {
    account_id: String,
    /// The mailbox the account signs in with.
    email: Option<String>,
    /// The account's own EVM wallet (EIP-55), where deposits settle by
    /// default; null until the first dashboard session that carried one.
    wallet_address: Option<String>,
    key_hint: Option<String>,
    generation: i64,
    created_at: String,
    rotated_at: Option<String>,
    previous_key_expires_at: Option<String>,
    revoked_at: Option<String>,
}
#[derive(Serialize, ToSchema)]
struct StatusChain {
    id: String,
    name: String,
    finalized_block: Option<String>,
    finalized_at: Option<String>,
    /// The open finality fault while the chain is halted.
    halted: Option<String>,
    indexer: StatusIndexer,
    sweeper: StatusSweeper,
    withdrawals: StatusWithdrawals,
    signers: Vec<StatusSigner>,
}
#[derive(Serialize, ToSchema)]
struct StatusIndexer {
    cursor_block: Option<String>,
    cursor_at: Option<String>,
    lag_blocks: Option<u64>,
}
#[derive(Serialize, ToSchema)]
struct StatusSweeper {
    /// `running`, `halted`, `failing`, `stale` or `unknown`.
    state: String,
    detail: Option<String>,
    queued: i64,
    in_flight: i64,
    oldest_uncollected_secs: Option<f64>,
}
#[derive(Serialize, ToSchema)]
struct StatusWithdrawals {
    authorized: i64,
    awaiting_attestation: i64,
    attested: i64,
    in_flight: i64,
}
#[derive(Serialize, ToSchema)]
struct StatusSigner {
    address: String,
    balance_wei: Option<String>,
    low_balance: bool,
    error: Option<String>,
    observed_at: String,
}
/// One entry per supported network.
#[derive(Serialize, ToSchema)]
struct ServiceStatus {
    chains: Vec<StatusChain>,
}
#[derive(Deserialize, ToSchema)]
struct WebhookRequest {
    url: String,
}
#[derive(Serialize, ToSchema)]
struct Webhook {
    id: String,
    url: String,
    created_at: String,
    /// Set once disabled: the endpoint is no longer listed and receives
    /// nothing, but it can still be read and its deliveries stay readable.
    disabled_at: Option<String>,
    /// The signing secret, present only on the `201` that created the
    /// endpoint.
    secret: Option<String>,
}
#[derive(Serialize, ToSchema)]
struct WebhookList {
    webhooks: Vec<Webhook>,
}
#[derive(Serialize, ToSchema)]
struct TestDelivery {
    delivery_id: String,
}
#[derive(Serialize, ToSchema)]
struct Delivery {
    id: String,
    event_id: String,
    endpoint_id: String,
    /// `pending`, `delivered`, or `failed` (after the twelfth failed attempt).
    state: String,
    attempt_count: i32,
    next_attempt_at: String,
    delivered_at: Option<String>,
    created_at: String,
    /// Immutable per-attempt history, oldest first.
    attempts: Vec<DeliveryAttempt>,
}
#[derive(Serialize, ToSchema)]
struct DeliveryPage {
    /// Newest first.
    deliveries: Vec<Delivery>,
    /// The last delivery id on this page when more exist; pass it as
    /// `starting_after`.
    next_cursor: Option<String>,
}
/// Both key mutations take the generation `GET /v1/account` reports, so a
/// rotation or revocation nobody expected is a `409` rather than a surprise.
#[derive(Deserialize, ToSchema)]
struct ApiKeyGeneration {
    expected_generation: i64,
}
#[derive(Serialize, ToSchema)]
struct DeliveryAttempt {
    number: i32,
    attempted_at: String,
    duration_ms: i64,
    status: Option<i32>,
    error: Option<String>,
}
#[derive(Deserialize, ToSchema)]
#[schema(example = json!({"name":"Globex","email":"ap@globex.example","details":"Net 30"}))]
struct CustomerRequest {
    /// 1–255 bytes.
    name: String,
    /// 3–254 bytes.
    email: Option<String>,
    /// At most 4000 bytes.
    details: Option<String>,
}
/// A partial update: a field left out keeps its value; `email` or `details`
/// sent as `null` is cleared. The merged record is validated whole.
#[derive(Deserialize, ToSchema)]
#[schema(example = json!({"details":"Net 45"}))]
struct UpdateCustomerRequest {
    /// 1–255 bytes.
    name: Option<String>,
    /// 3–254 bytes, or `null` to clear.
    #[schema(nullable)]
    email: Option<String>,
    /// At most 4000 bytes, or `null` to clear.
    #[schema(nullable)]
    details: Option<String>,
}
#[derive(Serialize, ToSchema)]
struct Customer {
    id: String,
    name: String,
    email: Option<String>,
    details: Option<String>,
    created_at: String,
    updated_at: String,
}
#[derive(Serialize, ToSchema)]
struct CustomerPage {
    customers: Vec<Customer>,
    next_cursor: Option<String>,
}
#[derive(Serialize, ToSchema)]
struct CustomerStats {
    /// Deposit requests addressed to this customer, of any status, in every
    /// currency together.
    request_count: i64,
    /// One entry per currency the customer has been asked for: two currencies
    /// never add up.
    totals: Vec<CustomerCurrencyTotal>,
}
#[derive(Serialize, ToSchema)]
struct CustomerCurrencyTotal {
    /// The currency's wire code (`USDC`, `USDT`).
    currency: String,
    /// Deposit requests in this currency addressed to the customer, of any status.
    request_count: i64,
    /// Confirmed on chain across this currency's deposit requests for the
    /// customer. Base units, like a deposit request's own `amount_base_units`
    /// — scale by the currency's decimals for display.
    collected_base_units: String,
    /// Outstanding on this currency's requests still open — awaiting deposit
    /// or partially paid. Base units.
    pending_base_units: String,
}
/// `getCustomer` only: a list of many customers would mean one aggregate
/// query per row.
#[derive(Serialize, ToSchema)]
struct CustomerDetail {
    id: String,
    name: String,
    email: Option<String>,
    details: Option<String>,
    created_at: String,
    updated_at: String,
    stats: CustomerStats,
}
#[derive(Deserialize, ToSchema)]
struct IssuerRequest {
    /// 1–255 bytes, and unique among your identities: case and surrounding
    /// space are not a difference.
    name: String,
    /// The mailbox payers are told to write to. 3–254 bytes, lowercased on
    /// write. Changing it clears the verification.
    contact_email: String,
    /// At most 4000 bytes.
    details: Option<String>,
}
/// A partial update: a field left out keeps its value; `details` sent as
/// `null` is cleared. A changed `contact_email` clears the verification, so
/// a rename alone never touches a proven mailbox.
#[derive(Deserialize, ToSchema)]
#[schema(example = json!({"name":"Acme Inc."}))]
struct UpdateIssuerRequest {
    /// 1–255 bytes, unique among your identities.
    name: Option<String>,
    /// 3–254 bytes, lowercased on write.
    contact_email: Option<String>,
    /// At most 4000 bytes, or `null` to clear.
    #[schema(nullable)]
    details: Option<String>,
}
#[derive(Deserialize, ToSchema)]
struct ConfirmIssuerEmail {
    /// The emailed one-time code.
    otp: String,
}
#[derive(Deserialize, ToSchema)]
struct SetIssuerPayoutAddresses {
    /// Replaces the whole set; at most 25, all of them yours.
    payout_address_ids: Vec<String>,
}
#[derive(Deserialize, ToSchema)]
struct PayoutAddressRequest {
    /// 20-byte hex address, stored EIP-55 checksummed. Repeating one you
    /// already saved returns the row you have.
    address: String,
    /// At most 20 characters, starting on a letter or digit, from letters,
    /// digits, spaces, and `. _ ' & ( ) -`.
    label: Option<String>,
}
#[derive(Serialize, ToSchema)]
struct PayoutAddress {
    id: String,
    /// EIP-55 checksummed, ready to send back as `payout_address`.
    address: String,
    label: Option<String>,
    created_at: String,
}
#[derive(Serialize, ToSchema)]
struct PayoutAddressList {
    payout_addresses: Vec<PayoutAddress>,
}
#[derive(Serialize, ToSchema)]
struct Issuer {
    id: String,
    name: String,
    contact_email: String,
    details: Option<String>,
    /// Whether the contact mailbox has been proven with an emailed code.
    email_verified: bool,
    email_verified_at: Option<String>,
    /// The addresses this identity may settle to, in association order.
    payout_addresses: Vec<PayoutAddress>,
    created_at: String,
    updated_at: String,
}
#[derive(Serialize, ToSchema)]
struct IssuerPage {
    issuers: Vec<Issuer>,
    next_cursor: Option<String>,
}
#[derive(Serialize, ToSchema)]
struct StartIssuerEmail {
    /// Where the code went; the address is never taken from the request.
    contact_email: String,
    resend_available_at: String,
}
#[derive(Deserialize, ToSchema)]
struct AttachmentRequest {
    /// 1–255 bytes, kept verbatim for display.
    filename: String,
}
#[derive(Serialize, ToSchema)]
struct AttachmentUpload {
    id: String,
    /// Presigned PUT; valid for 15 minutes.
    upload_url: String,
    /// Every header the PUT must send verbatim: they are signed. Includes
    /// `content-type: application/pdf`, `x-amz-tagging`, and
    /// `if-none-match: *`, which makes the key write-once: a second PUT to
    /// the same upload_url is refused with 412 Precondition Failed.
    #[schema(value_type = Object)]
    headers: std::collections::BTreeMap<String, String>,
    expires_at: String,
}
#[derive(Serialize, ToSchema)]
struct AttachmentCommitment {
    id: String,
    byte_length: String,
    sha256: String,
}
/// The document the deposit address was derived from (product plan §5.2).
#[derive(Serialize, ToSchema)]
struct CanonicalIssuanceSnapshot {
    schema: String,
    canonicalization: String,
    issuer: Party,
    payer: Party,
    /// The currency's wire code (`USDC`, `USDT`); every network below is
    /// that currency's contract on its chain.
    currency: String,
    /// Decimal: base units per whole unit, so `amount_base_units` reads
    /// without a registry.
    decimals: String,
    amount_base_units: String,
    notes: Option<String>,
    heading: Option<String>,
    reference: Option<String>,
    expiration_timestamp: String,
    payer_verification: Option<PayerVerification>,
    /// EIP-55 checksummed: Gum's own recovery wallet, the payment
    /// contract's recovery term.
    recovery_address: String,
    attachment: Option<AttachmentCommitment>,
    /// Every network the request may be paid on, ordered by chain id.
    networks: Vec<SnapshotNetwork>,
    receiver_address: String,
}
/// One committed network: decimal chain id, EIP-55 token and factory.
#[derive(Serialize, ToSchema)]
struct SnapshotNetwork {
    chain_id: String,
    token_address: String,
    factory_address: String,
}
/// EIP-712 typed data as a wallet signs it (`eth_signTypedData_v4`).
#[derive(Serialize, ToSchema)]
struct PayerAttestationTypedData {
    #[schema(value_type = Object)]
    domain: serde_json::Value,
    #[serde(rename = "primaryType")]
    primary_type: String,
    #[schema(value_type = Object)]
    types: serde_json::Value,
    #[schema(value_type = Object)]
    message: serde_json::Value,
}
/// The payer's wallet attestation: the exact typed data the wallet signed
/// (`{statement, attributionHash, wallet, nonce, expiresAt}` under the
/// Gum domain of the chain the payer chose), its EIP-712 digest, and the
/// signature. The proof's salt is `keccak256("GUM_SALT_V3" ||
/// attribution_hash || digest)` and the wallet is the address's recovery term.
#[derive(Serialize, ToSchema)]
struct PayerWalletAttestation {
    address: String,
    typed_data: PayerAttestationTypedData,
    digest: String,
    signature: String,
    /// `ecdsa`.
    method: String,
}
/// One fact Gum observed: `mailbox` (provider `auth0`) or `wallet`
/// (provider `gum`), and when.
#[derive(Serialize, ToSchema)]
struct VerificationFact {
    kind: String,
    provider: String,
    at: String,
}
#[derive(Serialize, ToSchema)]
struct ProofTransfer {
    transaction_hash: String,
    log_index: String,
    /// The attested wallet, or Relay's solver for a relayed transfer.
    sender: String,
    recipient: String,
    amount_base_units: String,
    block_number: String,
    /// Present when Relay's solver made the transfer for a cross-chain
    /// payment the attested wallet sent. Accepted offline only when the
    /// same block appears in the attestation's `relay_fills`.
    relay: Option<RelayAttribution>,
}
#[derive(Serialize, ToSchema)]
struct RelayAttribution {
    /// Relay's request id, `0x` hex, 32 bytes.
    request_id: String,
    /// Decimal chain id the wallet paid on.
    origin_chain_id: String,
    /// The transaction the wallet sent there, `0x` hex, 32 bytes.
    origin_transaction_hash: String,
    /// Always the attested wallet.
    origin_sender: String,
    /// `receipt` (read from a chain Gum serves) or `relay_api` (Relay's
    /// record of the depositor).
    attribution_source: String,
}
/// A relayed transfer as the attestation vouches for it.
#[derive(Serialize, ToSchema)]
struct AttestedRelayFill {
    transaction_hash: String,
    log_index: String,
    request_id: String,
    origin_chain_id: String,
    origin_transaction_hash: String,
    origin_sender: String,
    attribution_source: String,
}
#[derive(Serialize, ToSchema)]
struct VerificationAttestationPayload {
    version: String,
    payment_id: String,
    /// `settlement` for a request without the wallet-attestation add-on,
    /// `wallet_attributed` for one with it: the verifier refuses an
    /// attestation carried by a proof of the other scope.
    scope: String,
    /// The issuance commitment the attestation is bound to, so a genuine
    /// attestation cannot be transplanted onto another deposit request's package.
    attribution_hash: String,
    /// `0x` hex, 32 bytes: the issuance nonce the address's salt was derived
    /// from, disclosed so third parties can rederive the address.
    issuance_nonce: String,
    chain_id: String,
    payment_address: String,
    /// EIP-55 checksummed wallet the payer attested; `wallet_attributed`
    /// scope only.
    payer_wallet: Option<String>,
    /// The nonce inside the payer's signed attestation; Gum's word is
    /// that it was issued only after the identity add-ons passed.
    /// `wallet_attributed` scope only.
    wallet_nonce: Option<String>,
    /// `not_required`, `approved`, or `pending`.
    result: String,
    verified_at: Option<String>,
    /// When the payer's attestation was accepted; `wallet_attributed` scope
    /// only.
    wallet_bound_at: Option<String>,
    facts: Vec<VerificationFact>,
    /// The transfers Relay's solver made for cross-chain payments the
    /// attested wallet sent, each with the origin Gum verified; absent
    /// when every transfer came from the wallet itself.
    relay_fills: Option<Vec<AttestedRelayFill>>,
}
/// Gum-attested, not address-committed: verification happens after
/// issuance. `signature` recovers to `signer` over
/// `keccak256("GUM_VERIFICATION_ATTESTATION_V4" || JCS(payload))`.
#[derive(Serialize, ToSchema)]
struct SignedVerificationAttestation {
    payload: VerificationAttestationPayload,
    signer: String,
    signature: String,
}
#[derive(Serialize, ToSchema)]
struct ProofOfPayment {
    version: String,
    payment_id: String,
    canonical_issuance_snapshot: CanonicalIssuanceSnapshot,
    canonicalization: String,
    attribution_hash: String,
    payer_wallet: PayerWalletAttestation,
    salt: String,
    /// The network the payer chose among the snapshot's `networks`; the
    /// factory and token are that network's.
    chain_id: String,
    factory_address: String,
    payment_address: String,
    token_address: String,
    /// The payer's attested wallet.
    recovery_address: String,
    settlement_transaction_hash: String,
    transfers: Vec<ProofTransfer>,
    verification: SignedVerificationAttestation,
}

#[utoipa::path(post, path="/v1/deposit-requests", operation_id="createDepositRequest", tag="deposit-requests",
 request_body(content=CreateDepositRequest, description="Issue a deposit request. expires_in and expires_at are mutually exclusive; the default lifetime is 24 hours. Every field except the deadline is immutable once issued: a reused Idempotency-Key with any difference is a 409 idempotency_conflict."),
 params(("Idempotency-Key"=String, Header, description="Required, 1-255 bytes")),
 responses((status=201, description="Created", body=DepositRequest), (status=200, description="Idempotent replay", body=DepositRequest, headers(("Idempotency-Replayed"=String, description="true"))), (status=400, body=ErrorResponse), (status=401, body=ErrorResponse), (status=409, description="idempotency_conflict, attachment_not_ready (also when a finalized upload expired from storage before it was attached: 'The upload expired before it was attached; upload the PDF again'), or attachment_already_attached", body=ErrorResponse), (status=422, body=ErrorResponse), (status=429, body=ErrorResponse)), security(("apiKey"=[])))]
fn create_deposit_request() {}
#[utoipa::path(get, path="/v1/deposit-requests", operation_id="listDepositRequests", tag="deposit-requests",
 params(("starting_after"=Option<String>, Query, description="dr_ cursor returned as next_cursor"), ("status"=Option<DepositRequestStatus>, Query), ("reference"=Option<String>, Query), ("customer_id"=Option<String>, Query, description="Only requests addressed to this customer"), ("issuer_id"=Option<String>, Query, description="Only requests issued under this identity"), ("verification"=Option<String>, Query, description="not_required, pending, verified, or likely_unsolicited"), ("limit"=Option<u32>, Query, minimum=1, maximum=100)),
 responses((status=200, body=DepositRequestPage), (status=400, body=ErrorResponse), (status=401, body=ErrorResponse), (status=429, body=ErrorResponse)), security(("apiKey"=[])))]
fn list_deposit_requests() {}
#[utoipa::path(get, path="/v1/deposit-requests/{id}", operation_id="getDepositRequest", tag="deposit-requests", params(("id"=String, Path, description="Complete deposit request ID or deposit address"),("wait_for"=Option<String>,Query,description="Set to change for long polling"),("timeout"=Option<u64>,Query,minimum=1,maximum=30)), responses((status=200, body=DepositRequest),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn get_deposit_request() {}
#[utoipa::path(post, path="/v1/deposit-requests/{id}/cancel", operation_id="cancelDepositRequest", tag="deposit-requests", params(("id"=String, Path)), responses((status=200,description="The deposit request with cancellation_requested_at set. Cancellation is presentation only: it cannot disable the address or change the settlement terms it commits to",body=DepositRequest),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn cancel_deposit_request() {}
#[utoipa::path(get, path="/v1/deposit-requests/{id}/transfers", operation_id="listDepositRequestTransfers", tag="deposit-requests", params(("id"=String, Path)), responses((status=200,description="Finalized transfer provenance",body=TransferList),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn transfers() {}
#[utoipa::path(get, path="/v1/deposit-requests/{id}/attachment", operation_id="getDepositRequestAttachment", tag="deposit-requests", params(("id"=String, Path)), responses((status=200,description="Descriptor with a signed download_url valid for GUM_ATTACHMENT_DOWNLOAD_TTL_SECS",body=AttachmentDescriptor),(status=401,body=ErrorResponse),(status=404,description="deposit_request_not_found or attachment_not_found",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn deposit_request_attachment() {}
#[utoipa::path(get, path="/v1/deposit-requests/{id}/request.pdf", operation_id="getDepositRequestPdf", tag="deposit-requests", params(("id"=String, Path)), responses((status=200,description="Deterministic deposit request summary as application/pdf, served as an attachment"),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn request_pdf() {}
#[utoipa::path(get, path="/v1/deposit-requests/{id}/proof", operation_id="getProofOfPayment", tag="deposit-requests", params(("id"=String, Path)), responses((status=200,description="Verifiable offline; gum_core::verify_proof holds the checks",body=ProofOfPayment),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,description="deposit_request_not_settled, or deposit_sender_mismatch when credited funds came from a wallet other than the attested one",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn proof() {}
#[utoipa::path(get, path="/v1/deposit-requests/{id}/verification", operation_id="getDepositRequestVerification", tag="deposit-requests", params(("id"=String, Path)), responses((status=200,description="Every verification attempt on the deposit request with each fact reported separately",body=VerificationDetail),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn deposit_request_verification() {}
#[utoipa::path(post, path="/v1/deposit-requests/{id}/client-secret", operation_id="createDepositRequestClientSecret", tag="deposit-requests", params(("id"=String, Path)), responses((status=201,description="A fresh single-use client secret for a merchant_session deposit; earlier unspent secrets stay valid until they expire",body=ClientSecret),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,description="verification_not_required for permissionless deposits; verification_method_not_applicable for verified_email deposits",body=ErrorResponse),(status=410,description="deposit_request_not_payable: expired, or terminal without a completed verification",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn deposit_request_client_secret() {}
#[utoipa::path(post, path="/v1/customers", operation_id="createCustomer", tag="customers", request_body=CustomerRequest, responses((status=201,body=Customer),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn create_customer() {}
#[utoipa::path(get, path="/v1/customers", operation_id="listCustomers", tag="customers", params(("starting_after"=Option<String>, Query, description="Customer id returned as next_cursor"),("limit"=Option<u32>, Query, minimum=1, maximum=100)), responses((status=200,body=CustomerPage),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn list_customers() {}
#[utoipa::path(get, path="/v1/customers/{id}", operation_id="getCustomer", tag="customers", params(("id"=String, Path)), responses((status=200,body=CustomerDetail),(status=401,body=ErrorResponse),(status=404,description="customer_not_found",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn get_customer() {}
#[utoipa::path(patch, path="/v1/customers/{id}", operation_id="updateCustomer", tag="customers", params(("id"=String, Path)), request_body(content=UpdateCustomerRequest, description="Partial: a field left out keeps its value; email or details sent as null is cleared"), responses((status=200,body=Customer),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn update_customer() {}
#[utoipa::path(post, path="/v1/attachments", operation_id="createAttachment", tag="attachments", request_body=AttachmentRequest, responses((status=201,description="PUT the PDF (at most 5 MiB) to upload_url with exactly the returned headers, If-None-Match: * included; the key is write-once and a repeated PUT gets 412",body=AttachmentUpload),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn create_attachment() {}
#[utoipa::path(post, path="/v1/attachments/{id}/finalize", operation_id="finalizeAttachment", tag="attachments", params(("id"=String, Path)), responses((status=200,description="The object is a clean PDF; idempotent once decided",body=AttachmentDescriptor),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,description="attachment_scan_pending while the scan has not reported; attachment_not_ready before anything was uploaded, or once a finalized upload expired from storage before a deposit request was issued with it ('The upload expired before it was attached; upload the PDF again')",body=ErrorResponse),(status=422,description="attachment_rejected: not application/pdf, outside 1–5,242,880 bytes, no %PDF- magic, or flagged; the object is deleted from storage",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn finalize_attachment() {}
#[utoipa::path(get, path="/v1/account", operation_id="getAccount", tag="account", responses((status=200,body=Account),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn account() {}
#[utoipa::path(get, path="/v1/status", operation_id="getStatus", tag="status", responses((status=200,body=ServiceStatus),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse),(status=503,body=ErrorResponse)), security(("apiKey"=[])))]
fn status() {}
#[utoipa::path(post, path="/v1/webhooks", operation_id="createWebhook", tag="webhooks", request_body=WebhookRequest, responses((status=201,description="Secret is returned only once",body=Webhook),(status=400,description="invalid_request: not a credential-free public HTTPS URL",body=ErrorResponse),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse),(status=503,description="webhooks_unavailable: the deployment has no webhook encryption key",body=ErrorResponse)), security(("apiKey"=[])))]
fn add_webhook() {}
#[utoipa::path(get, path="/v1/webhooks", operation_id="listWebhooks", tag="webhooks", responses((status=200,description="Active endpoints, without secrets",body=WebhookList),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn list_webhooks() {}
#[utoipa::path(get, path="/v1/webhooks/{id}", operation_id="getWebhook", tag="webhooks", params(("id"=String,Path)), responses((status=200,description="The endpoint, disabled or not, without its secret",body=Webhook),(status=401,body=ErrorResponse),(status=404,description="webhook_not_found",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn get_webhook() {}
#[utoipa::path(delete, path="/v1/webhooks/{id}", operation_id="removeWebhook", tag="webhooks", params(("id"=String,Path)), responses((status=204,description="Endpoint disabled; idempotent, and its delivery history stays readable"),(status=401,body=ErrorResponse),(status=404,description="webhook_not_found",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn remove_webhook() {}
#[utoipa::path(post, path="/v1/webhooks/{id}/test", operation_id="testWebhook", tag="webhooks", params(("id"=String,Path)), responses((status=202,description="A webhook.test event queued for this endpoint only",body=TestDelivery),(status=401,body=ErrorResponse),(status=404,description="webhook_not_found, including a disabled endpoint",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn test_webhook() {}
#[utoipa::path(get, path="/v1/webhook-deliveries", operation_id="listWebhookDeliveries", tag="webhooks", params(("endpoint_id"=Option<String>, Query, description="Only this endpoint's deliveries, disabled or not"),("starting_after"=Option<String>, Query, description="Delivery id returned as next_cursor"),("limit"=Option<u32>, Query, minimum=1, maximum=100)), responses((status=200,body=DeliveryPage),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=404,description="webhook_not_found for a foreign endpoint_id",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn deliveries() {}

#[utoipa::path(post, path="/v1/account/api-key", operation_id="issueApiKey", tag="account", request_body(content=ApiKeyGeneration, description="expected_generation is the generation GET /v1/account reports; a signed-in account exists at generation 1 before it holds any key"), responses((status=200,description="Rotated key; the previous key keeps working for 24 hours",body=IssuedApiKey),(status=201,description="First key",body=IssuedApiKey),(status=400,body=ErrorResponse),(status=401,description="identity_unauthorized: an API key cannot mint a key; sign in to the dashboard",body=ErrorResponse),(status=409,description="api_key_generation_conflict",body=ErrorResponse)), security(("dashboardSession"=[])))]
fn issue_key() {}
#[utoipa::path(delete, path="/v1/account/api-key", operation_id="revokeApiKey", tag="account", request_body=ApiKeyGeneration, responses((status=204,description="Current and grace-period keys revoked"),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=409,description="api_key_generation_conflict",body=ErrorResponse)), security(("dashboardSession"=[])))]
fn revoke_key() {}

#[utoipa::path(post, path="/v1/issuers", operation_id="createIssuer", tag="issuers", request_body=IssuerRequest, responses((status=201,description="Created unverified; send a code to prove the contact address",body=Issuer),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=409,description="issuer_name_taken",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn create_issuer() {}
#[utoipa::path(get, path="/v1/issuers", operation_id="listIssuers", tag="issuers", params(("starting_after"=Option<String>, Query, description="Issuer id returned as next_cursor"),("limit"=Option<u32>, Query, minimum=1, maximum=100)), responses((status=200,body=IssuerPage),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn list_issuers() {}
#[utoipa::path(get, path="/v1/issuers/{id}", operation_id="getIssuer", tag="issuers", params(("id"=String, Path)), responses((status=200,body=Issuer),(status=401,body=ErrorResponse),(status=404,description="issuer_not_found",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn get_issuer() {}
#[utoipa::path(patch, path="/v1/issuers/{id}", operation_id="updateIssuer", tag="issuers", params(("id"=String, Path)), request_body(content=UpdateIssuerRequest, description="Partial: a field left out keeps its value; details sent as null is cleared; a different contact_email clears the verification"), responses((status=200,body=Issuer),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,description="issuer_name_taken",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn update_issuer() {}
#[utoipa::path(delete, path="/v1/issuers/{id}", operation_id="deleteIssuer", tag="issuers", params(("id"=String, Path)), responses((status=204,description="Deleted, with its payout-address associations"),(status=409,description="issuer_in_use: requests were issued under it",body=ErrorResponse),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn delete_issuer() {}
#[utoipa::path(post, path="/v1/issuers/{id}/verify/email/start", operation_id="startIssuerEmailVerification", tag="issuers", params(("id"=String, Path)), responses((status=202,description="A code was emailed to the stored contact address",body=StartIssuerEmail),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,description="issuer_email_already_verified",body=ErrorResponse),(status=429,description="otp_resend_cooldown: one code per identity per minute",body=ErrorResponse),(status=503,description="verification_unavailable",body=ErrorResponse)), security(("apiKey"=[])))]
fn start_issuer_email() {}
#[utoipa::path(post, path="/v1/issuers/{id}/verify/email/confirm", operation_id="confirmIssuerEmailVerification", tag="issuers", params(("id"=String, Path)), request_body=ConfirmIssuerEmail, responses((status=200,description="The contact address is proven",body=Issuer),(status=401,description="otp_invalid",body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,description="issuer_email_already_verified",body=ErrorResponse),(status=503,description="verification_unavailable",body=ErrorResponse)), security(("apiKey"=[])))]
fn confirm_issuer_email() {}
#[utoipa::path(put, path="/v1/issuers/{id}/payout-addresses", operation_id="setIssuerPayoutAddresses", tag="issuers", params(("id"=String, Path)), request_body=SetIssuerPayoutAddresses, responses((status=200,description="The identity with its new address set",body=Issuer),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=404,description="issuer_not_found or payout_address_not_found",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn set_issuer_payout_addresses() {}
#[utoipa::path(post, path="/v1/payout-addresses", operation_id="createPayoutAddress", tag="issuers", request_body=PayoutAddressRequest, responses((status=201,body=PayoutAddress),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn create_payout_address() {}
#[utoipa::path(get, path="/v1/payout-addresses", operation_id="listPayoutAddresses", tag="issuers", responses((status=200,body=PayoutAddressList),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn list_payout_addresses() {}
#[utoipa::path(delete, path="/v1/payout-addresses/{id}", operation_id="deletePayoutAddress", tag="issuers", params(("id"=String, Path)), responses((status=204,description="Deleted, with every association to it"),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn delete_payout_address() {}

#[derive(Deserialize, ToSchema)]
struct WithdrawalDestinationRequest {
    /// Decimal chain id of one of the deployment's networks.
    chain_id: String,
    /// The address the whole balance is sent or bridged to.
    address: String,
}
#[derive(Deserialize, ToSchema)]
struct CreateWithdrawal {
    /// `USDC` (the default) or `USDT`: the one currency the withdrawal moves.
    /// Every network holding USDC is swept into the destination; USDT moves
    /// the destination chain's balance alone.
    currency: Option<String>,
    destination: WithdrawalDestinationRequest,
}
#[derive(Deserialize, ToSchema)]
struct LegAuthorization {
    leg_id: String,
    /// `0x` hex, 65 bytes `r || s || v`, over the leg's `typed_data`.
    signature: String,
}
#[derive(Deserialize, ToSchema)]
struct WithdrawalAuthorizations {
    /// Any subset of the withdrawal's legs; every signature is verified
    /// before any is recorded.
    authorizations: Vec<LegAuthorization>,
}
#[derive(Serialize, ToSchema)]
struct WithdrawalDestination {
    chain: Chain,
    address: String,
}
#[derive(Serialize, ToSchema)]
struct WithdrawalNoncePreimage {
    /// Circle's CCTP domain of the destination chain.
    destination_domain: u32,
    /// The destination address, as the mint recipient.
    mint_recipient: String,
    /// `0x` hex, 32 bytes.
    salt: String,
}
/// What the merchant signs for one leg: an EIP-3009 authorization under the
/// source chain's contract for the withdrawal's currency, ready for
/// `eth_signTypedData_v4`.
#[derive(Serialize, ToSchema)]
struct WithdrawalAuthorization {
    /// `TransferWithAuthorization` (same-chain leg) or
    /// `ReceiveWithAuthorization` (bridge leg).
    primary_type: String,
    /// `{domain, primaryType, types, message}`; every `uint256` is a decimal
    /// string, `nonce` is `0x` hex.
    typed_data: serde_json::Value,
    /// When the token stops accepting the signature (24 hours after creation).
    expires_at: String,
    /// Bridge legs: the WithdrawalForwarder the authorization pays.
    forwarder: Option<String>,
    /// Bridge legs: `nonce = keccak256(abi.encode(destination_domain, bytes32(mint_recipient), salt))`,
    /// so a signer can check the destination the nonce commits to.
    nonce_preimage: Option<WithdrawalNoncePreimage>,
}
#[derive(Serialize, ToSchema)]
struct WithdrawalLeg {
    id: String,
    /// `transfer` when the funds already sit on the destination chain,
    /// `bridge` when they cross through CCTP.
    kind: String,
    source_chain: Chain,
    /// The contract the leg is signed under: the withdrawal's currency on
    /// the source chain.
    token: Token,
    amount: String,
    amount_base_units: String,
    /// `awaiting_signature`, `authorized`, `relaying`, `burned`, `attested`,
    /// `minting`, `completed`, `failed`, `expired`, or `cancelled`.
    state: String,
    /// Present while the leg awaits its signature.
    authorization: Option<WithdrawalAuthorization>,
    transfer_tx_hash: Option<String>,
    burn_tx_hash: Option<String>,
    mint_tx_hash: Option<String>,
    failure_reason: Option<String>,
}
#[derive(Serialize, ToSchema)]
struct Withdrawal {
    id: String,
    /// `awaiting_signature`, `in_progress`, `completed`, `failed`, or `cancelled`.
    status: String,
    /// The Gum wallet every leg is signed from.
    wallet_address: String,
    /// `USDC` or `USDT`: the one currency every leg moves.
    currency: String,
    destination: WithdrawalDestination,
    /// One per network the wallet held the currency on when the withdrawal was created.
    legs: Vec<WithdrawalLeg>,
    created_at: String,
    completed_at: Option<String>,
    cancelled_at: Option<String>,
    failed_at: Option<String>,
}
#[derive(Serialize, ToSchema)]
struct WithdrawalPage {
    withdrawals: Vec<Withdrawal>,
    next_cursor: Option<String>,
}

#[utoipa::path(post, path="/v1/withdrawals", operation_id="createWithdrawal", tag="withdrawals",
 request_body(content=CreateWithdrawal, description="Snapshot the Gum wallet's balance in one currency (USDC unless named) into legs towards one destination: every network for USDC, which bridges through CCTP at 1:1; the destination network alone for USDT, which does not bridge. Each leg carries the typed data to sign; nothing moves until it is signed. A reused Idempotency-Key with the same destination replays the withdrawal; with another destination it is a 409 idempotency_conflict."),
 params(("Idempotency-Key"=String, Header, description="Required, 1-255 bytes")),
 responses((status=201, description="Created; every leg is awaiting_signature", body=Withdrawal), (status=200, description="Idempotent replay", body=Withdrawal, headers(("Idempotency-Replayed"=String, description="true"))), (status=400, body=ErrorResponse), (status=401, body=ErrorResponse), (status=409, description="wallet_not_ready, withdrawal_in_progress, nothing_to_withdraw, or idempotency_conflict", body=ErrorResponse), (status=422, description="withdrawal_exceeds_bridge_limit: a bridge leg is above Circle's 10,000,000 USDC per-message burn limit", body=ErrorResponse), (status=429, body=ErrorResponse), (status=503, description="withdrawals_unavailable: a balance could not be read, or a chain the wallet holds funds on cannot bridge on this deployment", body=ErrorResponse)), security(("apiKey"=[])))]
fn create_withdrawal() {}
#[utoipa::path(get, path="/v1/withdrawals", operation_id="listWithdrawals", tag="withdrawals",
 params(("starting_after"=Option<String>, Query, description="wd_ cursor returned as next_cursor"), ("limit"=Option<u32>, Query, minimum=1, maximum=100)),
 responses((status=200, body=WithdrawalPage), (status=400, body=ErrorResponse), (status=401, body=ErrorResponse), (status=429, body=ErrorResponse)), security(("apiKey"=[])))]
fn list_withdrawals() {}
#[utoipa::path(get, path="/v1/withdrawals/{id}", operation_id="getWithdrawal", tag="withdrawals", params(("id"=String, Path)),
 responses((status=200, body=Withdrawal), (status=401, body=ErrorResponse), (status=404, description="withdrawal_not_found", body=ErrorResponse), (status=429, body=ErrorResponse)), security(("apiKey"=[])))]
fn get_withdrawal() {}
#[utoipa::path(post, path="/v1/withdrawals/{id}/authorizations", operation_id="authorizeWithdrawal", tag="withdrawals", params(("id"=String, Path)),
 request_body(content=WithdrawalAuthorizations, description="The merchant's signatures, each recovered against the withdrawal's wallet over the leg's typed data. Partial sets are accepted; a leg already signed with the same signature is unchanged."),
 responses((status=200, description="The withdrawal with the signed legs authorized; the relayer takes them from here", body=Withdrawal), (status=400, description="signature_invalid, naming the leg", body=ErrorResponse), (status=401, body=ErrorResponse), (status=404, description="withdrawal_not_found or withdrawal_leg_not_found", body=ErrorResponse), (status=409, description="leg_not_awaiting_signature, authorization_expired, or withdrawal_finished", body=ErrorResponse), (status=429, body=ErrorResponse)), security(("apiKey"=[])))]
fn authorize_withdrawal() {}
#[utoipa::path(post, path="/v1/withdrawals/{id}/cancel", operation_id="cancelWithdrawal", tag="withdrawals", params(("id"=String, Path)),
 responses((status=200, description="Cancelled; signatures already given are never used", body=Withdrawal), (status=401, body=ErrorResponse), (status=404, body=ErrorResponse), (status=409, description="withdrawal_not_cancellable once a leg has been relayed, or withdrawal_finished", body=ErrorResponse), (status=429, body=ErrorResponse)), security(("apiKey"=[])))]
fn cancel_withdrawal() {}

#[derive(OpenApi)]
#[openapi(paths(create_deposit_request,list_deposit_requests,get_deposit_request,cancel_deposit_request,transfers,deposit_request_attachment,request_pdf,proof,deposit_request_verification,deposit_request_client_secret,create_customer,list_customers,get_customer,update_customer,create_issuer,list_issuers,get_issuer,update_issuer,delete_issuer,start_issuer_email,confirm_issuer_email,set_issuer_payout_addresses,create_payout_address,list_payout_addresses,delete_payout_address,create_attachment,finalize_attachment,account,status,add_webhook,list_webhooks,get_webhook,remove_webhook,test_webhook,deliveries,issue_key,revoke_key,create_withdrawal,list_withdrawals,get_withdrawal,authorize_withdrawal,cancel_withdrawal),
 components(schemas(ErrorDetail,ErrorResponse,Chain,Token,AsOf,SelfSettlement,Attention,IndexerFreshness,Party,PayerVerification,EmailVerification,MerchantAuth,ClientSecret,AttachmentDescriptor,Attribution,CreateDepositRequest,DepositRequest,DepositRequestStatus,DepositRequestSummary,DepositRequestPage,Transfer,TransferList,VerificationFactStatus,VerificationRequirements,VerificationAttempt,VerificationDetail,CustomerRequest,UpdateCustomerRequest,Customer,CustomerPage,CustomerStats,CustomerDetail,IssuerRequest,UpdateIssuerRequest,ConfirmIssuerEmail,SetIssuerPayoutAddresses,PayoutAddressRequest,PayoutAddress,PayoutAddressList,Issuer,IssuerPage,StartIssuerEmail,AttachmentRequest,AttachmentUpload,AttachmentCommitment,CanonicalIssuanceSnapshot,ProofTransfer,VerificationAttestationPayload,SignedVerificationAttestation,ProofOfPayment,ApiKeyGeneration,IssuedApiKey,Account,StatusChain,StatusIndexer,StatusSweeper,StatusWithdrawals,StatusSigner,ServiceStatus,WebhookRequest,Webhook,WebhookList,TestDelivery,Delivery,DeliveryAttempt,DeliveryPage,CreateWithdrawal,WithdrawalDestinationRequest,WithdrawalAuthorizations,LegAuthorization,Withdrawal,WithdrawalDestination,WithdrawalLeg,WithdrawalAuthorization,WithdrawalNoncePreimage,WithdrawalPage)),
 modifiers(&Security), tags((name="withdrawals",description="Moving the Gum wallet's stablecoins to an address the merchant names: USDC across every network, USDT on the network it sits on"),(name="deposit-requests",description="Deposit request issuance, documents, and deposit tracking"),(name="customers",description="Merchant-owned counterparty records"),(name="issuers",description="Issuer identities and the payout addresses they settle to"),(name="attachments",description="PDF upload and finalization"),(name="webhooks",description="Webhook endpoint and delivery management")))]
struct ApiDoc;

struct Security;
impl utoipa::Modify for Security {
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{Http, HttpAuthScheme, SecurityScheme};
        let c = api.components.as_mut().unwrap();
        // Merchant routes take either an API key or a dashboard session (the
        // Privy identity token) under the same bearer header; the key
        // management routes take only the session.
        c.add_security_scheme(
            "apiKey",
            SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
        );
        c.add_security_scheme(
            "dashboardSession",
            SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
        );
        api.info.title = "Gum API".into();
        api.info.version = "1.0.0".into();
        api.info.description = Some(
            "Every id is a UUID behind a prefix naming its resource: dr_ deposit request, \
             cus_ customer, iss_ issuer identity, pa_ payout address, att_ attachment, wh_ \
             webhook endpoint, whd_ webhook delivery, evt_ webhook event, va_ verification \
             attempt, acct_ account. Only that canonical form is accepted back. Timestamps are \
             RFC 3339 in UTC to the second with a Z suffix; exact amounts are decimal strings."
                .into(),
        );
    }
}

pub async fn spec() -> axum::Json<utoipa::openapi::OpenApi> {
    axum::Json(document())
}
pub fn document() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}
pub async fn reference() -> Html<&'static str> {
    Html(
        r#"<!doctype html><html><head><title>Gum API</title><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><script id="api-reference" data-url="/api/openapi.json"></script><script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference"></script></body></html>"#,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    const ROUTES: &[(&str, &str)] = &[
        ("/v1/deposit-requests", "get"),
        ("/v1/deposit-requests", "post"),
        ("/v1/deposit-requests/{id}", "get"),
        ("/v1/deposit-requests/{id}/cancel", "post"),
        ("/v1/deposit-requests/{id}/transfers", "get"),
        ("/v1/deposit-requests/{id}/attachment", "get"),
        ("/v1/deposit-requests/{id}/request.pdf", "get"),
        ("/v1/deposit-requests/{id}/proof", "get"),
        ("/v1/deposit-requests/{id}/verification", "get"),
        ("/v1/deposit-requests/{id}/client-secret", "post"),
        ("/v1/customers", "get"),
        ("/v1/customers", "post"),
        ("/v1/customers/{id}", "get"),
        ("/v1/customers/{id}", "patch"),
        ("/v1/attachments", "post"),
        ("/v1/attachments/{id}/finalize", "post"),
        ("/v1/account", "get"),
        ("/v1/status", "get"),
        ("/v1/webhooks", "get"),
        ("/v1/webhooks", "post"),
        ("/v1/webhooks/{id}", "get"),
        ("/v1/webhooks/{id}", "delete"),
        ("/v1/webhooks/{id}/test", "post"),
        ("/v1/webhook-deliveries", "get"),
        ("/v1/account/api-key", "post"),
        ("/v1/account/api-key", "delete"),
        ("/v1/issuers", "post"),
        ("/v1/issuers", "get"),
        ("/v1/issuers/{id}", "get"),
        ("/v1/issuers/{id}", "patch"),
        ("/v1/issuers/{id}", "delete"),
        ("/v1/issuers/{id}/verify/email/start", "post"),
        ("/v1/issuers/{id}/verify/email/confirm", "post"),
        ("/v1/issuers/{id}/payout-addresses", "put"),
        ("/v1/payout-addresses", "post"),
        ("/v1/payout-addresses", "get"),
        ("/v1/payout-addresses/{id}", "delete"),
        ("/v1/withdrawals", "post"),
        ("/v1/withdrawals", "get"),
        ("/v1/withdrawals/{id}", "get"),
        ("/v1/withdrawals/{id}/authorizations", "post"),
        ("/v1/withdrawals/{id}/cancel", "post"),
    ];
    #[test]
    fn contract_covers_axum_public_api_routes_and_is_typed() {
        let d = serde_json::to_value(document()).unwrap();
        assert_eq!(d["openapi"], "3.1.0");
        for (p, m) in ROUTES {
            let op = &d["paths"][p][m];
            assert!(op["operationId"].is_string(), "{m} {p}");
            assert!(op["responses"].as_object().is_some_and(|r| !r.is_empty()));
            assert!(op["security"].is_array());
        }
        assert_eq!(
            d["paths"]
                .as_object()
                .unwrap()
                .values()
                .map(|p| p
                    .as_object()
                    .unwrap()
                    .keys()
                    .filter(|k| matches!(k.as_str(), "get" | "post" | "put" | "delete" | "patch"))
                    .count())
                .sum::<usize>(),
            ROUTES.len()
        );
        let deposit = &d["components"]["schemas"]["DepositRequest"]["properties"];
        assert!(deposit["as_of"].is_object());
        assert!(deposit["recovery_address"].is_object());
        assert!(deposit["refund_address"].is_null());
        assert!(deposit["memo"].is_null());
        for documented in [
            "issuer",
            "payer",
            "verification",
            "attachment",
            "attribution",
        ] {
            assert!(deposit[documented].is_object(), "{documented}");
        }
        let create = &d["components"]["schemas"]["CreateDepositRequest"];
        assert!(create["properties"]["refund_address"].is_null());
        assert!(create["properties"]["memo"].is_null());
        let required: Vec<&str> = create["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        // `amount` is the only required field: `verification` defaults to
        // none attached, the permissionless model.
        assert!(required.contains(&"amount"), "amount must be required");
        assert!(
            !required.contains(&"verification"),
            "verification is optional"
        );
        // The parties and the payout address may come from saved records.
        for field in ["payout_address", "issuer", "payer"] {
            assert!(!required.contains(&field), "{field} must be optional");
            assert!(create["properties"][field].is_object(), "{field}");
        }
        let summary = &d["components"]["schemas"]["DepositRequestSummary"]["properties"];
        for field in ["deposit_url", "updated_at", "expires_at"] {
            assert!(summary[field].is_object(), "{field}");
        }
        // Cancel answers with the deposit request itself; lists are enveloped.
        assert_eq!(
            d["paths"]["/v1/deposit-requests/{id}/cancel"]["post"]["responses"]["200"]["content"]["application/json"]
                ["schema"]["$ref"],
            "#/components/schemas/DepositRequest"
        );
        for (path, schema) in [
            ("/v1/deposit-requests/{id}/transfers", "TransferList"),
            ("/v1/webhooks", "WebhookList"),
            ("/v1/webhook-deliveries", "DeliveryPage"),
        ] {
            assert_eq!(
                d["paths"][path]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
                    ["$ref"],
                format!("#/components/schemas/{schema}"),
                "{path}"
            );
        }
        // Ids are prefixed strings, never bare UUIDs, everywhere they appear.
        for (schema, field) in [
            ("Customer", "id"),
            ("Issuer", "id"),
            ("PayoutAddress", "id"),
            ("AttachmentDescriptor", "id"),
            ("AttachmentUpload", "id"),
            ("Webhook", "id"),
            ("Delivery", "event_id"),
            ("CreateDepositRequest", "customer_id"),
            ("CustomerPage", "next_cursor"),
        ] {
            let property = &d["components"]["schemas"][schema]["properties"][field];
            let string_typed = property["type"] == "string"
                || property["type"] == serde_json::json!(["string", "null"]);
            assert!(string_typed, "{schema}.{field}: {property}");
            assert!(
                property.get("format").is_none(),
                "{schema}.{field}: {property}"
            );
        }
        assert!(
            d["info"]["description"]
                .as_str()
                .unwrap()
                .contains("cus_ customer")
        );
        assert!(d["paths"]["/v1/webhooks/{id}"]["delete"]["responses"]["204"].is_object());
        assert!(d["paths"]["/v1/webhooks/{id}"]["delete"]["responses"]["404"].is_object());
        assert!(d["paths"]["/v1/account/api-key"]["post"]["requestBody"].is_object());
        assert!(
            d["components"]["schemas"]["UpdateCustomerRequest"]["required"]
                .as_array()
                .is_none_or(|fields| fields.is_empty()),
            "every update field is optional"
        );
        assert!(
            d["components"]["schemas"]["PayerVerification"]["properties"]["wallet_attestation"]
                .is_object()
        );
        assert!(deposit["client_secret"].is_object());
        assert!(
            d["components"]["schemas"]["PayerVerification"]["properties"]["merchant_auth"]
                .is_object()
        );
        assert!(
            d["components"]["schemas"]["VerificationRequirements"]["properties"]
                ["merchant_session"]
                .is_object()
        );
        let proof = &d["components"]["schemas"]["ProofOfPayment"]["properties"];
        for documented in [
            "canonical_issuance_snapshot",
            "payer_wallet",
            "salt",
            "payment_address",
            "recovery_address",
            "transfers",
            "verification",
        ] {
            assert!(proof[documented].is_object(), "{documented}");
        }
        assert!(proof["attribution_nonce"].is_null());
        let deposit = &d["components"]["schemas"]["DepositRequest"]["properties"];
        assert!(deposit["payer_wallet"].is_object());
        assert!(deposit["wallet_bound_at"].is_object());
        let requirements = &d["components"]["schemas"]["VerificationRequirements"]["properties"];
        assert!(requirements["wallet"].is_object());
        let attempt = &d["components"]["schemas"]["VerificationAttempt"]["properties"];
        for documented in ["kind", "status", "verified_at", "created_at"] {
            assert!(attempt[documented].is_object(), "{documented}");
        }
        for withheld in [
            "provider_reference",
            "risk_codes",
            "review",
            "first_name",
            "last_name",
            "document_number",
            "date_of_birth",
        ] {
            assert!(attempt[withheld].is_null(), "{withheld}");
        }
        let upload = &d["components"]["schemas"]["AttachmentUpload"]["properties"];
        assert!(upload["upload_url"].is_object());
        assert!(upload["headers"].is_object());
        let status = &d["components"]["schemas"]["ServiceStatus"]["properties"];
        assert_eq!(
            status.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["chains"]
        );
        let chain = &d["components"]["schemas"]["StatusChain"]["properties"];
        assert!(chain["indexer"].is_object());
        assert!(chain["sweeper"].is_object());
        let create = &d["components"]["schemas"]["CreateDepositRequest"]["properties"];
        assert!(
            create["chain_id"].is_object(),
            "the merchant may pin the network"
        );
        assert!(
            !d["components"]["schemas"]["CreateDepositRequest"]["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field == "chain_id"),
            "pinning is optional"
        );
        assert!(create.get("token_address").is_none());
        let deposit_request = &d["components"]["schemas"]["DepositRequest"]["properties"];
        assert!(deposit_request["networks"].is_object());
        assert_eq!(
            d["paths"]["/v1/deposit-requests"]["post"]["responses"]["200"]["headers"]["Idempotency-Replayed"]
                ["schema"]["type"],
            "string"
        );
    }
}
