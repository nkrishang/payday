//! Customer-facing payment handlers backed by the internal invoice domain.

use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy_primitives::{Address, U256};
use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use serde::Deserialize;
use uuid::Uuid;

use gateway_core::{
    Amount, BeneficiaryAddress, ChainId, CreatePaymentRequest, FactoryAddress, Invoice,
    PaymentListResponse, PaymentResponse, RecoveryAddress, TokenAddress, USDC_DECIMALS,
    payment_id_prefix,
};

use crate::api::error::ApiError;
use crate::state::AppState;
use gateway_db::{AccountId, CreateInvoiceInput, DbInvoice};

/// An invoice must stay payable for at least this long. It leaves room for
/// the payer to act and for finality plus a sweep to land before the contract
/// starts routing to the recovery address.
pub const MIN_EXPIRATION_LEAD_SECS: u64 = 10 * 60;
/// Upper bound on the deadline. Catches timestamps given in milliseconds or
/// otherwise nonsensical values while still allowing long-lived invoices.
pub const MAX_EXPIRATION_LEAD_SECS: u64 = 366 * 24 * 60 * 60;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 255;
const DEFAULT_LIST_LIMIT: u32 = 20;
const MAX_LIST_LIMIT: u32 = 100;

#[derive(Debug, Deserialize)]
pub struct ListPaymentsQuery {
    limit: Option<u32>,
    starting_after: Option<String>,
}

/// Project a DB row onto the wire response, going through the domain model so
/// the row is never serialized directly. Fails only if the stored row is
/// outside the schema contract (see [`gateway_db::DbInvoiceError`]).
///
/// A free function rather than a `TryFrom` impl: the orphan rule forbids
/// implementing a foreign trait for the foreign `PaymentResponse` from a row
/// type that now also lives outside this crate.
fn to_response(row: DbInvoice, expires_in: Option<u64>) -> Result<PaymentResponse, ApiError> {
    Ok(PaymentResponse::from_invoice(
        Invoice::try_from(&row)?,
        expires_in,
    ))
}

// --- Handlers ---

pub async fn create_payment(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    headers: HeaderMap,
    Json(req): Json<CreatePaymentRequest>,
) -> Result<(axum::http::StatusCode, Json<PaymentResponse>), ApiError> {
    // 1. Extract idempotency key from header.
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(ApiError::missing_idempotency_key)?;
    if idempotency_key.is_empty() || idempotency_key.len() > MAX_IDEMPOTENCY_KEY_BYTES {
        return Err(ApiError::invalid_request(format!(
            "Idempotency-Key must be 1 to {MAX_IDEMPOTENCY_KEY_BYTES} bytes"
        )));
    }

    // 2. Parse and validate request fields.
    let chain_id: u64 = req
        .chain_id
        .parse()
        .map_err(|_| ApiError::invalid_request("chain_id must be a non-negative integer string"))?;

    let token_addr = Address::from_str(&req.token_address)
        .map_err(|e| ApiError::invalid_request(format!("invalid token_address: {e}")))?;
    let beneficiary_addr = Address::from_str(&req.payout_address)
        .map_err(|e| ApiError::invalid_request(format!("invalid payout_address: {e}")))?;
    if beneficiary_addr.is_zero() {
        return Err(ApiError::invalid_request(
            "payout_address must not be the zero address",
        ));
    }
    validate_expiration(req.expires_in)?;
    let expiration_timestamp = unix_now()
        .checked_add(req.expires_in)
        .ok_or_else(|| ApiError::invalid_request("expires_in is too large"))?;
    let recovery_addr = Address::from_str(&req.refund_address)
        .map_err(|e| ApiError::invalid_request(format!("invalid refund_address: {e}")))?;
    if recovery_addr.is_zero() {
        return Err(ApiError::invalid_request(
            "refund_address must not be the zero address",
        ));
    }

    // 3. Enforce the configured chain and Circle-issued USDC contract.
    if chain_id != state.chain_id.0 {
        return Err(ApiError::unsupported_chain());
    }

    let token = TokenAddress(token_addr);
    if token_addr != state.usdc_address {
        return Err(ApiError::unsupported_token());
    }

    // 4. Parse amount using token decimals.
    let decimals = USDC_DECIMALS;
    let amount = Amount::from_decimal_str(&req.amount, decimals)
        .map_err(|e| ApiError::invalid_amount(e.to_string()))?;

    if amount.0 == U256::ZERO {
        return Err(ApiError::invalid_amount("amount must be positive"));
    }

    // 5. Check for existing idempotency key before generating anything.
    if let Some(existing) = state
        .repo
        .find_by_idempotency_key(account, &idempotency_key)
        .await?
    {
        if same_request(
            &existing,
            chain_id,
            token_addr,
            beneficiary_addr,
            &amount.0,
            req.expires_in,
            recovery_addr,
        ) {
            return Ok((
                axum::http::StatusCode::OK,
                Json(to_response(
                    existing.clone(),
                    Some(existing.expires_in_secs as u64),
                )?),
            ));
        } else {
            return Err(ApiError::idempotency_conflict());
        }
    }

    // 6. Create the domain invoice (generates ID, salt, payment address).
    let invoice = Invoice::new(
        FactoryAddress(state.factory_address),
        ChainId(chain_id),
        token,
        BeneficiaryAddress(beneficiary_addr),
        amount,
        expiration_timestamp,
        RecoveryAddress(recovery_addr),
    );

    // 7. Persist. ON CONFLICT handles the race between our check and insert.
    let input = CreateInvoiceInput::from_invoice(
        &invoice,
        account,
        idempotency_key.clone(),
        decimals,
        req.expires_in,
    );

    let inserted = state.repo.insert(&input).await?;

    match inserted {
        Some(row) => {
            let expires_in = row.expires_in_secs as u64;
            Ok((
                axum::http::StatusCode::CREATED,
                Json(to_response(row, Some(expires_in))?),
            ))
        }
        None => {
            // Race: another request won. Fetch their row and compare.
            let existing = state
                .repo
                .find_by_idempotency_key(account, &idempotency_key)
                .await?
                .expect("idempotency key must exist after ON CONFLICT");

            if same_request(
                &existing,
                chain_id,
                token_addr,
                beneficiary_addr,
                &amount.0,
                req.expires_in,
                recovery_addr,
            ) {
                let expires_in = existing.expires_in_secs as u64;
                Ok((
                    axum::http::StatusCode::OK,
                    Json(to_response(existing, Some(expires_in))?),
                ))
            } else {
                Err(ApiError::idempotency_conflict())
            }
        }
    }
}

pub async fn get_payment(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
) -> Result<Json<PaymentResponse>, ApiError> {
    let prefix = payment_id_prefix(&id).ok_or_else(|| {
        ApiError::invalid_request("payment ID must start with pay_ and contain a UUIDv7 prefix")
    })?;
    let mut rows = state
        .repo
        .find_by_prefix_for_account(account, prefix)
        .await?;
    match rows.len() {
        0 => Err(ApiError::payment_not_found()),
        1 => Ok(Json(to_response(rows.pop().unwrap(), None)?)),
        _ => Err(ApiError::ambiguous_payment_id()),
    }
}

pub async fn list_payments(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Query(query): Query<ListPaymentsQuery>,
) -> Result<Json<PaymentListResponse>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_LIST_LIMIT);
    if !(1..=MAX_LIST_LIMIT).contains(&limit) {
        return Err(ApiError::invalid_request(format!(
            "limit must be between 1 and {MAX_LIST_LIMIT}"
        )));
    }
    let starting_after = query
        .starting_after
        .as_deref()
        .map(full_payment_id)
        .transpose()?;
    if let Some(cursor) = starting_after
        && state
            .repo
            .find_by_id_for_account(account, cursor)
            .await?
            .is_none()
    {
        return Err(ApiError::invalid_request(
            "starting_after does not identify one of your payments",
        ));
    }
    let mut rows = state
        .repo
        .list_for_account(account, starting_after, limit)
        .await?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = has_more.then(|| format!("pay_{}", rows.last().expect("nonzero limit").id));
    let payments = rows
        .into_iter()
        .map(|row| to_response(row, None))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(PaymentListResponse {
        payments,
        next_cursor,
    }))
}

fn full_payment_id(value: &str) -> Result<Uuid, ApiError> {
    let suffix = payment_id_prefix(value)
        .filter(|suffix| suffix.len() == 36)
        .ok_or_else(|| {
            ApiError::invalid_request("starting_after must be a complete pay_ payment ID")
        })?;
    Uuid::parse_str(suffix)
        .map_err(|_| ApiError::invalid_request("starting_after must be a complete pay_ payment ID"))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// The deadline must give the payer and the sweep pipeline room, and must be
/// a plausible seconds-based timestamp.
fn validate_expiration(expires_in: u64) -> Result<(), ApiError> {
    if expires_in < MIN_EXPIRATION_LEAD_SECS {
        return Err(ApiError::invalid_request(format!(
            "expires_in must be at least {MIN_EXPIRATION_LEAD_SECS} seconds"
        )));
    }
    if expires_in > MAX_EXPIRATION_LEAD_SECS {
        return Err(ApiError::invalid_request(format!(
            "expires_in must be at most {MAX_EXPIRATION_LEAD_SECS} seconds"
        )));
    }
    Ok(())
}

/// Compare a persisted DB row against request parameters for idempotency replay.
fn same_request(
    row: &DbInvoice,
    chain_id: u64,
    token: Address,
    beneficiary: Address,
    amount: &U256,
    expires_in: u64,
    recovery: Address,
) -> bool {
    row.chain_id as u64 == chain_id
        && row.token_address.as_slice() == token.as_slice()
        && row.beneficiary_address.as_slice() == beneficiary.as_slice()
        && row.expires_in_secs as u64 == expires_in
        && row.recovery_address.as_slice() == recovery.as_slice()
        && U256::from_str_radix(&row.amount, 10)
            .map(|a| &a == amount)
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiration_must_sit_inside_the_allowed_window() {
        assert!(validate_expiration(MIN_EXPIRATION_LEAD_SECS).is_ok());
        assert!(validate_expiration(MAX_EXPIRATION_LEAD_SECS).is_ok());
        for invalid in [
            0,
            MIN_EXPIRATION_LEAD_SECS - 1,
            MAX_EXPIRATION_LEAD_SECS + 1,
            u64::MAX,
        ] {
            assert!(
                validate_expiration(invalid).is_err(),
                "{invalid} must be rejected"
            );
        }
    }
}
