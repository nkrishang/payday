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
#[derive(Deserialize, ToSchema)]
#[schema(example = json!({"amount":"10.50","payout_address":"0x1111111111111111111111111111111111111111","refund_address":"0x2222222222222222222222222222222222222222","expires_in":3600,"reference":"order-42","metadata":{"customer":"cus_123"}}))]
struct CreatePayment {
    amount: String,
    payout_address: String,
    refund_address: Option<String>,
    chain_id: Option<String>,
    token_address: Option<String>,
    expires_in: Option<u64>,
    expires_at: Option<String>,
    memo: Option<String>,
    reference: Option<String>,
    #[schema(value_type = Object)]
    metadata: Option<serde_json::Value>,
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
    refund_address: String,
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
    reference: Option<String>,
    #[schema(value_type = Object)]
    metadata: serde_json::Value,
    memo: Option<String>,
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
    memo: Option<String>,
    reference: Option<String>,
    #[schema(value_type = Object)]
    metadata: serde_json::Value,
    created_at: String,
    status: PaymentStatus,
    amount: String,
    received: String,
    cancellation_requested_at: Option<String>,
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

#[utoipa::path(post, path="/v1/payments", operation_id="createPayment", tag="payments",
 request_body(content=CreatePayment, description="expires_in and expires_at are mutually exclusive; the default lifetime is 24 hours"),
 params(("Idempotency-Key"=String, Header, description="Required, 1-255 bytes")),
 responses((status=201, description="Created", body=Payment), (status=200, description="Idempotent replay", body=Payment, headers(("Idempotency-Replayed"=String, description="true"))), (status=400, body=ErrorResponse), (status=401, body=ErrorResponse), (status=409, body=ErrorResponse), (status=422, body=ErrorResponse), (status=429, body=ErrorResponse)), security(("apiKey"=[])))]
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
#[openapi(paths(create_payment,list_payments,get_payment,cancel_payment,transfers,account,status,add_webhook,list_webhooks,remove_webhook,test_webhook,deliveries,key_metadata,issue_key,revoke_key),
 components(schemas(ErrorDetail,ErrorResponse,Chain,Token,AsOf,SelfSettlement,Attention,IndexerFreshness,CreatePayment,Payment,PaymentStatus,PaymentSummary,PaymentPage,Transfer,CancelPayment,Account,StatusChain,StatusIndexer,StatusSweeper,ServiceStatus,WebhookRequest,Webhook,TestDelivery,Delivery,DeliveryAttempt)),
 modifiers(&Security), tags((name="payments",description="Canonical payment API"),(name="webhooks",description="Webhook endpoint and delivery management")))]
struct ApiDoc;

struct Security;
impl utoipa::Modify for Security {
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{Http, HttpAuthScheme, SecurityScheme};
        let c = api.components.as_mut().unwrap();
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
        assert!(d["components"]["schemas"]["Payment"]["properties"]["as_of"].is_object());
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
