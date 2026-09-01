//! Customer invoice handlers.

use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy_primitives::utils::format_units;
use alloy_primitives::{Address, B256, U256};
use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue};
use uuid::Uuid;

use gateway_core::{
    Amount, AsOfDto, BeneficiaryAddress, CancelPaymentResponse, ChainId, CreatePaymentRequest,
    FactoryAddress, IndexerFreshnessDto, Invoice, PaymentListResponse, PaymentResponse,
    PaymentStatus, PaymentSummaryResponse, RecoveryAddress, TokenAddress, TransferDto,
    USDC_DECIMALS, parse_expiration, validate_expiration_window,
};
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::state::AppState;
use gateway_db::{AccountId, CreateInvoiceInput, DbInvoice};

const MAX_IDEMPOTENCY_KEY_BYTES: usize = 255;
const MAX_MEMO_BYTES: usize = 2000;

/// Project a DB row onto the wire response, going through the domain model so
/// the row is never serialized directly. Fails only if the stored row is
/// outside the schema contract (see [`gateway_db::DbInvoiceError`]).
///
/// A free function rather than a `TryFrom` impl: the orphan rule forbids
/// implementing a foreign trait for the foreign `PaymentResponse` from a row
/// type that now also lives outside this crate.
async fn to_response(
    state: &AppState,
    account: AccountId,
    row: DbInvoice,
) -> Result<PaymentResponse, ApiError> {
    let (transfers, freshness) = state
        .repo
        .response_metadata_for_account(account, &[row.id])
        .await?;
    enrich_response(state, row, transfers, freshness)
}

fn enrich_response(
    state: &AppState,
    row: DbInvoice,
    transfers: Vec<gateway_db::DbInvoiceTransfer>,
    freshness: Vec<gateway_db::DbIndexerFreshness>,
) -> Result<PaymentResponse, ApiError> {
    let expires_in = row
        .expiration_intent
        .strip_prefix("in:")
        .and_then(|v| v.parse().ok());
    let invoice = Invoice::try_from(&row)?;
    let mut response = PaymentResponse::from_invoice(invoice.clone(), expires_in);
    response.payment_url = state.payer.payment_url(&invoice)?;
    response.address_explorer_url = state.payer.address_url(&response.address);
    response.memo = row.memo.clone();
    response.reference = row.reference.clone();
    response.metadata = row.metadata.0.clone();
    response.created_at = row.created_at.to_rfc3339();
    response.updated_at = row.updated_at.to_rfc3339();
    response.paid_at = row.paid_at.map(|value| value.to_rfc3339());
    response.paid_at_block = row.funded_at_block.map(|value| value.to_string());
    response.expired_at = row.expired_at.map(|value| value.to_rfc3339());
    response.settlement_tx_hash = row
        .settlement_tx_hash
        .as_deref()
        .map(B256::try_from)
        .transpose()
        .map_err(|_| ApiError::internal("invalid settlement transaction hash"))?
        .map(|value| value.to_string());
    response.settlement_explorer_url = response
        .settlement_tx_hash
        .as_deref()
        .and_then(|hash| state.payer.transaction_url(hash));
    response.transfers = transfers
        .into_iter()
        .filter(|t| t.invoice_id == row.id)
        .map(|t| {
            let sender = Address::try_from(t.sender_address.as_slice())
                .map_err(|_| ApiError::internal("invalid transfer sender"))?;
            let hash = B256::try_from(t.transaction_hash.as_slice())
                .map_err(|_| ApiError::internal("invalid transfer hash"))?;
            let timestamp = sqlx::types::chrono::DateTime::from_timestamp(t.block_timestamp, 0)
                .ok_or_else(|| ApiError::internal("invalid transfer timestamp"))?;
            let units = U256::from_str_radix(&t.amount, 10)
                .map_err(|_| ApiError::internal("invalid transfer amount"))?;
            Ok(TransferDto {
                timestamp: timestamp.to_rfc3339(),
                amount: format_units(units, USDC_DECIMALS).unwrap_or_default(),
                amount_base_units: t.amount,
                sender: sender.to_checksum(None),
                transaction_hash: hash.to_string(),
                explorer_url: state.payer.transaction_url(&hash.to_string()),
                block: t.block_number.to_string(),
                disposition: if t.disposition == "error" {
                    "zero".into()
                } else {
                    t.disposition
                },
                collected: t.collected,
            })
        })
        .collect::<Result<_, ApiError>>()?;
    let cursor = freshness
        .into_iter()
        .find(|f| f.chain_id == row.chain_id && f.token_address == row.token_address);
    response.as_of = cursor.as_ref().and_then(|cursor| {
        Some(AsOfDto {
            block: cursor.last_block.to_string(),
            at: sqlx::types::chrono::DateTime::from_timestamp(cursor.last_block_timestamp?, 0)?
                .to_rfc3339(),
        })
    });
    response.indexer_freshness = IndexerFreshnessDto {
        last_indexed_block: cursor.as_ref().map(|c| c.last_block.to_string()),
        last_finalized_block: cursor
            .as_ref()
            .and_then(|c| c.finalized_block)
            .map(|block| block.to_string()),
        cursor_updated_at: cursor.map(|c| c.updated_at.to_rfc3339()),
    };
    Ok(response)
}

// --- Handlers ---

pub async fn create_payment(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    headers: HeaderMap,
    Json(req): Json<CreatePaymentRequest>,
) -> Result<(axum::http::StatusCode, HeaderMap, Json<PaymentResponse>), ApiError> {
    let memo = req.memo.clone();
    if req.reference.is_some() && memo.is_some() && req.reference != memo {
        return Err(ApiError::invalid_request(
            "reference and memo must match when both are supplied",
        ));
    }
    let reference = req.reference.clone().or_else(|| memo.clone());
    validate_customer_fields(reference.as_deref(), &req.metadata)?;
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
    if memo
        .as_ref()
        .is_some_and(|memo| memo.len() > MAX_MEMO_BYTES)
    {
        return Err(ApiError::invalid_request(format!(
            "memo must be at most {MAX_MEMO_BYTES} bytes"
        )));
    }

    // 2. Parse and validate request fields.
    let chain_id: u64 = req
        .chain_id
        .as_deref()
        .map_or(Ok(state.chain_id.0), |value| {
            value.parse().map_err(|_| {
                ApiError::invalid_request("chain_id must be a non-negative integer string")
            })
        })?;

    let token_addr = req
        .token_address
        .as_deref()
        .map_or(Ok(state.usdc_address), |value| {
            Address::from_str(value)
                .map_err(|e| ApiError::invalid_request(format!("invalid token_address: {e}")))
        })?;
    let beneficiary_addr = Address::from_str(&req.payout_address)
        .map_err(|e| ApiError::invalid_request(format!("invalid payout_address: {e}")))?;
    if beneficiary_addr.is_zero() {
        return Err(ApiError::invalid_request(
            "payout_address must not be the zero address",
        ));
    }
    let expires_in_text = req.expires_in.map(|value| format!("{value}s"));
    let now = unix_now();
    let expiration = parse_expiration(
        expires_in_text.as_deref(),
        req.expires_at.as_deref(),
        None,
        now,
    )
    .map_err(|error| ApiError::invalid_request(error.to_string()))?;
    let expiration_timestamp = expiration.timestamp;
    let recovery_addr = req
        .refund_address
        .as_deref()
        .map_or(Ok(beneficiary_addr), |value| {
            Address::from_str(value)
                .map_err(|e| ApiError::invalid_request(format!("invalid refund_address: {e}")))
        })?;
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
    let requested = RequestedInvoice {
        chain_id,
        token: token_addr,
        beneficiary: beneficiary_addr,
        amount: amount.0,
        expiration_intent: &expiration.intent,
        recovery: recovery_addr,
        memo: memo.as_deref(),
        reference: reference.as_deref(),
        metadata: &req.metadata,
    };

    // 5. Check for existing idempotency key before generating anything.
    if let Some(existing) = state
        .repo
        .find_by_idempotency_key(account, &idempotency_key)
        .await?
    {
        if same_request(&existing, &requested) {
            let mut response_headers = HeaderMap::new();
            response_headers.insert("Idempotency-Replayed", HeaderValue::from_static("true"));
            return Ok((
                axum::http::StatusCode::OK,
                response_headers,
                Json(to_response(&state, account, existing).await?),
            ));
        } else {
            return Err(ApiError::idempotency_conflict());
        }
    }

    if !state.accounts.has_verified_email(account).await? {
        return Err(ApiError::account_contact_required());
    }
    validate_expiration_window(&expiration, now)
        .map_err(|error| ApiError::invalid_request(error.to_string()))?;

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
    let mut input = CreateInvoiceInput::from_invoice(
        &invoice,
        account,
        idempotency_key.clone(),
        decimals,
        memo.clone(),
        expiration_timestamp - now,
        expiration.intent.clone(),
    );
    input.reference = reference.clone();
    input.metadata = req.metadata.clone();

    let inserted = state.repo.insert(&input).await?;

    match inserted {
        Some(row) => Ok((
            axum::http::StatusCode::CREATED,
            HeaderMap::new(),
            Json(to_response(&state, account, row).await?),
        )),
        None => {
            // Race: another request won. Fetch their row and compare.
            let existing = state
                .repo
                .find_by_idempotency_key(account, &idempotency_key)
                .await?
                .expect("idempotency key must exist after ON CONFLICT");

            if same_request(&existing, &requested) {
                let mut response_headers = HeaderMap::new();
                response_headers.insert("Idempotency-Replayed", HeaderValue::from_static("true"));
                Ok((
                    axum::http::StatusCode::OK,
                    response_headers,
                    Json(to_response(&state, account, existing).await?),
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
    Path(reference): Path<String>,
    Query(query): Query<GetQuery>,
) -> Result<Json<PaymentResponse>, ApiError> {
    let mut row = resolve_payment(&state, account, &reference).await?;
    if query.wait_for.is_none() && query.timeout.is_some() {
        return Err(ApiError::invalid_request(
            "timeout requires wait_for=change",
        ));
    }
    if let Some(wait_for) = query.wait_for {
        if wait_for != "change" {
            return Err(ApiError::invalid_request("wait_for must be change"));
        }
        let timeout = query.timeout.unwrap_or(30);
        if !(1..=30).contains(&timeout) {
            return Err(ApiError::invalid_request(
                "timeout must be between 1 and 30 seconds",
            ));
        }
        let initial = row.updated_at;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
        loop {
            tokio::time::sleep_until(
                (tokio::time::Instant::now() + Duration::from_millis(250)).min(deadline),
            )
            .await;
            row = resolve_payment(&state, account, &reference).await?;
            if row.updated_at != initial || tokio::time::Instant::now() >= deadline {
                break;
            }
        }
    }

    Ok(Json(to_response(&state, account, row).await?))
}

pub async fn transfers(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(reference): Path<String>,
) -> Result<Json<Vec<TransferDto>>, ApiError> {
    let row = resolve_payment(&state, account, &reference).await?;
    Ok(Json(to_response(&state, account, row).await?.transfers))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetQuery {
    wait_for: Option<String>,
    timeout: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    status: Option<String>,
    reference: Option<String>,
    limit: Option<u32>,
    starting_after: Option<String>,
}

pub async fn list_payments(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Query(query): Query<ListQuery>,
) -> Result<Json<PaymentListResponse>, ApiError> {
    let status = query
        .status
        .as_deref()
        .map(str::parse::<PaymentStatus>)
        .transpose()
        .map_err(|_| ApiError::invalid_request("unknown payment status"))?;
    let limit = query.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::invalid_request("limit must be between 1 and 100"));
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
        .list_for_account(
            account,
            status.map(PaymentStatus::as_str),
            query.reference.as_deref(),
            starting_after,
            limit,
        )
        .await?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = has_more.then(|| format!("pay_{}", rows.last().expect("nonzero limit").id));
    let payments = rows
        .into_iter()
        .map(|row| -> Result<_, ApiError> {
            let response = PaymentResponse::from_invoice(Invoice::try_from(&row)?, None);
            Ok(PaymentSummaryResponse {
                id: response.id,
                memo: row.memo,
                reference: row.reference,
                metadata: row.metadata.0,
                created_at: row.created_at.to_rfc3339(),
                status: response.status,
                amount: response.amount,
                received: response.received,
                cancellation_requested_at: row.cancellation_requested_at.map(|v| v.to_rfc3339()),
            })
        })
        .collect::<Result<_, _>>()?;
    Ok(Json(PaymentListResponse {
        payments,
        next_cursor,
    }))
}

fn full_payment_id(value: &str) -> Result<Uuid, ApiError> {
    let suffix = value.strip_prefix("pay_").ok_or_else(|| {
        ApiError::invalid_request("starting_after must be a complete pay_ payment ID")
    })?;
    Uuid::parse_str(suffix)
        .map_err(|_| ApiError::invalid_request("starting_after must be a complete pay_ payment ID"))
}

pub async fn cancel_payment(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(reference): Path<String>,
) -> Result<Json<CancelPaymentResponse>, ApiError> {
    let invoice = resolve_payment(&state, account, &reference).await?;
    let row = state
        .repo
        .request_cancellation(account, invoice.id)
        .await?
        .ok_or_else(ApiError::payment_not_found)?;
    Ok(Json(CancelPaymentResponse {
        payment: to_response(&state, account, row).await?,
        advisory: "Cancellation is advisory only and does not alter the payment contract or its encoded settlement terms".into(),
    }))
}

async fn resolve_payment(
    state: &AppState,
    account: AccountId,
    reference: &str,
) -> Result<DbInvoice, ApiError> {
    if let Some(uuid) = gateway_core::payment_id(reference) {
        return state
            .repo
            .find_by_id_for_account(account, uuid)
            .await?
            .ok_or_else(ApiError::payment_not_found);
    }
    if let Ok(address) = Address::from_str(reference) {
        return state
            .repo
            .find_by_payment_address_for_account(account, address.as_slice())
            .await?
            .ok_or_else(ApiError::payment_not_found);
    }
    Err(ApiError::invalid_payment_reference())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Compare a persisted DB row against request parameters for idempotency replay.
struct RequestedInvoice<'a> {
    chain_id: u64,
    token: Address,
    beneficiary: Address,
    amount: U256,
    expiration_intent: &'a str,
    recovery: Address,
    memo: Option<&'a str>,
    reference: Option<&'a str>,
    metadata: &'a serde_json::Value,
}

fn same_request(row: &DbInvoice, request: &RequestedInvoice<'_>) -> bool {
    row.chain_id as u64 == request.chain_id
        && row.token_address.as_slice() == request.token.as_slice()
        && row.beneficiary_address.as_slice() == request.beneficiary.as_slice()
        && row.expiration_intent == request.expiration_intent
        && row.recovery_address.as_slice() == request.recovery.as_slice()
        && row.memo.as_deref() == request.memo
        && row.reference.as_deref() == request.reference
        && row.metadata.0 == *request.metadata
        && U256::from_str_radix(&row.amount, 10)
            .map(|amount| amount == request.amount)
            .unwrap_or(false)
}

fn validate_customer_fields(
    reference: Option<&str>,
    metadata: &serde_json::Value,
) -> Result<(), ApiError> {
    if reference.is_some_and(|value| value.chars().count() > 128) {
        return Err(ApiError::invalid_request(
            "reference must contain at most 128 characters",
        ));
    }
    let serde_json::Value::Object(entries) = metadata else {
        return Err(ApiError::invalid_request("metadata must be a JSON object"));
    };
    if entries.len() > 16 {
        return Err(ApiError::invalid_request(
            "metadata must contain at most 16 keys",
        ));
    }
    if entries
        .values()
        .any(|value| serde_json::to_vec(value).is_ok_and(|encoded| encoded.len() > 512))
    {
        return Err(ApiError::invalid_request(
            "each metadata value must be at most 512 bytes",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_wire_shape_accepts_optional_defaults_and_memo() {
        let json = serde_json::json!({
            "chain_id": "1", "token_address": "0x0000000000000000000000000000000000000001",
            "payout_address": "0x0000000000000000000000000000000000000002",
            "amount": "1", "expires_in": 3600,
            "refund_address": "0x0000000000000000000000000000000000000003",
            "memo": "order-42"
        });
        let request: CreatePaymentRequest = serde_json::from_value(json).unwrap();
        assert_eq!(request.memo.as_deref(), Some("order-42"));
        assert_eq!(request.amount, "1");
    }
}
