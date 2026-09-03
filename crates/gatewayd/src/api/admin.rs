use crate::{
    api::{auth::Reviewer, error::ApiError},
    state::AppState,
};
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use gateway_core::{Invoice, PaymentResponse};
use gateway_db::ReleasePaymentError;
use uuid::Uuid;

pub async fn release(
    State(state): State<AppState>,
    Extension(reviewer): Extension<Reviewer>,
    Path(reference): Path<String>,
) -> Result<Json<PaymentResponse>, ApiError> {
    let id = reference
        .strip_prefix("pay_")
        .unwrap_or(&reference)
        .parse::<Uuid>()
        .map_err(|_| ApiError::invalid_request("invalid payment ID"))?;
    let row = state
        .repo
        .release_blocked(id)
        .await
        .map_err(|error| match error {
            ReleasePaymentError::NotFound => ApiError::payment_not_found(),
            ReleasePaymentError::NotBlocked => ApiError::invoice_not_blocked(),
            ReleasePaymentError::Database(error) => ApiError::from(error),
        })?;
    tracing::info!(payment_id = %id, status = %row.status, reviewer = %reviewer.0, "payment released by operator");
    Ok(Json(PaymentResponse::from_invoice(
        Invoice::try_from(&row)?,
        None,
    )))
}
