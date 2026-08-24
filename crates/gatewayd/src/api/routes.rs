use axum::routing::{get, post};
use axum::{Router, middleware};

use crate::api::auth::{self, ApiKey};
use crate::api::health;
use crate::api::invoices;
use crate::state::AppState;

pub fn router(state: AppState, api_key: ApiKey) -> Router {
    let authenticated = Router::new()
        .route("/v1/invoices", post(invoices::create_invoice))
        .route("/v1/invoices/{id}", get(invoices::get_invoice))
        .route_layer(middleware::from_fn_with_state(
            api_key,
            auth::require_api_key,
        ));

    Router::new()
        .route("/health", get(health::health))
        .merge(authenticated)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use alloy_primitives::Address;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use gateway_core::ChainId;
    use gateway_db::InvoiceRepository;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    fn app() -> Router {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgresql://localhost/gateway")
            .unwrap();
        let state = AppState::new(
            InvoiceRepository::new(pool),
            ChainId(1),
            Address::ZERO,
            Address::ZERO,
        );
        router(state, ApiKey::new(KEY).unwrap())
    }

    #[tokio::test]
    async fn health_is_public() {
        let response = app()
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    async fn assert_unauthorized(request: Request<Body>) {
        let response = app().oneshot(request).await.unwrap();

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

    #[tokio::test]
    async fn get_invoice_rejects_missing_and_wrong_credentials() {
        assert_unauthorized(
            Request::get("/v1/invoices/not-an-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_unauthorized(
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

    #[tokio::test]
    async fn create_invoice_rejects_missing_and_wrong_credentials() {
        assert_unauthorized(
            Request::post("/v1/invoices")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
        assert_unauthorized(
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

    #[tokio::test]
    async fn invoice_routes_accept_the_configured_credential() {
        let response = app()
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
}
