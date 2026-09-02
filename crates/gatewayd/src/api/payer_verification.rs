//! Payer email verification (product plan §4.6, §7.1).
//!
//! ```text
//! POST /v1/payer/payments/{id}/verify/email/start
//! POST /v1/payer/payments/{id}/verify/email/confirm
//! GET  /v1/payer/payments/{id}/verify
//! ```
//!
//! The payer never supplies an email: the gateway sends the code to the
//! address the merchant asserted at issuance. What comes back is an opaque
//! payer session in `Payday-Payer-Session`, which the public reads honour.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use chrono::{SecondsFormat, Utc};
use gateway_core::{Invoice, InvoiceStatus, PayerPolicyMode, VerificationRequirementsResponse};
use gateway_db::{DbInvoice, DbPayerSession, PAYER_SESSION_TTL, StartEmailVerificationError};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::attachments::no_store;
use crate::api::error::ApiError;
use crate::api::payer::parse_invoice_id;
use crate::state::AppState;

/// The bearer header carrying an opaque payer session.
pub const PAYER_SESSION_HEADER: &str = "payday-payer-session";
/// One code per invoice per minute, whichever session asks: the merchant's
/// customer must not be flooded by anyone holding the link.
const RESEND_COOLDOWN: Duration = Duration::from_secs(60);
const MAX_SESSION_TOKEN_LEN: usize = 128;
const MAX_OTP_LEN: usize = 16;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmEmailRequest {
    pub otp: String,
}

#[derive(Serialize)]
pub struct StartEmailResponse {
    pub payer_session: String,
    pub expires_at: String,
}

#[derive(Serialize)]
pub struct VerificationStatusResponse {
    pub requirements: VerificationRequirementsResponse,
    /// Whether the payer may start the identity step now. The identity
    /// routes arrive in Slice 4; until then nothing is startable.
    pub identity_start_available: bool,
}

/// The session token a request carries, if any. Tokens are opaque; only
/// their length is bounded here, the lookup decides everything else.
pub fn session_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(PAYER_SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|token| !token.is_empty() && token.len() <= MAX_SESSION_TOKEN_LEN)
}

fn status_response(mode: PayerPolicyMode, session: &DbPayerSession) -> VerificationStatusResponse {
    VerificationStatusResponse {
        requirements: VerificationRequirementsResponse::from_facts(mode, session.facts()),
        identity_start_available: false,
    }
}

fn unix_now() -> u64 {
    Utc::now().timestamp().max(0) as u64
}

/// The gated invoice behind `id`, still open to verification: permissionless
/// invoices have nothing to verify, and verification after the deadline or
/// after settlement cannot change anything, so both are refused up front.
async fn gated_invoice(state: &AppState, id: &str) -> Result<(DbInvoice, Invoice), ApiError> {
    let uuid = parse_invoice_id(id)?;
    let row = state
        .repo
        .find_by_id(uuid)
        .await?
        .ok_or_else(ApiError::payer_unauthorized)?;
    let invoice = Invoice::try_from(&row)?;
    if !invoice.issuance_snapshot.payer_policy.mode().is_gated() {
        return Err(ApiError::verification_not_required());
    }
    let open = matches!(
        invoice.status,
        InvoiceStatus::Created | InvoiceStatus::Funded | InvoiceStatus::Deploying
    );
    if !open || unix_now() > invoice.expiration_timestamp {
        return Err(ApiError::payment_not_payable());
    }
    Ok((row, invoice))
}

/// The address the code goes to: the merchant's assertion, normalized the
/// way the Auth0 Action normalizes the proven mailbox.
fn expected_email(invoice: &Invoice) -> String {
    invoice
        .issuance_snapshot
        .payer_policy
        .expected_email()
        .expect("a gated policy carries an expected email")
        .trim()
        .to_lowercase()
}

pub async fn start_email(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let verification = state
        .payer_verification
        .as_ref()
        .ok_or_else(ApiError::verification_unavailable)?;
    let (row, invoice) = gated_invoice(&state, &id).await?;
    // A resend reuses the caller's session when it is valid for this
    // invoice; anything else starts a fresh one.
    let existing = match session_token(&headers) {
        Some(token) => state
            .payer_sessions
            .find_active(token, row.id)
            .await?
            .map(|session| (session, token.to_owned())),
        None => None,
    };
    let (session_id, token, expires_at) = match existing {
        Some((session, token)) => (session.id, token, session.expires_at),
        None => {
            let created = state
                .payer_sessions
                .create(row.id, PAYER_SESSION_TTL)
                .await?;
            (created.id, created.token, created.expires_at)
        }
    };
    match state
        .payer_sessions
        .begin_email_verification(session_id, RESEND_COOLDOWN)
        .await
    {
        Ok(_) => {}
        Err(StartEmailVerificationError::Cooldown { retry_after }) => {
            let seconds = retry_after.as_secs().max(1);
            let mut response = ApiError::otp_resend_cooldown(seconds).into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(seconds));
            return Ok(response);
        }
        Err(StartEmailVerificationError::Database(error)) => return Err(error.into()),
    }
    verification.send_code(&expected_email(&invoice)).await?;
    Ok((
        no_store(),
        Json(StartEmailResponse {
            payer_session: token,
            expires_at: expires_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        }),
    )
        .into_response())
}

pub async fn confirm_email(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ConfirmEmailRequest>,
) -> Result<(HeaderMap, Json<VerificationStatusResponse>), ApiError> {
    let verification = state
        .payer_verification
        .as_ref()
        .ok_or_else(ApiError::verification_unavailable)?;
    let otp = request.otp.trim();
    if otp.is_empty() || otp.len() > MAX_OTP_LEN || !otp.chars().all(char::is_alphanumeric) {
        return Err(ApiError::invalid_request(
            "otp must be the code from the verification email",
        ));
    }
    let (row, invoice) = gated_invoice(&state, &id).await?;
    let token = session_token(&headers).ok_or_else(ApiError::payer_session_invalid)?;
    let session = state
        .payer_sessions
        .find_active(token, row.id)
        .await?
        .ok_or_else(ApiError::payer_session_invalid)?;
    if !state
        .payer_sessions
        .has_pending_email_verification(session.id)
        .await?
    {
        return Err(ApiError::verification_not_started());
    }
    let expected = expected_email(&invoice);
    let identity = verification.confirm_code(&expected, otp).await?;
    let reference = verification.payer_ref(row.account_id, &expected);
    let completion = state
        .payer_sessions
        .approve_email(
            session.id,
            reference,
            Utc::now(),
            &identity.authentication_event_id,
        )
        .await?;
    if completion.invoice_completed_at.is_some() {
        tracing::info!(invoice_id = %row.id, "payer verification completed");
    }
    let mode = invoice.issuance_snapshot.payer_policy.mode();
    Ok((no_store(), Json(status_response(mode, &completion.session))))
}

/// What the caller's session has established, or, without a session, what
/// the invoice as a whole has.
pub async fn status(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Json<VerificationStatusResponse>), ApiError> {
    let uuid: Uuid = parse_invoice_id(&id)?;
    let row = state
        .repo
        .find_by_id(uuid)
        .await?
        .ok_or_else(ApiError::payer_unauthorized)?;
    let mode: PayerPolicyMode = row
        .payer_policy_mode
        .parse()
        .map_err(|_| ApiError::internal("invalid payer policy mode in stored invoice"))?;
    let response = match session_token(&headers) {
        Some(token) => {
            let session = state
                .payer_sessions
                .find_active(token, row.id)
                .await?
                .ok_or_else(ApiError::payer_session_invalid)?;
            status_response(mode, &session)
        }
        None => VerificationStatusResponse {
            requirements: VerificationRequirementsResponse::for_mode(
                mode,
                row.verification_completed_at.is_some(),
            ),
            identity_start_available: false,
        },
    };
    Ok((no_store(), Json(response)))
}
