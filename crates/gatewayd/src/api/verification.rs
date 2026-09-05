//! The merchant's view of a payer's verification (product plan §7.2).
//!
//! ```text
//! GET /v1/payments/{id}/verification
//! ```
//!
//! Each fact the policy needs on its own, and every attempt made against the
//! invoice. Nothing here names the payer's session or the code they typed.

use axum::extract::{Path, State};
use axum::{Extension, Json};
use chrono::{DateTime, SecondsFormat, Utc};
use gateway_core::{
    PayerPolicyMode, VerificationAttemptResponse, VerificationDetailResponse, VerificationFacts,
    VerificationRequirementsResponse,
};
use gateway_db::{AccountId, DbInvoice};

use crate::api::error::ApiError;
use crate::api::invoices::resolve_payment;
use crate::state::AppState;

fn rfc3339(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

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
    let facts = if row.verification_completed_at.is_some() {
        VerificationRequirementsResponse::for_mode(mode, true)
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
                id: attempt.id.to_string(),
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
    let row = resolve_payment(&state, account, &reference).await?;
    Ok(Json(detail(&state, account, &row).await?))
}
