use crate::{
    api::{auth::Reviewer, error::ApiError, json::Json},
    state::AppState,
};
use axum::{
    Extension,
    extract::{Path, State},
};
use gateway_core::{DepositRequestResponse, Invoice};
use gateway_db::ReleasePaymentError;
use uuid::Uuid;

pub async fn release(
    State(state): State<AppState>,
    Extension(reviewer): Extension<Reviewer>,
    Path(reference): Path<String>,
) -> Result<Json<DepositRequestResponse>, ApiError> {
    let id = reference
        .strip_prefix("dr_")
        .unwrap_or(&reference)
        .parse::<Uuid>()
        .map_err(|_| ApiError::invalid_request("invalid deposit request ID"))?;
    let row = state
        .repo
        .release_blocked(id)
        .await
        .map_err(|error| match error {
            ReleasePaymentError::NotFound => ApiError::deposit_request_not_found(),
            ReleasePaymentError::NotBlocked => ApiError::deposit_request_not_blocked(),
            ReleasePaymentError::Database(error) => ApiError::from(error),
        })?;
    tracing::info!(payment_id = %id, status = %row.status, reviewer = %reviewer.0, "deposit request released by operator");
    Ok(Json(DepositRequestResponse::from_invoice(
        Invoice::try_from(&row)?,
        None,
    )))
}
