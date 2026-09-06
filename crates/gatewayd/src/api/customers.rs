//! Customer routes (product plan §4.4): merchant-owned counterparty records
//! that provide defaults when issuing; invoices snapshot what they used.

use axum::Extension;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use gateway_core::{CustomerId, rfc3339};
use gateway_db::{AccountId, CreateCustomerInput, DbCustomer};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use crate::api::deposit_requests::validate_party_fields;
use crate::api::error::ApiError;
use crate::api::json::{Json, Query};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomerRequest {
    name: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    details: Option<String>,
}

/// A partial update: a field left out keeps its value, a field sent as
/// `null` is cleared. Telling the two apart is what `patch_field` is for.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateCustomerRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, deserialize_with = "patch_field")]
    email: Option<Option<String>>,
    #[serde(default, deserialize_with = "patch_field")]
    details: Option<Option<String>>,
}

/// `Some(None)` for an explicit `null`, `Some(Some(v))` for a value; the
/// `default` attribute on the field supplies `None` when it is absent.
pub(crate) fn patch_field<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Serialize)]
pub struct CustomerResponse {
    id: CustomerId,
    name: String,
    email: Option<String>,
    details: Option<String>,
    created_at: String,
    updated_at: String,
}

impl From<DbCustomer> for CustomerResponse {
    fn from(row: DbCustomer) -> Self {
        Self {
            id: CustomerId(row.id),
            name: row.name,
            email: row.email,
            details: row.details,
            created_at: rfc3339(row.created_at),
            updated_at: rfc3339(row.updated_at),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CustomerPage {
    customers: Vec<CustomerResponse>,
    next_cursor: Option<CustomerId>,
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
    starting_after: Option<CustomerId>,
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
            .get_for_account(account, cursor.0)
            .await?
            .is_none()
    {
        return Err(ApiError::invalid_request(
            "starting_after does not identify one of your customers",
        ));
    }
    let mut rows = state
        .customers
        .list_for_account(account, limit, query.starting_after.map(Uuid::from))
        .await?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = has_more.then(|| CustomerId(rows.last().expect("nonzero limit").id));
    Ok(Json(CustomerPage {
        customers: rows.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

/// Changes only the fields the body names; the merged record is validated
/// whole, so a partial body can never leave an invalid customer behind.
pub async fn update(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
    Json(request): Json<UpdateCustomerRequest>,
) -> Result<Json<CustomerResponse>, ApiError> {
    let id = customer_id(&id)?;
    let current = state
        .customers
        .get_for_account(account, id)
        .await?
        .ok_or_else(ApiError::customer_not_found)?;
    let merged = CustomerRequest {
        name: request.name.unwrap_or(current.name),
        email: request.email.unwrap_or(current.email),
        details: request.details.unwrap_or(current.details),
    };
    validate(&merged)?;
    state
        .customers
        .update(account, id, merged.name, merged.email, merged.details)
        .await?
        .map(|row| Json(row.into()))
        .ok_or_else(ApiError::customer_not_found)
}

/// Anything but a canonical `cus_` id is a missing customer.
fn customer_id(value: &str) -> Result<Uuid, ApiError> {
    CustomerId::parse(value)
        .map(Uuid::from)
        .ok_or_else(ApiError::customer_not_found)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_tells_an_absent_field_from_an_explicit_null() {
        let absent: UpdateCustomerRequest = serde_json::from_str(r#"{"name":"Globex"}"#).unwrap();
        assert_eq!(absent.name.as_deref(), Some("Globex"));
        assert_eq!(absent.email, None);
        let cleared: UpdateCustomerRequest =
            serde_json::from_str(r#"{"email":null,"details":"Net 45"}"#).unwrap();
        assert_eq!(cleared.name, None);
        assert_eq!(cleared.email, Some(None));
        assert_eq!(cleared.details, Some(Some("Net 45".into())));
        assert!(serde_json::from_str::<UpdateCustomerRequest>(r#"{"nam":"x"}"#).is_err());
    }
}
