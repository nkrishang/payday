//! POST /v1/invoices and GET /v1/invoices/:id handlers.

use std::str::FromStr;

use alloy_primitives::{Address, U256, utils::format_units};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use gateway_core::{
    Amount, BeneficiaryAddress, ChainId, FactoryAddress, Invoice, NATIVE_TOKEN_DECIMALS,
    TokenAddress,
};

use crate::api::error::ApiError;
use crate::db::{CreateInvoiceInput, DbInvoice};
use crate::state::AppState;

// --- Request DTO ---

#[derive(Debug, Deserialize)]
pub struct CreateInvoiceRequest {
    pub chain_id: String,
    pub token_address: String,
    pub beneficiary_address: String,
    pub amount: String,
}

// --- Response DTO ---

#[derive(Debug, Serialize)]
pub struct InvoiceResponse {
    pub id: String,
    pub chain_id: String,
    pub factory_address: String,
    pub token: TokenDto,
    pub beneficiary_address: String,
    pub amount: String,
    pub amount_base_units: String,
    pub salt: String,
    pub payment_address: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct TokenDto {
    pub address: String,
    pub kind: &'static str,
    pub decimals: u8,
}

impl From<Invoice> for InvoiceResponse {
    fn from(inv: Invoice) -> Self {
        let decimals = NATIVE_TOKEN_DECIMALS;
        let amount_human = format_units(inv.amount.0, decimals).unwrap_or_default();
        let kind = if inv.token.is_native() {
            "native"
        } else {
            "erc20"
        };

        InvoiceResponse {
            id: inv.id.0.to_string(),
            chain_id: inv.chain_id.0.to_string(),
            factory_address: inv.factory.0.to_checksum(None),
            token: TokenDto {
                address: inv.token.0.to_checksum(None),
                kind,
                decimals,
            },
            beneficiary_address: inv.beneficiary.0.to_checksum(None),
            amount: amount_human,
            amount_base_units: inv.amount.0.to_string(),
            salt: inv.salt.0.to_string(),
            payment_address: inv.payment_address.0.to_checksum(None),
            status: match inv.status {
                gateway_core::InvoiceStatus::Created => "created",
                gateway_core::InvoiceStatus::Funded => "funded",
                gateway_core::InvoiceStatus::Deploying => "deploying",
                gateway_core::InvoiceStatus::Fulfilled => "fulfilled",
                gateway_core::InvoiceStatus::Failed => "failed",
            }
            .to_string(),
        }
    }
}

impl From<DbInvoice> for InvoiceResponse {
    fn from(row: DbInvoice) -> Self {
        let inv = row.to_domain();
        Self::from(inv)
    }
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

    // 3. Enforce milestone constraints: one chain, native token only.
    if chain_id != state.chain_id.0 {
        return Err(ApiError::unsupported_chain());
    }

    let token = TokenAddress(token_addr);
    if !token.is_native() {
        return Err(ApiError::unsupported_token());
    }

    // 4. Parse amount using token decimals.
    let decimals = NATIVE_TOKEN_DECIMALS;
    let amount = Amount::from_decimal_str(&req.amount, decimals)
        .map_err(|e| ApiError::invalid_amount(e.to_string()))?;

    if amount.0 == U256::ZERO {
        return Err(ApiError::invalid_amount("amount must be positive"));
    }

    // 5. Check for existing idempotency key before generating anything.
    if let Some(existing) = state.repo.find_by_idempotency_key(&idempotency_key).await? {
        if same_request(&existing, chain_id, token_addr, beneficiary_addr, &amount.0) {
            return Ok((
                axum::http::StatusCode::OK,
                Json(InvoiceResponse::from(existing)),
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
    );

    // 7. Persist. ON CONFLICT handles the race between our check and insert.
    let input = CreateInvoiceInput {
        id: invoice.id.0,
        idempotency_key: idempotency_key.clone(),
        chain_id,
        factory_address: state.factory_address.into(),
        token_address: token_addr.into(),
        token_decimals: decimals,
        beneficiary_address: beneficiary_addr.into(),
        amount: invoice.amount.0.to_string(),
        salt: invoice.salt.0.into(),
        payment_address: invoice.payment_address.0.into(),
    };

    let inserted = state.repo.insert(&input).await?;

    match inserted {
        Some(row) => Ok((
            axum::http::StatusCode::CREATED,
            Json(InvoiceResponse::from(row)),
        )),
        None => {
            // Race: another request won. Fetch their row and compare.
            let existing = state
                .repo
                .find_by_idempotency_key(&idempotency_key)
                .await?
                .expect("idempotency key must exist after ON CONFLICT");

            if same_request(&existing, chain_id, token_addr, beneficiary_addr, &amount.0) {
                Ok((
                    axum::http::StatusCode::OK,
                    Json(InvoiceResponse::from(existing)),
                ))
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

    Ok(Json(InvoiceResponse::from(row)))
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
