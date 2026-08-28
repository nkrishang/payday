use axum::routing::{delete, get, post};
use axum::{Router, middleware};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;
use tracing::field;

use crate::api::accounts;
use crate::api::auth;
use crate::api::health;
use crate::api::invoices;
use crate::api::status;
use crate::api::webhooks;
use crate::api::{middleware as api_middleware, openapi};
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    let authenticated = Router::new()
        .route("/v1/account", get(accounts::get_account))
        .route(
            "/v1/payments",
            post(invoices::create_payment).get(invoices::list_payments),
        )
        .route("/v1/payments/{id}", get(invoices::get_payment))
        .route("/v1/payments/{id}/cancel", post(invoices::cancel_payment))
        .route("/v1/payments/{id}/transfers", get(invoices::transfers))
        .route("/v1/status", get(status::get))
        .route("/v1/webhooks", post(webhooks::add).get(webhooks::list))
        .route("/v1/webhooks/{id}", delete(webhooks::remove))
        .route("/v1/webhooks/{id}/test", post(webhooks::test))
        .route("/v1/webhook-deliveries", get(webhooks::deliveries))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_api_key,
        ))
        .layer(RequestBodyLimitLayer::new(64 * 1024));

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

    Router::new()
        .route("/health", get(health::health))
        .route("/openapi.json", get(openapi::spec))
        .route("/docs", get(openapi::reference))
        .route("/api", get(openapi::reference))
        .route("/api/openapi.json", get(openapi::spec))
        .merge(authenticated)
        .merge(account_management)
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

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use alloy_primitives::Address;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use gateway_core::ChainId;
    use gateway_db::{AccountRepository, InvoiceRepository};
    use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, encode};
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    use serde::Serialize;
    use serde_json::{Value, json};
    use sqlx::PgPool;
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

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
    }

    async fn app(pool: PgPool) -> Router {
        let accounts = AccountRepository::new(pool.clone());
        if accounts
            .find_by_identity("https://test.issuer/", "email|test-user")
            .await
            .unwrap()
            .is_none()
        {
            let _ = accounts
                .issue_api_key(
                    "https://test.issuer/",
                    "email|test-user",
                    None,
                    &Uuid::now_v7().to_string(),
                    KEY,
                )
                .await
                .unwrap();
        }
        let state = AppState::new(
            InvoiceRepository::new(pool),
            accounts,
            None,
            ChainId(1),
            Address::ZERO,
            Address::ZERO,
            "payday_live_".into(),
            None,
        );
        router(state)
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
            "refund_address": "0x0000000000000000000000000000000000000003"
        })
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
                    "message": "A valid bearer API key is required"
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
                    "Bearer fedcba9876543210fedcba9876543210",
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
                    "Bearer fedcba9876543210fedcba9876543210",
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

        let prefix_response = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payments/{}", &id[..16]))
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(prefix_response.status(), StatusCode::OK);
        assert_eq!(json_body(prefix_response).await["id"], id);

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
        let ambiguous = app
            .clone()
            .oneshot(
                Request::get("/v1/payments/pay_0")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ambiguous.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(ambiguous).await["error"]["code"],
            "ambiguous_payment_id"
        );

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
    async fn address_lookup_cancel_and_bounded_list_shape_work(pool: PgPool) {
        let app = app(pool).await;
        let body = json!({
            "payout_address": "0x0000000000000000000000000000000000000002",
            "amount": "1",
            "memo": "Order 1234"
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
        assert_eq!(created["refund_address"], body["payout_address"]);
        assert_eq!(created["memo"], "Order 1234");
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
        const FIRST: &str = "first-account-0123456789abcdef0123456789abcdef";
        const SECOND: &str = "second-account-0123456789abcdef0123456789abcdef";
        let accounts = AccountRepository::new(pool.clone());
        accounts
            .issue_api_key(
                "https://issuer.example/",
                "email|first",
                None,
                "first-event",
                FIRST,
            )
            .await
            .unwrap();
        accounts
            .issue_api_key(
                "https://issuer.example/",
                "email|second",
                None,
                "second-event",
                SECOND,
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
        let state = AppState::new(
            InvoiceRepository::new(pool),
            accounts,
            Some(verifier),
            ChainId(1),
            Address::ZERO,
            Address::ZERO,
            "payday_live_".into(),
            None,
        );
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
}
