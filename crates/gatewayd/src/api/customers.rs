//! Customer routes (product plan §4.4): merchant-owned counterparty records
//! that provide defaults when issuing; invoices snapshot what they used.

use std::str::FromStr;

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use gateway_db::{AccountId, CreateCustomerInput, DbCustomer};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::invoices::validate_party_fields;
use crate::state::AppState;

/// Create and update share one body; update replaces every editable field,
/// so an omitted or null `email`/`details` clears it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomerRequest {
    name: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    details: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CustomerResponse {
    id: Uuid,
    name: String,
    email: Option<String>,
    details: Option<String>,
    created_at: String,
    updated_at: String,
}

impl From<DbCustomer> for CustomerResponse {
    fn from(row: DbCustomer) -> Self {
        Self {
            id: row.id,
            name: row.name,
            email: row.email,
            details: row.details,
            created_at: row.created_at.to_rfc3339(),
            updated_at: row.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CustomerPage {
    customers: Vec<CustomerResponse>,
    next_cursor: Option<Uuid>,
}

/// How many requests this customer has been billed, how much of that has
/// actually been confirmed on chain, and how much is still outstanding on
/// the ones still open. Base units, like an invoice's own
/// `amount_base_units` — the caller scales for display.
#[derive(Debug, Serialize)]
pub struct CustomerStats {
    request_count: i64,
    collected_base_units: String,
    pending_base_units: String,
}

impl From<gateway_db::CustomerInvoiceStats> for CustomerStats {
    fn from(row: gateway_db::CustomerInvoiceStats) -> Self {
        Self {
            request_count: row.request_count,
            collected_base_units: row.collected_base_units,
            pending_base_units: row.pending_base_units,
        }
    }
}

/// Only `get` returns stats: a list of many customers would mean one
/// aggregate query per row, and the list and the composer's picker need
/// nothing past the fields on [`CustomerResponse`] itself.
#[derive(Debug, Serialize)]
pub struct CustomerDetailResponse {
    #[serde(flatten)]
    customer: CustomerResponse,
    stats: CustomerStats,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    limit: Option<u32>,
    starting_after: Option<Uuid>,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Json(request): Json<CustomerRequest>,
) -> Result<(StatusCode, Json<CustomerResponse>), ApiError> {
    validate(&request)?;
    let row = state
        .customers
        .create(&CreateCustomerInput {
            id: Uuid::now_v7(),
            account_id: account,
            name: request.name,
            email: request.email,
            details: request.details,
        })
        .await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
) -> Result<Json<CustomerDetailResponse>, ApiError> {
    let id = customer_id(&id)?;
    let row = state
        .customers
        .get_for_account(account, id)
        .await?
        .ok_or_else(ApiError::customer_not_found)?;
    let stats = state.repo.customer_stats(account, id).await?;
    Ok(Json(CustomerDetailResponse {
        customer: row.into(),
        stats: stats.into(),
    }))
}

pub async fn list(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Query(query): Query<ListQuery>,
) -> Result<Json<CustomerPage>, ApiError> {
    let limit = query.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::invalid_request("limit must be between 1 and 100"));
    }
    if let Some(cursor) = query.starting_after
        && state
            .customers
            .get_for_account(account, cursor)
            .await?
            .is_none()
    {
        return Err(ApiError::invalid_request(
            "starting_after does not identify one of your customers",
        ));
    }
    let mut rows = state
        .customers
        .list_for_account(account, limit, query.starting_after)
        .await?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = has_more.then(|| rows.last().expect("nonzero limit").id);
    Ok(Json(CustomerPage {
        customers: rows.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

pub async fn update(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
    Json(request): Json<CustomerRequest>,
) -> Result<Json<CustomerResponse>, ApiError> {
    let id = customer_id(&id)?;
    validate(&request)?;
    state
        .customers
        .update(account, id, request.name, request.email, request.details)
        .await?
        .map(|row| Json(row.into()))
        .ok_or_else(ApiError::customer_not_found)
}

fn customer_id(value: &str) -> Result<Uuid, ApiError> {
    Uuid::from_str(value).map_err(|_| ApiError::customer_not_found())
}

/// The same limits as an invoice party, since a customer is what one is
/// built from.
fn validate(request: &CustomerRequest) -> Result<(), ApiError> {
    validate_party_fields(
        "",
        &request.name,
        request.email.as_deref(),
        request.details.as_deref(),
    )
}
