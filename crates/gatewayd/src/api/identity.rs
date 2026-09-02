//! Identity verification: the payer's hosted session, the provider's
//! callback, and the merchant and operator views of the outcome (product
//! plan §4.6, §6.2, §6.3, §7.1).
//!
//! ```text
//! POST /v1/payer/payments/{id}/verify/identity/start   payer, hosted checkout only
//! POST /v1/webhooks/identity                            provider callback
//! GET  /v1/payments/{id}/verification                   merchant
//! POST /v1/payments/{id}/verification/review            merchant
//! POST /v1/admin/verifications/{id}/decision            operator
//! ```
//!
//! Nothing here logs a callback body, a decision, or anything the provider
//! extracted; the types cannot carry the latter and the handlers never read
//! the former beyond its two identifiers.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use chrono::{DateTime, SecondsFormat, Utc};
use gateway_core::{
    PayerPolicyMode, VerificationAttemptResponse, VerificationDetailResponse,
    VerificationFactStatus, VerificationFacts, VerificationRequirementsResponse,
    VerificationReviewResponse, expected_identity_hash,
};
use gateway_db::{
    AccountId, CredentialKind, DbInvoice, DbPayerSession, DbVerificationAttempt,
    DbVerificationReview, IdentityState, IdentityStatus, ReviewDecision as DbReviewDecision,
    ReviewError, StartIdentityError,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::attachments::no_store;
use crate::api::auth::Reviewer;
use crate::api::error::ApiError;
use crate::api::invoices::resolve_payment;
use crate::api::payer_verification::{gated_invoice, session_token};
use crate::identity::didit::PROVIDER;
use crate::identity::{IdentityProviderError, IdentitySessionRequest};
use crate::state::AppState;

#[derive(Serialize)]
pub struct StartIdentityResponse {
    pub outcome: IdentityStartOutcome,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IdentityStartOutcome {
    /// An earlier credential of this merchant's satisfied the policy; the
    /// session is unlocked without a hosted flow.
    Reused,
    /// Send the payer to the provider's hosted session.
    Redirect { url: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewDecisionRequest {
    pub decision: ReviewDecision,
    pub note: Option<String>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approve,
    Decline,
}

const MAX_REVIEW_NOTE_LEN: usize = 2000;

fn rfc3339(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn attempt_response(
    attempt: &DbVerificationAttempt,
    reviews: &[DbVerificationReview],
) -> VerificationAttemptResponse {
    VerificationAttemptResponse {
        id: attempt.id.to_string(),
        kind: attempt.kind.clone(),
        status: attempt.status.clone(),
        provider: attempt.provider.clone(),
        provider_reference: attempt.provider_reference.clone(),
        attempt_number: attempt.attempt_number.max(0) as u16,
        document: DbVerificationAttempt::fact_status(attempt.document_status.as_deref()),
        liveness: DbVerificationAttempt::fact_status(attempt.liveness_status.as_deref()),
        identity_match: DbVerificationAttempt::fact_status(
            attempt.identity_match_status.as_deref(),
        ),
        risk_codes: attempt.risk_codes.clone(),
        country_code: attempt.country_code.clone(),
        verified_at: attempt.verified_at.map(rfc3339),
        expires_at: attempt.expires_at.map(rfc3339),
        created_at: rfc3339(attempt.created_at),
        review: reviews.last().map(|review| VerificationReviewResponse {
            requested_at: rfc3339(review.requested_at),
            decision: review.decision.clone(),
            reviewer: review.reviewer.clone(),
            note: review.note.clone(),
            decided_at: review.decided_at.map(rfc3339),
        }),
    }
}

/// The merchant's view: every attempt, each fact on its own, and what may
/// happen next. Never the payer's assertions beyond what the policy holds.
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
        .verifications
        .attempts_for_invoice(account.0, row.id)
        .await?;
    let latest_identity = attempts
        .iter()
        .rev()
        .find(|(attempt, _)| attempt.kind == "identity")
        .map(|(attempt, _)| attempt);
    let facts = if row.verification_completed_at.is_some() {
        VerificationRequirementsResponse::for_mode(mode, true)
    } else {
        let email = attempts
            .iter()
            .any(|(attempt, _)| attempt.kind == "email" && attempt.status == "approved");
        let established = |value: Option<&str>| {
            DbVerificationAttempt::fact_status(value) == VerificationFactStatus::Approved
        };
        let mut requirements = VerificationRequirementsResponse::from_facts(
            mode,
            VerificationFacts {
                email,
                document: latest_identity
                    .is_some_and(|attempt| established(attempt.document_status.as_deref())),
                liveness: latest_identity
                    .is_some_and(|attempt| established(attempt.liveness_status.as_deref())),
                identity_match: latest_identity
                    .is_some_and(|attempt| established(attempt.identity_match_status.as_deref())),
            },
        );
        if let Some(attempt) = latest_identity {
            refine_declined(&mut requirements, attempt);
        }
        requirements
    };
    let review_available = latest_identity.is_some_and(|attempt| {
        matches!(
            attempt.status(),
            IdentityStatus::Declined | IdentityStatus::ReviewRequired
        )
    });
    let reviewed = attempts.iter().any(|(_, reviews)| !reviews.is_empty());
    let retry_available = row.verification_completed_at.is_none()
        && !reviewed
        && latest_identity.is_some_and(|attempt| {
            matches!(
                attempt.status(),
                IdentityStatus::Declined | IdentityStatus::Expired | IdentityStatus::Abandoned
            )
        });
    Ok(VerificationDetailResponse {
        payer_policy_mode: mode,
        verification_completed_at: row.verification_completed_at.map(rfc3339),
        likely_unsolicited_at: row.likely_unsolicited_at.map(rfc3339),
        facts,
        attempts: attempts
            .iter()
            .map(|(attempt, reviews)| attempt_response(attempt, reviews))
            .collect(),
        review_available,
        retry_available,
    })
}

/// A fact the latest attempt declined reads as declined rather than
/// pending, so both the payer and the merchant see why the invoice is stuck.
pub fn refine_declined(
    requirements: &mut VerificationRequirementsResponse,
    attempt: &DbVerificationAttempt,
) {
    if !matches!(
        attempt.status(),
        IdentityStatus::Declined | IdentityStatus::ReviewRequired
    ) {
        return;
    }
    let refine = |current: &mut VerificationFactStatus, stored: Option<&str>| {
        if *current == VerificationFactStatus::Pending
            && DbVerificationAttempt::fact_status(stored) == VerificationFactStatus::Declined
        {
            *current = VerificationFactStatus::Declined;
        }
    };
    refine(
        &mut requirements.document,
        attempt.document_status.as_deref(),
    );
    refine(
        &mut requirements.liveness,
        attempt.liveness_status.as_deref(),
    );
    refine(
        &mut requirements.identity_match,
        attempt.identity_match_status.as_deref(),
    );
}

/// Whether a session may start a hosted identity flow now: an identity
/// policy, a proven mailbox, an incomplete session, a configured provider,
/// and no attempt that automation has stopped on.
pub fn identity_start_available(
    state: &AppState,
    mode: PayerPolicyMode,
    session: &DbPayerSession,
    identity: &IdentityState,
) -> bool {
    let facts = session.facts();
    matches!(
        mode,
        PayerPolicyMode::VerifiedIdentity | PayerPolicyMode::VerifiedIdentityUnattributed
    ) && facts.email
        && !facts.satisfy(mode)
        && state.identity.is_some()
        && identity.start_available()
}

pub async fn start(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Json<StartIdentityResponse>), ApiError> {
    let provider = state
        .identity
        .as_ref()
        .ok_or_else(ApiError::identity_verification_unavailable)?;
    let (row, invoice) = gated_invoice(&state, &id).await?;
    if !matches!(
        invoice.status,
        gateway_core::InvoiceStatus::Created
            | gateway_core::InvoiceStatus::Funded
            | gateway_core::InvoiceStatus::Deploying
    ) {
        return Err(ApiError::payment_not_payable());
    }
    let policy = &invoice.issuance_snapshot.payer_policy;
    let mode = policy.mode();
    if !matches!(
        mode,
        PayerPolicyMode::VerifiedIdentity | PayerPolicyMode::VerifiedIdentityUnattributed
    ) {
        return Err(ApiError::verification_not_required());
    }
    let token = session_token(&headers).ok_or_else(ApiError::payer_session_invalid)?;
    let session = state
        .payer_sessions
        .find_active(token, row.id)
        .await?
        .ok_or_else(ApiError::payer_session_invalid)?;
    if !session.facts().email {
        return Err(ApiError::email_verification_required());
    }
    let payer_ref = session
        .payer_ref
        .as_deref()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .map(alloy_primitives::B256::from)
        .ok_or_else(ApiError::email_verification_required)?;
    if session.satisfies(mode) {
        return Ok((
            no_store(),
            Json(StartIdentityResponse {
                outcome: IdentityStartOutcome::Reused,
            }),
        ));
    }
    let expected = policy.expected_identity().cloned();
    let hash = expected.as_ref().map(expected_identity_hash);

    // Reuse before anything else: the same merchant's earlier credential,
    // generic for unattributed mode, bound to this exact assertion for
    // matched mode.
    let (kind, binding) = match mode {
        PayerPolicyMode::VerifiedIdentity => (CredentialKind::MatchedIdentity, hash),
        _ => (CredentialKind::DocumentLiveness, None),
    };
    if let Some(credential) = state
        .verifications
        .find_active_credential(row.account_id, payer_ref, kind, binding)
        .await?
    {
        let completion = state
            .verifications
            .reuse_credential(session.id, &credential, PROVIDER)
            .await?;
        if completion.invoice_completed_at.is_some() {
            tracing::info!(invoice_id = %row.id, "payer verification completed by credential reuse");
        }
        return Ok((
            no_store(),
            Json(StartIdentityResponse {
                outcome: IdentityStartOutcome::Reused,
            }),
        ));
    }

    let attempt = match state
        .verifications
        .begin_identity_verification(session.id, payer_ref, hash, PROVIDER)
        .await
    {
        Ok(attempt) => attempt,
        Err(StartIdentityError::ReviewRequired) => return Err(ApiError::review_required()),
        Err(StartIdentityError::InReview) => return Err(ApiError::identity_in_review()),
        Err(StartIdentityError::Database(error)) => return Err(error.into()),
    };
    let created = provider
        .create_session(IdentitySessionRequest {
            verification_id: attempt.id,
            payer_ref,
            expected_identity: expected,
            callback_url: state.payer.checkout_url(&invoice.id),
        })
        .await;
    let created = match created {
        Ok(created) => created,
        Err(error) => {
            state.verifications.abandon(attempt.id).await?;
            let reason = match &error {
                IdentityProviderError::Unavailable(reason)
                | IdentityProviderError::Rejected(reason) => reason.as_str(),
                IdentityProviderError::InvalidCallback(reason) => reason,
            };
            tracing::warn!(reason, "identity session could not be created");
            return Err(ApiError::identity_provider_unavailable());
        }
    };
    state
        .verifications
        .record_provider_session(attempt.id, &created.provider_reference, created.status)
        .await?;
    tracing::info!(invoice_id = %row.id, verification_id = %attempt.id, "identity session started");
    Ok((
        no_store(),
        Json(StartIdentityResponse {
            outcome: IdentityStartOutcome::Redirect {
                url: created.hosted_url,
            },
        }),
    ))
}

/// The provider's callback. Verified, deduplicated, and turned into a poll:
/// the decision itself is fetched by the reconciler, never here.
pub async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let provider = state
        .identity
        .as_ref()
        .ok_or_else(ApiError::identity_verification_unavailable)?;
    let callback = provider
        .verify_callback(&headers, &body)
        .map_err(|error| match error {
            IdentityProviderError::InvalidCallback(reason) => {
                tracing::warn!(reason, "identity callback rejected");
                ApiError::webhook_signature_invalid()
            }
            _ => ApiError::webhook_signature_invalid(),
        })?;
    let new = state
        .verifications
        .record_webhook(PROVIDER, &callback.event_id, &callback.provider_reference)
        .await?;
    if new {
        tracing::info!(event_id = %callback.event_id, "identity callback received");
    }
    Ok(StatusCode::ACCEPTED)
}

pub async fn merchant_detail(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(reference): Path<String>,
) -> Result<Json<VerificationDetailResponse>, ApiError> {
    let row = resolve_payment(&state, account, &reference).await?;
    Ok(Json(detail(&state, account, &row).await?))
}

pub async fn request_review(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(reference): Path<String>,
) -> Result<Json<VerificationDetailResponse>, ApiError> {
    let row = resolve_payment(&state, account, &reference).await?;
    match state.verifications.request_review(account.0, row.id).await {
        Ok(attempt) => {
            tracing::info!(invoice_id = %row.id, verification_id = %attempt.id, "verification review requested");
        }
        Err(ReviewError::NotAvailable) | Err(ReviewError::NotFound) => {
            return Err(ApiError::review_not_available());
        }
        Err(ReviewError::Database(error)) => return Err(error.into()),
    }
    Ok(Json(detail(&state, account, &row).await?))
}

pub async fn admin_decision(
    State(state): State<AppState>,
    Extension(reviewer): Extension<Reviewer>,
    Path(id): Path<String>,
    Json(request): Json<ReviewDecisionRequest>,
) -> Result<Json<VerificationAttemptResponse>, ApiError> {
    let id: Uuid = id
        .parse()
        .map_err(|_| ApiError::invalid_request("invalid verification ID"))?;
    let note = request
        .note
        .as_deref()
        .map(str::trim)
        .filter(|note| !note.is_empty());
    if note.is_some_and(|note| note.len() > MAX_REVIEW_NOTE_LEN) {
        return Err(ApiError::invalid_request(format!(
            "note must be at most {MAX_REVIEW_NOTE_LEN} bytes"
        )));
    }
    let decision = match request.decision {
        ReviewDecision::Approve => DbReviewDecision::Approve,
        ReviewDecision::Decline => DbReviewDecision::Decline,
    };
    let (attempt, review) = state
        .verifications
        .decide_review(id, &reviewer.0, decision, note)
        .await
        .map_err(|error| match error {
            ReviewError::NotFound => ApiError::verification_not_found(),
            ReviewError::NotAvailable => ApiError::review_not_available(),
            ReviewError::Database(error) => error.into(),
        })?;
    tracing::info!(
        verification_id = %attempt.id,
        invoice_id = %attempt.invoice_id,
        decision = attempt.status.as_str(),
        "verification decided by reviewer"
    );
    Ok(Json(attempt_response(
        &attempt,
        std::slice::from_ref(&review),
    )))
}
