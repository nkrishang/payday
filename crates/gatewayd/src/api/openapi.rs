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
struct ExpectedIdentity {
    first_name: String,
    last_name: String,
}
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum PayerPolicyMode {
    Permissionless,
    VerifiedEmail,
    VerifiedIdentity,
    VerifiedIdentityUnattributed,
}
/// `expected_email` is required for every verified mode; `expected_identity`
/// is required for `verified_identity` and forbidden elsewhere.
#[derive(Serialize, Deserialize, ToSchema)]
#[schema(example = json!({"mode":"verified_email","expected_email":"alice@example.com"}))]
struct PayerPolicy {
    mode: PayerPolicyMode,
    expected_email: Option<String>,
    expected_identity: Option<ExpectedIdentity>,
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
    address: String,
    address_explorer_url: Option<String>,
    payout_address: String,
    /// Payday's custodial recovery wallet for overpayments, expired balances,
    /// and late transfers; configured by the platform, not the merchant.
    recovery_address: String,
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
    self_settlement: SelfSettlement,
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
/// Each fact the policy needs, on its own.
#[derive(Serialize, ToSchema)]
struct VerificationRequirements {
    email: VerificationFactStatus,
    document: VerificationFactStatus,
    liveness: VerificationFactStatus,
    identity_match: VerificationFactStatus,
    complete: bool,
}
#[derive(Serialize, ToSchema)]
struct VerificationReview {
    requested_at: String,
    /// approved or declined once decided.
    decision: Option<String>,
    reviewer: Option<String>,
    note: Option<String>,
    decided_at: Option<String>,
}
/// One attempt: statuses, the provider's reference, and allowlisted risk
/// categories. Never anything the provider extracted.
#[derive(Serialize, ToSchema)]
struct VerificationAttempt {
    id: String,
    /// email or identity.
    kind: String,
    /// pending, approved, declined, in_review, expired, abandoned, or review_required.
    status: String,
    /// auth0, didit, or manual.
    provider: String,
    provider_reference: Option<String>,
    attempt_number: u16,
    document: VerificationFactStatus,
    liveness: VerificationFactStatus,
    identity_match: VerificationFactStatus,
    risk_codes: Vec<String>,
    country_code: Option<String>,
    verified_at: Option<String>,
    expires_at: Option<String>,
    created_at: String,
    review: Option<VerificationReview>,
}
#[derive(Serialize, ToSchema)]
struct VerificationDetail {
    payer_policy_mode: PayerPolicyMode,
    verification_completed_at: Option<String>,
    likely_unsolicited_at: Option<String>,
    facts: VerificationRequirements,
    attempts: Vec<VerificationAttempt>,
    /// The latest identity attempt was declined and a human may be asked.
    review_available: bool,
    /// The payer may resubmit from the checkout on their own.
    retry_available: bool,
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
    recovery_address: String,
    factory_address: String,
}
#[derive(Serialize, ToSchema)]
struct ProofTransfer {
    transaction_hash: String,
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
    payer_policy_mode: PayerPolicyMode,
    /// `not_required`, `approved`, or `pending`.
    result: String,
    verified_at: Option<String>,
}
/// Payday-attested, not address-committed: verification happens after
/// issuance. `signature` recovers to `signer` over
/// `keccak256("PAYDAY_VERIFICATION_ATTESTATION_V1" || JCS(payload))`.
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
    attribution_nonce: String,
    attribution_hash: String,
    salt: String,
    chain_id: String,
    factory_address: String,
    payment_address: String,
    token_address: String,
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
 params(("starting_after"=Option<String>, Query, description="pay_ cursor returned as next_cursor"), ("status"=Option<PaymentStatus>, Query), ("reference"=Option<String>, Query), ("limit"=Option<u32>, Query, minimum=1, maximum=100)),
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
#[utoipa::path(get, path="/v1/payments/{id}/proof", operation_id="getProofOfPayment", tag="payments", params(("id"=String, Path)), responses((status=200,description="Verifiable offline with payday proof verify",body=ProofOfPayment),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,description="payment_not_settled",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn proof() {}
#[utoipa::path(get, path="/v1/payments/{id}/verification", operation_id="getPaymentVerification", tag="payments", params(("id"=String, Path)), responses((status=200,description="Every verification attempt with each fact reported separately; provider references and risk categories only, never extracted identity",body=VerificationDetail),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn payment_verification() {}
#[utoipa::path(post, path="/v1/payments/{id}/verification/review", operation_id="requestVerificationReview", tag="payments", params(("id"=String, Path)), responses((status=200,description="A human review was requested for the latest declined identity attempt; automated resubmission stops",body=VerificationDetail),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,description="review_not_available: no declined identity attempt",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn request_verification_review() {}
#[utoipa::path(post, path="/v1/customers", operation_id="createCustomer", tag="customers", request_body=CustomerRequest, responses((status=201,body=Customer),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn create_customer() {}
#[utoipa::path(get, path="/v1/customers", operation_id="listCustomers", tag="customers", params(("starting_after"=Option<uuid::Uuid>, Query, description="Customer id returned as next_cursor"),("limit"=Option<u32>, Query, minimum=1, maximum=100)), responses((status=200,body=CustomerPage),(status=400,body=ErrorResponse),(status=401,body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
fn list_customers() {}
#[utoipa::path(get, path="/v1/customers/{id}", operation_id="getCustomer", tag="customers", params(("id"=uuid::Uuid, Path)), responses((status=200,body=Customer),(status=401,body=ErrorResponse),(status=404,description="customer_not_found",body=ErrorResponse),(status=429,body=ErrorResponse)), security(("apiKey"=[])))]
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

#[utoipa::path(get, path="/v1/account/api-key", operation_id="getApiKeyMetadata", tag="account", responses((status=200,description="Key metadata"),(status=401,body=ErrorResponse)), security(("auth0"=[])))]
fn key_metadata() {}
#[utoipa::path(post, path="/v1/account/api-key", operation_id="issueApiKey", tag="account", responses((status=200,description="Rotated key"),(status=201,description="First key"),(status=401,body=ErrorResponse),(status=409,body=ErrorResponse)), security(("auth0"=[])))]
fn issue_key() {}
#[utoipa::path(delete, path="/v1/account/api-key", operation_id="revokeApiKey", tag="account", responses((status=204,description="Revoked"),(status=401,body=ErrorResponse),(status=409,body=ErrorResponse)), security(("auth0"=[])))]
fn revoke_key() {}

#[derive(OpenApi)]
#[openapi(paths(create_payment,list_payments,get_payment,cancel_payment,transfers,payment_attachment,invoice_pdf,proof,payment_verification,request_verification_review,create_customer,list_customers,get_customer,update_customer,create_attachment,finalize_attachment,account,status,add_webhook,list_webhooks,remove_webhook,test_webhook,deliveries,key_metadata,issue_key,revoke_key),
 components(schemas(ErrorDetail,ErrorResponse,Chain,Token,AsOf,SelfSettlement,Attention,IndexerFreshness,Party,ExpectedIdentity,PayerPolicyMode,PayerPolicy,AttachmentDescriptor,Attribution,CreatePayment,Payment,PaymentStatus,PaymentSummary,PaymentPage,Transfer,VerificationFactStatus,VerificationRequirements,VerificationReview,VerificationAttempt,VerificationDetail,CancelPayment,CustomerRequest,Customer,CustomerPage,AttachmentRequest,AttachmentUpload,AttachmentCommitment,CanonicalIssuanceSnapshot,ProofTransfer,VerificationAttestationPayload,SignedVerificationAttestation,ProofOfPayment,Account,StatusChain,StatusIndexer,StatusSweeper,ServiceStatus,WebhookRequest,Webhook,TestDelivery,Delivery,DeliveryAttempt)),
 modifiers(&Security), tags((name="payments",description="Invoice issuance, documents, and payment tracking"),(name="customers",description="Merchant-owned counterparty records"),(name="attachments",description="PDF upload and finalization"),(name="webhooks",description="Webhook endpoint and delivery management")))]
struct ApiDoc;

struct Security;
impl utoipa::Modify for Security {
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{Http, HttpAuthScheme, SecurityScheme};
        let c = api.components.as_mut().unwrap();
        // Merchant routes take either an API key or an Auth0 access token
        // from the dashboard or CLI application under the same scheme.
        c.add_security_scheme(
            "apiKey",
            SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
        );
        c.add_security_scheme(
            "auth0",
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
        ("/v1/payments/{id}/verification/review", "post"),
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
            4
        );
        let proof = &d["components"]["schemas"]["ProofOfPayment"]["properties"];
        for documented in [
            "canonical_issuance_snapshot",
            "attribution_nonce",
            "salt",
            "payment_address",
            "transfers",
            "verification",
        ] {
            assert!(proof[documented].is_object(), "{documented}");
        }
        let attempt = &d["components"]["schemas"]["VerificationAttempt"]["properties"];
        for documented in [
            "provider_reference",
            "risk_codes",
            "document",
            "liveness",
            "identity_match",
            "review",
        ] {
            assert!(attempt[documented].is_object(), "{documented}");
        }
        for withheld in [
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
