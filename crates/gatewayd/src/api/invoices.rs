//! POST /v1/invoices and GET /v1/invoices/:id handlers.

use std::str::FromStr;

use alloy_primitives::{Address, U256};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use uuid::Uuid;

use gateway_core::{
    Amount, BeneficiaryAddress, ChainId, CreateInvoiceRequest, FactoryAddress, Invoice,
    InvoiceResponse, TokenAddress, USDC_DECIMALS,
};

use crate::api::error::ApiError;
use crate::state::AppState;
use gateway_db::{CreateInvoiceInput, DbInvoice};

/// Project a DB row onto the wire response, going through the domain model so
/// the row is never serialized directly. Fails only if the stored row is
/// outside the schema contract (see [`gateway_db::DbInvoiceError`]).
///
/// A free function rather than a `TryFrom` impl: the orphan rule forbids
/// implementing a foreign trait for the foreign `InvoiceResponse` from a row
/// type that now also lives outside this crate.
fn to_response(row: DbInvoice) -> Result<InvoiceResponse, ApiError> {
    Ok(Invoice::try_from(&row)?.into())
}

// --- Handlers ---

pub async fn create_invoice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateInvoiceRequest>,
) -> Result<(axum::http::StatusCode, Json<InvoiceResponse>), ApiError> {
    // 1. Extract idempotency key from header.
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(ApiError::missing_idempotency_key)?;

    // 2. Parse and validate request fields.
    let chain_id: u64 = req
        .chain_id
        .parse()
        .map_err(|_| ApiError::invalid_request("chain_id must be a non-negative integer string"))?;

    let token_addr = Address::from_str(&req.token_address)
        .map_err(|e| ApiError::invalid_request(format!("invalid token_address: {e}")))?;
    let beneficiary_addr = Address::from_str(&req.beneficiary_address)
        .map_err(|e| ApiError::invalid_request(format!("invalid beneficiary_address: {e}")))?;

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
    if let Some(existing) = state.repo.find_by_idempotency_key(&idempotency_key).await? {
        if same_request(&existing, chain_id, token_addr, beneficiary_addr, &amount.0) {
            return Ok((axum::http::StatusCode::OK, Json(to_response(existing)?)));
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
    );

    // 7. Persist. ON CONFLICT handles the race between our check and insert.
    let input = CreateInvoiceInput::from_invoice(&invoice, idempotency_key.clone(), decimals);

    let inserted = state.repo.insert(&input).await?;

    match inserted {
        Some(row) => Ok((axum::http::StatusCode::CREATED, Json(to_response(row)?))),
        None => {
            // Race: another request won. Fetch their row and compare.
            let existing = state
                .repo
                .find_by_idempotency_key(&idempotency_key)
                .await?
                .expect("idempotency key must exist after ON CONFLICT");

            if same_request(&existing, chain_id, token_addr, beneficiary_addr, &amount.0) {
                Ok((axum::http::StatusCode::OK, Json(to_response(existing)?)))
            } else {
                Err(ApiError::idempotency_conflict())
            }
        }
    }
}

pub async fn get_invoice(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<InvoiceResponse>, ApiError> {
    let uuid =
        Uuid::from_str(&id).map_err(|_| ApiError::invalid_request("invalid invoice ID format"))?;

    let row = state
        .repo
        .find_by_id(uuid)
        .await?
        .ok_or_else(ApiError::invoice_not_found)?;

    Ok(Json(to_response(row)?))
}

/// Compare a persisted DB row against request parameters for idempotency replay.
fn same_request(
    row: &DbInvoice,
    chain_id: u64,
    token: Address,
    beneficiary: Address,
    amount: &U256,
) -> bool {
    row.chain_id as u64 == chain_id
        && row.token_address.as_slice() == token.as_slice()
        && row.beneficiary_address.as_slice() == beneficiary.as_slice()
        && U256::from_str_radix(&row.amount, 10)
            .map(|a| &a == amount)
            .unwrap_or(false)
}
