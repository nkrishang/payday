use crate::{api::error::ApiError, state::AppState};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use gateway_db::AccountId;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddRequest {
    url: String,
}
#[derive(Serialize)]
pub struct EndpointResponse {
    id: Uuid,
    url: String,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    secret: Option<String>,
}
fn endpoint(row: gateway_db::WebhookEndpoint, secret: Option<String>) -> EndpointResponse {
    EndpointResponse {
        id: row.id,
        url: row.url,
        created_at: row.created_at.to_rfc3339(),
        secret,
    }
}
fn key(state: &AppState) -> Result<[u8; 32], ApiError> {
    state
        .webhook_encryption_key
        .ok_or_else(|| ApiError::internal("webhook encryption is not configured"))
}

pub async fn add(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Json(req): Json<AddRequest>,
) -> Result<(StatusCode, Json<EndpointResponse>), ApiError> {
    let url = reqwest::Url::parse(&req.url)
        .map_err(|_| ApiError::invalid_request("url must be a valid HTTPS URL"))?;
    crate::webhook_worker::resolve_public(&url)
        .await
        .map_err(ApiError::invalid_request)?;
    let mut random = [0u8; 32];
    rand::rng().fill_bytes(&mut random);
    let secret = format!("whsec_{}", hex::encode(random));
    let mut nonce = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce);
    let cipher = Aes256Gcm::new_from_slice(&key(&state)?).unwrap();
    let mut encrypted = nonce.to_vec();
    encrypted.extend(
        cipher
            .encrypt(&Nonce::from(nonce), secret.as_bytes())
            .map_err(|_| ApiError::internal("secret encryption failed"))?,
    );
    let row = state
        .webhooks
        .add(account, url.as_str(), secret.as_bytes(), &encrypted)
        .await?;
    Ok((StatusCode::CREATED, Json(endpoint(row, Some(secret)))))
}
pub async fn list(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
) -> Result<Json<Vec<EndpointResponse>>, ApiError> {
    Ok(Json(
        state
            .webhooks
            .list(account)
            .await?
            .into_iter()
            .map(|x| endpoint(x, None))
            .collect(),
    ))
}
pub async fn test(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let id = state
        .webhooks
        .test_event(account, id)
        .await?
        .ok_or_else(|| ApiError::invalid_request("webhook not found or is disabled"))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"delivery_id":id})),
    ))
}
pub async fn remove(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !state.webhooks.disable(account, id).await? {
        return Err(ApiError::invalid_request("webhook not found"));
    }
    Ok(Json(serde_json::json!({"id":id,"disabled":true})))
}
pub async fn deliveries(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let rows = state.webhooks.deliveries(account).await?;
    let mut output = Vec::with_capacity(rows.len());
    for row in rows {
        let attempts = state.webhooks.attempts(row.id).await?;
        output.push(serde_json::json!({"id":row.id,"event_id":row.event_id,"endpoint_id":row.endpoint_id,"state":row.state,"attempt_count":row.attempt_count,"next_attempt_at":row.next_attempt_at.to_rfc3339(),"delivered_at":row.delivered_at.map(|v|v.to_rfc3339()),"attempts":attempts.into_iter().map(|a|serde_json::json!({"number":a.attempt_number,"attempted_at":a.attempted_at.to_rfc3339(),"duration_ms":a.duration_ms,"status":a.status,"error":a.error})).collect::<Vec<_>>() }));
    }
    Ok(Json(serde_json::Value::Array(output)))
}
