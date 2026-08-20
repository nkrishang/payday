use axum::Router;
use axum::routing::get;

use crate::api::health;

pub fn router() -> Router {
    Router::new().route("/health", get(health::health))
}
