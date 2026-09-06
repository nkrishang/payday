//! Pregenerating a merchant's embedded wallet before they sign in
//! (`pregenerated_wallet.rs`).
//!
//! ```text
//! POST /v1/wallets/pregenerate
//! ```
//!
//! Called by the sign-up dialog the moment a merchant submits their email, in
//! parallel with Privy sending the code. Fire-and-forget from the browser's
//! side: nothing here is surfaced to a merchant, and sign-in does not depend
//! on it — its own explicit wallet creation is the correctness guarantee.

use axum::extract::State;
use axum::http::StatusCode;
use gateway_core::valid_email;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::json::Json;
use crate::pregenerated_wallet::PregenerateError;
use crate::state::AppState;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PregenerateRequest {
    pub email: String,
}

pub async fn pregenerate(
    State(state): State<AppState>,
    Json(request): Json<PregenerateRequest>,
) -> Result<StatusCode, ApiError> {
    let email = request.email.trim().to_lowercase();
    if !valid_email(&email) {
        return Err(ApiError::invalid_request("email must be a valid address"));
    }
    let pregenerator = state.pregenerate_wallet_for(&email)?;
    if let Err(PregenerateError::Unavailable(reason)) = pregenerator.pregenerate(&email).await {
        tracing::warn!(reason, "wallet pregeneration failed");
        return Err(ApiError::identity_provider_unavailable());
    }
    Ok(StatusCode::NO_CONTENT)
}
