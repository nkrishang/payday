//! The merchant-session payer mode (product plan §3.2, §4.6): the merchant's
//! own application has authenticated the payer, and opens the hosted checkout
//! for them with a single-use client secret.
//!
//! ```text
//! POST /v1/deposit-requests/{id}/client-secret        merchant: mint a fresh secret
//! POST /v1/payer/deposit-requests/{id}/session        checkout: exchange it for a payer session
//! ```
//!
//! The secret is returned once and stored hashed. Exchanging it is the whole
//! verification: it mints the payer session with the merchant-session fact,
//! records an approved attempt, completes the invoice's verification while it
//! is live, and so raises `verification.approved`. Payday's only added claim
//! is that the merchant's server released this secret before the session was
//! opened; who the payer is remains the merchant's assertion, carried as
//! `payer_reference` in the policy and every webhook.

use axum::Extension;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use gateway_core::{
    Invoice, InvoiceStatus, PayerPolicyMode, VerificationRequirementsResponse, deposit_request_id,
    rfc3339,
};
use gateway_db::{
    AccountId, CLIENT_SECRET_PREFIX, CLIENT_SECRET_TTL, ExchangeClientSecretError,
    PAYER_SESSION_TTL,
};
use serde::{Deserialize, Serialize};

use crate::api::attachments::no_store;
use crate::api::deposit_requests::resolve_deposit_request;
use crate::api::error::ApiError;
use crate::api::json::Json;
use crate::state::AppState;

/// `cs_` plus 43 characters of unpadded base64url; anything else is refused
/// before the database is asked.
const CLIENT_SECRET_LEN: usize = CLIENT_SECRET_PREFIX.len() + 43;

#[derive(Serialize)]
pub struct ClientSecretResponse {
    /// Returned once; the API stores only its hash.
    pub client_secret: String,
    pub expires_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeClientSecretRequest {
    pub client_secret: String,
}

#[derive(Serialize)]
pub struct ExchangeClientSecretResponse {
    pub payer_session: String,
    pub expires_at: String,
    pub requirements: VerificationRequirementsResponse,
}

/// Whether a secret may still be minted or exchanged for this invoice: it is
/// live, or it is terminal and was verified, in which case the session is a
/// receipt. The same rule the email routes apply to a gated invoice.
pub(crate) fn openable(row: &gateway_db::DbInvoice, invoice: &Invoice) -> bool {
    let open = matches!(
        invoice.status,
        InvoiceStatus::Created | InvoiceStatus::Funded | InvoiceStatus::Deploying
    );
    let now = Utc::now().timestamp().max(0) as u64;
    let receipt = !open && row.verification_completed_at.is_some();
    (open && now <= invoice.expiration_timestamp) || receipt
}

fn merchant_session_only(invoice: &Invoice) -> Result<(), ApiError> {
    match invoice.issuance_snapshot.payer_policy.mode() {
        PayerPolicyMode::MerchantSession => Ok(()),
        PayerPolicyMode::Permissionless => Err(ApiError::verification_not_required()),
        PayerPolicyMode::VerifiedEmail => Err(ApiError::verification_method_not_applicable(
            "This deposit request verifies the payer by email; client secrets apply to merchant_session deposit requests",
        )),
    }
}

/// Mint a client secret for a merchant-session invoice the merchant owns.
pub async fn mint(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(reference): Path<String>,
) -> Result<Response, ApiError> {
    let row = resolve_deposit_request(&state, account, &reference).await?;
    let invoice = Invoice::try_from(&row)?;
    merchant_session_only(&invoice)?;
    if !openable(&row, &invoice) {
        return Err(ApiError::deposit_request_not_payable());
    }
    let minted = state
        .payer_sessions
        .create_client_secret(row.id, account.0, CLIENT_SECRET_TTL)
        .await?;
    tracing::info!(invoice_id = %row.id, "client secret minted");
    Ok((
        StatusCode::CREATED,
        no_store(),
        Json(ClientSecretResponse {
            client_secret: minted.secret,
            expires_at: rfc3339(minted.expires_at),
        }),
    )
        .into_response())
}

/// Exchange a client secret for the payer session it opens. Answers only the
/// hosted checkout's origin, like every other verification write.
pub async fn exchange(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ExchangeClientSecretRequest>,
) -> Result<Response, ApiError> {
    let secret = request.client_secret.trim();
    if secret.len() != CLIENT_SECRET_LEN
        || !secret.starts_with(CLIENT_SECRET_PREFIX)
        || !secret[CLIENT_SECRET_PREFIX.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(ApiError::client_secret_invalid());
    }
    let uuid = deposit_request_id(&id).ok_or_else(ApiError::payer_unauthorized)?;
    let row = state
        .repo
        .find_by_id(uuid)
        .await?
        .ok_or_else(ApiError::payer_unauthorized)?;
    let invoice = Invoice::try_from(&row)?;
    merchant_session_only(&invoice)?;
    if !openable(&row, &invoice) {
        return Err(ApiError::deposit_request_not_payable());
    }
    let exchange = match state
        .payer_sessions
        .exchange_client_secret(secret, row.id, PAYER_SESSION_TTL)
        .await
    {
        Ok(exchange) => exchange,
        Err(ExchangeClientSecretError::Used) => return Err(ApiError::client_secret_used()),
        Err(ExchangeClientSecretError::Unknown | ExchangeClientSecretError::Expired) => {
            return Err(ApiError::client_secret_invalid());
        }
        Err(ExchangeClientSecretError::Database(error)) => return Err(error.into()),
    };
    if exchange.invoice_completed_at.is_some() {
        tracing::info!(invoice_id = %row.id, "payer verification completed by merchant session");
    }
    Ok((
        no_store(),
        Json(ExchangeClientSecretResponse {
            payer_session: exchange.token,
            expires_at: rfc3339(exchange.session.expires_at),
            requirements: VerificationRequirementsResponse::from_facts(
                PayerPolicyMode::MerchantSession,
                exchange.session.facts(),
            ),
        }),
    )
        .into_response())
}
