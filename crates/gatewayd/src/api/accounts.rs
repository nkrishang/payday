//! The account behind a credential, and the API key it hands its own server.
//!
//! Reading the account works with either credential. Issuing and revoking a
//! key take a dashboard session and nothing else: whoever holds a key must
//! not be able to mint another from it, so those two routes extract
//! [`Session`], which an API-key request does not carry.

use axum::Extension;
use axum::extract::State;
use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use gateway_core::rfc3339;
use gateway_db::{AccountId, IssueApiKeyError};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::auth::Session;
use crate::api::error::ApiError;
use crate::api::json::Json;
use crate::state::AppState;

#[derive(Serialize)]
pub struct IssuedApiKey {
    api_key: String,
    generation: i64,
    replaced_previous_key: bool,
}

#[derive(Serialize)]
pub struct AccountResponse {
    account_id: String,
    /// The mailbox the account signs in with.
    email: Option<String>,
    /// The account's own wallet, where deposits settle by default; `null`
    /// until the first session that carried one.
    wallet_address: Option<String>,
    key_hint: Option<String>,
    generation: i64,
    created_at: String,
    rotated_at: Option<String>,
    previous_key_expires_at: Option<String>,
    revoked_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueApiKeyRequest {
    #[serde(default)]
    expected_generation: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeApiKeyRequest {
    expected_generation: i64,
}

pub async fn issue(
    State(state): State<AppState>,
    Session(identity): Session,
    Json(request): Json<IssueApiKeyRequest>,
) -> Result<(StatusCode, Json<IssuedApiKey>), ApiError> {
    if request.expected_generation.is_some_and(|value| value < 1) {
        return Err(ApiError::invalid_request(
            "expected_generation must be positive",
        ));
    }
    let key = generate_api_key(&state.api_key_prefix);
    let issued = state
        .accounts
        .issue_api_key_with_email(
            &identity.issuer,
            &identity.subject,
            request.expected_generation,
            // Each request is its own event: the session is the credential,
            // and the generation check below is what stops two rotations
            // from racing each other.
            &Uuid::now_v7().to_string(),
            &key,
            &identity.email,
        )
        .await
        .map_err(issue_error)?;
    let status = if issued.replaced_previous_key {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((
        status,
        Json(IssuedApiKey {
            api_key: key,
            generation: issued.generation,
            replaced_previous_key: issued.replaced_previous_key,
        }),
    ))
}

pub async fn get_account(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
) -> Result<Json<AccountResponse>, ApiError> {
    Ok(Json(account_response(
        state.accounts.metadata(account).await?,
    )))
}

pub async fn revoke(
    State(state): State<AppState>,
    Session(identity): Session,
    Json(request): Json<RevokeApiKeyRequest>,
) -> Result<StatusCode, ApiError> {
    if request.expected_generation < 1 {
        return Err(ApiError::invalid_request(
            "expected_generation must be positive",
        ));
    }
    state
        .accounts
        .revoke_api_key(
            &identity.issuer,
            &identity.subject,
            request.expected_generation,
            &Uuid::now_v7().to_string(),
        )
        .await
        .map_err(issue_error)?;
    Ok(StatusCode::NO_CONTENT)
}

fn issue_error(error: IssueApiKeyError) -> ApiError {
    match error {
        IssueApiKeyError::AuthenticationEventAlreadyUsed => {
            ApiError::authentication_event_already_used()
        }
        IssueApiKeyError::GenerationConflict => ApiError::api_key_generation_conflict(),
        IssueApiKeyError::AccountDisabled => ApiError::account_disabled(),
        IssueApiKeyError::Database(error) => ApiError::from(error),
    }
}

fn account_response(metadata: gateway_db::ApiKeyMetadata) -> AccountResponse {
    AccountResponse {
        account_id: gateway_core::AccountId(metadata.account_id).to_string(),
        email: metadata.email,
        wallet_address: metadata.wallet_address,
        key_hint: metadata.hint,
        generation: metadata.generation,
        created_at: rfc3339(metadata.created_at),
        rotated_at: metadata.rotated_at.map(rfc3339),
        previous_key_expires_at: metadata.previous_key_expires_at.map(rfc3339),
        revoked_at: metadata.revoked_at.map(rfc3339),
    }
}

fn generate_api_key(prefix: &str) -> String {
    let mut random = [0_u8; 32];
    rand::rng().fill_bytes(&mut random);
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(random))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_keys_use_each_configured_prefix_and_256_bits_of_entropy() {
        let first = generate_api_key("payday_live_");
        let second = generate_api_key("payday_live_");
        assert!(first.starts_with("payday_live_"));
        assert_eq!(first.len(), 55);
        assert_ne!(first, second);
        let test = generate_api_key("payday_test_");
        assert!(test.starts_with("payday_test_"));
        assert_eq!(test.len(), 55);
    }
}
