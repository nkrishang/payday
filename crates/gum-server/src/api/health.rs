use axum::extract::State;
use axum::http::StatusCode;

use crate::state::AppState;

/// Liveness for the load balancer. A database round-trip is part of the check
/// so a task that cannot serve invoice requests is taken out of rotation
/// instead of reporting healthy while every request fails.
pub async fn health(State(state): State<AppState>) -> (StatusCode, &'static str) {
    match state.repo.ping().await {
        Ok(()) => (StatusCode::OK, "ok"),
        Err(error) => {
            tracing::error!(error = ?error, "health check failed to reach the database");
            (StatusCode::SERVICE_UNAVAILABLE, "database unavailable")
        }
    }
}
