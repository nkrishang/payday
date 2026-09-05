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
}
#[derive(Serialize, ToSchema)]
struct Token {
    symbol: String,
    address: String,
    decimals: u8,
}
#[derive(Serialize, ToSchema)]
struct AsOf {
    block: String,
    at: String,
}
#[derive(Serialize, ToSchema)]
struct SelfSettlement {
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
/// One side of the invoice: bounded free text rendered verbatim.
#[derive(Serialize, Deserialize, ToSchema)]
struct Party {
    /// 1–255 bytes.
    name: String,
    /// 3–254 bytes.
    email: Option<String>,
    /// At most 4000 bytes.
    details: Option<String>,
}
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum PayerPolicyMode {
    Permissionless,
    VerifiedEmail,
}
/// `expected_email` is required for `verified_email` and forbidden for
/// `permissionless`.
#[derive(Serialize, Deserialize, ToSchema)]
#[schema(example = json!({"mode":"verified_email","expected_email":"alice@example.com"}))]
struct PayerPolicy {
    mode: PayerPolicyMode,
    expected_email: Option<String>,
}
#[derive(Serialize, ToSchema)]
struct AttachmentDescriptor {
    id: uuid::Uuid,
    filename: String,
    /// Always `application/pdf`.
    mime_type: String,
    byte_length: String,
    /// `0x`-prefixed SHA-256 of the PDF bytes.
    sha256: String,
    /// Short-lived signed link, present on attachment routes only.
    download_url: Option<String>,
}
/// The commitment the payment address was derived from (product plan §5).
#[derive(Serialize, ToSchema)]
struct Attribution {
    version: u16,
    hash: String,
}
#[derive(Deserialize, ToSchema)]
#[schema(example = json!({"amount":"10.50","payout_address":"0x1111111111111111111111111111111111111111","issuer":{"name":"Acme Corp"},"bill_to":{"name":"Globex"},"payer_policy":{"mode":"permissionless"},"expires_in":3600,"reference":"INV-42","metadata":{"customer":"cus_123"}}))]
struct CreatePayment {
    amount: String,
    payout_address: String,
    issuer: Party,
    bill_to: Party,
    payer_policy: PayerPolicy,
    chain_id: Option<String>,
    token_address: Option<String>,
    customer_id: Option<uuid::Uuid>,
    /// The saved issuer identity this is issued under. `issuer` above is still
    /// the snapshot the document carries; this records which identity it came
    /// from and survives that identity being renamed.
    issuer_id: Option<uuid::Uuid>,
    /// At most 4000 bytes.
    notes: Option<String>,
    /// At most 200 bytes; shown to the payer before verification.
    heading: Option<String>,
    reference: Option<String>,
    #[schema(value_type = Object)]
    metadata: Option<serde_json::Value>,
    /// A finalized PDF upload; at most one per invoice.
    attachment_id: Option<uuid::Uuid>,
    expires_in: Option<u64>,
    expires_at: Option<String>,
}
#[derive(Serialize, ToSchema)]
struct Payment {
    id: String,
    payment_url: String,
    status: PaymentStatus,
    chain: Chain,
    currency: String,
    token: Token,
    /// The one-time payment address; null until the payer attests the
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
    bill_to: Party,
    notes: Option<String>,
    heading: Option<String>,
    reference: Option<String>,
    customer_id: Option<String>,
    issuer_id: Option<String>,
    /// The full policy including merchant assertions; never shown to payers.
    payer_policy: PayerPolicy,
    attachment: Option<AttachmentDescriptor>,
    verification_completed_at: Option<String>,
    likely_unsolicited_at: Option<String>,
    attribution: Attribution,
    #[schema(value_type = Object)]
    metadata: serde_json::Value,
    created_at: String,
    updated_at: String,
    expires_at: String,
    paid_at: Option<String>,
    paid_at_block: Option<String>,
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
enum PaymentStatus {
    AwaitingPayment,
    PartiallyPaid,
    Paid,
    Settled,
    Expired,
    Returned,
    NeedsAttention,
}
#[derive(Serialize, ToSchema)]
struct PaymentPage {
    payments: Vec<PaymentSummary>,
    next_cursor: Option<String>,
}
#[derive(Serialize, ToSchema)]
struct PaymentSummary {
    id: String,
    heading: Option<String>,
    bill_to_name: String,
    reference: Option<String>,
    #[schema(value_type = Object)]
    metadata: serde_json::Value,
    created_at: String,
    status: PaymentStatus,
    amount: String,
    received: String,
    payer_policy_mode: PayerPolicyMode,
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
/// Each fact the policy needs, on its own. `wallet` is never `not_required`:
/// every request needs the payer's wallet attestation before it has an
/// address. `complete` is the identity policy alone.
#[derive(Serialize, ToSchema)]
struct VerificationRequirements {
    email: VerificationFactStatus,
    wallet: VerificationFactStatus,
    complete: bool,
}
/// One attempt: what was attempted and where it stands.
#[derive(Serialize, ToSchema)]
struct VerificationAttempt {
    id: String,
    /// email or wallet.
    kind: String,
    /// pending, approved, or abandoned.
    status: String,
    verified_at: Option<String>,
    created_at: String,
}
#[derive(Serialize, ToSchema)]
struct VerificationDetail {
    payer_policy_mode: PayerPolicyMode,
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
}
#[derive(Serialize, ToSchema)]
struct CancelPayment {
    payment: Payment,
    advisory: String,
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
}
#[derive(Serialize, ToSchema)]
struct StatusIndexer {
    cursor_block: Option<String>,
    cursor_at: Option<String>,
    lag_blocks: Option<u64>,
}
#[derive(Serialize, ToSchema)]
struct StatusSweeper {
    state: String,
    queued: i64,
}
#[derive(Serialize, ToSchema)]
struct ServiceStatus {
    chain: StatusChain,
    indexer: StatusIndexer,
    sweeper: StatusSweeper,
}
#[derive(Deserialize, ToSchema)]
struct WebhookRequest {
    url: String,
}
#[derive(Serialize, ToSchema)]
struct Webhook {
    id: uuid::Uuid,
    url: String,
    created_at: String,
    secret: Option<String>,
}
#[derive(Serialize, ToSchema)]
struct TestDelivery {
    delivery_id: uuid::Uuid,
}
#[derive(Serialize, ToSchema)]
struct Delivery {
    id: uuid::Uuid,
    event_id: uuid::Uuid,
    endpoint_id: uuid::Uuid,
    state: String,
    attempt_count: i32,
    next_attempt_at: String,
    delivered_at: Option<String>,
    attempts: Vec<DeliveryAttempt>,
}
#[derive(Serialize, ToSchema)]
struct DeliveryAttempt {
    number: i32,
    attempted_at: String,
    duration_ms: i64,
    status: Option<i32>,
    error: Option<String>,
}
/// Create and update share this body; update replaces every editable field.
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
#[derive(Serialize, ToSchema)]
struct Customer {
    id: uuid::Uuid,
    name: String,
    email: Option<String>,
    details: Option<String>,
    created_at: String,
    updated_at: String,
}
#[derive(Serialize, ToSchema)]
struct CustomerPage {
    customers: Vec<Customer>,
    next_cursor: Option<uuid::Uuid>,
}
#[derive(Serialize, ToSchema)]
struct CustomerStats {
    /// Deposit requests billed to this customer, of any status.
    request_count: i64,
    /// Confirmed on chain across all of this customer's invoices. Base units,
    /// like an invoice's own `amount_base_units` — scale for display.
    collected_base_units: String,
    /// Outstanding on the ones still open — awaiting payment or partially paid. Base units.
    pending_base_units: String,
}
/// `getCustomer` only: a list of many customers would mean one aggregate
/// query per row.
#[derive(Serialize, ToSchema)]
struct CustomerDetail {
    id: uuid::Uuid,
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
#[derive(Deserialize, ToSchema)]
struct ConfirmIssuerEmail {
    /// The emailed one-time code.
    otp: String,
}
#[derive(Deserialize, ToSchema)]
struct SetIssuerPayoutAddresses {
    /// Replaces the whole set; at most 25, all of them yours.
    payout_address_ids: Vec<uuid::Uuid>,
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
    id: uuid::Uuid,
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
    id: uuid::Uuid,
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
    next_cursor: Option<uuid::Uuid>,
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
    id: uuid::Uuid,
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
    id: uuid::Uuid,
    byte_length: String,
    sha256: String,
}
/// The document the payment address was derived from (product plan §5.2).
#[derive(Serialize, ToSchema)]
struct CanonicalIssuanceSnapshot {
    schema: String,
    canonicalization: String,
    issuer: Party,
    bill_to: Party,
    amount_base_units: String,
    notes: Option<String>,
    heading: Option<String>,
    reference: Option<String>,
    expiration_timestamp: String,
    payer_policy: PayerPolicy,
    attachment: Option<AttachmentCommitment>,
    chain_id: String,
    token_address: String,
    receiver_address: String,
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
/// Payday domain), its EIP-712 digest, and the signature. The proof's salt
/// is `keccak256("PAYDAY_SALT_V2" || attribution_hash || digest)` and the
/// wallet is the address's recovery term.
#[derive(Serialize, ToSchema)]
struct PayerWalletAttestation {
    address: String,
    typed_data: PayerAttestationTypedData,
    digest: String,
    signature: String,
    /// `ecdsa`.
    method: String,
}
/// One fact Payday observed: `mailbox` (provider `auth0`) or `wallet`
/// (provider `payday`), and when.
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
    sender: String,
    recipient: String,
    amount_base_units: String,
    block_number: String,
}
#[derive(Serialize, ToSchema)]
struct VerificationAttestationPayload {
    version: String,
    payment_id: String,
    /// The issuance commitment the attestation is bound to, so a genuine
    /// attestation cannot be transplanted onto another invoice's package.
    attribution_hash: String,
    chain_id: String,
    payment_address: String,
    payer_wallet: String,
    /// The nonce inside the payer's signed attestation; Payday's word is
    /// that it was issued only after the policy passed.
    wallet_nonce: String,
    payer_policy_mode: PayerPolicyMode,
    /// `not_required`, `approved`, or `pending`.
    result: String,
    verified_at: Option<String>,
    wallet_bound_at: String,
    facts: Vec<VerificationFact>,
}
/// Payday-attested, not address-committed: verification happens after
/// issuance. `signature` recovers to `signer` over
/// `keccak256("PAYDAY_VERIFICATION_ATTESTATION_V2" || JCS(payload))`.
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

#[utoipa::path(post, path="/v1/payments", operation_id="createPayment", tag="payments",
 request_body(content=CreatePayment, description="Issue an invoice. expires_in and expires_at are mutually exclusive; the default lifetime is 24 hours. Every field except the deadline is immutable once issued: a reused Idempotency-Key with any difference is a 409 idempotency_conflict."),
 params(("Idempotency-Key"=String, Header, description="Required, 1-255 bytes")),
 responses((status=201, description="Created", body=Payment), (status=200, description="Idempotent replay", body=Payment, headers(("Idempotency-Replayed"=String, description="true"))), (status=400, body=ErrorResponse), (status=401, body=ErrorResponse), (status=409, description="idempotency_conflict, attachment_not_ready (also when a finalized upload expired from storage before it was attached: 'The upload expired before it was attached; upload the PDF again'), or attachment_already_attached", body=ErrorResponse), (status=422, body=ErrorResponse), (status=429, body=ErrorResponse)), security(("apiKey"=[])))]
fn create_payment() {}
#[utoipa::path(get, path="/v1/payments", operation_id="listPayments", tag="payments",
 params(("starting_after"=Option<String>, Query, description="pay_ cursor returned as next_cursor"), ("status"=Option<PaymentStatus>, Query), ("reference"=Option<String>, Query), ("customer_id"=Option<uuid::Uuid>, Query, description="Only requests billed to this customer"), ("issuer_id"=Option<uuid::Uuid>, Query, description="Only requests issued under this identity"), ("verification"=Option<String>, Query, description="not_required, pending, verified, or likely_unsolicited"), ("limit"=Option<u32>, Query, minimum=1, maximum=100)),
 responses((status=200, body=PaymentPage), (status=400, body=ErrorResponse), (status=401, body=ErrorResponse), (status=429, body=ErrorResponse)), security(("apiKey"=[])))]
fn list_payments() {}
#[utoipa::path(get, path="/v1/payments/{id}", operation_id="getPayment", tag="payments", params(("id"=String, Path, description="Complete payment ID or payment address"),("wait_for"=Option<String>,Query,description="Set to change for long polling"),("timeout"=Option<u64>,Query,minimum=1,maximum=30)), responses((status=200, body=Payment),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn get_payment() {}
#[utoipa::path(post, path="/v1/payments/{id}/cancel", operation_id="cancelPayment", tag="payments", params(("id"=String, Path)), responses((status=200,description="Presentation-only cancellation; on-chain rules are unchanged",body=CancelPayment),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn cancel_payment() {}
#[utoipa::path(get, path="/v1/payments/{id}/transfers", operation_id="listPaymentTransfers", tag="payments", params(("id"=String, Path)), responses((status=200,body=[Transfer]),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn transfers() {}
#[utoipa::path(get, path="/v1/payments/{id}/attachment", operation_id="getPaymentAttachment", tag="payments", params(("id"=String, Path)), responses((status=200,description="Descriptor with a signed download_url valid for PAYDAY_ATTACHMENT_DOWNLOAD_TTL_SECS",body=AttachmentDescriptor),(status=401,body=ErrorResponse),(status=404,description="payment_not_found or attachment_not_found",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn payment_attachment() {}
#[utoipa::path(get, path="/v1/payments/{id}/invoice.pdf", operation_id="getInvoicePdf", tag="payments", params(("id"=String, Path)), responses((status=200,description="Deterministic invoice summary as application/pdf, served as an attachment"),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn invoice_pdf() {}
#[utoipa::path(get, path="/v1/payments/{id}/proof", operation_id="getProofOfPayment", tag="payments", params(("id"=String, Path)), responses((status=200,description="Verifiable offline; gateway_core::verify_proof holds the checks",body=ProofOfPayment),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,description="payment_not_settled, or payment_sender_mismatch when credited funds came from a wallet other than the attested one",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn proof() {}
#[utoipa::path(get, path="/v1/payments/{id}/verification", operation_id="getPaymentVerification", tag="payments", params(("id"=String, Path)), responses((status=200,description="Every verification attempt on the invoice with each fact reported separately",body=VerificationDetail),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn payment_verification() {}
#[utoipa::path(post, path="/v1/customers", operation_id="createCustomer", tag="customers", request_body=CustomerRequest, responses((status=201,body=Customer),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn create_customer() {}
#[utoipa::path(get, path="/v1/customers", operation_id="listCustomers", tag="customers", params(("starting_after"=Option<uuid::Uuid>, Query, description="Customer id returned as next_cursor"),("limit"=Option<u32>, Query, minimum=1, maximum=100)), responses((status=200,body=CustomerPage),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn list_customers() {}
#[utoipa::path(get, path="/v1/customers/{id}", operation_id="getCustomer", tag="customers", params(("id"=uuid::Uuid, Path)), responses((status=200,body=CustomerDetail),(status=401,body=ErrorResponse),(status=404,description="customer_not_found",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn get_customer() {}
#[utoipa::path(patch, path="/v1/customers/{id}", operation_id="updateCustomer", tag="customers", params(("id"=uuid::Uuid, Path)), request_body(content=CustomerRequest, description="Replaces every editable field; an omitted or null email or details clears it"), responses((status=200,body=Customer),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn update_customer() {}
#[utoipa::path(post, path="/v1/attachments", operation_id="createAttachment", tag="attachments", request_body=AttachmentRequest, responses((status=201,description="PUT the PDF (at most 5 MiB) to upload_url with exactly the returned headers, If-None-Match: * included; the key is write-once and a repeated PUT gets 412",body=AttachmentUpload),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn create_attachment() {}
#[utoipa::path(post, path="/v1/attachments/{id}/finalize", operation_id="finalizeAttachment", tag="attachments", params(("id"=uuid::Uuid, Path)), responses((status=200,description="The object is a clean PDF; idempotent once decided",body=AttachmentDescriptor),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,description="attachment_scan_pending while the scan has not reported; attachment_not_ready before anything was uploaded, or once a finalized upload expired from storage before an invoice was issued with it ('The upload expired before it was attached; upload the PDF again')",body=ErrorResponse),(status=422,description="attachment_rejected: not application/pdf, outside 1–5,242,880 bytes, no %PDF- magic, or flagged; the object is deleted from storage",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn finalize_attachment() {}
#[utoipa::path(get, path="/v1/account", operation_id="getAccount", tag="account", responses((status=200,body=Account),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn account() {}
#[utoipa::path(get, path="/v1/status", operation_id="getStatus", tag="status", responses((status=200,body=ServiceStatus),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse),(status=503,body=ErrorResponse)), security(("apiKey"=[])))]
fn status() {}
#[utoipa::path(post, path="/v1/webhooks", operation_id="createWebhook", tag="webhooks", request_body=WebhookRequest, responses((status=201,description="Secret is returned only once",body=Webhook),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn add_webhook() {}
#[utoipa::path(get, path="/v1/webhooks", operation_id="listWebhooks", tag="webhooks", responses((status=200,body=[Webhook]),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn list_webhooks() {}
#[utoipa::path(delete, path="/v1/webhooks/{id}", operation_id="removeWebhook", tag="webhooks", params(("id"=uuid::Uuid,Path)), responses((status=200,description="Endpoint disabled"),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn remove_webhook() {}
#[utoipa::path(post, path="/v1/webhooks/{id}/test", operation_id="testWebhook", tag="webhooks", params(("id"=uuid::Uuid,Path)), responses((status=202,body=TestDelivery),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn test_webhook() {}
#[utoipa::path(get, path="/v1/webhook-deliveries", operation_id="listWebhookDeliveries", tag="webhooks", responses((status=200,body=[Delivery]),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn deliveries() {}

#[utoipa::path(get, path="/v1/account/api-key", operation_id="getApiKeyMetadata", tag="account", responses((status=200,description="Key metadata",body=Account),(status=401,body=ErrorResponse)), security(("dashboardSession"=[])))]
fn key_metadata() {}
#[utoipa::path(post, path="/v1/account/api-key", operation_id="issueApiKey", tag="account", responses((status=200,description="Rotated key"),(status=201,description="First key"),(status=401,description="identity_unauthorized: an API key cannot mint a key; sign in to the dashboard",body=ErrorResponse),(status=409,body=ErrorResponse)), security(("dashboardSession"=[])))]
fn issue_key() {}
#[utoipa::path(delete, path="/v1/account/api-key", operation_id="revokeApiKey", tag="account", responses((status=204,description="Revoked"),(status=401,body=ErrorResponse),(status=409,body=ErrorResponse)), security(("dashboardSession"=[])))]
fn revoke_key() {}

#[utoipa::path(post, path="/v1/issuers", operation_id="createIssuer", tag="issuers", request_body=IssuerRequest, responses((status=201,description="Created unverified; send a code to prove the contact address",body=Issuer),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=409,description="issuer_name_taken",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn create_issuer() {}
#[utoipa::path(get, path="/v1/issuers", operation_id="listIssuers", tag="issuers", params(("starting_after"=Option<uuid::Uuid>, Query, description="Issuer id returned as next_cursor"),("limit"=Option<u32>, Query, minimum=1, maximum=100)), responses((status=200,body=IssuerPage),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn list_issuers() {}
#[utoipa::path(get, path="/v1/issuers/{id}", operation_id="getIssuer", tag="issuers", params(("id"=uuid::Uuid, Path)), responses((status=200,body=Issuer),(status=401,body=ErrorResponse),(status=404,description="issuer_not_found",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn get_issuer() {}
#[utoipa::path(patch, path="/v1/issuers/{id}", operation_id="updateIssuer", tag="issuers", params(("id"=uuid::Uuid, Path)), request_body(content=IssuerRequest, description="Replaces every editable field; a different contact_email clears the verification"), responses((status=200,body=Issuer),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,description="issuer_name_taken",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn update_issuer() {}
#[utoipa::path(delete, path="/v1/issuers/{id}", operation_id="deleteIssuer", tag="issuers", params(("id"=uuid::Uuid, Path)), responses((status=204,description="Deleted, with its payout-address associations"),(status=409,description="issuer_in_use: requests were issued under it",body=ErrorResponse),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn delete_issuer() {}
#[utoipa::path(post, path="/v1/issuers/{id}/verify/email/start", operation_id="startIssuerEmailVerification", tag="issuers", params(("id"=uuid::Uuid, Path)), responses((status=202,description="A code was emailed to the stored contact address",body=StartIssuerEmail),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,description="issuer_email_already_verified",body=ErrorResponse),(status=429,description="otp_resend_cooldown: one code per identity per minute",body=ErrorResponse),(status=503,description="verification_unavailable",body=ErrorResponse)), security(("apiKey"=[])))]
fn start_issuer_email() {}
#[utoipa::path(post, path="/v1/issuers/{id}/verify/email/confirm", operation_id="confirmIssuerEmailVerification", tag="issuers", params(("id"=uuid::Uuid, Path)), request_body=ConfirmIssuerEmail, responses((status=200,description="The contact address is proven",body=Issuer),(status=401,description="otp_invalid",body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,description="issuer_email_already_verified",body=ErrorResponse),(status=503,description="verification_unavailable",body=ErrorResponse)), security(("apiKey"=[])))]
fn confirm_issuer_email() {}
#[utoipa::path(put, path="/v1/issuers/{id}/payout-addresses", operation_id="setIssuerPayoutAddresses", tag="issuers", params(("id"=uuid::Uuid, Path)), request_body=SetIssuerPayoutAddresses, responses((status=200,description="The identity with its new address set",body=Issuer),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=404,description="issuer_not_found or payout_address_not_found",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn set_issuer_payout_addresses() {}
#[utoipa::path(post, path="/v1/payout-addresses", operation_id="createPayoutAddress", tag="issuers", request_body=PayoutAddressRequest, responses((status=201,body=PayoutAddress),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn create_payout_address() {}
#[utoipa::path(get, path="/v1/payout-addresses", operation_id="listPayoutAddresses", tag="issuers", responses((status=200,body=PayoutAddressList),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn list_payout_addresses() {}
#[utoipa::path(delete, path="/v1/payout-addresses/{id}", operation_id="deletePayoutAddress", tag="issuers", params(("id"=uuid::Uuid, Path)), responses((status=204,description="Deleted, with every association to it"),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn delete_payout_address() {}

#[derive(OpenApi)]
#[openapi(paths(create_payment,list_payments,get_payment,cancel_payment,transfers,payment_attachment,invoice_pdf,proof,payment_verification,create_customer,list_customers,get_customer,update_customer,create_issuer,list_issuers,get_issuer,update_issuer,delete_issuer,start_issuer_email,confirm_issuer_email,set_issuer_payout_addresses,create_payout_address,list_payout_addresses,delete_payout_address,create_attachment,finalize_attachment,account,status,add_webhook,list_webhooks,remove_webhook,test_webhook,deliveries,key_metadata,issue_key,revoke_key),
 components(schemas(ErrorDetail,ErrorResponse,Chain,Token,AsOf,SelfSettlement,Attention,IndexerFreshness,Party,PayerPolicyMode,PayerPolicy,AttachmentDescriptor,Attribution,CreatePayment,Payment,PaymentStatus,PaymentSummary,PaymentPage,Transfer,VerificationFactStatus,VerificationRequirements,VerificationAttempt,VerificationDetail,CancelPayment,CustomerRequest,Customer,CustomerPage,CustomerStats,CustomerDetail,IssuerRequest,ConfirmIssuerEmail,SetIssuerPayoutAddresses,PayoutAddressRequest,PayoutAddress,PayoutAddressList,Issuer,IssuerPage,StartIssuerEmail,AttachmentRequest,AttachmentUpload,AttachmentCommitment,CanonicalIssuanceSnapshot,ProofTransfer,VerificationAttestationPayload,SignedVerificationAttestation,ProofOfPayment,Account,StatusChain,StatusIndexer,StatusSweeper,ServiceStatus,WebhookRequest,Webhook,TestDelivery,Delivery,DeliveryAttempt)),
 modifiers(&Security), tags((name="payments",description="Invoice issuance, documents, and payment tracking"),(name="customers",description="Merchant-owned counterparty records"),(name="issuers",description="Issuer identities and the payout addresses they settle to"),(name="attachments",description="PDF upload and finalization"),(name="webhooks",description="Webhook endpoint and delivery management")))]
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
        api.info.title = "Payday API".into();
        api.info.version = "1.0.0".into();
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
        r#"<!doctype html><html><head><title>Payday API</title><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><script id="api-reference" data-url="/api/openapi.json"></script><script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference"></script></body></html>"#,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    const ROUTES: &[(&str, &str)] = &[
        ("/v1/payments", "get"),
        ("/v1/payments", "post"),
        ("/v1/payments/{id}", "get"),
        ("/v1/payments/{id}/cancel", "post"),
        ("/v1/payments/{id}/transfers", "get"),
        ("/v1/payments/{id}/attachment", "get"),
        ("/v1/payments/{id}/invoice.pdf", "get"),
        ("/v1/payments/{id}/proof", "get"),
        ("/v1/payments/{id}/verification", "get"),
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
        ("/v1/webhooks/{id}", "delete"),
        ("/v1/webhooks/{id}/test", "post"),
        ("/v1/webhook-deliveries", "get"),
        ("/v1/account/api-key", "get"),
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
        let payment = &d["components"]["schemas"]["Payment"]["properties"];
        assert!(payment["as_of"].is_object());
        assert!(payment["recovery_address"].is_object());
        assert!(payment["refund_address"].is_null());
        assert!(payment["memo"].is_null());
        for documented in [
            "issuer",
            "bill_to",
            "payer_policy",
            "attachment",
            "attribution",
        ] {
            assert!(payment[documented].is_object(), "{documented}");
        }
        let create = &d["components"]["schemas"]["CreatePayment"];
        assert!(create["properties"]["refund_address"].is_null());
        assert!(create["properties"]["memo"].is_null());
        let required: Vec<&str> = create["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        for field in [
            "amount",
            "payout_address",
            "issuer",
            "bill_to",
            "payer_policy",
        ] {
            assert!(required.contains(&field), "{field} must be required");
        }
        assert_eq!(
            d["components"]["schemas"]["PayerPolicyMode"]["enum"]
                .as_array()
                .unwrap()
                .len(),
            2
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
        let payment = &d["components"]["schemas"]["Payment"]["properties"];
        assert!(payment["payer_wallet"].is_object());
        assert!(payment["wallet_bound_at"].is_object());
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
            ["chain", "indexer", "sweeper"]
        );
        assert_eq!(
            d["paths"]["/v1/payments"]["post"]["responses"]["200"]["headers"]["Idempotency-Replayed"]
                ["schema"]["type"],
            "string"
        );
    }
}
