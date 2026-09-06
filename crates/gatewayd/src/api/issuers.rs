//! Issuer identity routes: the merchant's own side of an invoice, saved once
//! rather than retyped per issuance.
//!
//! ```text
//! POST   /v1/issuers                            GET /v1/issuers
//! GET    /v1/issuers/{id}                       PATCH /v1/issuers/{id}
//! DELETE /v1/issuers/{id}
//! POST   /v1/issuers/{id}/verify/email/start
//! POST   /v1/issuers/{id}/verify/email/confirm
//! PUT    /v1/issuers/{id}/payout-addresses
//! POST   /v1/payout-addresses                   GET /v1/payout-addresses
//! DELETE /v1/payout-addresses/{id}
//! ```
//!
//! An identity is a saved party plus the addresses it may settle to. Issuance
//! is unchanged: `POST /v1/deposit-requests` still takes the party and the address
//! inline and snapshots them, so what these rows do is stop a merchant
//! retyping — and, for the contact mailbox, prove it. Payers are told to write
//! to that address, so it is established with the same emailed code the rest
//! of the product uses rather than merely typed.

use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

use alloy_primitives::Address;
use axum::Extension;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use gateway_core::rfc3339;
use gateway_db::{
    AccountId, CreateIssuerInput, CreatePayoutAddressInput, DbIssuer, DbPayoutAddress,
    IssuerRepository, StartIssuerEmailError, is_duplicate_issuer_name,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::customers::patch_field;
use crate::api::deposit_requests::validate_party_fields;
use crate::api::error::ApiError;
use crate::api::json::{Json, Query};
use crate::state::AppState;

/// One code per identity per minute, matching the payer flow's window. An
/// authenticated merchant must not be able to point this route at someone
/// else's mailbox and hold it down.
const RESEND_COOLDOWN: Duration = Duration::from_secs(60);
const MAX_OTP_LEN: usize = 16;
/// A label is a handle read in a picker, not free text.
const MAX_LABEL_LEN: usize = 20;
/// A ceiling on one identity's wallets; the UI picks from these, and a list
/// nobody can read is not a feature.
const MAX_ADDRESSES_PER_ISSUER: usize = 25;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssuerRequest {
    name: String,
    contact_email: String,
    #[serde(default)]
    details: Option<String>,
}

/// A partial update: a field left out keeps its value; `details` sent as
/// `null` is cleared. A changed `contact_email` starts unproven again.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateIssuerRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    contact_email: Option<String>,
    #[serde(default, deserialize_with = "patch_field")]
    details: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmEmailRequest {
    otp: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayoutAddressRequest {
    address: String,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetPayoutAddressesRequest {
    payout_address_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    limit: Option<u32>,
    starting_after: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct PayoutAddressResponse {
    id: Uuid,
    /// EIP-55 checksummed, ready to send back as `payout_address`.
    address: String,
    label: Option<String>,
    created_at: String,
}

impl From<DbPayoutAddress> for PayoutAddressResponse {
    fn from(row: DbPayoutAddress) -> Self {
        Self {
            id: row.id,
            address: row.address,
            label: row.label,
            created_at: rfc3339(row.created_at),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct IssuerResponse {
    id: Uuid,
    name: String,
    contact_email: String,
    details: Option<String>,
    /// Whether the contact mailbox has been proven with an emailed code.
    email_verified: bool,
    email_verified_at: Option<String>,
    /// The addresses this identity may settle to, association order.
    payout_addresses: Vec<PayoutAddressResponse>,
    created_at: String,
    updated_at: String,
}

impl IssuerResponse {
    fn build(row: DbIssuer, payout_addresses: Vec<PayoutAddressResponse>) -> Self {
        Self {
            id: row.id,
            name: row.name,
            contact_email: row.contact_email,
            details: row.details,
            email_verified: row.email_verified_at.is_some(),
            email_verified_at: row.email_verified_at.map(rfc3339),
            payout_addresses,
            created_at: rfc3339(row.created_at),
            updated_at: rfc3339(row.updated_at),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct IssuerPage {
    issuers: Vec<IssuerResponse>,
    next_cursor: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct PayoutAddressList {
    payout_addresses: Vec<PayoutAddressResponse>,
}

#[derive(Debug, Serialize)]
pub struct StartEmailResponse {
    /// Where the code went, so a UI can say it without holding the value.
    contact_email: String,
    /// When another code may be asked for.
    resend_available_at: String,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Json(request): Json<IssuerRequest>,
) -> Result<(StatusCode, Json<IssuerResponse>), ApiError> {
    let contact_email = validate(&request)?;
    let row = state
        .issuers
        .create(&CreateIssuerInput {
            id: Uuid::now_v7(),
            account_id: account,
            name: request.name,
            contact_email,
            details: request.details,
        })
        .await
        .map_err(duplicate_name_or)?;
    Ok((
        StatusCode::CREATED,
        Json(IssuerResponse::build(row, vec![])),
    ))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
) -> Result<Json<IssuerResponse>, ApiError> {
    let id = issuer_id(&id)?;
    let row = load(&state.issuers, account, id).await?;
    Ok(Json(with_addresses(&state.issuers, account, row).await?))
}

pub async fn list(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Query(query): Query<ListQuery>,
) -> Result<Json<IssuerPage>, ApiError> {
    let limit = query.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::invalid_request("limit must be between 1 and 100"));
    }
    if let Some(cursor) = query.starting_after
        && state
            .issuers
            .get_for_account(account, cursor)
            .await?
            .is_none()
    {
        return Err(ApiError::invalid_request(
            "starting_after does not identify one of your issuer identities",
        ));
    }
    let mut rows = state
        .issuers
        .list_for_account(account, limit, query.starting_after)
        .await?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = has_more.then(|| rows.last().expect("nonzero limit").id);

    // One query for every identity's addresses rather than one per row.
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut attached: HashMap<Uuid, Vec<PayoutAddressResponse>> = HashMap::new();
    for link in state.issuers.addresses_for_issuers(account, &ids).await? {
        attached
            .entry(link.issuer_id)
            .or_default()
            .push(PayoutAddressResponse {
                id: link.id,
                address: link.address,
                label: link.label,
                created_at: rfc3339(link.created_at),
            });
    }

    Ok(Json(IssuerPage {
        issuers: rows
            .into_iter()
            .map(|row| {
                let addresses = attached.remove(&row.id).unwrap_or_default();
                IssuerResponse::build(row, addresses)
            })
            .collect(),
        next_cursor,
    }))
}

/// Changes only the fields the body names; the merged identity is validated
/// whole. Renaming an identity therefore never touches its proven mailbox.
pub async fn update(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
    Json(request): Json<UpdateIssuerRequest>,
) -> Result<Json<IssuerResponse>, ApiError> {
    let id = issuer_id(&id)?;
    let current = load(&state.issuers, account, id).await?;
    let merged = IssuerRequest {
        name: request.name.unwrap_or(current.name),
        contact_email: request.contact_email.unwrap_or(current.contact_email),
        details: request.details.unwrap_or(current.details),
    };
    let contact_email = validate(&merged)?;
    let row = state
        .issuers
        .update(account, id, merged.name, contact_email, merged.details)
        .await
        .map_err(duplicate_name_or)?
        .ok_or_else(ApiError::issuer_not_found)?;
    Ok(Json(with_addresses(&state.issuers, account, row).await?))
}

pub async fn delete(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = issuer_id(&id)?;
    if state.issuers.has_invoices(account, id).await? {
        return Err(ApiError::issuer_in_use());
    }
    if state.issuers.delete(account, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::issuer_not_found())
    }
}

/// Emails a code to the identity's contact address. The address is never taken
/// from the request: it is the one already stored, so this route cannot be
/// pointed at an arbitrary mailbox.
pub async fn start_email_verification(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<StartEmailResponse>), ApiError> {
    let id = issuer_id(&id)?;
    let verification = state
        .payer_verification
        .as_ref()
        .ok_or_else(ApiError::verification_unavailable)?;

    let cooldown = RESEND_COOLDOWN.as_secs() as i64;
    let claimed = state
        .issuers
        .start_email_verification(account, id, cooldown)
        .await
        .map_err(|error| match error {
            StartIssuerEmailError::NoSuchIssuer => ApiError::issuer_not_found(),
            StartIssuerEmailError::AlreadyVerified => ApiError::issuer_email_already_verified(),
            StartIssuerEmailError::TooSoon(sent_at) => {
                let elapsed = (Utc::now() - sent_at).num_seconds().max(0);
                ApiError::otp_resend_cooldown((cooldown - elapsed).max(1) as u64)
            }
        })?;

    verification.send_code(&claimed.contact_email).await?;
    let resend_available_at = claimed
        .email_code_sent_at
        .unwrap_or_else(Utc::now)
        .checked_add_signed(chrono::Duration::seconds(cooldown))
        .unwrap_or_else(Utc::now);
    Ok((
        StatusCode::ACCEPTED,
        Json(StartEmailResponse {
            contact_email: claimed.contact_email,
            resend_available_at: rfc3339(resend_available_at),
        }),
    ))
}

/// Exchanges the code and records the proof. The identity provider decides
/// whether the code is good; all this adds is that the mailbox it proves is
/// the one the identity still carries.
pub async fn confirm_email_verification(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
    Json(request): Json<ConfirmEmailRequest>,
) -> Result<Json<IssuerResponse>, ApiError> {
    let id = issuer_id(&id)?;
    let otp = request.otp.trim();
    if otp.is_empty() || otp.len() > MAX_OTP_LEN {
        return Err(ApiError::otp_invalid());
    }
    let verification = state
        .payer_verification
        .as_ref()
        .ok_or_else(ApiError::verification_unavailable)?;

    let row = load(&state.issuers, account, id).await?;
    if row.email_verified_at.is_some() {
        return Err(ApiError::issuer_email_already_verified());
    }
    verification.confirm_code(&row.contact_email, otp).await?;

    let verified = state
        .issuers
        .mark_email_verified(account, id, &row.contact_email)
        .await?
        // The address changed between the code going out and coming back, so
        // the proof belongs to nothing that is still stored.
        .ok_or_else(ApiError::otp_invalid)?;
    Ok(Json(
        with_addresses(&state.issuers, account, verified).await?,
    ))
}

/// Replaces the whole set of addresses this identity may settle to. A
/// replacement rather than add/remove, because that is what a picker does.
pub async fn set_payout_addresses(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
    Json(request): Json<SetPayoutAddressesRequest>,
) -> Result<Json<IssuerResponse>, ApiError> {
    let id = issuer_id(&id)?;
    if request.payout_address_ids.len() > MAX_ADDRESSES_PER_ISSUER {
        return Err(ApiError::invalid_request(format!(
            "an identity may hold at most {MAX_ADDRESSES_PER_ISSUER} payout addresses"
        )));
    }
    let row = load(&state.issuers, account, id).await?;
    // Checked here rather than left to the foreign key, so an id belonging to
    // another account reads as a missing address instead of a server error.
    for address in &request.payout_address_ids {
        if state
            .issuers
            .get_payout_address(account, *address)
            .await?
            .is_none()
        {
            return Err(ApiError::payout_address_not_found());
        }
    }
    state
        .issuers
        .set_payout_addresses(account, id, &request.payout_address_ids)
        .await?;
    Ok(Json(with_addresses(&state.issuers, account, row).await?))
}

pub async fn create_payout_address(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Json(request): Json<PayoutAddressRequest>,
) -> Result<(StatusCode, Json<PayoutAddressResponse>), ApiError> {
    let address = Address::from_str(request.address.trim())
        .map_err(|error| ApiError::invalid_request(format!("invalid address: {error}")))?;
    if address.is_zero() {
        return Err(ApiError::invalid_request(
            "address must not be the zero address",
        ));
    }
    let label = match request.label.map(|value| value.trim().to_owned()) {
        Some(value) if value.is_empty() => None,
        Some(value) if !label_shape_ok(&value) => {
            return Err(ApiError::invalid_request(format!(
                "label must be at most {MAX_LABEL_LEN} characters, start with a letter or digit, \
                 and use only letters, digits, spaces, and . _ ' & ( ) -"
            )));
        }
        other => other,
    };
    let row = state
        .issuers
        .create_payout_address(&CreatePayoutAddressInput {
            id: Uuid::now_v7(),
            account_id: account,
            // Stored checksummed, which is also what makes "the same address
            // twice is one row" true regardless of how it was typed.
            address: address.to_checksum(None),
            label,
        })
        .await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

pub async fn list_payout_addresses(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
) -> Result<Json<PayoutAddressList>, ApiError> {
    Ok(Json(PayoutAddressList {
        payout_addresses: state
            .issuers
            .list_payout_addresses(account)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    }))
}

pub async fn delete_payout_address(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = Uuid::from_str(&id).map_err(|_| ApiError::payout_address_not_found())?;
    if state.issuers.delete_payout_address(account, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::payout_address_not_found())
    }
}

/// A name already spoken for is the caller's problem, not the database's.
fn duplicate_name_or(error: sqlx::Error) -> ApiError {
    if is_duplicate_issuer_name(&error) {
        ApiError::issuer_name_taken()
    } else {
        ApiError::from(error)
    }
}

async fn load(
    issuers: &IssuerRepository,
    account: AccountId,
    id: Uuid,
) -> Result<DbIssuer, ApiError> {
    issuers
        .get_for_account(account, id)
        .await?
        .ok_or_else(ApiError::issuer_not_found)
}

async fn with_addresses(
    issuers: &IssuerRepository,
    account: AccountId,
    row: DbIssuer,
) -> Result<IssuerResponse, ApiError> {
    let addresses = issuers.addresses_for_issuer(account, row.id).await?;
    Ok(IssuerResponse::build(
        row,
        addresses.into_iter().map(Into::into).collect(),
    ))
}

/// The same shape the column enforces: short, printable, and starting on a
/// letter or digit.
fn label_shape_ok(label: &str) -> bool {
    let mut characters = label.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && label.chars().count() <= MAX_LABEL_LEN
        && characters.all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, ' ' | '.' | '_' | '\'' | '&' | '(' | ')' | '-')
        })
}

fn issuer_id(value: &str) -> Result<Uuid, ApiError> {
    Uuid::from_str(value).map_err(|_| ApiError::issuer_not_found())
}

/// The same limits an invoice party carries, since that is what this becomes,
/// plus a contact address that is required rather than optional: it is the
/// thing being proven. Returns the normalized address to store.
fn validate(request: &IssuerRequest) -> Result<String, ApiError> {
    validate_party_fields(
        "",
        &request.name,
        Some(&request.contact_email),
        request.details.as_deref(),
    )?;
    let email = request.contact_email.trim().to_lowercase();
    if email.is_empty() {
        return Err(ApiError::invalid_request("contact_email is required"));
    }
    Ok(email)
}
