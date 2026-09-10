//! Deposit request handlers: the merchant API for issuing, reading, listing,
//! and cancelling deposit requests.

use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy_primitives::utils::format_units;
use alloy_primitives::{Address, B256, U256};
use axum::Extension;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use uuid::Uuid;

use gateway_core::{
    Amount, AsOfDto, BeneficiaryAddress, CanonicalIssuanceSnapshot, ChainId, CreateDepositRequest,
    CustomerId, DepositRequestListResponse, DepositRequestResponse, DepositRequestStatus,
    DepositRequestSummaryResponse, IndexerFreshnessDto, Invoice, IssuerId,
    OnboardingDepositResponse, PDF_MIME_TYPE, Party, PayerAttestation, PayerPolicy,
    PayerPolicyMode, PaymentBinding, SnapshotNetwork, TransferDto, TransferListResponse,
    USDC_DECIMALS, parse_expiration, payer_wallet_attestation, rfc3339, validate_expiration_window,
};
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::json::{Json, Query};
use crate::attachments::{AttachmentError, StorageError, content_disposition};
use crate::onboarding_payer::OnboardingPayerSigner;
use crate::request_pdf::render_request_pdf;
use crate::state::AppState;
use gateway_db::{
    AccountId, AttachmentStatus, BindPayerWallet, CLIENT_SECRET_TTL, CreateInvoiceInput,
    DbAttachment, DbInvoice, InsertIssuedInvoiceError, IssuanceRequest, OnboardingClaim,
    PAYER_SESSION_TTL, StartEmailVerificationError, same_issuance,
};

use crate::api::payer_wallet::WALLET_CHALLENGE_TTL;

const MAX_IDEMPOTENCY_KEY_BYTES: usize = 255;
const MAX_PARTY_NAME_BYTES: usize = 255;
const PARTY_EMAIL_BYTES: std::ops::RangeInclusive<usize> = 3..=254;
const MAX_PARTY_DETAILS_BYTES: usize = 4000;
const MAX_NOTES_BYTES: usize = 4000;
const MAX_HEADING_BYTES: usize = 200;
const MAX_REFERENCE_CHARS: usize = 128;
/// The onboarding walkthrough's reserved, Payday-owned mailbox. The only
/// thing `onboarding_deposit` can ever pay is an invoice addressed to this
/// exact email — never a real payer's.
const ONBOARDING_EMAIL: &str = "onboarding@payday.sh";

/// Project a DB row onto the wire response, going through the domain model so
/// the row is never serialized directly. Fails only if the stored row is
/// outside the schema contract (see [`gateway_db::DbInvoiceError`]).
///
/// A free function rather than a `TryFrom` impl: the orphan rule forbids
/// implementing a foreign trait for the foreign `DepositRequestResponse` from a row
/// type that now also lives outside this crate.
async fn to_response(
    state: &AppState,
    account: AccountId,
    row: DbInvoice,
) -> Result<DepositRequestResponse, ApiError> {
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
) -> Result<DepositRequestResponse, ApiError> {
    let expires_in = row
        .expiration_intent
        .strip_prefix("in:")
        .and_then(|v| v.parse().ok());
    let invoice = Invoice::try_from(&row)?;
    let mut response = DepositRequestResponse::from_invoice(invoice.clone(), expires_in);
    let chain_id = invoice.network().map(|network| network.chain_id.0);
    response.deposit_url = state.payer.deposit_url(&invoice)?;
    response.address_explorer_url = response
        .address
        .as_deref()
        .zip(chain_id)
        .and_then(|(address, chain_id)| state.payer.address_url(chain_id, address));
    response.metadata = row.metadata.0.clone();
    response.customer_id = row.customer_id.map(|id| CustomerId(id).to_string());
    response.issuer_id = row.issuer_id.map(|id| IssuerId(id).to_string());
    // The signed download link is added by the attachment route.
    response.attachment = attachment.as_ref().and_then(DbAttachment::descriptor);
    response.verification_completed_at = row.verification_completed_at.map(rfc3339);
    response.likely_unsolicited_at = row.likely_unsolicited_at.map(rfc3339);
    response.created_at = rfc3339(row.created_at);
    response.updated_at = rfc3339(row.updated_at);
    response.deposited_at = row.paid_at.map(rfc3339);
    response.deposited_at_block = row.funded_at_block.map(|value| value.to_string());
    response.expired_at = row.expired_at.map(rfc3339);
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
        .zip(chain_id)
        .and_then(|(hash, chain_id)| state.payer.transaction_url(chain_id, hash));
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
                timestamp: rfc3339(timestamp),
                amount: format_units(units, USDC_DECIMALS).unwrap_or_default(),
                amount_base_units: t.amount,
                sender: sender.to_checksum(None),
                transaction_hash: hash.to_string(),
                explorer_url: chain_id
                    .and_then(|chain_id| state.payer.transaction_url(chain_id, &hash.to_string())),
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
    // Freshness is per chain: an unbound request has no chain yet, so it
    // reports none.
    let cursor = freshness.into_iter().find(|f| {
        Some(f.chain_id) == row.chain_id && Some(&f.token_address) == row.token_address.as_ref()
    });
    response.as_of = cursor.as_ref().and_then(|cursor| {
        Some(AsOfDto {
            block: cursor.last_block.to_string(),
            at: rfc3339(sqlx::types::chrono::DateTime::from_timestamp(
                cursor.last_block_timestamp?,
                0,
            )?),
        })
    });
    response.indexer_freshness = IndexerFreshnessDto {
        last_indexed_block: cursor.as_ref().map(|c| c.last_block.to_string()),
        last_finalized_block: cursor
            .as_ref()
            .and_then(|c| c.finalized_block)
            .map(|block| block.to_string()),
        cursor_updated_at: cursor.map(|c| rfc3339(c.updated_at)),
    };
    Ok(response)
}

// --- Handlers ---

pub async fn create_deposit_request(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    headers: HeaderMap,
    Json(req): Json<CreateDepositRequest>,
) -> Result<(StatusCode, HeaderMap, Json<DepositRequestResponse>), ApiError> {
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

    // 2. Resolve the parties, then validate every request field. A saved
    //    customer or issuer identity stands in for a party the request left
    //    out; what is snapshotted is the same either way, and a reference to
    //    a record that is not this account's reads as an invalid field.
    let customer = match req.customer_id {
        Some(customer_id) => Some(
            state
                .customers
                .get_for_account(account, customer_id.0)
                .await?
                .ok_or_else(|| {
                    ApiError::invalid_request("customer_id does not identify one of your customers")
                })?,
        ),
        None => None,
    };
    let identity = match req.issuer_id {
        Some(issuer_id) => Some(
            state
                .issuers
                .get_for_account(account, issuer_id.0)
                .await?
                .ok_or_else(|| {
                    ApiError::invalid_request(
                        "issuer_id does not identify one of your issuer identities",
                    )
                })?,
        ),
        None => None,
    };
    let issuer = match (&req.issuer, &identity) {
        (Some(party), _) => party.clone(),
        (None, Some(identity)) => Party {
            name: identity.name.clone(),
            email: Some(identity.contact_email.clone()),
            details: identity.details.clone(),
        },
        (None, None) => {
            return Err(ApiError::invalid_request(
                "issuer is required unless issuer_id names a saved issuer identity",
            ));
        }
    };
    let payer = match (&req.payer, &customer) {
        (Some(party), _) => party.clone(),
        (None, Some(customer)) => Party {
            name: customer.name.clone(),
            email: customer.email.clone(),
            details: customer.details.clone(),
        },
        (None, None) => {
            return Err(ApiError::invalid_request(
                "payer is required unless customer_id names a saved customer",
            ));
        }
    };
    let payout_address = match (&req.payout_address, &identity) {
        (Some(address), _) => address.clone(),
        (None, Some(identity)) => state
            .issuers
            .addresses_for_issuer(account, identity.id)
            .await?
            .into_iter()
            .next()
            .map(|saved| saved.address)
            .ok_or_else(|| {
                ApiError::invalid_request(
                    "payout_address is required unless issuer_id names an identity with a saved payout address",
                )
            })?,
        (None, None) => {
            return Err(ApiError::invalid_request(
                "payout_address is required unless issuer_id names an identity with a saved payout address",
            ));
        }
    };
    validate_document(&issuer, &payer, &req)?;
    let payer_policy = req.payer_policy.normalized();
    payer_policy
        .validate()
        .map_err(|error| ApiError::invalid_request(error.to_string()))?;
    let beneficiary_addr = Address::from_str(&payout_address)
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

    // 3. The request is payable on every network this deployment offers,
    // each with Circle's native USDC there; the payer picks one when they
    // sign. Nothing about the chain is a request field.
    let networks = state.networks.networks();

    // 4. Parse amount using token decimals.
    let decimals = USDC_DECIMALS;
    let amount = Amount::from_decimal_str(&req.amount, decimals)
        .map_err(|e| ApiError::invalid_amount(e.to_string()))?;

    if amount.0 == U256::ZERO {
        return Err(ApiError::invalid_amount("amount must be positive"));
    }

    // 5. The attachment must be this account's. Its readiness is checked only
    // for a new issuance: a replay compares against a document that is, by
    // then, attached.
    let attachment = match req.attachment_id {
        Some(attachment_id) => Some(
            state
                .attachments
                .get_for_account(account, attachment_id.0)
                .await?
                .ok_or_else(|| {
                    ApiError::invalid_request("attachment_id does not identify one of your uploads")
                })?,
        ),
        None => None,
    };

    // Recovery is not a request field: it is the payer's attested wallet,
    // known only when the payer binds it, so it is no part of issuance.
    let attachment_commitment = attachment.as_ref().and_then(DbAttachment::commitment);
    let committed_networks: Vec<SnapshotNetwork> =
        networks.iter().map(SnapshotNetwork::from).collect();
    let requested = IssuanceRequest {
        networks: &committed_networks,
        token_decimals: decimals,
        beneficiary: beneficiary_addr.as_slice(),
        amount: amount.0,
        expiration_intent: &expiration.intent,
        issuer: &issuer,
        bill_to: &payer,
        notes: req.notes.as_deref(),
        heading: req.heading.as_deref(),
        reference: req.reference.as_deref(),
        metadata: &req.metadata,
        customer_id: req.customer_id.map(Uuid::from),
        issuer_id: req.issuer_id.map(Uuid::from),
        payer_policy: &payer_policy,
        attachment_id: req.attachment_id.map(Uuid::from),
        attachment: attachment_commitment.as_ref(),
    };

    // 6. Check for existing idempotency key before generating anything.
    if let Some(existing) = state
        .repo
        .find_by_idempotency_key(account, &idempotency_key)
        .await?
    {
        if same_issuance(&existing, &requested) {
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

    // 7. Commit to the document. The address is derived later, when the
    // payer binds the wallet they will pay from on the network they chose.
    let beneficiary = BeneficiaryAddress(beneficiary_addr);
    let mut snapshot = CanonicalIssuanceSnapshot::new(
        issuer.clone(),
        payer.clone(),
        payer_policy.clone(),
        &networks,
        beneficiary,
        amount,
        expiration_timestamp,
    );
    snapshot.notes = req.notes.clone();
    snapshot.heading = req.heading.clone();
    snapshot.reference = req.reference.clone();
    snapshot.attachment = requested.attachment.cloned();
    let invoice = Invoice::issue(
        &networks,
        beneficiary,
        amount,
        expiration_timestamp,
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
    input.customer_id = req.customer_id.map(Uuid::from);
    input.issuer_id = req.issuer_id.map(Uuid::from);
    input.metadata = req.metadata.clone();
    input.payer_notification_email = payer_notification_email(&payer, &payer_policy);

    let issued = match state
        .repo
        .insert_issued(&input, req.attachment_id.map(Uuid::from))
        .await
    {
        Ok(issued) => issued,
        Err(error) => {
            // The retag above preceded a transaction that did not commit, so
            // the object would otherwise sit in the bucket forever, exempt
            // from expiry and bound to nothing. Hand it back to the lifecycle
            // rule. A typed race can mean the winner attached this same object,
            // though, in which case restoring it would expire live content.
            let restore_attachment = if let Some(attachment) = &attachment {
                match state
                    .attachments
                    .get_for_account(account, attachment.id)
                    .await
                {
                    Ok(Some(current)) => current.invoice_id.is_none(),
                    Ok(None) => false,
                    Err(lookup) => {
                        tracing::warn!(error = %lookup, attachment_id = %attachment.id, "failed to determine whether attachment tag needs restoring");
                        false
                    }
                }
            } else {
                false
            };
            if restore_attachment
                && let Some(attachment) = &attachment
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
    let invoice_id = issued.row.id;
    let mut response =
        enrich_response(&state, issued.row, issued.attachment, transfers, freshness)?;
    if issued.replayed {
        return Ok(replayed(response));
    }
    // A merchant-session invoice is useless without the secret that opens it,
    // so the first response carries one. A replay does not: the secret exists
    // only in the response that minted it, and a merchant that lost it mints
    // another through the client-secret route.
    if payer_policy.mode() == PayerPolicyMode::MerchantSession {
        let minted = state
            .payer_sessions
            .create_client_secret(invoice_id, account.0, CLIENT_SECRET_TTL)
            .await?;
        response.client_secret = Some(minted.secret);
        response.client_secret_expires_at = Some(rfc3339(minted.expires_at));
    }
    Ok((StatusCode::CREATED, HeaderMap::new(), Json(response)))
}

fn replayed(
    response: DepositRequestResponse,
) -> (StatusCode, HeaderMap, Json<DepositRequestResponse>) {
    let mut response_headers = HeaderMap::new();
    response_headers.insert("Idempotency-Replayed", HeaderValue::from_static("true"));
    (StatusCode::OK, response_headers, Json(response))
}

pub async fn get_deposit_request(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(reference): Path<String>,
    Query(query): Query<GetQuery>,
) -> Result<Json<DepositRequestResponse>, ApiError> {
    let mut row = resolve_deposit_request(&state, account, &reference).await?;
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
            row = resolve_deposit_request(&state, account, &reference).await?;
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
) -> Result<Json<TransferListResponse>, ApiError> {
    let row = resolve_deposit_request(&state, account, &reference).await?;
    Ok(Json(TransferListResponse {
        transfers: to_response(&state, account, row).await?.transfers,
    }))
}

/// Payday's invoice summary as a PDF download, rendered deterministically
/// from the same response the JSON route serves.
pub async fn request_pdf(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(reference): Path<String>,
) -> Result<Response, ApiError> {
    let row = resolve_deposit_request(&state, account, &reference).await?;
    let response = to_response(&state, account, row).await?;
    let bytes = render_request_pdf(&response).map_err(|error| {
        tracing::error!(error = %error, payment_id = %response.id, "invoice PDF rendering failed");
        ApiError::internal("failed to render the deposit request PDF")
    })?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(PDF_MIME_TYPE),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition(&format!(
            "deposit-request-{}.pdf",
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
    customer_id: Option<CustomerId>,
    issuer_id: Option<IssuerId>,
    /// Verification is a separate fact from the payment's status, so it is a
    /// separate filter: `not_required`, `pending`, `verified`, or
    /// `likely_unsolicited`.
    verification: Option<String>,
    limit: Option<u32>,
    starting_after: Option<String>,
}

const VERIFICATION_FILTERS: [&str; 4] =
    ["not_required", "pending", "verified", "likely_unsolicited"];

pub async fn list_deposit_requests(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Query(query): Query<ListQuery>,
) -> Result<Json<DepositRequestListResponse>, ApiError> {
    let status = query
        .status
        .as_deref()
        .map(str::parse::<DepositRequestStatus>)
        .transpose()
        .map_err(|_| ApiError::invalid_request("unknown deposit request status"))?;
    if let Some(verification) = query.verification.as_deref()
        && !VERIFICATION_FILTERS.contains(&verification)
    {
        return Err(ApiError::invalid_request("unknown verification filter"));
    }
    let limit = query.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::invalid_request("limit must be between 1 and 100"));
    }
    let starting_after = query
        .starting_after
        .as_deref()
        .map(full_deposit_request_id)
        .transpose()?;
    if let Some(cursor) = starting_after
        && state
            .repo
            .find_by_id_for_account(account, cursor)
            .await?
            .is_none()
    {
        return Err(ApiError::invalid_request(
            "starting_after does not identify one of your deposit requests",
        ));
    }
    let mut rows = state
        .repo
        .list_for_account(
            account,
            status.map(DepositRequestStatus::as_str),
            query.reference.as_deref(),
            query.customer_id.map(Uuid::from),
            query.issuer_id.map(Uuid::from),
            query.verification.as_deref(),
            starting_after,
            limit,
        )
        .await?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = has_more.then(|| format!("dr_{}", rows.last().expect("nonzero limit").id));
    let payments = rows
        .into_iter()
        .map(|row| -> Result<_, ApiError> {
            let invoice = Invoice::try_from(&row)?;
            let has_attachment = invoice.issuance_snapshot.attachment.is_some();
            let deposit_url = state.payer.deposit_url(&invoice)?;
            let response = DepositRequestResponse::from_invoice(invoice, None);
            Ok(DepositRequestSummaryResponse {
                id: response.id,
                deposit_url,
                heading: response.heading,
                payer_name: response.payer.name,
                reference: row.reference,
                metadata: row.metadata.0,
                created_at: rfc3339(row.created_at),
                updated_at: rfc3339(row.updated_at),
                expires_at: response.expires_at,
                status: response.status,
                amount: response.amount,
                received: response.received,
                payer_policy_mode: response.payer_policy.mode(),
                customer_id: row.customer_id.map(|id| CustomerId(id).to_string()),
                issuer_id: row.issuer_id.map(|id| IssuerId(id).to_string()),
                has_attachment,
                verification_completed_at: row.verification_completed_at.map(rfc3339),
                likely_unsolicited_at: row.likely_unsolicited_at.map(rfc3339),
                cancellation_requested_at: row.cancellation_requested_at.map(rfc3339),
            })
        })
        .collect::<Result<_, _>>()?;
    Ok(Json(DepositRequestListResponse {
        deposit_requests: payments,
        next_cursor,
    }))
}

/// The onboarding signer pays on exactly one chain with that chain's USDC. A
/// binding names the network the payer chose; paying it from here on any other
/// chain would send real funds to an address no indexer of that chain is
/// watching, so every broadcast path must pass this guard first.
fn ensure_onboarding_network(
    signer: &OnboardingPayerSigner,
    binding: &PaymentBinding,
    payment_id: Uuid,
) -> Result<(), ApiError> {
    let chain = ChainId(signer.chain_id());
    if binding.network.chain_id != chain || binding.network.token.0 != signer.usdc() {
        tracing::error!(
            %payment_id,
            bound_chain = binding.network.chain_id.0,
            signer_chain = chain.0,
            "onboarding demo payment is bound to a network the onboarding signer does not pay on"
        );
        return Err(ApiError::onboarding_deposit_not_eligible());
    }
    Ok(())
}

fn full_deposit_request_id(value: &str) -> Result<Uuid, ApiError> {
    let suffix = value.strip_prefix("dr_").ok_or_else(|| {
        ApiError::invalid_request("starting_after must be a complete dr_ deposit request ID")
    })?;
    Uuid::parse_str(suffix).map_err(|_| {
        ApiError::invalid_request("starting_after must be a complete dr_ deposit request ID")
    })
}

/// Records the cancellation and answers with the deposit request itself, as
/// every other route does. Cancellation is presentation only: it sets
/// `cancellation_requested_at` and cannot disable the address or change the
/// settlement terms the address commits to, which the reference documents.
pub async fn cancel_deposit_request(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(reference): Path<String>,
) -> Result<Json<DepositRequestResponse>, ApiError> {
    let invoice = resolve_deposit_request(&state, account, &reference).await?;
    let row = state
        .repo
        .request_cancellation(account, invoice.id)
        .await?
        .ok_or_else(ApiError::deposit_request_not_found)?;
    Ok(Json(to_response(&state, account, row).await?))
}

/// The onboarding walkthrough's one real demo transfer and verification
/// (see `docs/local-development.md`-adjacent design notes: the walkthrough
/// issues a real, self-billed `verified_email` invoice through the normal
/// create endpoint, then calls this one to make it real end to end).
///
/// This is deliberately narrow, not a general "settle any invoice" or
/// "verify any payer" affordance: it refuses anything not addressed to
/// Payday's own reserved mailbox, and at most one call per account ever
/// reaches the chain (`gateway_db::OnboardingDemoPaymentRepository`).
/// Verification is completed the same way `payer_verification::confirm_email`
/// does after a real Auth0 code checks out — minting a session, then
/// approving its email fact — except there is no code to check: Payday
/// owns `onboarding@payday.sh`, so proving control of it here would only
/// ever be proving Payday's own address to Payday. The wallet step is real:
/// the demo payer signs the same attestation a payer's wallet would, which
/// is what gives the request its address.
pub async fn onboarding_deposit(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(reference): Path<String>,
) -> Result<Json<OnboardingDepositResponse>, ApiError> {
    let signer = state.onboarding_payer()?;
    let payer_verification = state
        .payer_verification
        .as_ref()
        .ok_or_else(ApiError::onboarding_deposit_unavailable)?;

    let row = resolve_deposit_request(&state, account, &reference).await?;
    let invoice = Invoice::try_from(&row)?;
    let eligible = row.status == "created"
        && invoice.issuance_snapshot.bill_to.email.as_deref() == Some(ONBOARDING_EMAIL)
        && matches!(
            &invoice.issuance_snapshot.payer_policy,
            PayerPolicy::VerifiedEmail { expected_email } if expected_email == ONBOARDING_EMAIL
        );
    if !eligible {
        return Err(ApiError::onboarding_deposit_not_eligible());
    }

    // The onboarding signer pays on exactly one chain with that chain's USDC.
    // A request already bound on another network is not payable by it; refuse
    // before the claim below consumes the account's one-shot demo payment, so
    // an operator fixing the binding can still re-run the walkthrough.
    if let Some(binding) = &invoice.binding {
        ensure_onboarding_network(&signer, binding, row.id)?;
    }

    let claim = state
        .onboarding_demo_payments
        .claim(account, row.id)
        .await?;
    if claim == OnboardingClaim::Conflict {
        return Err(ApiError::onboarding_deposit_already_claimed());
    }

    // Minting and approving a session is cheap and safe to repeat on a retry
    // (it only ever adds harmless extra rows for this one demo invoice); the
    // on-chain transfer below is the part that must never happen twice, and
    // that is what `claim` above actually guards.
    let session = state
        .payer_sessions
        .create(row.id, PAYER_SESSION_TTL)
        .await?;
    state
        .payer_sessions
        .begin_email_verification(session.id, Duration::ZERO)
        .await
        .map_err(|error| match error {
            StartEmailVerificationError::Cooldown { .. } => {
                ApiError::internal("unexpected onboarding verification cooldown")
            }
            StartEmailVerificationError::Database(error) => error.into(),
        })?;
    let payer_ref = payer_verification.payer_ref(account.0, ONBOARDING_EMAIL);
    // A fresh event id every call: unlike a real payer's OTP, there is no
    // single external event to key on, and minting another approved session
    // on retry is harmless, so nothing needs deduplicating here.
    state
        .payer_sessions
        .approve_email(
            session.id,
            payer_ref,
            Utc::now(),
            &Uuid::now_v7().to_string(),
        )
        .await?;

    // The demo payer attests its wallet exactly as a payer's would: a
    // challenge on the session for the demo chain, the EIP-712 signature,
    // the binding. A retry finds the binding already in place.
    let chain = ChainId(signer.chain_id());
    let binding = match &invoice.binding {
        Some(binding) => binding.clone(),
        None => {
            let network = *invoice
                .network_for(chain)
                .ok_or_else(ApiError::onboarding_deposit_not_eligible)?;
            let challenge = state
                .payer_sessions
                .issue_wallet_challenge(session.id, chain.0, WALLET_CHALLENGE_TTL)
                .await?;
            let message = PayerAttestation::new(
                invoice.attribution_hash,
                signer.address(),
                challenge.nonce,
                challenge.expires_at.timestamp().max(0) as u64,
            );
            let digest = message.digest(network.chain_id.0, network.factory.0);
            let signature = signer.sign_hash(&digest).await.map_err(|error| {
                tracing::error!(%error, payment_id = %row.id, "onboarding payer attestation failed");
                ApiError::internal("failed to sign the onboarding payer attestation")
            })?;
            let attestation = payer_wallet_attestation(
                &message,
                network.chain_id.0,
                network.factory.0,
                &signature,
            );
            let now = Utc::now();
            let binding = invoice
                .bind_payer_wallet(chain, attestation, rfc3339(now))
                .map_err(|error| {
                    tracing::error!(%error, payment_id = %row.id, "onboarding payer attestation did not verify");
                    ApiError::internal("failed to verify the onboarding payer attestation")
                })?;
            match state
                .repo
                .bind_payer_wallet(row.id, session.id, &binding, now)
                .await?
            {
                BindPayerWallet::Bound(_) => binding,
                // A concurrent caller won the binding, possibly on another
                // network than the one this signer pays on: keep the winner's
                // whole binding so the network check below applies to it too.
                BindPayerWallet::AlreadyBound(bound) => Invoice::try_from(&bound)?
                    .binding
                    .ok_or_else(|| ApiError::internal("bound invoice has no binding"))?,
                BindPayerWallet::NotBindable(_) => {
                    return Err(ApiError::deposit_request_not_payable());
                }
            }
        }
    };
    // The guard above ran before the claim for a pre-existing binding; this
    // one covers a binding a concurrent call just won, possibly on another
    // network than the one this signer pays on. Paying either from here on
    // any other chain would send real funds to an address no indexer of that
    // chain is watching.
    ensure_onboarding_network(&signer, &binding, row.id)?;
    let payment_address = binding.payment_address;

    let tx_hash = match claim {
        OnboardingClaim::AlreadySubmitted(tx_hash) => tx_hash,
        OnboardingClaim::Claimed | OnboardingClaim::PendingRetry => {
            let tx_hash = signer
                .send_usdc(payment_address.0, invoice.amount.0)
                .await
                .map_err(|error| {
                    tracing::error!(%error, payment_id = %row.id, "onboarding demo transfer failed");
                    ApiError::internal("failed to submit the onboarding demo transfer")
                })?
                .to_string();
            state
                .onboarding_demo_payments
                .record_tx_hash(account, row.id, &tx_hash)
                .await?;
            tx_hash
        }
        OnboardingClaim::Conflict => unreachable!("handled above"),
    };

    Ok(Json(OnboardingDepositResponse {
        payer_session: session.token,
        tx_hash,
    }))
}

#[derive(serde::Serialize)]
pub struct PreviewSessionResponse {
    pub payer_session: String,
    pub expires_at: String,
}

/// A session that lets the deposit request's own issuing merchant open its
/// payer view exactly as a verified payer would see it — the dashboard's
/// "Open the payer's view" and "Track this request" links use this instead
/// of the bare `deposit_url`, which otherwise looks exactly as locked to the
/// issuing merchant as it does to a stranger holding the link.
///
/// This is deliberately not verification: it records no attempt and never
/// touches the invoice's own `verification_completed_at` or fires a
/// `verification.*` webhook. A merchant looking at their own request proves
/// nothing about a payer, and must never be confused with one actually
/// completing it — see `PayerSessionRepository::create_merchant_preview`.
pub async fn preview_session(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(reference): Path<String>,
) -> Result<Json<PreviewSessionResponse>, ApiError> {
    let row = resolve_deposit_request(&state, account, &reference).await?;
    let invoice = Invoice::try_from(&row)?;
    if !crate::api::merchant_session::openable(&row, &invoice) {
        return Err(ApiError::deposit_request_not_payable());
    }
    let session = state
        .payer_sessions
        .create_merchant_preview(row.id, PAYER_SESSION_TTL)
        .await?;
    Ok(Json(PreviewSessionResponse {
        payer_session: session.token,
        expires_at: rfc3339(session.expires_at),
    }))
}

pub(crate) async fn resolve_deposit_request(
    state: &AppState,
    account: AccountId,
    reference: &str,
) -> Result<DbInvoice, ApiError> {
    if let Some(uuid) = gateway_core::deposit_request_id(reference) {
        return state
            .repo
            .find_by_id_for_account(account, uuid)
            .await?
            .ok_or_else(ApiError::deposit_request_not_found);
    }
    if let Ok(address) = Address::from_str(reference) {
        return state
            .repo
            .find_by_payment_address_for_account(account, address.as_slice())
            .await?
            .ok_or_else(ApiError::deposit_request_not_found);
    }
    Err(ApiError::invalid_deposit_reference())
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
/// Whom to email the request to, if anyone: the payer the merchant named,
/// provided the link in that email would open for them. A merchant-session
/// request opens only from inside the merchant's own app, and the onboarding
/// walkthrough's payer is Payday's reserved mailbox.
fn payer_notification_email(payer: &Party, policy: &PayerPolicy) -> Option<String> {
    let email = payer.email.as_deref()?.trim();
    if email.is_empty()
        || email.eq_ignore_ascii_case(ONBOARDING_EMAIL)
        || policy.mode() == PayerPolicyMode::MerchantSession
    {
        return None;
    }
    Some(email.to_owned())
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

/// The document as it will be issued: the parties already resolved (inline,
/// or from the saved records the request named) plus the request's own text
/// and metadata.
fn validate_document(
    issuer: &Party,
    payer: &Party,
    req: &CreateDepositRequest,
) -> Result<(), ApiError> {
    validate_party("issuer", issuer)?;
    validate_party("payer", payer)?;
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

    fn request(overrides: serde_json::Value) -> CreateDepositRequest {
        let mut json = serde_json::json!({
            "payout_address": "0x0000000000000000000000000000000000000002",
            "amount": "1",
            "issuer": {"name": "Acme"},
            "payer": {"name": "Globex"},
            "payer_policy": {"mode": "permissionless"}
        });
        for (key, value) in overrides.as_object().unwrap() {
            json[key] = value.clone();
        }
        serde_json::from_value(json).unwrap()
    }

    /// The document check on a request whose parties were given inline.
    fn validate_inline(request: &CreateDepositRequest) -> Result<(), ApiError> {
        validate_document(
            request.issuer.as_ref().unwrap(),
            request.payer.as_ref().unwrap(),
            request,
        )
    }

    #[test]
    fn parties_and_payout_address_may_be_left_to_saved_records() {
        let request: CreateDepositRequest = serde_json::from_value(serde_json::json!({
            "amount": "1",
            "issuer_id": "iss_0198f80c-1111-7dc1-a369-90556a64f700",
            "customer_id": "cus_0198f80c-2222-7dc1-a369-90556a64f700",
            "payer_policy": {"mode": "permissionless"}
        }))
        .unwrap();
        assert!(request.issuer.is_none());
        assert!(request.payer.is_none());
        assert!(request.payout_address.is_none());
        assert!(request.issuer_id.is_some() && request.customer_id.is_some());
        // A bare UUID is refused with a message naming the form wanted.
        let bare = serde_json::from_value::<CreateDepositRequest>(serde_json::json!({
            "amount": "1",
            "customer_id": "0198f80c-2222-7dc1-a369-90556a64f700",
            "payer_policy": {"mode": "permissionless"}
        }))
        .unwrap_err();
        assert!(
            bare.to_string()
                .contains("expected a customer id like cus_"),
            "{bare}"
        );
    }

    #[test]
    fn create_wire_shape_defaults_metadata_and_takes_the_document() {
        let request = request(serde_json::json!({
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
        validate_inline(&request).unwrap();
    }

    #[test]
    fn payer_email_is_queued_only_where_the_link_would_open() {
        let policy = |json: serde_json::Value| {
            serde_json::from_value::<PayerPolicy>(json)
                .unwrap()
                .normalized()
        };
        let open = policy(serde_json::json!({"mode": "permissionless"}));
        let gated = policy(
            serde_json::json!({"mode": "verified_email", "expected_email": "alice@example.com"}),
        );
        let embedded =
            policy(serde_json::json!({"mode": "merchant_session", "payer_reference": "user-1"}));
        let payer = |email: Option<&str>| Party {
            name: "Globex".into(),
            email: email.map(str::to_owned),
            details: None,
        };
        assert_eq!(
            payer_notification_email(&payer(Some(" Bob@Example.com ")), &open).as_deref(),
            Some("Bob@Example.com")
        );
        assert_eq!(
            payer_notification_email(&payer(Some("bob@example.com")), &gated).as_deref(),
            Some("bob@example.com")
        );
        assert_eq!(payer_notification_email(&payer(None), &open), None);
        assert_eq!(payer_notification_email(&payer(Some("   ")), &open), None);
        assert_eq!(
            payer_notification_email(&payer(Some("bob@example.com")), &embedded),
            None
        );
        assert_eq!(
            payer_notification_email(&payer(Some("Onboarding@payday.sh")), &gated),
            None
        );
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
                "long payer",
                serde_json::json!({"payer": {"name": long(255)}}),
            ),
            (
                "short email",
                serde_json::json!({"issuer": {"name": "Acme", "email": "ab"}}),
            ),
            (
                "long details",
                serde_json::json!({"payer": {"name": "Globex", "details": long(4000)}}),
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
                serde_json::json!({"payer": {"name": "Glo\u{0}bex"}}),
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
            let error = validate_inline(&request(overrides)).unwrap_err();
            assert_eq!(error.code, "invalid_request", "{name}");
        }
        validate_inline(&request(serde_json::json!({
            "issuer": {"name": "x".repeat(255), "email": "a@b", "details": "x".repeat(4000)},
            "notes": "x".repeat(4000), "heading": "x".repeat(200), "reference": "é".repeat(128)
        })))
        .unwrap();
        // Free text that wraps may hold line breaks and tabs.
        validate_inline(&request(serde_json::json!({
            "issuer": {"name": "Acme", "details": "1 Main St\r\nSpringfield\tUSA"},
            "notes": "Net 30\nThank you", "metadata": {"po": "4\n2"}
        })))
        .unwrap();
    }
}
