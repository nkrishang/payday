use std::time::Duration;

use axum::http::{HeaderName, Method, header};
use axum::routing::{delete, get, post};
use axum::{Router, middleware};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;
use tracing::field;

use crate::api::accounts;
use crate::api::admin;
use crate::api::attachments;
use crate::api::auth;
use crate::api::customers;
use crate::api::health;
use crate::api::invoices;
use crate::api::payer;
use crate::api::proof;
use crate::api::status;
use crate::api::webhooks;
use crate::api::{middleware as api_middleware, openapi};
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    // The dashboard is a browser app served from the public origin, and it is
    // the only origin the merchant API answers to: a one-element list, so any
    // other origin gets no allow header at all rather than a mismatching one.
    // The layer sits outside the authentication route layer so preflights,
    // which carry no credential, are answered before it.
    tracing::info!(
        origin = state.payer.origin(),
        "merchant routes answer browser requests from the dashboard origin"
    );
    let merchant_cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list([state.payer.origin_header()]))
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::OPTIONS])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
            HeaderName::from_static("idempotency-key"),
        ])
        .max_age(Duration::from_secs(86_400));

    // Merchant routes: an API key or a dashboard/CLI identity token, one
    // account either way.
    let authenticated = Router::new()
        .route("/v1/account", get(accounts::get_account))
        .route(
            "/v1/payments",
            post(invoices::create_payment).get(invoices::list_payments),
        )
        .route("/v1/payments/{id}", get(invoices::get_payment))
        .route("/v1/payments/{id}/cancel", post(invoices::cancel_payment))
        .route("/v1/payments/{id}/transfers", get(invoices::transfers))
        .route(
            "/v1/payments/{id}/attachment",
            get(attachments::payment_attachment),
        )
        .route("/v1/payments/{id}/invoice.pdf", get(invoices::invoice_pdf))
        .route("/v1/payments/{id}/proof", get(proof::get_proof))
        .route(
            "/v1/customers",
            post(customers::create).get(customers::list),
        )
        .route(
            "/v1/customers/{id}",
            get(customers::get).patch(customers::update),
        )
        .route("/v1/attachments", post(attachments::create))
        .route("/v1/attachments/{id}/finalize", post(attachments::finalize))
        .route("/v1/status", get(status::get))
        .route("/v1/webhooks", post(webhooks::add).get(webhooks::list))
        .route("/v1/webhooks/{id}", delete(webhooks::remove))
        .route("/v1/webhooks/{id}/test", post(webhooks::test))
        .route("/v1/webhook-deliveries", get(webhooks::deliveries))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_account,
        ))
        .layer(RequestBodyLimitLayer::new(64 * 1024))
        .layer(merchant_cors);

    let account_management = Router::new()
        .route(
            "/v1/account/api-key",
            post(accounts::issue)
                .get(accounts::metadata)
                .delete(accounts::revoke),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_identity,
        ))
        .layer(RequestBodyLimitLayer::new(16 * 1024));

    let administration = Router::new()
        .route("/v1/admin/payments/{id}/release", post(admin::release))
        .route_layer(middleware::from_fn(auth::require_admin));

    // A payment link is public by design: anyone holding it may read the payment
    // and fulfil it. These routes accept no API key and return no merchant data,
    // so allowing any browser origin grants exactly what curl already has — and
    // it is what lets a merchant render their own checkout, as the docs invite.
    // This must never be extended to the API-key routes.
    let payer = Router::new()
        .route("/v1/payer/payments/{id}", get(payer::get))
        .route("/v1/payer/payments/{id}/qr", get(payer::qr))
        .route("/v1/payer/payments/{id}/attachment", get(payer::attachment))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::HEAD])
                .max_age(Duration::from_secs(86_400)),
        );

    Router::new()
        .route("/health", get(health::health))
        .route("/pay/{id}", get(payer::page))
        .merge(payer)
        .route("/openapi.json", get(openapi::spec))
        .route("/docs", get(openapi::reference))
        .route("/api", get(openapi::reference))
        .route("/api/openapi.json", get(openapi::spec))
        .merge(authenticated)
        .merge(account_management)
        .merge(administration)
        .with_state(state)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::extract::Request| {
                    let request_id = request
                        .extensions()
                        .get::<api_middleware::RequestId>()
                        .map(|value| value.0.as_str())
                        .unwrap_or("unknown");
                    tracing::info_span!(
                        "http_request", method = %request.method(), path = %request.uri().path(),
                        request_id, status = field::Empty, latency_ms = field::Empty,
                        account_id = field::Empty
                    )
                })
                .on_response(
                    |response: &axum::response::Response,
                     latency: std::time::Duration,
                     span: &tracing::Span| {
                        span.record("status", response.status().as_u16());
                        span.record("latency_ms", latency.as_millis());
                        tracing::info!(parent: span, "request completed");
                    },
                ),
        )
        .layer(middleware::from_fn(api_middleware::request_id))
}

pub fn status_router(state: AppState) -> Router {
    Router::new()
        .route("/live", get(health::live))
        .route("/v1/status", get(status::public_json))
        .route("/", get(status::html))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use alloy_primitives::{Address, address};
    use alloy_signer_local::PrivateKeySigner;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use gateway_core::{ChainId, Invoice, ProofError, ProofOfPayment, verify_proof};
    use gateway_db::{AccountId, AccountRepository, InvoiceRepository};
    use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, encode};
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    use serde::Serialize;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use sqlx::PgPool;
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::*;
    use crate::attachments::memory::MemoryObjectStorage;
    use crate::attachments::{AttachmentStore, CLEAN_SCAN, SCAN_STATUS_TAG};
    use crate::attestation::VerificationAttestor;

    const KEY: &str = "payday_live_0123456789abcdef0123456789abcdef";
    /// The platform recovery wallet this test deployment is configured with.
    const RECOVERY: Address = address!("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc");
    /// A minimal PDF, byte for byte what the e2e suite uploads.
    const PDF: &[u8] = b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF";

    fn attestor() -> VerificationAttestor {
        VerificationAttestor::local(PrivateKeySigner::from_slice(&[7u8; 32]).unwrap())
    }

    /// The router plus the memory bucket behind it, so a test can play the
    /// uploading client and the malware scanner.
    struct TestApp {
        router: Router,
        storage: Arc<MemoryObjectStorage>,
    }

    fn test_state(
        pool: PgPool,
        accounts: AccountRepository,
        identity_verifier: Option<auth::Auth0Verifier>,
        factory: Address,
        recovery: Address,
    ) -> (AppState, Arc<MemoryObjectStorage>) {
        let storage = Arc::new(MemoryObjectStorage::default());
        let store = AttachmentStore::new(storage.clone(), Duration::from_secs(300));
        let state = AppState::new(
            InvoiceRepository::new(pool),
            accounts,
            identity_verifier,
            ChainId(1),
            factory,
            Address::ZERO,
            recovery,
            payer_access(),
            "payday_live_".into(),
            None,
            120,
            Some(store),
            Some(attestor()),
        );
        (state, storage)
    }

    #[derive(Serialize)]
    struct IdentityClaims<'a> {
        sub: &'a str,
        iss: &'a str,
        aud: &'a str,
        exp: u64,
        azp: &'a str,
        #[serde(rename = "https://api.payday.sh/auth/method")]
        authentication_method: &'a str,
        #[serde(rename = "https://api.payday.sh/auth/client_id")]
        authentication_client_id: &'a str,
        #[serde(rename = "https://api.payday.sh/auth/authenticated_at")]
        authenticated_at: u64,
        #[serde(rename = "https://api.payday.sh/auth/event_id")]
        authentication_event_id: &'a str,
        #[serde(rename = "https://api.payday.sh/auth/email")]
        email: &'a str,
    }

    async fn app(pool: PgPool) -> Router {
        app_with_recovery(pool, RECOVERY).await
    }

    async fn app_with_recovery(pool: PgPool, recovery: Address) -> Router {
        build(pool, None, Address::ZERO, recovery).await.router
    }

    /// The same deployment reconfigured with another factory.
    async fn app_with_factory(pool: PgPool, factory: Address) -> Router {
        build(pool, None, factory, RECOVERY).await.router
    }

    async fn test_app(pool: PgPool) -> TestApp {
        build(pool, None, Address::ZERO, RECOVERY).await
    }

    async fn build(
        pool: PgPool,
        identity_verifier: Option<auth::Auth0Verifier>,
        factory: Address,
        recovery: Address,
    ) -> TestApp {
        let accounts = AccountRepository::new(pool.clone());
        if accounts
            .find_by_identity("https://test.issuer/", "email|test-user")
            .await
            .unwrap()
            .is_none()
        {
            let _ = accounts
                .issue_api_key_with_email(
                    "https://test.issuer/",
                    "email|test-user",
                    None,
                    &Uuid::now_v7().to_string(),
                    KEY,
                    "merchant@example.com",
                )
                .await
                .unwrap();
        }
        let (state, storage) = test_state(pool, accounts, identity_verifier, factory, recovery);
        TestApp {
            router: router(state),
            storage,
        }
    }

    /// A second account with its own key.
    async fn other_account(pool: &PgPool, key: &str) -> AccountId {
        AccountRepository::new(pool.clone())
            .issue_api_key_with_email(
                "https://issuer.example/",
                &format!("email|{key}"),
                None,
                &format!("event-{key}"),
                key,
                "other@example.com",
            )
            .await
            .unwrap()
            .account_id
    }

    fn get_request(key: &str, path: &str) -> Request<Body> {
        Request::get(path)
            .header(header::AUTHORIZATION, format!("Bearer {key}"))
            .body(Body::empty())
            .unwrap()
    }

    fn json_request(method: &str, key: &str, path: &str, body: &Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {key}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("0x{}", hex::encode(Sha256::digest(bytes)))
    }

    fn payer_access() -> payer::PayerAccess {
        payer::PayerAccess::new("http://127.0.0.1:3000", None).unwrap()
    }

    fn unix_now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    /// A request body that passes every validation rule; tests override one field.
    fn valid_body() -> Value {
        json!({
            "chain_id": "1",
            "token_address": "0x0000000000000000000000000000000000000000",
            "payout_address": "0x0000000000000000000000000000000000000002",
            "amount": "1",
            "expires_in": 3600,
            "issuer": {"name": "Acme"},
            "bill_to": {"name": "Globex"},
            "payer_policy": {"mode": "permissionless"}
        })
    }

    /// The account `app()` provisions for `KEY`.
    async fn test_account(pool: &PgPool) -> AccountId {
        AccountRepository::new(pool.clone())
            .find_by_identity("https://test.issuer/", "email|test-user")
            .await
            .unwrap()
            .expect("app() provisions the test account")
    }

    /// A finalized upload, exactly as the attachment routes leave it.
    async fn ready_attachment(pool: &PgPool, account: AccountId, filename: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query(
            r#"INSERT INTO invoice_attachments
                 (id, account_id, object_key, original_filename, mime_type, byte_length, sha256,
                  status, scan_result, finalized_at)
               VALUES ($1, $2, $3, $4, 'application/pdf', 1234, $5, 'ready', 'NO_THREATS_FOUND', now())"#,
        )
        .bind(id)
        .bind(account.0)
        .bind(format!("{}/{id}.pdf", account.0))
        .bind(filename)
        .bind([0xAB_u8; 32].as_slice())
        .execute(pool)
        .await
        .unwrap();
        id
    }

    /// An upload that was staged but never finalized.
    async fn pending_attachment(pool: &PgPool, account: AccountId) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO invoice_attachments (id, account_id, object_key, original_filename, mime_type, status) VALUES ($1, $2, $3, 'pending.pdf', 'application/pdf', 'pending_upload')",
        )
        .bind(id)
        .bind(account.0)
        .bind(format!("{}/{id}.pdf", account.0))
        .execute(pool)
        .await
        .unwrap();
        id
    }

    async fn customer(pool: &PgPool, account: AccountId, name: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO customers (id, account_id, name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(account.0)
            .bind(name)
            .execute(pool)
            .await
            .unwrap();
        id
    }

    fn create_request(key: &str, idempotency_key: &str, body: &Value) -> Request<Body> {
        Request::post("/v1/payments")
            .header(header::AUTHORIZATION, format!("Bearer {key}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header("idempotency-key", idempotency_key)
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn json_body(response: axum::response::Response) -> Value {
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn identity_verifier_and_tokens() -> (auth::Auth0Verifier, String, String, String) {
        let private = rsa::RsaPrivateKey::new(&mut rand_08::thread_rng(), 2048).unwrap();
        let private_pem = private.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_pem = private
            .to_public_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let verifier = auth::Auth0Verifier::for_test(
            "https://issuer.example/",
            "https://api.payday.sh",
            "payday-cli",
            Some("payday-dashboard"),
            "test-key",
            DecodingKey::from_rsa_pem(public_pem.as_bytes()).unwrap(),
        );
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-key".into());
        let key = EncodingKey::from_rsa_pem(private_pem.as_bytes()).unwrap();
        let authenticated_at = unix_now();
        let token = |event_id| {
            encode(
                &header,
                &IdentityClaims {
                    sub: "email|user",
                    iss: "https://issuer.example/",
                    aud: "https://api.payday.sh",
                    exp: u64::MAX,
                    azp: "payday-cli",
                    authentication_method: "email_otp",
                    authentication_client_id: "payday-cli",
                    authenticated_at,
                    authentication_event_id: event_id,
                    email: "merchant@example.com",
                },
                &key,
            )
            .unwrap()
        };
        (
            verifier,
            token("event-1"),
            token("event-2"),
            token("event-3"),
        )
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn health_reflects_database_reachability(pool: PgPool) {
        let app = app(pool.clone()).await;
        let response = app
            .clone()
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        pool.close().await;
        let response = app
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    async fn assert_unauthorized(app: Router, request: Request<Body>) {
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Bearer");
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let mut body = serde_json::from_slice::<serde_json::Value>(&body).unwrap();
        assert!(body["request_id"].as_str().is_some_and(|id| !id.is_empty()));
        body.as_object_mut().unwrap().remove("request_id");
        assert_eq!(
            body,
            serde_json::json!({
                "error": {
                    "code": "unauthorized",
                    "message": "A valid bearer API key or dashboard access token is required"
                }
            })
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn get_invoice_rejects_missing_and_wrong_credentials(pool: PgPool) {
        assert_unauthorized(
            app(pool.clone()).await,
            Request::get("/v1/payments/not-an-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_unauthorized(
            app(pool).await,
            Request::get("/v1/payments/not-an-id")
                .header(
                    header::AUTHORIZATION,
                    "Bearer payday_live_fedcba9876543210fedcba9876543210",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn create_invoice_rejects_missing_and_wrong_credentials(pool: PgPool) {
        assert_unauthorized(
            app(pool.clone()).await,
            Request::post("/v1/payments")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
        assert_unauthorized(
            app(pool).await,
            Request::post("/v1/payments")
                .header(
                    header::AUTHORIZATION,
                    "Bearer payday_live_fedcba9876543210fedcba9876543210",
                )
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn invoice_routes_accept_the_configured_credential(pool: PgPool) {
        let response = app(pool)
            .await
            .oneshot(
                Request::get("/v1/payments/not-an-id")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // The request reached the handler, which rejects the deliberately
        // malformed invoice ID before making a database query.
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn invoice_routes_reject_oversized_requests(pool: PgPool) {
        let response = app(pool)
            .await
            .oneshot(
                Request::post("/v1/payments")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(vec![b'x'; 64 * 1024 + 1]))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn create_invoice_returns_settlement_fields_and_a_single_use_address(pool: PgPool) {
        let app = app(pool).await;
        let response = app
            .clone()
            .oneshot(create_request(KEY, "fields", &valid_body()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = json_body(response).await;
        assert_eq!(body["status"], "awaiting_payment");
        assert_eq!(body["amount"], "1.000000");
        assert_eq!(body["amount_base_units"], "1000000");
        assert_eq!(body["received"], "0.000000");
        assert_eq!(body["received_base_units"], "0");
        assert!(body["settlement_tx_hash"].is_null());
        assert!(body["settled_block"].is_null());
        assert!(body["attention"].is_null());
        assert!(body["address"].as_str().unwrap().starts_with("0x"));
        let id = body["id"].as_str().unwrap();
        assert!(id.starts_with("pay_"));

        let replay = app
            .clone()
            .oneshot(create_request(KEY, "fields", &valid_body()))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        let replay = json_body(replay).await;
        assert_eq!(replay["id"], id);
        assert_eq!(replay["expires_at"], body["expires_at"]);
        assert_eq!(replay["expires_in"], 3_600);

        let rejected_prefix = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payments/{}", &id[..16]))
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected_prefix.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            json_body(rejected_prefix).await["error"]["code"],
            "invalid_request"
        );

        let list_response = app
            .clone()
            .oneshot(
                Request::get("/v1/payments")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);
        assert_eq!(json_body(list_response).await["payments"][0]["id"], id);

        let second = app
            .clone()
            .oneshot(create_request(KEY, "second-fields", &valid_body()))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::CREATED);
        let second_id = json_body(second).await["id"].as_str().unwrap().to_owned();

        let first_page = app
            .clone()
            .oneshot(
                Request::get("/v1/payments?limit=1")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let first_page = json_body(first_page).await;
        assert_eq!(first_page["payments"].as_array().unwrap().len(), 1);
        assert_eq!(first_page["payments"][0]["id"], second_id);
        assert_eq!(first_page["next_cursor"], second_id);
        let second_page = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payments?limit=1&starting_after={second_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let second_page = json_body(second_page).await;
        assert_eq!(second_page["payments"][0]["id"], id);
        assert!(second_page["next_cursor"].is_null());

        let legacy_route = app
            .oneshot(
                Request::get("/v1/invoices")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(legacy_route.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn relative_expiry_replays_after_wall_clock_moves(pool: PgPool) {
        let app = app(pool).await;
        let body = valid_body();
        let first = app
            .clone()
            .oneshot(create_request(KEY, "relative-replay", &body))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::CREATED);
        let first = json_body(first).await;
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

        let replay = app
            .oneshot(create_request(KEY, "relative-replay", &body))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        let replay = json_body(replay).await;
        assert_eq!(replay["id"], first["id"]);
        assert_eq!(replay["expires_at"], first["expires_at"]);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn exact_expiry_replays_after_the_creation_window(pool: PgPool) {
        let app = app(pool).await;
        let mut body = valid_body();
        body.as_object_mut().unwrap().remove("expires_in");
        body["expires_at"] = json!(
            sqlx::types::chrono::DateTime::from_timestamp((unix_now() + 602) as i64, 0)
                .unwrap()
                .to_rfc3339()
        );
        let first = app
            .clone()
            .oneshot(create_request(KEY, "exact-replay", &body))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::CREATED);
        let first = json_body(first).await;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let replay = app
            .oneshot(create_request(KEY, "exact-replay", &body))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(json_body(replay).await["id"], first["id"]);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn payment_freshness_distinguishes_indexed_cursor_from_finalized_head(pool: PgPool) {
        let app = app(pool.clone()).await;
        let created = app
            .clone()
            .oneshot(create_request(KEY, "freshness", &valid_body()))
            .await
            .unwrap();
        let id = json_body(created).await["id"].as_str().unwrap().to_owned();
        sqlx::query("INSERT INTO indexer_cursor(chain_id,token_address,last_block,last_block_hash,last_block_timestamp) VALUES(1,$1,117,$2,1700000000)")
            .bind(Address::ZERO.as_slice()).bind([1_u8; 32].as_slice()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO indexer_status(chain_id,token_address,finalized_block,finalized_block_hash,finalized_block_timestamp) VALUES(1,$1,120,$2,1700000012)")
            .bind(Address::ZERO.as_slice()).bind([2_u8; 32].as_slice()).execute(&pool).await.unwrap();

        let response = app
            .oneshot(
                Request::get(format!("/v1/payments/{id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let payment = json_body(response).await;
        assert_eq!(payment["as_of"]["block"], "117");
        assert_eq!(payment["indexer_freshness"]["last_indexed_block"], "117");
        assert_eq!(payment["indexer_freshness"]["last_finalized_block"], "120");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn public_status_tracks_real_workers_and_liveness_survives_database_loss(pool: PgPool) {
        sqlx::query("INSERT INTO api_status(chain_id) VALUES(1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO indexer_cursor(chain_id,token_address,last_block,last_block_hash,last_block_timestamp) VALUES(1,$1,100,$2,1700000000)")
            .bind(Address::ZERO.as_slice()).bind([1_u8; 32].as_slice()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO indexer_status(chain_id,token_address,finalized_block,finalized_block_hash,finalized_block_timestamp) VALUES(1,$1,100,$2,1700000000)")
            .bind(Address::ZERO.as_slice()).bind([2_u8; 32].as_slice()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO sweeper_status(chain_id,state) VALUES(1,'running')")
            .execute(&pool)
            .await
            .unwrap();
        let (state, _) = test_state(
            pool.clone(),
            AccountRepository::new(pool.clone()),
            None,
            Address::ZERO,
            RECOVERY,
        );
        let app = status_router(state);

        let healthy = app
            .clone()
            .oneshot(Request::get("/v1/status").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(json_body(healthy).await["status"], "operational");

        sqlx::query("UPDATE indexer_status SET finalized_block=1101 WHERE chain_id=1")
            .execute(&pool)
            .await
            .unwrap();
        let lagging = app
            .clone()
            .oneshot(Request::get("/v1/status").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let lagging = json_body(lagging).await;
        assert_eq!(lagging["status"], "degraded");
        assert_eq!(
            lagging["components"]["payment_indexing_and_settlement"]["status"],
            "degraded"
        );

        pool.close().await;
        let live = app
            .oneshot(Request::get("/live").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(live.status(), StatusCode::OK);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn address_lookup_cancel_and_bounded_list_shape_work(pool: PgPool) {
        let app = app(pool.clone()).await;
        let body = json!({
            "payout_address": "0x0000000000000000000000000000000000000002",
            "amount": "1",
            "issuer": {"name": "Acme"},
            "bill_to": {"name": "Globex"},
            "payer_policy": {"mode": "permissionless"},
            "reference": "Order 1234"
        });
        let created = app
            .clone()
            .oneshot(create_request(KEY, "lookup-and-cancel", &body))
            .await
            .unwrap();
        let created = json_body(created).await;
        let id = created["id"].as_str().unwrap();
        let address = created["address"].as_str().unwrap();
        assert_eq!(created["chain"]["id"], "1");
        assert_eq!(created["token"]["address"], Address::ZERO.to_checksum(None));
        assert_eq!(created["recovery_address"], RECOVERY.to_checksum(None));
        assert_eq!(created["reference"], "Order 1234");
        assert!(created.get("memo").is_none());
        assert_eq!(created["issuer"]["name"], "Acme");
        assert_eq!(created["bill_to"]["name"], "Globex");
        assert_eq!(created["payer_policy"]["mode"], "permissionless");
        assert_eq!(created["attribution"]["version"], 1);
        assert!(created["attachment"].is_null());
        assert_eq!(created["expires_in"], 86_400);

        let found = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payments/{address}"))
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(found.status(), StatusCode::OK);
        assert_eq!(json_body(found).await["id"], id);

        let listed = app
            .clone()
            .oneshot(
                Request::get("/v1/payments")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let listed = json_body(listed).await;
        assert_eq!(listed["payments"][0]["id"], id);
        assert!(listed["payments"][0].get("transfers").is_none());
        assert_eq!(listed["payments"][0]["bill_to_name"], "Globex");
        assert_eq!(listed["payments"][0]["payer_policy_mode"], "permissionless");
        assert_eq!(listed["payments"][0]["has_attachment"], false);

        // A retry that names a staged-but-unfinalized upload is a different
        // request from the one issued without an attachment.
        let mut with_pending = body.clone();
        with_pending["attachment_id"] =
            json!(pending_attachment(&pool, test_account(&pool).await).await);
        let conflict = app
            .clone()
            .oneshot(create_request(KEY, "lookup-and-cancel", &with_pending))
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(conflict).await["error"]["code"],
            "idempotency_conflict"
        );

        let cancelled = app
            .oneshot(
                Request::post(format!("/v1/payments/{id}/cancel"))
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cancelled.status(), StatusCode::OK);
        let cancelled = json_body(cancelled).await;
        assert_eq!(cancelled["payment"]["status"], "awaiting_payment");
        assert!(cancelled["payment"]["cancellation_requested_at"].is_string());
        assert!(
            cancelled["advisory"]
                .as_str()
                .unwrap()
                .contains("does not alter")
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn scoped_payment_link_serves_status_and_qr_without_merchant_data(pool: PgPool) {
        let app = app(pool.clone()).await;
        let created = app
            .clone()
            .oneshot(create_request(KEY, "payer-link", &valid_body()))
            .await
            .unwrap();
        let created = json_body(created).await;
        let payment_url = created["payment_url"].as_str().unwrap();
        let id = created["id"].as_str().unwrap();
        let uuid = Uuid::parse_str(id.strip_prefix("pay_").unwrap()).unwrap();
        assert!(!payment_url.contains("token"));
        assert!(payment_url.ends_with(&format!("/pay/{id}")));

        // The checkout is hosted at PAYDAY_PUBLIC_BASE_URL, so this service only
        // forwards the link that merchants have already shared.
        let page = app
            .clone()
            .oneshot(
                Request::get(format!("/pay/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::MOVED_PERMANENTLY);
        assert_eq!(
            page.headers()[header::LOCATION],
            format!("http://127.0.0.1:3000/pay/{id}").as_str()
        );
        assert_eq!(page.headers()[header::REFERRER_POLICY], "no-referrer");

        // An id that is not a payment must not reach the Location header.
        let bogus = app
            .clone()
            .oneshot(
                Request::get("/pay/https:%2F%2Fevil.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bogus.status(), StatusCode::UNAUTHORIZED);
        assert!(!bogus.headers().contains_key(header::LOCATION));

        // Browsers on the checkout origin read these routes directly.
        let cors = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/payments/{id}"))
                    .header(header::ORIGIN, "https://payday.sh")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cors.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");

        let status = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/payments/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
        assert_eq!(status.headers()[header::CACHE_CONTROL], "no-store");
        let status = json_body(status).await;
        assert_eq!(status["id"], id);
        assert_eq!(status["status"], "awaiting_payment");
        assert_eq!(status["content_unlocked"], true);
        assert_eq!(status["issuer_name"], "Acme");
        assert_eq!(status["amount_base_units"], "1000000");
        assert_eq!(status["invoice"]["bill_to"]["name"], "Globex");
        assert_eq!(status["payer_policy"]["mode"], "permissionless");
        assert_eq!(status["requirements"]["complete"], true);
        assert!(status["payment_uri"].as_str().unwrap().starts_with(
            "ethereum:0x0000000000000000000000000000000000000000@1/transfer?address="
        ));
        for private in [
            "payout_address",
            "recovery_address",
            "refund_address",
            "self_settlement",
            "metadata",
            "customer_id",
            "attribution",
            "expected_email",
            "expected_identity",
        ] {
            assert!(
                status.get(private).is_none(),
                "payer response leaked {private}"
            );
        }

        let qr = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/payments/{id}/qr"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(qr.status(), StatusCode::OK);
        assert_eq!(
            qr.headers()[header::CONTENT_TYPE],
            "image/svg+xml; charset=utf-8"
        );
        assert_eq!(qr.headers()[header::CACHE_CONTROL], "no-store");

        sqlx::query("UPDATE invoices SET confirmed_received = '250000' WHERE id = $1")
            .bind(uuid)
            .execute(&pool)
            .await
            .unwrap();
        let partial = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/payments/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let partial = json_body(partial).await;
        assert_eq!(partial["remaining_base_units"], "750000");
        assert_eq!(partial["status"], "partially_paid");
        assert_eq!(partial["payable"], true);
        assert!(
            partial["payment_uri"]
                .as_str()
                .unwrap()
                .ends_with("uint256=750000")
        );

        sqlx::query("UPDATE invoices SET status = 'expired' WHERE id = $1")
            .bind(uuid)
            .execute(&pool)
            .await
            .unwrap();
        let expired = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/payments/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let expired = json_body(expired).await;
        assert_eq!(expired["payable"], false);
        assert!(expired["payment_uri"].is_null());
        let closed_qr = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/payments/{id}/qr"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(closed_qr.status(), StatusCode::GONE);
        assert_eq!(
            json_body(closed_qr).await["error"]["code"],
            "payment_not_payable"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn idempotency_key_conflicts_on_every_immutable_issuance_field(pool: PgPool) {
        let app = app(pool.clone()).await;
        let account = test_account(&pool).await;
        let customer_id = customer(&pool, account, "Globex").await;
        let first_pdf = ready_attachment(&pool, account, "contract.pdf").await;
        let second_pdf = ready_attachment(&pool, account, "other.pdf").await;
        let pending_pdf = pending_attachment(&pool, account).await;
        let body = json!({
            "payout_address": "0x0000000000000000000000000000000000000002",
            "amount": "25",
            "expires_in": 3600,
            "issuer": {"name": "Acme Corp", "email": "billing@acme.example", "details": "1 Main St"},
            "bill_to": {"name": "Globex"},
            "notes": "Net 30",
            "heading": "March retainer",
            "reference": "INV-1",
            "metadata": {"po": "42"},
            "customer_id": customer_id,
            "payer_policy": {
                "mode": "verified_identity",
                "expected_email": "Alice@Example.com",
                "expected_identity": {"first_name": "Alice", "last_name": "Smith"}
            },
            "attachment_id": first_pdf
        });
        let created = app
            .clone()
            .oneshot(create_request(KEY, "document", &body))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = json_body(created).await;
        assert_eq!(created["customer_id"], customer_id.to_string());
        assert_eq!(created["attachment"]["id"], first_pdf.to_string());
        assert_eq!(created["attachment"]["filename"], "contract.pdf");
        assert_eq!(created["attachment"]["byte_length"], "1234");
        assert!(created["attachment"].get("download_url").is_none());
        assert_eq!(
            created["payer_policy"]["expected_email"],
            "alice@example.com"
        );
        assert_eq!(created["heading"], "March retainer");
        assert_eq!(created["notes"], "Net 30");

        // The document is attached now; an identical retry still replays.
        let replay = app
            .clone()
            .oneshot(create_request(KEY, "document", &body))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(replay.headers()["Idempotency-Replayed"], "true");
        assert_eq!(json_body(replay).await["id"], created["id"]);

        let mut variations: Vec<(&str, Value)> = Vec::new();
        let mut vary = |name: &'static str, edit: &dyn Fn(&mut Value)| {
            let mut variant = body.clone();
            edit(&mut variant);
            variations.push((name, variant));
        };
        vary("issuer", &|b| {
            b["issuer"]["email"] = json!("ap@acme.example")
        });
        vary("bill_to", &|b| b["bill_to"]["name"] = json!("Initech"));
        vary("notes", &|b| b["notes"] = json!("Net 60"));
        vary("heading", &|b| {
            b.as_object_mut().unwrap().remove("heading");
        });
        vary("reference", &|b| b["reference"] = json!("INV-2"));
        vary("metadata", &|b| b["metadata"] = json!({"po": "43"}));
        vary("customer_id", &|b| {
            b.as_object_mut().unwrap().remove("customer_id");
        });
        vary(
            "policy mode",
            &|b| {
                b["payer_policy"] =
                    json!({"mode": "verified_email", "expected_email": "alice@example.com"})
            },
        );
        vary("expected_email", &|b| {
            b["payer_policy"]["expected_email"] = json!("bob@example.com")
        });
        vary("expected_identity", &|b| {
            b["payer_policy"]["expected_identity"]["last_name"] = json!("Smyth")
        });
        vary("expiration intent", &|b| b["expires_in"] = json!(7200));
        vary("amount", &|b| b["amount"] = json!("26"));
        vary("attachment id", &|b| b["attachment_id"] = json!(second_pdf));
        vary("attachment removed", &|b| {
            b.as_object_mut().unwrap().remove("attachment_id");
        });
        vary("attachment not yet finalized", &|b| {
            b["attachment_id"] = json!(pending_pdf)
        });
        for (name, variant) in variations {
            let response = app
                .clone()
                .oneshot(create_request(KEY, "document", &variant))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT, "{name}");
            assert_eq!(
                json_body(response).await["error"]["code"],
                "idempotency_conflict",
                "{name}"
            );
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invoices")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "conflicts issue nothing");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn create_rejects_foreign_customers_and_unusable_attachments(pool: PgPool) {
        let app = app(pool.clone()).await;
        let account = test_account(&pool).await;
        let other = other_account(&pool, "payday_live_other0123456789abcdef0123456789abcdef").await;
        let foreign_customer = customer(&pool, other, "Theirs").await;
        let foreign_pdf = ready_attachment(&pool, other, "theirs.pdf").await;
        let pending_pdf = pending_attachment(&pool, account).await;
        let attached_pdf = ready_attachment(&pool, account, "attached.pdf").await;
        let mut first = valid_body();
        first["attachment_id"] = json!(attached_pdf);
        assert_eq!(
            app.clone()
                .oneshot(create_request(KEY, "attached", &first))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );

        let cases: Vec<(&str, &str, Value, StatusCode, &str)> = vec![
            (
                "foreign customer",
                "customer_id",
                json!(foreign_customer),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                "foreign attachment",
                "attachment_id",
                json!(foreign_pdf),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                "unknown attachment",
                "attachment_id",
                json!(Uuid::now_v7()),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                "pending attachment",
                "attachment_id",
                json!(pending_pdf),
                StatusCode::CONFLICT,
                "attachment_not_ready",
            ),
            (
                "attached attachment",
                "attachment_id",
                json!(attached_pdf),
                StatusCode::CONFLICT,
                "attachment_already_attached",
            ),
            (
                "blank issuer",
                "issuer",
                json!({"name": " "}),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                "long heading",
                "heading",
                json!("h".repeat(201)),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                "bad expected email",
                "payer_policy",
                json!({"mode": "verified_email", "expected_email": "not-an-email"}),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            // Legal JSON, but Postgres cannot store it: refused before the
            // insert rather than failing inside it.
            (
                "nul in notes",
                "notes",
                json!("Net\u{0}30"),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                "nul in issuer name",
                "issuer",
                json!({"name": "Ac\u{0}me"}),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
        ];
        for (name, field, value, status, code) in cases {
            let mut body = valid_body();
            body[field] = value;
            let response = app
                .clone()
                .oneshot(create_request(KEY, name, &body))
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{name}");
            assert_eq!(json_body(response).await["error"]["code"], code, "{name}");
        }

        // The mode rules are enforced by the wire shape itself.
        let mut body = valid_body();
        body["payer_policy"] = json!({
            "mode": "verified_identity_unattributed",
            "expected_email": "alice@example.com",
            "expected_identity": {"first_name": "Alice", "last_name": "Smith"}
        });
        let response = app
            .clone()
            .oneshot(create_request(KEY, "unattributed identity", &body))
            .await
            .unwrap();
        assert!(response.status().is_client_error());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invoices")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "only the first issuance succeeded");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn gated_invoice_reveals_only_issuer_heading_and_requirements_until_verified(
        pool: PgPool,
    ) {
        let app = app(pool.clone()).await;
        let account = test_account(&pool).await;
        let pdf = ready_attachment(&pool, account, "contract.pdf").await;
        let mut body = valid_body();
        body["heading"] = json!("March retainer");
        body["notes"] = json!("Net 30");
        body["reference"] = json!("INV-1");
        body["attachment_id"] = json!(pdf);
        body["payer_policy"] =
            json!({"mode": "verified_email", "expected_email": "alice@example.com"});
        let created = app
            .clone()
            .oneshot(create_request(KEY, "gated", &body))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = json_body(created).await;
        let id = created["id"].as_str().unwrap();
        assert_eq!(
            created["payer_policy"]["expected_email"],
            "alice@example.com"
        );

        let payer = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/payments/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(payer.status(), StatusCode::OK);
        let payer = json_body(payer).await;
        assert_eq!(payer["content_unlocked"], false);
        assert_eq!(payer["issuer_name"], "Acme");
        assert_eq!(payer["heading"], "March retainer");
        assert_eq!(payer["status"], "awaiting_payment");
        assert_eq!(payer["payer_policy"]["mode"], "verified_email");
        assert_eq!(
            payer["payer_policy"]["expected_email_hint"],
            "a****@e***.com"
        );
        assert_eq!(payer["requirements"]["email"], "pending");
        assert_eq!(payer["requirements"]["document"], "not_required");
        assert_eq!(payer["requirements"]["complete"], false);
        for withheld in [
            "amount",
            "amount_base_units",
            "received",
            "remaining",
            "address",
            "address_explorer_url",
            "payment_uri",
            "chain",
            "token",
            "invoice",
            "settlement_tx_hash",
            "settlement_explorer_url",
        ] {
            assert!(payer[withheld].is_null(), "{withheld} must be withheld");
        }
        let serialized = payer.to_string();
        for secret in [
            "alice@example.com",
            "Globex",
            "Net 30",
            "INV-1",
            "contract.pdf",
            "0xabab",
        ] {
            assert!(
                !serialized.contains(secret),
                "payer response leaked {secret}"
            );
        }
        assert!(!serialized.contains(created["address"].as_str().unwrap()));

        let qr = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/payments/{id}/qr"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(qr.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(qr).await["error"]["code"],
            "verification_required"
        );
        // The PDF is content: it stays behind the same gate as the QR.
        let withheld = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/payments/{id}/attachment"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(withheld.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(withheld).await["error"]["code"],
            "verification_required"
        );
        let merchant_pdf = app
            .clone()
            .oneshot(get_request(KEY, &format!("/v1/payments/{id}/attachment")))
            .await
            .unwrap();
        assert_eq!(merchant_pdf.status(), StatusCode::OK);
        assert!(
            json_body(merchant_pdf).await["download_url"]
                .as_str()
                .is_some_and(|url| url.starts_with("memory://"))
        );

        // Once verification completes the requirements read approved; the
        // content itself unlocks per payer session in Slice 3. The settlement
        // transaction is content too: on chain it names the payment address,
        // the amount, and the payout address.
        let settlement = alloy_primitives::B256::repeat_byte(0x5e);
        sqlx::query(
            "UPDATE invoices SET verification_completed_at = now(), settlement_tx_hash = $2 WHERE id = $1",
        )
        .bind(Uuid::parse_str(id.strip_prefix("pay_").unwrap()).unwrap())
        .bind(settlement.as_slice())
        .execute(&pool)
        .await
        .unwrap();
        let completed = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/payments/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let completed = json_body(completed).await;
        assert_eq!(completed["requirements"]["email"], "approved");
        assert_eq!(completed["requirements"]["complete"], true);
        assert_eq!(completed["content_unlocked"], false);
        assert!(completed["settlement_tx_hash"].is_null());
        assert!(completed["settlement_explorer_url"].is_null());
        assert!(!completed.to_string().contains("5e5e5e5e"));

        // The merchant, and the payer of a permissionless invoice, still see it.
        let open = app
            .clone()
            .oneshot(create_request(KEY, "open", &valid_body()))
            .await
            .unwrap();
        let open_id = json_body(open).await["id"].as_str().unwrap().to_owned();
        sqlx::query("UPDATE invoices SET settlement_tx_hash = $2 WHERE id = $1")
            .bind(Uuid::parse_str(open_id.strip_prefix("pay_").unwrap()).unwrap())
            .bind(settlement.as_slice())
            .execute(&pool)
            .await
            .unwrap();
        let open_payer = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/payments/{open_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            json_body(open_payer).await["settlement_tx_hash"],
            settlement.to_string()
        );

        let merchant = app
            .oneshot(
                Request::get(format!("/v1/payments/{id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let merchant = json_body(merchant).await;
        assert_eq!(merchant["attachment"]["filename"], "contract.pdf");
        assert!(merchant["verification_completed_at"].is_string());
        assert_eq!(merchant["settlement_tx_hash"], settlement.to_string());
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn create_rejects_unknown_refund_address(pool: PgPool) {
        let app = app(pool).await;
        let mut body = valid_body();
        body["refund_address"] = json!("0x0000000000000000000000000000000000000003");
        let response = app
            .clone()
            .oneshot(create_request(KEY, "refund", &body))
            .await
            .unwrap();
        assert!(response.status().is_client_error(), "{}", response.status());
        assert_eq!(
            json_body(response).await["error"]["code"],
            "invalid_request"
        );

        let listed = app
            .oneshot(
                Request::get("/v1/payments")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            json_body(listed).await["payments"]
                .as_array()
                .unwrap()
                .is_empty(),
            "no invoice may be created from a rejected request"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn create_uses_configured_recovery_address(pool: PgPool) {
        let app = app(pool.clone()).await;
        let created = app
            .clone()
            .oneshot(create_request(KEY, "platform-recovery", &valid_body()))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = json_body(created).await;
        assert_eq!(created["recovery_address"], RECOVERY.to_checksum(None));
        assert!(created.get("refund_address").is_none());

        let id = created["id"].as_str().unwrap();
        let uuid = Uuid::parse_str(id.strip_prefix("pay_").unwrap()).unwrap();
        let row = InvoiceRepository::new(pool.clone())
            .find_by_id(uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.recovery_address, RECOVERY.to_vec());
        let invoice = Invoice::try_from(&row).unwrap();
        assert_eq!(invoice.recovery.0, RECOVERY);
        assert!(
            invoice.address_matches_parameters(),
            "the payment address must commit to the platform recovery wallet"
        );
        assert_eq!(
            created["address"],
            invoice.payment_address.0.to_checksum(None)
        );

        // The wallet is committed into the address, so a replay against a
        // deployment with a different recovery wallet cannot be the same
        // request.
        let other = address!("0x000000000000000000000000000000000000dEaD");
        let replay = app_with_recovery(pool, other)
            .await
            .oneshot(create_request(KEY, "platform-recovery", &valid_body()))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(replay).await["error"]["code"],
            "idempotency_conflict"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn create_invoice_rejects_malformed_parameters(pool: PgPool) {
        let app = app(pool).await;
        let cases: Vec<(&str, Value, &str)> = vec![
            ("negative amount", json!("-1.5"), "invalid_amount"),
            ("signed amount", json!("+1"), "invalid_amount"),
            ("zero amount", json!("0"), "invalid_amount"),
            ("too many decimals", json!("1.0000001"), "invalid_amount"),
        ];
        for (name, amount, code) in cases {
            let mut body = valid_body();
            body["amount"] = amount;
            let response = app
                .clone()
                .oneshot(create_request(KEY, name, &body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{name}");
            assert_eq!(json_body(response).await["error"]["code"], code, "{name}");
        }

        let expiration_cases = [
            ("deadline too soon", 60),
            ("beyond a year", 400 * 24 * 3600),
        ];
        for (name, expiration) in expiration_cases {
            let mut body = valid_body();
            body["expires_in"] = json!(expiration);
            let response = app
                .clone()
                .oneshot(create_request(KEY, name, &body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{name}");
            assert_eq!(
                json_body(response).await["error"]["code"],
                "invalid_request",
                "{name}"
            );
        }

        let mut body = valid_body();
        body["payout_address"] = json!("0x0000000000000000000000000000000000000000");
        let response = app
            .clone()
            .oneshot(create_request(KEY, "zero beneficiary", &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(
            json_body(response).await["error"]["message"]
                .as_str()
                .unwrap()
                .contains("payout_address")
        );

        for (name, key) in [("empty key", String::new()), ("long key", "k".repeat(256))] {
            let response = app
                .clone()
                .oneshot(create_request(KEY, &key, &valid_body()))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{name}");
            assert_eq!(
                json_body(response).await["error"]["code"],
                "invalid_request",
                "{name}"
            );
        }

        let response = app
            .oneshot(create_request(KEY, "k".repeat(255).as_str(), &valid_body()))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "255-byte keys are allowed"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn invoices_and_idempotency_keys_are_isolated_by_account(pool: PgPool) {
        const FIRST: &str = "payday_live_first0123456789abcdef0123456789abcdef";
        const SECOND: &str = "payday_live_second0123456789abcdef0123456789abcdef";
        let accounts = AccountRepository::new(pool.clone());
        accounts
            .issue_api_key_with_email(
                "https://issuer.example/",
                "email|first",
                None,
                "first-event",
                FIRST,
                "first@example.com",
            )
            .await
            .unwrap();
        accounts
            .issue_api_key_with_email(
                "https://issuer.example/",
                "email|second",
                None,
                "second-event",
                SECOND,
                "second@example.com",
            )
            .await
            .unwrap();
        let app = app(pool).await;
        let body = valid_body();

        let first = app
            .clone()
            .oneshot(create_request(FIRST, "same-key", &body))
            .await
            .unwrap();
        let second = app
            .clone()
            .oneshot(create_request(SECOND, "same-key", &body))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::CREATED);
        assert_eq!(second.status(), StatusCode::CREATED);
        let first_id = json_body(first).await["id"].as_str().unwrap().to_string();
        let second_id = json_body(second).await["id"].as_str().unwrap().to_string();
        assert_ne!(first_id, second_id);

        let cross_account = app
            .oneshot(
                Request::get(format!("/v1/payments/{first_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {SECOND}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cross_account.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn authenticated_user_can_rotate_inspect_and_revoke_keys(pool: PgPool) {
        let (verifier, token, replacement_token, revocation_token) = identity_verifier_and_tokens();
        let accounts = AccountRepository::new(pool.clone());
        let (state, _) = test_state(pool, accounts, Some(verifier), Address::ZERO, RECOVERY);
        let app = router(state);
        let identity_request = |method: &str, path: &str, body: Body| {
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(body)
                .unwrap()
        };

        let provisioned = app
            .clone()
            .oneshot(identity_request(
                "POST",
                "/v1/account/api-key",
                Body::from("{}"),
            ))
            .await
            .unwrap();
        assert_eq!(provisioned.status(), StatusCode::CREATED);
        let provisioned_json = json_body(provisioned).await;
        assert_eq!(provisioned_json["generation"], 1);
        assert_eq!(provisioned_json["replaced_previous_key"], false);
        let first_key = provisioned_json["api_key"].as_str().unwrap().to_string();

        let duplicate = app
            .clone()
            .oneshot(identity_request(
                "POST",
                "/v1/account/api-key",
                Body::from("{}"),
            ))
            .await
            .unwrap();
        assert_eq!(duplicate.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(duplicate).await["error"]["code"],
            "api_key_generation_conflict"
        );

        let replaced = app
            .clone()
            .oneshot(
                Request::post("/v1/account/api-key")
                    .header(header::AUTHORIZATION, format!("Bearer {replacement_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"expected_generation":1}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replaced.status(), StatusCode::OK);
        let replaced_json = json_body(replaced).await;
        assert_eq!(replaced_json["generation"], 2);
        assert_eq!(replaced_json["replaced_previous_key"], true);
        let second_key = replaced_json["api_key"].as_str().unwrap().to_string();
        assert_ne!(first_key, second_key);

        let metadata = app
            .clone()
            .oneshot(
                Request::get("/v1/account")
                    .header(header::AUTHORIZATION, format!("Bearer {second_key}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(metadata.status(), StatusCode::OK);
        let metadata = json_body(metadata).await;
        assert_eq!(metadata["generation"], 2);
        assert!(metadata["account_id"].is_string());
        assert!(metadata["key_hint"].is_string());
        assert!(metadata["previous_key_expires_at"].is_string());
        assert!(metadata["revoked_at"].is_null());

        let invoice_request = |key: &str| {
            Request::get("/v1/payments/not-an-id")
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .body(Body::empty())
                .unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(invoice_request(&first_key))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            app.clone()
                .oneshot(invoice_request(&second_key))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );

        let revoked = app
            .clone()
            .oneshot(
                Request::delete("/v1/account/api-key")
                    .header(header::AUTHORIZATION, format!("Bearer {revocation_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"expected_generation":2}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(revoked.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            app.oneshot(invoice_request(&second_key))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    fn session_verifier_and_key() -> (auth::Auth0Verifier, EncodingKey) {
        let private = rsa::RsaPrivateKey::new(&mut rand_08::thread_rng(), 2048).unwrap();
        let private_pem = private.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_pem = private
            .to_public_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let verifier = auth::Auth0Verifier::for_test(
            "https://issuer.example/",
            "https://api.payday.sh",
            "payday-cli",
            Some("payday-dashboard"),
            "test-key",
            DecodingKey::from_rsa_pem(public_pem.as_bytes()).unwrap(),
        );
        (
            verifier,
            EncodingKey::from_rsa_pem(private_pem.as_bytes()).unwrap(),
        )
    }

    fn session_token(
        key: &EncodingKey,
        azp: &str,
        audience: &str,
        authenticated_at: u64,
    ) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-key".into());
        encode(
            &header,
            &IdentityClaims {
                sub: "email|dashboard-user",
                iss: "https://issuer.example/",
                aud: audience,
                exp: u64::MAX,
                azp,
                authentication_method: "email_otp",
                authentication_client_id: azp,
                authenticated_at,
                authentication_event_id: "session-event",
                email: "dashboard@example.com",
            },
            key,
        )
        .unwrap()
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn dashboard_and_cli_session_tokens_authenticate_without_an_api_key(pool: PgPool) {
        let (verifier, key) = session_verifier_and_key();
        let app = build(pool.clone(), Some(verifier), Address::ZERO, RECOVERY)
            .await
            .router;
        let audience = "https://api.payday.sh";
        // An hour-old token: fine for a session, far too old to issue a key.
        let stale = unix_now() - 3600;
        let dashboard = session_token(&key, "payday-dashboard", audience, stale);

        let created = app
            .clone()
            .oneshot(create_request(
                &dashboard,
                "dashboard-session",
                &valid_body(),
            ))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let id = json_body(created).await["id"].as_str().unwrap().to_owned();

        // The CLI's token names the same identity, hence the same account.
        let cli = session_token(&key, "payday-cli", audience, stale);
        let listed = app
            .clone()
            .oneshot(get_request(&cli, "/v1/payments"))
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        assert_eq!(json_body(listed).await["payments"][0]["id"], id);

        let accounts = AccountRepository::new(pool.clone());
        let account = accounts
            .find_by_identity("https://issuer.example/", "email|dashboard-user")
            .await
            .unwrap()
            .expect("the first session provisions the account");
        let metadata = accounts.metadata(account).await.unwrap();
        assert!(metadata.hint.is_none(), "no key was issued");
        assert!(accounts.has_verified_email(account).await.unwrap());

        for bad in [
            session_token(&key, "other-client", audience, stale),
            session_token(&key, "payday-dashboard", "wrong-audience", stale),
        ] {
            assert_unauthorized(app.clone(), get_request(&bad, "/v1/payments")).await;
        }

        // Key management keeps the fresh-OTP path: a stale token or a
        // dashboard token is refused, a fresh CLI token is not.
        let key_route = |token: &str| get_request(token, "/v1/account/api-key");
        for refused in [&dashboard, &cli] {
            let response = app.clone().oneshot(key_route(refused)).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(
                json_body(response).await["error"]["code"],
                "identity_unauthorized"
            );
        }
        let fresh_cli = session_token(&key, "payday-cli", audience, unix_now());
        let metadata = app.clone().oneshot(key_route(&fresh_cli)).await.unwrap();
        assert_eq!(metadata.status(), StatusCode::OK);
        let metadata = json_body(metadata).await;
        assert_eq!(metadata["account_id"], account.0.to_string());
        assert!(metadata["key_hint"].is_null());
        assert_eq!(metadata["generation"], 1);
    }

    /// Reserve an upload slot; returns the attachment id and its object key.
    async fn reserve_upload(app: &Router, key: &str, filename: &str) -> (Uuid, String, Value) {
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                key,
                "/v1/attachments",
                &json!({"filename": filename}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = json_body(response).await;
        let id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
        let url = body["upload_url"].as_str().unwrap();
        let object_key = url
            .strip_prefix("memory://")
            .and_then(|rest| rest.split('?').next())
            .unwrap()
            .to_owned();
        (id, object_key, body)
    }

    async fn finalize(app: &Router, key: &str, id: Uuid) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::post(format!("/v1/attachments/{id}/finalize"))
                    .header(header::AUTHORIZATION, format!("Bearer {key}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        (status, json_body(response).await)
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn attachment_upload_finalize_issuance_and_download_follow_the_scan_verdict(
        pool: PgPool,
    ) {
        let TestApp {
            router: app,
            storage,
        } = test_app(pool.clone()).await;
        let account = test_account(&pool).await;

        let (id, object_key, reserved) = reserve_upload(&app, KEY, "contract.pdf").await;
        assert_eq!(
            object_key,
            format!("uploads/{}/{id}.pdf", account.0),
            "the key is reserved under the account"
        );
        assert_eq!(reserved["headers"]["content-type"], "application/pdf");
        assert_eq!(
            reserved["headers"]["x-amz-tagging"],
            "payday-upload=pending"
        );
        assert_eq!(
            reserved["headers"]["if-none-match"], "*",
            "the key is write-once"
        );
        assert!(reserved["expires_at"].is_string());

        // Nothing uploaded yet.
        let (status, body) = finalize(&app, KEY, id).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "attachment_not_ready");

        // Uploaded, but the scanner has not spoken.
        let version = storage.put(&object_key, "application/pdf", PDF);
        let (status, body) = finalize(&app, KEY, id).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "attachment_scan_pending");

        storage.tag(&object_key, SCAN_STATUS_TAG, CLEAN_SCAN);
        let (status, ready) = finalize(&app, KEY, id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ready["id"], id.to_string());
        assert_eq!(ready["filename"], "contract.pdf");
        assert_eq!(ready["mime_type"], "application/pdf");
        assert_eq!(ready["byte_length"], PDF.len().to_string());
        assert_eq!(ready["sha256"], sha256_hex(PDF));
        assert!(ready.get("download_url").is_none());
        let (status, again) = finalize(&app, KEY, id).await;
        assert_eq!(status, StatusCode::OK, "finalize is idempotent");
        assert_eq!(again, ready);
        let pinned: Option<String> =
            sqlx::query_scalar("SELECT version_id FROM invoice_attachments WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(pinned.as_deref(), Some(version.as_str()));

        // A second PUT that somehow lands is never what the invoice serves:
        // the retag and the download link both name the hashed version.
        let replaced = storage.put(&object_key, "application/pdf", b"%PDF-1.4 replaced");
        assert_ne!(replaced, version);

        let mut body = valid_body();
        body["attachment_id"] = json!(id);
        let created = app
            .clone()
            .oneshot(create_request(KEY, "with-pdf", &body))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = json_body(created).await;
        let payment_id = created["id"].as_str().unwrap().to_owned();
        assert_eq!(created["attachment"]["sha256"], sha256_hex(PDF));
        let tags = storage.tags_of_version(&object_key, Some(&version));
        assert_eq!(
            tags["payday-upload"], "attached",
            "issuance retags the hashed version"
        );
        assert_eq!(tags[SCAN_STATUS_TAG], CLEAN_SCAN, "the verdict is kept");
        assert_eq!(
            storage.tags_of_version(&object_key, Some(&replaced))["payday-upload"],
            "pending",
            "the replacement is left to the lifecycle rule"
        );

        let reuse = app
            .clone()
            .oneshot(create_request(KEY, "reuse-pdf", &body))
            .await
            .unwrap();
        assert_eq!(reuse.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(reuse).await["error"]["code"],
            "attachment_already_attached"
        );
        let (status, _) = finalize(&app, KEY, id).await;
        assert_eq!(status, StatusCode::OK, "an attached upload still answers");

        let merchant = app
            .clone()
            .oneshot(get_request(
                KEY,
                &format!("/v1/payments/{payment_id}/attachment"),
            ))
            .await
            .unwrap();
        assert_eq!(merchant.status(), StatusCode::OK);
        assert_eq!(merchant.headers()[header::CACHE_CONTROL], "no-store");
        let merchant = json_body(merchant).await;
        assert_eq!(merchant["sha256"], sha256_hex(PDF));
        let download_url = merchant["download_url"].as_str().unwrap();
        assert!(download_url.starts_with(&format!("memory://{object_key}?download")));
        assert!(
            download_url.ends_with(&format!("&versionId={version}")),
            "the download serves the hashed version: {download_url}"
        );

        // A permissionless invoice's PDF is open to its payer, from any origin.
        let payer = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/payments/{payment_id}/attachment"))
                    .header(header::ORIGIN, "https://payday.sh")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(payer.status(), StatusCode::OK);
        assert_eq!(payer.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(payer.headers()[header::CACHE_CONTROL], "no-store");
        let payer = json_body(payer).await;
        assert_eq!(payer["filename"], "contract.pdf");
        assert!(
            payer["download_url"]
                .as_str()
                .unwrap()
                .starts_with("memory://")
        );

        // An invoice without a document has nothing to serve.
        let plain = app
            .clone()
            .oneshot(create_request(KEY, "no-pdf", &valid_body()))
            .await
            .unwrap();
        let plain_id = json_body(plain).await["id"].as_str().unwrap().to_owned();
        for path in [
            format!("/v1/payments/{plain_id}/attachment"),
            format!("/v1/payer/payments/{plain_id}/attachment"),
        ] {
            let response = app.clone().oneshot(get_request(KEY, &path)).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(
                json_body(response).await["error"]["code"],
                "attachment_not_found",
                "{path}"
            );
        }
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn finalize_rejects_non_pdf_oversized_dirty_and_foreign_uploads(pool: PgPool) {
        let TestApp {
            router: app,
            storage,
        } = test_app(pool.clone()).await;
        let oversized = vec![b'%'; 5 * 1024 * 1024 + 1];
        let cases: Vec<(&str, &str, &[u8], &str)> = vec![
            ("wrong content type", "text/plain", PDF, CLEAN_SCAN),
            ("empty", "application/pdf", b"", CLEAN_SCAN),
            ("oversized", "application/pdf", &oversized, CLEAN_SCAN),
            ("dirty", "application/pdf", PDF, "THREATS_FOUND"),
            (
                "not a pdf",
                "application/pdf",
                b"hello, not a pdf",
                CLEAN_SCAN,
            ),
        ];
        for (name, content_type, bytes, verdict) in cases {
            let (id, object_key, _) = reserve_upload(&app, KEY, "upload.pdf").await;
            storage.put(&object_key, content_type, bytes);
            storage.tag(&object_key, SCAN_STATUS_TAG, verdict);
            let (status, body) = finalize(&app, KEY, id).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{name}");
            assert_eq!(body["error"]["code"], "attachment_rejected", "{name}");
            assert!(
                !storage.exists(&object_key),
                "{name}: a rejected upload's object is deleted"
            );
            // The decision is final: a retry does not re-inspect the object.
            storage.put(&object_key, "application/pdf", PDF);
            let (status, body) = finalize(&app, KEY, id).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{name} retry");
            assert_eq!(body["error"]["code"], "attachment_rejected", "{name} retry");
            let mut issue = valid_body();
            issue["attachment_id"] = json!(id);
            let issued = app
                .clone()
                .oneshot(create_request(KEY, name, &issue))
                .await
                .unwrap();
            assert_eq!(issued.status(), StatusCode::CONFLICT, "{name}");
            assert_eq!(
                json_body(issued).await["error"]["code"],
                "attachment_not_ready",
                "{name}"
            );
        }
        let rejected: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM invoice_attachments WHERE status = 'rejected' AND scan_result IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rejected, 5);

        // Another account sees none of it.
        const OTHER: &str = "payday_live_other0123456789abcdef0123456789abcdef";
        other_account(&pool, OTHER).await;
        let (id, object_key, _) = reserve_upload(&app, KEY, "mine.pdf").await;
        storage.put(&object_key, "application/pdf", PDF);
        storage.tag(&object_key, SCAN_STATUS_TAG, CLEAN_SCAN);
        let (status, body) = finalize(&app, OTHER, id).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "attachment_not_found");
        for missing in [Uuid::now_v7().to_string(), "not-a-uuid".into()] {
            let response = app
                .clone()
                .oneshot(
                    Request::post(format!("/v1/attachments/{missing}/finalize"))
                        .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        let (status, _) = finalize(&app, KEY, id).await;
        assert_eq!(status, StatusCode::OK);
        let mut issue = valid_body();
        issue["attachment_id"] = json!(id);
        let created = app
            .clone()
            .oneshot(create_request(KEY, "mine", &issue))
            .await
            .unwrap();
        let payment_id = json_body(created).await["id"].as_str().unwrap().to_owned();
        let foreign = app
            .clone()
            .oneshot(get_request(
                OTHER,
                &format!("/v1/payments/{payment_id}/attachment"),
            ))
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body(foreign).await["error"]["code"],
            "payment_not_found"
        );

        for (name, body) in [
            ("blank", json!({"filename": "  "})),
            ("long", json!({"filename": "x".repeat(256)})),
            ("unknown field", json!({"filename": "a.pdf", "size": 1})),
        ] {
            let response = app
                .clone()
                .oneshot(json_request("POST", KEY, "/v1/attachments", &body))
                .await
                .unwrap();
            assert!(response.status().is_client_error(), "{name}");
            assert_eq!(
                json_body(response).await["error"]["code"],
                "invalid_request",
                "{name}"
            );
        }
    }

    /// A finalized upload, exactly as a client and the scanner leave it in
    /// the bucket, then admitted by the finalize route.
    async fn finalized_upload(app: &Router, storage: &MemoryObjectStorage) -> (Uuid, String) {
        let (id, object_key, _) = reserve_upload(app, KEY, "contract.pdf").await;
        storage.put(&object_key, "application/pdf", PDF);
        storage.tag(&object_key, SCAN_STATUS_TAG, CLEAN_SCAN);
        let (status, _) = finalize(app, KEY, id).await;
        assert_eq!(status, StatusCode::OK);
        (id, object_key)
    }

    async fn attachment_row(pool: &PgPool, id: Uuid) -> (String, Option<String>) {
        sqlx::query_as("SELECT status, scan_result FROM invoice_attachments WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn an_upload_the_bucket_expired_cannot_be_issued_and_says_so(pool: PgPool) {
        let TestApp {
            router: app,
            storage,
        } = test_app(pool.clone()).await;
        const EXPIRED: &str = "The upload expired before it was attached; upload the PDF again";

        // Issuance discovers the expiry while retagging.
        let (id, object_key) = finalized_upload(&app, &storage).await;
        storage.expire(&object_key);
        let mut body = valid_body();
        body["attachment_id"] = json!(id);
        for attempt in ["expired", "expired again"] {
            let response = app
                .clone()
                .oneshot(create_request(KEY, attempt, &body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT, "{attempt}");
            let error = json_body(response).await["error"].clone();
            assert_eq!(error["code"], "attachment_not_ready", "{attempt}");
            assert_eq!(error["message"], EXPIRED, "{attempt}");
        }
        assert_eq!(
            attachment_row(&pool, id).await,
            ("rejected".into(), Some("expired".into()))
        );
        let (status, body) = finalize(&app, KEY, id).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["message"], EXPIRED);

        // Finalize discovers it too, so a client that re-checks before
        // issuing learns to upload again instead of being told 500 later.
        let (id, object_key) = finalized_upload(&app, &storage).await;
        storage.expire(&object_key);
        let (status, body) = finalize(&app, KEY, id).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "attachment_not_ready");
        assert_eq!(body["error"]["message"], EXPIRED);
        assert_eq!(
            attachment_row(&pool, id).await,
            ("rejected".into(), Some("expired".into()))
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invoices")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    /// The retag precedes the issuance transaction. When that transaction
    /// then loses an idempotency race, the object must go back to the
    /// lifecycle rule instead of sitting in the bucket forever, tagged
    /// attached and bound to nothing.
    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_lost_idempotency_race_after_the_retag_restores_the_pending_tag(pool: PgPool) {
        let TestApp {
            router: app,
            storage,
        } = test_app(pool.clone()).await;
        let (id, object_key) = finalized_upload(&app, &storage).await;

        // An invoice issued under another key is the template for the racer.
        let seed = app
            .clone()
            .oneshot(create_request(KEY, "seed", &valid_body()))
            .await
            .unwrap();
        assert_eq!(seed.status(), StatusCode::CREATED);
        let seed_id = json_body(seed).await["id"].as_str().unwrap().to_owned();
        let seed_uuid = Uuid::parse_str(seed_id.strip_prefix("pay_").unwrap()).unwrap();

        // The racer: a row under the key "raced", inserted but not yet
        // committed. The handler's pre-insert lookup cannot see it, so the
        // request passes the ready check, retags the object, and then blocks
        // inside the insert on the uncommitted unique key.
        let mut racer = pool.begin().await.unwrap();
        sqlx::query(
            "CREATE TEMP TABLE racer ON COMMIT DROP AS SELECT * FROM invoices WHERE id = $1",
        )
        .bind(seed_uuid)
        .execute(&mut *racer)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE racer SET id = $1, idempotency_key = 'raced', payment_address = $2, salt = $3",
        )
        .bind(Uuid::now_v7())
        .bind(Address::repeat_byte(0x77).as_slice())
        .bind([0x77u8; 32].as_slice())
        .execute(&mut *racer)
        .await
        .unwrap();
        sqlx::query("INSERT INTO invoices SELECT * FROM racer")
            .execute(&mut *racer)
            .await
            .unwrap();

        let mut body = valid_body();
        body["attachment_id"] = json!(id);
        let request = tokio::spawn(app.clone().oneshot(create_request(KEY, "raced", &body)));
        // Wait until the request is blocked on the racer's lock, so the
        // conflict is decided inside the transaction, after the retag.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let waiting: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting > 0 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the request never reached the insert"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            storage.tags_of(&object_key)["payday-upload"],
            "attached",
            "the retag happened before the insert"
        );
        racer.commit().await.unwrap();

        let response = request.await.unwrap().unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(response).await["error"]["code"],
            "idempotency_conflict"
        );
        assert_eq!(
            storage.tags_of(&object_key)["payday-upload"],
            "pending",
            "the object is handed back to the lifecycle rule"
        );
        assert_eq!(storage.tags_of(&object_key)[SCAN_STATUS_TAG], CLEAN_SCAN);
        assert_eq!(attachment_row(&pool, id).await.0, "ready");
        let bound: Option<Uuid> =
            sqlx::query_scalar("SELECT invoice_id FROM invoice_attachments WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(bound.is_none());
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_replay_against_another_factory_conflicts(pool: PgPool) {
        let app = app(pool.clone()).await;
        let created = app
            .clone()
            .oneshot(create_request(KEY, "factory", &valid_body()))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);

        // The factory is committed into the payment address, so the same
        // request against a redeployed factory is not the same issuance.
        let redeployed = app_with_factory(pool.clone(), Address::repeat_byte(0x0f)).await;
        let response = redeployed
            .oneshot(create_request(KEY, "factory", &valid_body()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(response).await["error"]["code"],
            "idempotency_conflict"
        );
        let replay = app
            .oneshot(create_request(KEY, "factory", &valid_body()))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(replay.headers()["Idempotency-Replayed"], "true");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn merchant_routes_answer_cors_for_the_dashboard_origin_only(pool: PgPool) {
        let app = app(pool.clone()).await;
        let origin = payer_access().origin().to_owned();
        assert_eq!(origin, "http://127.0.0.1:3000");
        let preflight = |origin: &str| {
            Request::options("/v1/payments")
                .header(header::ORIGIN, origin)
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .header(
                    header::ACCESS_CONTROL_REQUEST_HEADERS,
                    "authorization, content-type, idempotency-key",
                )
                .body(Body::empty())
                .unwrap()
        };

        // The preflight carries no credential and is answered before auth.
        let allowed = app.clone().oneshot(preflight(&origin)).await.unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
        let headers = allowed.headers();
        assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], origin);
        let methods = headers[header::ACCESS_CONTROL_ALLOW_METHODS]
            .to_str()
            .unwrap()
            .to_owned();
        for method in ["GET", "POST", "PATCH", "OPTIONS"] {
            assert!(methods.contains(method), "{methods}");
        }
        let allowed_headers = headers[header::ACCESS_CONTROL_ALLOW_HEADERS]
            .to_str()
            .unwrap()
            .to_ascii_lowercase();
        for name in ["authorization", "content-type", "accept", "idempotency-key"] {
            assert!(allowed_headers.contains(name), "{allowed_headers}");
        }
        assert_eq!(headers[header::ACCESS_CONTROL_MAX_AGE], "86400");

        let foreign = app
            .clone()
            .oneshot(preflight("https://evil.example"))
            .await
            .unwrap();
        assert!(
            foreign
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none(),
            "a foreign origin is not allowed"
        );

        // Actual requests echo the exact origin, never a wildcard.
        let get = app
            .clone()
            .oneshot(
                Request::get("/v1/account")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .header(header::ORIGIN, &origin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::OK);
        assert_eq!(get.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], origin);
        let foreign_get = app
            .oneshot(
                Request::get("/v1/account")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .header(header::ORIGIN, "https://evil.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            foreign_get
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn customers_are_created_listed_updated_and_scoped_to_the_account(pool: PgPool) {
        let app = app(pool.clone()).await;
        let post = |body: Value| json_request("POST", KEY, "/v1/customers", &body);

        let created = app
            .clone()
            .oneshot(post(json!({
                "name": "Globex", "email": "ap@globex.example", "details": "Net 30"
            })))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = json_body(created).await;
        let id = created["id"].as_str().unwrap().to_owned();
        assert!(Uuid::parse_str(&id).is_ok());
        assert_eq!(created["name"], "Globex");
        assert_eq!(created["email"], "ap@globex.example");
        assert_eq!(created["details"], "Net 30");
        assert!(created["created_at"].is_string());
        assert_eq!(created["created_at"], created["updated_at"]);

        let second = app
            .clone()
            .oneshot(post(json!({"name": "Initech"})))
            .await
            .unwrap();
        let second = json_body(second).await;
        assert!(second["email"].is_null());
        assert!(second["details"].is_null());
        let second_id = second["id"].as_str().unwrap().to_owned();

        let fetched = app
            .clone()
            .oneshot(get_request(KEY, &format!("/v1/customers/{id}")))
            .await
            .unwrap();
        assert_eq!(fetched.status(), StatusCode::OK);
        assert_eq!(json_body(fetched).await["name"], "Globex");

        // Update replaces every editable field: the omitted email clears.
        let updated = app
            .clone()
            .oneshot(json_request(
                "PATCH",
                KEY,
                &format!("/v1/customers/{id}"),
                &json!({"name": "Globex Corp", "details": "Net 45"}),
            ))
            .await
            .unwrap();
        assert_eq!(updated.status(), StatusCode::OK);
        let updated = json_body(updated).await;
        assert_eq!(updated["name"], "Globex Corp");
        assert!(updated["email"].is_null());
        assert_eq!(updated["details"], "Net 45");
        assert!(updated["updated_at"].as_str() >= created["updated_at"].as_str());

        let page = app
            .clone()
            .oneshot(get_request(KEY, "/v1/customers?limit=1"))
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        let page = json_body(page).await;
        assert_eq!(page["customers"].as_array().unwrap().len(), 1);
        assert_eq!(page["customers"][0]["id"], second_id, "newest first");
        assert_eq!(page["next_cursor"], second_id);
        let rest = app
            .clone()
            .oneshot(get_request(
                KEY,
                &format!("/v1/customers?limit=1&starting_after={second_id}"),
            ))
            .await
            .unwrap();
        let rest = json_body(rest).await;
        assert_eq!(rest["customers"][0]["id"], id);
        assert!(rest["next_cursor"].is_null());

        for query in [
            format!("starting_after={}", Uuid::now_v7()),
            "limit=0".into(),
            "limit=101".into(),
            "sort=name".into(),
        ] {
            let response = app
                .clone()
                .oneshot(get_request(KEY, &format!("/v1/customers?{query}")))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
        }
        for (name, body) in [
            ("blank name", json!({"name": "  "})),
            ("long name", json!({"name": "x".repeat(256)})),
            ("short email", json!({"name": "x", "email": "ab"})),
            (
                "long details",
                json!({"name": "x", "details": "d".repeat(4001)}),
            ),
            ("unknown field", json!({"name": "x", "phone": "1"})),
            // Postgres cannot store U+0000; it is refused before the insert.
            // Details may wrap, a name may not.
            ("nul in name", json!({"name": "Glo\u{0}bex"})),
            (
                "nul in details",
                json!({"name": "Globex", "details": "Net\u{0}30"}),
            ),
            ("line break in name", json!({"name": "Globex\nCorp"})),
        ] {
            let response = app.clone().oneshot(post(body)).await.unwrap();
            assert!(response.status().is_client_error(), "{name}");
            assert_eq!(
                json_body(response).await["error"]["code"],
                "invalid_request",
                "{name}"
            );
        }
        let wrapped = app
            .clone()
            .oneshot(post(
                json!({"name": "Wrapped", "details": "1 Main St\nSpringfield"}),
            ))
            .await
            .unwrap();
        assert_eq!(wrapped.status(), StatusCode::CREATED);

        const OTHER: &str = "payday_live_other0123456789abcdef0123456789abcdef";
        other_account(&pool, OTHER).await;
        let foreign_get = app
            .clone()
            .oneshot(get_request(OTHER, &format!("/v1/customers/{id}")))
            .await
            .unwrap();
        assert_eq!(foreign_get.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body(foreign_get).await["error"]["code"],
            "customer_not_found"
        );
        let foreign_update = app
            .clone()
            .oneshot(json_request(
                "PATCH",
                OTHER,
                &format!("/v1/customers/{id}"),
                &json!({"name": "Taken"}),
            ))
            .await
            .unwrap();
        assert_eq!(foreign_update.status(), StatusCode::NOT_FOUND);
        let foreign_list = app
            .clone()
            .oneshot(get_request(OTHER, "/v1/customers"))
            .await
            .unwrap();
        assert!(
            json_body(foreign_list).await["customers"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let malformed = app
            .clone()
            .oneshot(get_request(KEY, "/v1/customers/not-a-uuid"))
            .await
            .unwrap();
        assert_eq!(malformed.status(), StatusCode::NOT_FOUND);

        // The record is what an invoice links to.
        let mut body = valid_body();
        body["customer_id"] = json!(id);
        let invoice = app
            .clone()
            .oneshot(create_request(KEY, "for-customer", &body))
            .await
            .unwrap();
        assert_eq!(invoice.status(), StatusCode::CREATED);
        assert_eq!(json_body(invoice).await["customer_id"], id);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn invoice_pdf_is_served_as_a_deterministic_download(pool: PgPool) {
        let app = app(pool).await;
        let mut body = valid_body();
        body["heading"] = json!("March retainer");
        body["reference"] = json!("INV-1");
        body["notes"] = json!("Net 30");
        let created = app
            .clone()
            .oneshot(create_request(KEY, "pdf", &body))
            .await
            .unwrap();
        let id = json_body(created).await["id"].as_str().unwrap().to_owned();

        let fetch = || {
            app.clone()
                .oneshot(get_request(KEY, &format!("/v1/payments/{id}/invoice.pdf")))
        };
        let first = fetch().await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(first.headers()[header::CONTENT_TYPE], "application/pdf");
        assert_eq!(
            first.headers()[header::CONTENT_DISPOSITION],
            format!("attachment; filename=\"invoice-{id}.pdf\"").as_str()
        );
        assert_eq!(first.headers()[header::CACHE_CONTROL], "no-store");
        let first = to_bytes(first.into_body(), 1024 * 1024).await.unwrap();
        assert!(first.starts_with(b"%PDF-"));
        let text = String::from_utf8_lossy(&first);
        assert!(text.contains("(INV-1)"));
        assert!(text.contains("(Net 30)"));

        let second = fetch().await.unwrap();
        let second = to_bytes(second.into_body(), 1024 * 1024).await.unwrap();
        assert_eq!(first, second, "byte-identical on every fetch");

        assert_unauthorized(
            app.clone(),
            Request::get(format!("/v1/payments/{id}/invoice.pdf"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn proof_is_served_after_settlement_and_verifies_offline(pool: PgPool) {
        let app = app(pool.clone()).await;
        let created = app
            .clone()
            .oneshot(create_request(KEY, "proof", &valid_body()))
            .await
            .unwrap();
        let created = json_body(created).await;
        let id = created["id"].as_str().unwrap().to_owned();
        let uuid = Uuid::parse_str(id.strip_prefix("pay_").unwrap()).unwrap();
        let address =
            Address::parse_checksummed(created["address"].as_str().unwrap(), None).unwrap();
        let proof_request = |key: &str| get_request(key, &format!("/v1/payments/{id}/proof"));

        let early = app.clone().oneshot(proof_request(KEY)).await.unwrap();
        assert_eq!(early.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(early).await["error"]["code"],
            "payment_not_settled"
        );

        // Settle it the way the indexer would: one credited transfer, then
        // the fulfilment transaction.
        let settlement = [9u8; 32];
        sqlx::query(
            r#"INSERT INTO payment_observations
                 (chain_id, token_address, block_number, block_hash, block_timestamp,
                  transaction_hash, transaction_index, log_index, sender_address,
                  recipient_address, invoice_id, amount, disposition)
               VALUES (1, $1, 3, $2, 1800000000, $3, 0, 0, $4, $5, $6, '1000000', 'credited')"#,
        )
        .bind(Address::ZERO.as_slice())
        .bind([1u8; 32].as_slice())
        .bind([3u8; 32].as_slice())
        .bind(Address::repeat_byte(0xf3).as_slice())
        .bind(address.as_slice())
        .bind(uuid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE invoices SET status = 'fulfilled', confirmed_received = '1000000', settlement_tx_hash = $2, resolved_at_block = 9, settled_at = now() WHERE id = $1",
        )
        .bind(uuid)
        .bind(settlement.as_slice())
        .execute(&pool)
        .await
        .unwrap();

        let served = app.clone().oneshot(proof_request(KEY)).await.unwrap();
        assert_eq!(served.status(), StatusCode::OK);
        let proof: ProofOfPayment = serde_json::from_value(json_body(served).await).unwrap();
        assert_eq!(proof.payment_id, id);
        assert_eq!(proof.payment_address, address.to_checksum(None));
        assert_eq!(proof.verification.payload.result, "not_required");
        assert_eq!(
            proof.verification.payload.payer_policy_mode,
            "permissionless"
        );
        assert!(proof.verification.payload.verified_at.is_none());
        assert_eq!(
            proof.verification.signer,
            attestor().address().to_checksum(None)
        );
        assert_eq!(proof.transfers.len(), 1);
        assert_eq!(proof.transfers[0].amount_base_units, "1000000");
        // The proof names the fulfilment transaction recorded on the row;
        // the transfers are what paid the address.
        assert_eq!(
            proof.settlement_transaction_hash,
            alloy_primitives::B256::from(settlement).to_string()
        );
        assert_eq!(
            proof.transfers[0].transaction_hash,
            alloy_primitives::B256::from([3u8; 32]).to_string()
        );

        let verified = verify_proof(&proof, None, &[attestor().address()])
            .unwrap_or_else(|error| panic!("the served proof must verify offline: {error}"));
        assert_eq!(verified.payment_address, address);
        assert_eq!(verified.attestation_signer, attestor().address());

        let mut tampered = proof.clone();
        tampered.canonical_issuance_snapshot.notes = Some("edited after payment".into());
        assert!(matches!(
            verify_proof(&tampered, None, &[attestor().address()]).unwrap_err(),
            ProofError::AttributionHashMismatch
        ));
        assert!(matches!(
            verify_proof(&proof, None, &[Address::repeat_byte(0x01)]).unwrap_err(),
            ProofError::UntrustedAttestor
        ));

        const OTHER: &str = "payday_live_other0123456789abcdef0123456789abcdef";
        other_account(&pool, OTHER).await;
        let foreign = app.oneshot(proof_request(OTHER)).await.unwrap();
        assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
    }
}
