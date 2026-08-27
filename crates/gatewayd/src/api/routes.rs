use axum::routing::{get, post};
use axum::{Router, middleware};
use tower_http::limit::RequestBodyLimitLayer;

use crate::api::accounts;
use crate::api::auth;
use crate::api::health;
use crate::api::invoices;
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    let authenticated = Router::new()
        .route("/v1/invoices", post(invoices::create_invoice))
        .route("/v1/invoices/{id}", get(invoices::get_invoice))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_api_key,
        ))
        .layer(RequestBodyLimitLayer::new(64 * 1024));

    let account_management = Router::new()
        .route(
            "/v1/account/api-key",
            post(accounts::issue).get(accounts::metadata),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_identity,
        ))
        .layer(RequestBodyLimitLayer::new(16 * 1024));

    Router::new()
        .route("/health", get(health::health))
        .merge(authenticated)
        .merge(account_management)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use alloy_primitives::Address;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use gateway_core::ChainId;
    use gateway_db::{AccountRepository, InvoiceRepository};
    use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, encode};
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    use serde::Serialize;
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
        );
        router(state)
    }

    fn identity_verifier_and_tokens() -> (auth::Auth0Verifier, String, String) {
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
        let authenticated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
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
        (verifier, token("event-1"), token("event-2"))
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn health_is_public(pool: PgPool) {
        let response = app(pool)
            .await
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    async fn assert_unauthorized(app: Router, request: Request<Body>) {
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Bearer");
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
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
            Request::get("/v1/invoices/not-an-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_unauthorized(
            app(pool).await,
            Request::get("/v1/invoices/not-an-id")
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
            Request::post("/v1/invoices")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
        assert_unauthorized(
            app(pool).await,
            Request::post("/v1/invoices")
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
                Request::get("/v1/invoices/not-an-id")
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
                Request::post("/v1/invoices")
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
        let body = serde_json::json!({
            "chain_id": "1",
            "token_address": "0x0000000000000000000000000000000000000000",
            "beneficiary_address": "0x0000000000000000000000000000000000000002",
            "amount": "1",
            "expiration_timestamp": "1900000000",
            "recovery_address": "0x0000000000000000000000000000000000000003"
        })
        .to_string();

        let create = |key: &'static str| {
            Request::post("/v1/invoices")
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "same-key")
                .body(Body::from(body.clone()))
                .unwrap()
        };
        let first = app.clone().oneshot(create(FIRST)).await.unwrap();
        let second = app.clone().oneshot(create(SECOND)).await.unwrap();
        assert_eq!(first.status(), StatusCode::CREATED);
        assert_eq!(second.status(), StatusCode::CREATED);
        let first_body = to_bytes(first.into_body(), 16 * 1024).await.unwrap();
        let second_body = to_bytes(second.into_body(), 16 * 1024).await.unwrap();
        let first_id = serde_json::from_slice::<serde_json::Value>(&first_body).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let second_id = serde_json::from_slice::<serde_json::Value>(&second_body).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_ne!(first_id, second_id);

        let cross_account = app
            .oneshot(
                Request::get(format!("/v1/invoices/{first_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {SECOND}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cross_account.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn authenticated_user_can_create_and_replace_one_key(pool: PgPool) {
        let (verifier, token, replacement_token) = identity_verifier_and_tokens();
        let accounts = AccountRepository::new(pool.clone());
        let state = AppState::new(
            InvoiceRepository::new(pool),
            accounts,
            Some(verifier),
            ChainId(1),
            Address::ZERO,
            Address::ZERO,
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
        let provisioned_body = to_bytes(provisioned.into_body(), 4096).await.unwrap();
        let provisioned_json =
            serde_json::from_slice::<serde_json::Value>(&provisioned_body).unwrap();
        assert_eq!(provisioned_json["generation"], 1);
        assert_eq!(provisioned_json["replaced_previous_key"], false);
        let first_key =
            serde_json::from_slice::<serde_json::Value>(&provisioned_body).unwrap()["api_key"]
                .as_str()
                .unwrap()
                .to_string();

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
        let duplicate_body = to_bytes(duplicate.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&duplicate_body).unwrap()["error"]["code"],
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
        let rotated_body = to_bytes(replaced.into_body(), 4096).await.unwrap();
        let replaced_json = serde_json::from_slice::<serde_json::Value>(&rotated_body).unwrap();
        assert_eq!(replaced_json["generation"], 2);
        assert_eq!(replaced_json["replaced_previous_key"], true);
        let second_key =
            serde_json::from_slice::<serde_json::Value>(&rotated_body).unwrap()["api_key"]
                .as_str()
                .unwrap()
                .to_string();
        assert_ne!(first_key, second_key);

        let invoice_request = |key: &str| {
            Request::get("/v1/invoices/not-an-id")
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
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.oneshot(invoice_request(&second_key))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
}
