//! Customer invoice handlers.

use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy_primitives::utils::format_units;
use alloy_primitives::{Address, B256, U256};
use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use gateway_core::{
    Amount, AsOfDto, AttachmentCommitment, BeneficiaryAddress, CancelPaymentResponse,
    CanonicalIssuanceSnapshot, ChainId, CreatePaymentRequest, FactoryAddress, IndexerFreshnessDto,
    Invoice, PDF_MIME_TYPE, Party, PayerPolicy, PaymentListResponse, PaymentResponse,
    PaymentStatus, PaymentSummaryResponse, RecoveryAddress, TokenAddress, TransferDto,
    USDC_DECIMALS, parse_expiration, validate_expiration_window,
};
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::attachments::{AttachmentError, StorageError, content_disposition};
use crate::invoice_pdf::render_invoice_pdf;
use crate::state::AppState;
use gateway_db::{
    AccountId, AttachmentStatus, CreateInvoiceInput, DbAttachment, DbInvoice,
    InsertIssuedInvoiceError,
};

const MAX_IDEMPOTENCY_KEY_BYTES: usize = 255;
const MAX_PARTY_NAME_BYTES: usize = 255;
const PARTY_EMAIL_BYTES: std::ops::RangeInclusive<usize> = 3..=254;
const MAX_PARTY_DETAILS_BYTES: usize = 4000;
const MAX_NOTES_BYTES: usize = 4000;
const MAX_HEADING_BYTES: usize = 200;
const MAX_REFERENCE_CHARS: usize = 128;

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
    let attachment = state.attachments.find_by_invoice(row.id).await?;
    enrich_response(state, row, attachment, transfers, freshness)
}

fn enrich_response(
    state: &AppState,
    row: DbInvoice,
    attachment: Option<DbAttachment>,
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
    response.metadata = row.metadata.0.clone();
    response.customer_id = row.customer_id.map(|id| id.to_string());
    // The signed download link is added by the attachment route.
    response.attachment = attachment.as_ref().and_then(DbAttachment::descriptor);
    response.verification_completed_at = row.verification_completed_at.map(|v| v.to_rfc3339());
    response.likely_unsolicited_at = row.likely_unsolicited_at.map(|v| v.to_rfc3339());
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
) -> Result<(StatusCode, HeaderMap, Json<PaymentResponse>), ApiError> {
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

    // 2. Parse and validate every request field.
    validate_document(&req)?;
    let payer_policy = req.payer_policy.normalized();
    payer_policy
        .validate()
        .map_err(|error| ApiError::invalid_request(error.to_string()))?;
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

    // 5. The customer link and the attachment must be this account's. The
    // attachment's readiness is checked only for a new issuance: a replay
    // compares against a document that is, by then, attached.
    if let Some(customer_id) = req.customer_id
        && state
            .customers
            .get_for_account(account, customer_id)
            .await?
            .is_none()
    {
        return Err(ApiError::invalid_request(
            "customer_id does not identify one of your customers",
        ));
    }
    let attachment = match req.attachment_id {
        Some(attachment_id) => Some(
            state
                .attachments
                .get_for_account(account, attachment_id)
                .await?
                .ok_or_else(|| {
                    ApiError::invalid_request("attachment_id does not identify one of your uploads")
                })?,
        ),
        None => None,
    };

    // Recovery is the platform wallet, never a request field. It is part of
    // the replay comparison because it is committed into the payment address:
    // a replay after the platform wallet changed cannot return the old address
    // as if it were equivalent.
    let requested = RequestedInvoice {
        chain_id,
        factory: state.factory_address,
        token: token_addr,
        beneficiary: beneficiary_addr,
        amount: amount.0,
        expiration_intent: &expiration.intent,
        recovery: state.recovery_address,
        issuer: &req.issuer,
        bill_to: &req.bill_to,
        notes: req.notes.as_deref(),
        heading: req.heading.as_deref(),
        reference: req.reference.as_deref(),
        metadata: &req.metadata,
        customer_id: req.customer_id,
        payer_policy: &payer_policy,
        attachment_id: req.attachment_id,
        attachment: attachment.as_ref().and_then(DbAttachment::commitment),
    };

    // 6. Check for existing idempotency key before generating anything.
    if let Some(existing) = state
        .repo
        .find_by_idempotency_key(account, &idempotency_key)
        .await?
    {
        if same_request(&existing, &requested) {
            return Ok(replayed(to_response(&state, account, existing).await?));
        } else {
            return Err(ApiError::idempotency_conflict());
        }
    }

    if !state.accounts.has_verified_email(account).await? {
        return Err(ApiError::account_contact_required());
    }
    validate_expiration_window(&expiration, now)
        .map_err(|error| ApiError::invalid_request(error.to_string()))?;
    if let Some(attachment) = &attachment {
        match attachment.status.parse::<AttachmentStatus>() {
            Ok(AttachmentStatus::Ready) => {}
            Ok(AttachmentStatus::Attached) => return Err(ApiError::attachment_already_attached()),
            _ if attachment.is_expired() => return Err(ApiError::attachment_expired()),
            _ => return Err(ApiError::attachment_not_ready()),
        }
        // The bucket expires uploads that are still tagged pending; retag
        // before the row is bound so an attached PDF can never be expired.
        // Failing here aborts issuance rather than issuing an invoice whose
        // document could vanish. A ready upload nobody issued with in time
        // is already gone: the row learns it here.
        match state
            .attachment_store()?
            .mark_attached(&attachment.object_key, attachment.version_id.as_deref())
            .await
        {
            Ok(()) => {}
            Err(AttachmentError::Storage(StorageError::NotFound)) => {
                crate::api::attachments::expire(&state, account, attachment).await?;
                return Err(ApiError::attachment_expired());
            }
            Err(error) => {
                tracing::error!(error = %error, attachment_id = %attachment.id, "failed to retag the attachment as attached");
                return Err(ApiError::internal("Attachment storage is unavailable"));
            }
        }
    }

    // 7. Commit to the document and derive the address from that commitment.
    let factory = FactoryAddress(state.factory_address);
    let beneficiary = BeneficiaryAddress(beneficiary_addr);
    let recovery = RecoveryAddress(state.recovery_address);
    let mut snapshot = CanonicalIssuanceSnapshot::new(
        req.issuer.clone(),
        req.bill_to.clone(),
        payer_policy.clone(),
        factory,
        ChainId(chain_id),
        token,
        beneficiary,
        amount,
        expiration_timestamp,
        recovery,
    );
    snapshot.notes = req.notes.clone();
    snapshot.heading = req.heading.clone();
    snapshot.reference = req.reference.clone();
    snapshot.attachment = requested.attachment.clone();
    let invoice = Invoice::issue(
        factory,
        ChainId(chain_id),
        token,
        beneficiary,
        amount,
        expiration_timestamp,
        recovery,
        snapshot,
    )
    .map_err(|error| {
        tracing::error!(error = %error, "invoice issuance failed");
        ApiError::internal("failed to issue the invoice")
    })?;

    // 8. Persist invoice and attachment atomically. The repository locks the
    // key and compares every immutable field again, so a race with another
    // request for the same key resolves to a replay or a conflict.
    let mut input = CreateInvoiceInput::from_invoice(
        &invoice,
        account,
        idempotency_key.clone(),
        decimals,
        expiration_timestamp - now,
        expiration.intent.clone(),
    );
    input.customer_id = req.customer_id;
    input.metadata = req.metadata.clone();

    let issued = match state.repo.insert_issued(&input, req.attachment_id).await {
        Ok(issued) => issued,
        Err(error) => {
            // The retag above preceded a transaction that did not commit, so
            // the object would otherwise sit in the bucket forever, exempt
            // from expiry and bound to nothing. Hand it back to the lifecycle
            // rule; the request's outcome is the insert's error either way.
            if let Some(attachment) = &attachment
                && let Err(restore) = state
                    .attachment_store()?
                    .restore_pending(&attachment.object_key, attachment.version_id.as_deref())
                    .await
            {
                tracing::warn!(error = %restore, attachment_id = %attachment.id, "failed to restore the pending tag after issuance did not commit");
            }
            return Err(match error {
                InsertIssuedInvoiceError::IdempotencyConflict => ApiError::idempotency_conflict(),
                InsertIssuedInvoiceError::AttachmentNotFound => {
                    ApiError::invalid_request("attachment_id does not identify one of your uploads")
                }
                InsertIssuedInvoiceError::AttachmentNotReady => ApiError::attachment_not_ready(),
                InsertIssuedInvoiceError::AttachmentAlreadyAttached => {
                    ApiError::attachment_already_attached()
                }
                InsertIssuedInvoiceError::Database(error) => error.into(),
            });
        }
    };
    let (transfers, freshness) = state
        .repo
        .response_metadata_for_account(account, &[issued.row.id])
        .await?;
    let response = enrich_response(&state, issued.row, issued.attachment, transfers, freshness)?;
    if issued.replayed {
        Ok(replayed(response))
    } else {
        Ok((StatusCode::CREATED, HeaderMap::new(), Json(response)))
    }
}

fn replayed(response: PaymentResponse) -> (StatusCode, HeaderMap, Json<PaymentResponse>) {
    let mut response_headers = HeaderMap::new();
    response_headers.insert("Idempotency-Replayed", HeaderValue::from_static("true"));
    (StatusCode::OK, response_headers, Json(response))
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

/// Payday's invoice summary as a PDF download, rendered deterministically
/// from the same response the JSON route serves.
pub async fn invoice_pdf(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(reference): Path<String>,
) -> Result<Response, ApiError> {
    let row = resolve_payment(&state, account, &reference).await?;
    let response = to_response(&state, account, row).await?;
    let bytes = render_invoice_pdf(&response).map_err(|error| {
        tracing::error!(error = %error, payment_id = %response.id, "invoice PDF rendering failed");
        ApiError::internal("failed to render the invoice PDF")
    })?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(PDF_MIME_TYPE),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition(&format!(
            "invoice-{}.pdf",
            response.id
        )))
        .map_err(|_| ApiError::internal("invalid download filename"))?,
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((headers, bytes).into_response())
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
            let invoice = Invoice::try_from(&row)?;
            let has_attachment = invoice.issuance_snapshot.attachment.is_some();
            let response = PaymentResponse::from_invoice(invoice, None);
            Ok(PaymentSummaryResponse {
                id: response.id,
                heading: response.heading,
                bill_to_name: response.bill_to.name,
                reference: row.reference,
                metadata: row.metadata.0,
                created_at: row.created_at.to_rfc3339(),
                status: response.status,
                amount: response.amount,
                received: response.received,
                payer_policy_mode: response.payer_policy.mode(),
                customer_id: row.customer_id.map(|id| id.to_string()),
                has_attachment,
                verification_completed_at: row.verification_completed_at.map(|v| v.to_rfc3339()),
                likely_unsolicited_at: row.likely_unsolicited_at.map(|v| v.to_rfc3339()),
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

pub(crate) async fn resolve_payment(
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

/// Everything the merchant asserted, for comparison against the row already
/// issued under an idempotency key. What the server generated (id, salt,
/// address, nonce, the resolved deadline of a relative expiry) is not part of
/// the request and is not compared.
struct RequestedInvoice<'a> {
    chain_id: u64,
    /// Configured, not requested, but committed into the payment address
    /// like recovery: the repository compares it too, and both paths must
    /// agree on what counts as the same issuance.
    factory: Address,
    token: Address,
    beneficiary: Address,
    amount: U256,
    expiration_intent: &'a str,
    recovery: Address,
    issuer: &'a Party,
    bill_to: &'a Party,
    notes: Option<&'a str>,
    heading: Option<&'a str>,
    reference: Option<&'a str>,
    metadata: &'a serde_json::Value,
    customer_id: Option<Uuid>,
    payer_policy: &'a PayerPolicy,
    attachment_id: Option<Uuid>,
    /// The attachment's id, byte length, and SHA-256, as the snapshot commits
    /// to them. `None` for an upload that is not finalized, which is why the
    /// id is compared separately: a pending upload is not "no attachment".
    attachment: Option<AttachmentCommitment>,
}

fn same_request(row: &DbInvoice, request: &RequestedInvoice<'_>) -> bool {
    row.chain_id as u64 == request.chain_id
        && row.factory_address.as_slice() == request.factory.as_slice()
        && row.token_address.as_slice() == request.token.as_slice()
        && row.beneficiary_address.as_slice() == request.beneficiary.as_slice()
        && row.expiration_intent == request.expiration_intent
        && row.recovery_address.as_slice() == request.recovery.as_slice()
        && row.issuer.as_ref().map(|party| &party.0) == Some(request.issuer)
        && row.bill_to.as_ref().map(|party| &party.0) == Some(request.bill_to)
        && row.notes.as_deref() == request.notes
        && row.heading.as_deref() == request.heading
        && row.reference.as_deref() == request.reference
        && row.metadata.0 == *request.metadata
        && row.customer_id == request.customer_id
        && row.payer_policy_mode == request.payer_policy.mode().as_str()
        && row.expected_email.as_deref() == request.payer_policy.expected_email()
        && row.expected_identity.as_ref().map(|identity| &identity.0)
            == request.payer_policy.expected_identity()
        && committed_attachment(row).map(|commitment| commitment.id) == request.attachment_id
        && committed_attachment(row) == request.attachment.as_ref()
        && U256::from_str_radix(&row.amount, 10)
            .map(|amount| amount == request.amount)
            .unwrap_or(false)
}

fn committed_attachment(row: &DbInvoice) -> Option<&AttachmentCommitment> {
    row.issuance_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.0.attachment.as_ref())
}

fn validate_party(field: &str, party: &Party) -> Result<(), ApiError> {
    validate_party_fields(
        &format!("{field}."),
        &party.name,
        party.email.as_deref(),
        party.details.as_deref(),
    )
}

/// The limits of a party (product plan §4.1); customers share them since an
/// invoice party is what a customer becomes. `prefix` names the object in
/// messages (`issuer.`, or nothing for a customer body).
pub(crate) fn validate_party_fields(
    prefix: &str,
    name: &str,
    email: Option<&str>,
    details: Option<&str>,
) -> Result<(), ApiError> {
    if name.trim().is_empty() || name.len() > MAX_PARTY_NAME_BYTES {
        return Err(ApiError::invalid_request(format!(
            "{prefix}name must be 1 to {MAX_PARTY_NAME_BYTES} bytes and not blank"
        )));
    }
    reject_control_characters(&format!("{prefix}name"), name, Text::SingleLine)?;
    if email.is_some_and(|email| !PARTY_EMAIL_BYTES.contains(&email.len())) {
        return Err(ApiError::invalid_request(format!(
            "{prefix}email must be {} to {} bytes",
            PARTY_EMAIL_BYTES.start(),
            PARTY_EMAIL_BYTES.end()
        )));
    }
    if let Some(email) = email {
        reject_control_characters(&format!("{prefix}email"), email, Text::SingleLine)?;
    }
    if details.is_some_and(|details| details.len() > MAX_PARTY_DETAILS_BYTES) {
        return Err(ApiError::invalid_request(format!(
            "{prefix}details must be at most {MAX_PARTY_DETAILS_BYTES} bytes"
        )));
    }
    if let Some(details) = details {
        reject_control_characters(&format!("{prefix}details"), details, Text::MultiLine)?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Text {
    SingleLine,
    MultiLine,
}

/// Control characters are refused up front: Postgres cannot store U+0000 in
/// text or JSONB at all, and the rest have no place in a name or a heading.
/// Only free text that is meant to wrap (notes, party details) may hold line
/// breaks and tabs. Checking here keeps a bad document from reaching the
/// insert, which by then has already retagged the attachment in the bucket.
fn reject_control_characters(field: &str, value: &str, text: Text) -> Result<(), ApiError> {
    let forbidden = |c: char| {
        c.is_control() && !(matches!(text, Text::MultiLine) && matches!(c, '\n' | '\r' | '\t'))
    };
    if value.chars().any(forbidden) {
        return Err(ApiError::invalid_request(match text {
            Text::SingleLine => format!("{field} must not contain control characters"),
            Text::MultiLine => {
                format!(
                    "{field} must not contain control characters other than line breaks and tabs"
                )
            }
        }));
    }
    Ok(())
}

/// JSON allows `\u0000` inside strings; Postgres JSONB does not.
fn reject_nul_in_json(field: &str, value: &serde_json::Value) -> Result<(), ApiError> {
    fn contains_nul(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::String(text) => text.contains('\0'),
            serde_json::Value::Array(items) => items.iter().any(contains_nul),
            serde_json::Value::Object(entries) => entries
                .iter()
                .any(|(key, item)| key.contains('\0') || contains_nul(item)),
            _ => false,
        }
    }
    if contains_nul(value) {
        return Err(ApiError::invalid_request(format!(
            "{field} must not contain U+0000"
        )));
    }
    Ok(())
}

fn validate_document(req: &CreatePaymentRequest) -> Result<(), ApiError> {
    validate_party("issuer", &req.issuer)?;
    validate_party("bill_to", &req.bill_to)?;
    if req
        .notes
        .as_ref()
        .is_some_and(|notes| notes.len() > MAX_NOTES_BYTES)
    {
        return Err(ApiError::invalid_request(format!(
            "notes must be at most {MAX_NOTES_BYTES} bytes"
        )));
    }
    if let Some(notes) = &req.notes {
        reject_control_characters("notes", notes, Text::MultiLine)?;
    }
    if req
        .heading
        .as_ref()
        .is_some_and(|heading| heading.len() > MAX_HEADING_BYTES)
    {
        return Err(ApiError::invalid_request(format!(
            "heading must be at most {MAX_HEADING_BYTES} bytes"
        )));
    }
    if let Some(heading) = &req.heading {
        reject_control_characters("heading", heading, Text::SingleLine)?;
    }
    if req
        .reference
        .as_ref()
        .is_some_and(|value| value.chars().count() > MAX_REFERENCE_CHARS)
    {
        return Err(ApiError::invalid_request(format!(
            "reference must contain at most {MAX_REFERENCE_CHARS} characters"
        )));
    }
    if let Some(reference) = &req.reference {
        reject_control_characters("reference", reference, Text::SingleLine)?;
    }
    reject_nul_in_json("metadata", &req.metadata)?;
    let serde_json::Value::Object(entries) = &req.metadata else {
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

    fn request(overrides: serde_json::Value) -> CreatePaymentRequest {
        let mut json = serde_json::json!({
            "payout_address": "0x0000000000000000000000000000000000000002",
            "amount": "1",
            "issuer": {"name": "Acme"},
            "bill_to": {"name": "Globex"},
            "payer_policy": {"mode": "permissionless"}
        });
        for (key, value) in overrides.as_object().unwrap() {
            json[key] = value.clone();
        }
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn create_wire_shape_defaults_metadata_and_takes_the_document() {
        let request = request(serde_json::json!({
            "chain_id": "1", "token_address": "0x0000000000000000000000000000000000000001",
            "expires_in": 3600, "reference": "order-42", "heading": "March retainer",
            "payer_policy": {"mode": "verified_email", "expected_email": "Alice@Example.com"}
        }));
        assert_eq!(request.reference.as_deref(), Some("order-42"));
        assert_eq!(request.heading.as_deref(), Some("March retainer"));
        assert_eq!(request.metadata, serde_json::json!({}));
        assert_eq!(
            request.payer_policy.normalized().expected_email(),
            Some("alice@example.com")
        );
        validate_document(&request).unwrap();
    }

    #[test]
    fn document_limits_are_enforced_in_bytes() {
        let long = |bytes: usize| "é".repeat(bytes / 2 + 1);
        for (name, overrides) in [
            (
                "blank issuer",
                serde_json::json!({"issuer": {"name": "  "}}),
            ),
            (
                "long bill_to",
                serde_json::json!({"bill_to": {"name": long(255)}}),
            ),
            (
                "short email",
                serde_json::json!({"issuer": {"name": "Acme", "email": "ab"}}),
            ),
            (
                "long details",
                serde_json::json!({"bill_to": {"name": "Globex", "details": long(4000)}}),
            ),
            ("long notes", serde_json::json!({"notes": long(4000)})),
            ("long heading", serde_json::json!({"heading": long(200)})),
            (
                "long reference",
                serde_json::json!({"reference": "x".repeat(129)}),
            ),
            ("metadata array", serde_json::json!({"metadata": []})),
            ("nul in notes", serde_json::json!({"notes": "Net\u{0}30"})),
            (
                "nul in party name",
                serde_json::json!({"bill_to": {"name": "Glo\u{0}bex"}}),
            ),
            (
                "escape in heading",
                serde_json::json!({"heading": "March\u{1b}[31m retainer"}),
            ),
            (
                "line break in name",
                serde_json::json!({"issuer": {"name": "Acme\nCorp"}}),
            ),
            (
                "line break in email",
                serde_json::json!({"issuer": {"name": "Acme", "email": "a@b\n"}}),
            ),
            (
                "tab in reference",
                serde_json::json!({"reference": "INV\t1"}),
            ),
            (
                "nul in metadata",
                serde_json::json!({"metadata": {"po": "4\u{0}2"}}),
            ),
            (
                "nul in metadata key",
                serde_json::json!({"metadata": {"p\u{0}o": "42"}}),
            ),
        ] {
            let error = validate_document(&request(overrides)).unwrap_err();
            assert_eq!(error.code, "invalid_request", "{name}");
        }
        validate_document(&request(serde_json::json!({
            "issuer": {"name": "x".repeat(255), "email": "a@b", "details": "x".repeat(4000)},
            "notes": "x".repeat(4000), "heading": "x".repeat(200), "reference": "é".repeat(128)
        })))
        .unwrap();
        // Free text that wraps may hold line breaks and tabs.
        validate_document(&request(serde_json::json!({
            "issuer": {"name": "Acme", "details": "1 Main St\r\nSpringfield\tUSA"},
            "notes": "Net 30\nThank you", "metadata": {"po": "4\n2"}
        })))
        .unwrap();
    }
}
