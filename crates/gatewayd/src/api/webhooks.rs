//! Webhook endpoints and their delivery history.
//!
//! ```text
//! POST   /v1/webhooks                 GET /v1/webhooks
//! GET    /v1/webhooks/{id}            DELETE /v1/webhooks/{id}
//! POST   /v1/webhooks/{id}/test
//! GET    /v1/webhook-deliveries
//! ```
//!
//! The same conventions as every other merchant resource: a missing or
//! foreign endpoint is `404 webhook_not_found`, a disable is `204`, lists
//! are enveloped, and deliveries page with `limit` and `starting_after`.

use std::collections::HashMap;
use std::str::FromStr;

use crate::{
    api::{
        error::ApiError,
        json::{Json, Query},
    },
    state::AppState,
};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use axum::{
    Extension,
    extract::{Path, State},
    http::StatusCode,
};
use gateway_core::rfc3339;
use gateway_db::{AccountId, WebhookAttempt, WebhookDelivery};
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
    /// Set once the endpoint was disabled; such an endpoint is no longer
    /// listed and receives nothing, but its deliveries stay readable.
    disabled_at: Option<String>,
    /// The signing secret, on the `201` that created the endpoint only.
    #[serde(skip_serializing_if = "Option::is_none")]
    secret: Option<String>,
}

#[derive(Serialize)]
pub struct EndpointList {
    webhooks: Vec<EndpointResponse>,
}

#[derive(Serialize)]
pub struct TestDeliveryResponse {
    delivery_id: Uuid,
}

#[derive(Serialize)]
pub struct DeliveryAttemptResponse {
    number: i32,
    attempted_at: String,
    duration_ms: i64,
    status: Option<i32>,
    error: Option<String>,
}

#[derive(Serialize)]
pub struct DeliveryResponse {
    id: Uuid,
    event_id: Uuid,
    endpoint_id: Uuid,
    /// `pending`, `delivered`, or `failed`.
    state: String,
    attempt_count: i32,
    next_attempt_at: String,
    delivered_at: Option<String>,
    created_at: String,
    attempts: Vec<DeliveryAttemptResponse>,
}

#[derive(Serialize)]
pub struct DeliveryPage {
    deliveries: Vec<DeliveryResponse>,
    next_cursor: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveriesQuery {
    /// Only this endpoint's deliveries, disabled or not.
    endpoint_id: Option<String>,
    limit: Option<u32>,
    starting_after: Option<String>,
}

fn endpoint(row: gateway_db::WebhookEndpoint, secret: Option<String>) -> EndpointResponse {
    EndpointResponse {
        id: row.id,
        url: row.url,
        created_at: rfc3339(row.created_at),
        disabled_at: row.disabled_at.map(rfc3339),
        secret,
    }
}

fn delivery(row: WebhookDelivery, attempts: Vec<WebhookAttempt>) -> DeliveryResponse {
    DeliveryResponse {
        id: row.id,
        event_id: row.event_id,
        endpoint_id: row.endpoint_id,
        state: row.state,
        attempt_count: row.attempt_count,
        next_attempt_at: rfc3339(row.next_attempt_at),
        delivered_at: row.delivered_at.map(rfc3339),
        created_at: rfc3339(row.created_at),
        attempts: attempts
            .into_iter()
            .map(|attempt| DeliveryAttemptResponse {
                number: attempt.attempt_number,
                attempted_at: rfc3339(attempt.attempted_at),
                duration_ms: attempt.duration_ms,
                status: attempt.status,
                error: attempt.error,
            })
            .collect(),
    }
}

fn key(state: &AppState) -> Result<[u8; 32], ApiError> {
    state
        .webhook_encryption_key
        .ok_or_else(ApiError::webhooks_unavailable)
}

/// A malformed id is a missing endpoint, as for every other resource.
fn webhook_id(value: &str) -> Result<Uuid, ApiError> {
    Uuid::from_str(value).map_err(|_| ApiError::webhook_not_found())
}

pub async fn add(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Json(req): Json<AddRequest>,
) -> Result<(StatusCode, Json<EndpointResponse>), ApiError> {
    let key = key(&state)?;
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
    let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
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
) -> Result<Json<EndpointList>, ApiError> {
    Ok(Json(EndpointList {
        webhooks: state
            .webhooks
            .list(account)
            .await?
            .into_iter()
            .map(|row| endpoint(row, None))
            .collect(),
    }))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
) -> Result<Json<EndpointResponse>, ApiError> {
    let id = webhook_id(&id)?;
    let row = state
        .webhooks
        .get(account, id)
        .await?
        .ok_or_else(ApiError::webhook_not_found)?;
    Ok(Json(endpoint(row, None)))
}

pub async fn test(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<TestDeliveryResponse>), ApiError> {
    let id = webhook_id(&id)?;
    let delivery_id = state
        .webhooks
        .test_event(account, id)
        .await?
        .ok_or_else(ApiError::webhook_not_found)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(TestDeliveryResponse { delivery_id }),
    ))
}

/// Disables the endpoint. Idempotent: disabling one already disabled is
/// still `204`, and its delivery history stays readable.
pub async fn remove(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = webhook_id(&id)?;
    if !state.webhooks.disable(account, id).await? {
        return Err(ApiError::webhook_not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn deliveries(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Query(query): Query<DeliveriesQuery>,
) -> Result<Json<DeliveryPage>, ApiError> {
    let limit = query.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::invalid_request("limit must be between 1 and 100"));
    }
    let endpoint_id = match query.endpoint_id.as_deref() {
        Some(value) => {
            let id = webhook_id(value)?;
            if state.webhooks.get(account, id).await?.is_none() {
                return Err(ApiError::webhook_not_found());
            }
            Some(id)
        }
        None => None,
    };
    let starting_after = match query.starting_after.as_deref() {
        Some(value) => {
            let id = Uuid::from_str(value).ok();
            match id {
                Some(id) if state.webhooks.delivery(account, id).await?.is_some() => Some(id),
                _ => {
                    return Err(ApiError::invalid_request(
                        "starting_after does not identify one of your webhook deliveries",
                    ));
                }
            }
        }
        None => None,
    };
    let mut rows = state
        .webhooks
        .deliveries(account, endpoint_id, starting_after, i64::from(limit))
        .await?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = has_more.then(|| rows.last().expect("nonzero limit").id);
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut attempts: HashMap<Uuid, Vec<WebhookAttempt>> = HashMap::new();
    for attempt in state.webhooks.attempts_for(&ids).await? {
        attempts
            .entry(attempt.delivery_id)
            .or_default()
            .push(attempt);
    }
    Ok(Json(DeliveryPage {
        deliveries: rows
            .into_iter()
            .map(|row| {
                let history = attempts.remove(&row.id).unwrap_or_default();
                delivery(row, history)
            })
            .collect(),
        next_cursor,
    }))
}
