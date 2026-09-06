//! The merchant's view of a payer's verification (product plan §7.2).
//!
//! ```text
//! GET /v1/deposit-requests/{id}/verification
//! ```
//!
//! Each fact the policy needs on its own, and every attempt made against the
//! invoice. Nothing here names the payer's session or the code they typed.

use axum::Extension;
use axum::extract::{Path, State};
use gateway_core::{
    PayerPolicyMode, VerificationAttemptId, VerificationAttemptResponse,
    VerificationDetailResponse, VerificationFacts, VerificationRequirementsResponse, rfc3339,
};
use gateway_db::{AccountId, DbInvoice};

use crate::api::deposit_requests::resolve_deposit_request;
use crate::api::error::ApiError;
use crate::api::json::Json;
use crate::state::AppState;

async fn detail(
    state: &AppState,
    account: AccountId,
    row: &DbInvoice,
) -> Result<VerificationDetailResponse, ApiError> {
    let mode: PayerPolicyMode = row
        .payer_policy_mode
        .parse()
        .map_err(|_| ApiError::internal("invalid payer policy mode in stored invoice"))?;
    let attempts = state
        .payer_sessions
        .attempts_for_invoice(account.0, row.id)
        .await?;
    let wallet = row.payment_address.is_some();
    let facts = if row.verification_completed_at.is_some() {
        VerificationRequirementsResponse::for_mode(mode, true, wallet)
    } else {
        let approved = |kind: &str| {
            attempts
                .iter()
                .any(|attempt| attempt.kind == kind && attempt.status == "approved")
        };
        VerificationRequirementsResponse::from_facts(
            mode,
            VerificationFacts {
                email: approved("email"),
                wallet,
                merchant_session: approved("merchant_session"),
            },
        )
    };
    Ok(VerificationDetailResponse {
        payer_policy_mode: mode,
        verification_completed_at: row.verification_completed_at.map(rfc3339),
        likely_unsolicited_at: row.likely_unsolicited_at.map(rfc3339),
        facts,
        attempts: attempts
            .iter()
            .map(|attempt| VerificationAttemptResponse {
                id: VerificationAttemptId(attempt.id).to_string(),
                kind: attempt.kind.clone(),
                status: attempt.status.clone(),
                verified_at: attempt.verified_at.map(rfc3339),
                created_at: rfc3339(attempt.created_at),
            })
            .collect(),
    })
}

pub async fn merchant_detail(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(reference): Path<String>,
) -> Result<Json<VerificationDetailResponse>, ApiError> {
    let row = resolve_deposit_request(&state, account, &reference).await?;
    Ok(Json(detail(&state, account, &row).await?))
}
