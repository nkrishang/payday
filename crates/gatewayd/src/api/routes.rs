use axum::routing::{get, post};
use axum::Router;

use crate::api::health;
use crate::api::invoices;
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health::health))
        .route("/v1/invoices", post(invoices::create_invoice))
        .route("/v1/invoices/{id}", get(invoices::get_invoice))
        .with_state(state)
}
