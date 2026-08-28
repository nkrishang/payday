use axum::Extension;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use gateway_db::{AccountId, IssueApiKeyError};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::api::auth::Identity;
use crate::api::error::ApiError;
use crate::state::AppState;

#[derive(Serialize)]
pub struct IssuedApiKey {
    api_key: String,
    generation: i64,
    replaced_previous_key: bool,
}

#[derive(Serialize)]
pub struct ApiKeyMetadataResponse {
    account_id: String,
    key_hint: Option<String>,
    generation: i64,
    created_at: String,
    rotated_at: Option<String>,
    previous_key_expires_at: Option<String>,
    revoked_at: Option<String>,
}

#[derive(Deserialize)]
pub struct IssueApiKeyRequest {
    expected_generation: Option<i64>,
}

#[derive(Deserialize)]
pub struct RevokeApiKeyRequest {
    expected_generation: i64,
}

pub async fn issue(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
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
            &identity.authentication_event_id,
            &key,
            &identity.email,
        )
        .await
        .map_err(|error| match error {
            IssueApiKeyError::AuthenticationEventAlreadyUsed => {
                ApiError::authentication_event_already_used()
            }
            IssueApiKeyError::GenerationConflict => ApiError::api_key_generation_conflict(),
            IssueApiKeyError::AccountDisabled => ApiError::account_disabled(),
            IssueApiKeyError::Database(error) => ApiError::from(error),
        })?;
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

pub async fn metadata(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<ApiKeyMetadataResponse>, ApiError> {
    let account = account_for_identity(&state, &identity).await?;
    let metadata = state.accounts.metadata(account).await?;
    Ok(Json(metadata_response(metadata)))
}

pub async fn get_account(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
) -> Result<Json<ApiKeyMetadataResponse>, ApiError> {
    Ok(Json(metadata_response(
        state.accounts.metadata(account).await?,
    )))
}

pub async fn revoke(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
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
            &identity.authentication_event_id,
        )
        .await
        .map_err(|error| match error {
            IssueApiKeyError::AuthenticationEventAlreadyUsed => {
                ApiError::authentication_event_already_used()
            }
            IssueApiKeyError::GenerationConflict => ApiError::api_key_generation_conflict(),
            IssueApiKeyError::AccountDisabled => ApiError::account_disabled(),
            IssueApiKeyError::Database(error) => ApiError::from(error),
        })?;
    Ok(StatusCode::NO_CONTENT)
}

fn metadata_response(metadata: gateway_db::ApiKeyMetadata) -> ApiKeyMetadataResponse {
    ApiKeyMetadataResponse {
        account_id: metadata.account_id.to_string(),
        key_hint: metadata.hint,
        generation: metadata.generation,
        created_at: metadata.created_at.to_rfc3339(),
        rotated_at: metadata.rotated_at.map(|value| value.to_rfc3339()),
        previous_key_expires_at: metadata
            .previous_key_expires_at
            .map(|value| value.to_rfc3339()),
        revoked_at: metadata.revoked_at.map(|value| value.to_rfc3339()),
    }
}

async fn account_for_identity(
    state: &AppState,
    identity: &Identity,
) -> Result<AccountId, ApiError> {
    state
        .accounts
        .find_by_identity(&identity.issuer, &identity.subject)
        .await?
        .ok_or_else(ApiError::account_not_provisioned)
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
