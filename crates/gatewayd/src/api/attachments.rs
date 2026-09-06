//! Attachment routes (guide §Slice 2.4): reserve an upload slot, admit the
//! uploaded object once it is a clean PDF, and hand out signed download links.

use std::collections::BTreeMap;

use axum::Extension;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use gateway_core::{AttachmentDescriptor, AttachmentId, rfc3339};
use gateway_db::{AccountId, AttachmentStatus, CreateAttachmentUpload, DbAttachment};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::deposit_requests::resolve_deposit_request;
use crate::api::error::ApiError;
use crate::api::json::Json;
use crate::attachments::{AttachmentError, AttachmentStore, CLEAN_SCAN};
use crate::state::AppState;

const MAX_FILENAME_BYTES: usize = 255;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAttachmentRequest {
    filename: String,
}

#[derive(Debug, Serialize)]
pub struct AttachmentUploadResponse {
    id: AttachmentId,
    upload_url: String,
    /// Every header the PUT must carry verbatim; they are part of the
    /// signature.
    headers: BTreeMap<String, String>,
    expires_at: String,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Json(request): Json<CreateAttachmentRequest>,
) -> Result<(StatusCode, Json<AttachmentUploadResponse>), ApiError> {
    // The name is stored verbatim for display; it is never used as a key.
    if request.filename.trim().is_empty() || request.filename.len() > MAX_FILENAME_BYTES {
        return Err(ApiError::invalid_request(format!(
            "filename must be 1 to {MAX_FILENAME_BYTES} bytes and not blank"
        )));
    }
    if request.filename.chars().any(char::is_control) {
        return Err(ApiError::invalid_request(
            "filename must not contain control characters",
        ));
    }
    let store = state.attachment_store()?;
    let id = Uuid::now_v7();
    let row = state
        .attachments
        .create_upload(&CreateAttachmentUpload {
            id,
            account_id: account,
            object_key: AttachmentStore::object_key(account, id),
            original_filename: request.filename,
        })
        .await?;
    let upload = store
        .presign_upload(account, row.id)
        .await
        .map_err(|error| storage_failed(row.id, error))?;
    Ok((
        StatusCode::CREATED,
        Json(AttachmentUploadResponse {
            id: AttachmentId(upload.attachment_id),
            upload_url: upload.upload_url,
            headers: upload.headers,
            expires_at: rfc3339(upload.expires_at),
        }),
    ))
}

pub async fn finalize(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(id): Path<String>,
) -> Result<Json<AttachmentDescriptor>, ApiError> {
    // Anything but a canonical `att_` id is a missing attachment.
    let id = AttachmentId::parse(&id)
        .map(Uuid::from)
        .ok_or_else(ApiError::attachment_not_found)?;
    let attachment = state
        .attachments
        .get_for_account(account, id)
        .await?
        .ok_or_else(ApiError::attachment_not_found)?;
    // Finalization is idempotent: a decided upload answers from its row. A
    // ready upload is confirmed to still be in the bucket first, because
    // the lifecycle rule expires it if no invoice is issued in time.
    let store = state.attachment_store()?;
    match attachment.status.parse::<AttachmentStatus>() {
        Ok(AttachmentStatus::PendingUpload | AttachmentStatus::Scanning) => {}
        Ok(AttachmentStatus::Ready) => {
            let present = store
                .is_present(&attachment.object_key, attachment.version_id.as_deref())
                .await
                .map_err(|error| storage_failed(id, error))?;
            if !present {
                expire(&state, account, &attachment).await?;
                return Err(ApiError::attachment_expired());
            }
            return finalized(&attachment).map(Json);
        }
        _ => return finalized(&attachment).map(Json),
    }
    match store.inspect_and_hash(&attachment.object_key).await {
        Ok(inspected) => {
            let row = state
                .attachments
                .mark_ready(
                    account,
                    id,
                    inspected.byte_length,
                    inspected.sha256,
                    inspected.version_id.as_deref(),
                    CLEAN_SCAN,
                )
                .await?;
            // A concurrent finalize may have decided first; the row is
            // authoritative either way.
            let row = match row {
                Some(row) => row,
                None => state
                    .attachments
                    .get_for_account(account, id)
                    .await?
                    .ok_or_else(ApiError::attachment_not_found)?,
            };
            finalized(&row).map(Json)
        }
        Err(AttachmentError::NotUploaded) => Err(ApiError::attachment_not_uploaded()),
        Err(AttachmentError::ScanPending) => Err(ApiError::attachment_scan_pending()),
        Err(AttachmentError::Rejected(reason)) => {
            state
                .attachments
                .mark_rejected(account, id, &reason)
                .await?;
            // The decision is recorded; the object itself is only clutter
            // now, so its removal is best-effort and the verdict stands.
            if let Err(error) = store.delete(&attachment.object_key).await {
                tracing::warn!(error = %error, attachment_id = %id, "failed to delete a rejected upload");
            }
            Err(ApiError::attachment_rejected(&reason))
        }
        Err(error) => Err(storage_failed(id, error)),
    }
}

/// Record that a ready upload's object is gone from the bucket. The row is
/// re-read when it was no longer ready, so a concurrent issuance that won the
/// race keeps its answer.
pub(crate) async fn expire(
    state: &AppState,
    account: AccountId,
    attachment: &DbAttachment,
) -> Result<(), ApiError> {
    if state
        .attachments
        .mark_expired(account, attachment.id)
        .await?
        .is_none()
    {
        tracing::warn!(attachment_id = %attachment.id, "attachment object is missing but the row was no longer ready");
    } else {
        tracing::warn!(attachment_id = %attachment.id, "attachment object expired before an invoice was issued with it");
    }
    Ok(())
}

/// The merchant's view of an issued invoice's PDF, with a fresh signed link.
pub async fn deposit_request_attachment(
    State(state): State<AppState>,
    Extension(account): Extension<AccountId>,
    Path(reference): Path<String>,
) -> Result<(HeaderMap, Json<AttachmentDescriptor>), ApiError> {
    let row = resolve_deposit_request(&state, account, &reference).await?;
    let attachment = state
        .attachments
        .find_by_invoice(row.id)
        .await?
        .ok_or_else(ApiError::attachment_not_found)?;
    Ok((
        no_store(),
        Json(signed_descriptor(&state, &attachment).await?),
    ))
}

/// The descriptor plus a download link that expires in minutes. Responses
/// carrying it must not be cached.
pub(crate) async fn signed_descriptor(
    state: &AppState,
    attachment: &DbAttachment,
) -> Result<AttachmentDescriptor, ApiError> {
    let mut descriptor = finalized(attachment)?;
    let url = state
        .attachment_store()?
        .presign_download(
            &attachment.object_key,
            attachment.version_id.as_deref(),
            &attachment.original_filename,
        )
        .await
        .map_err(|error| storage_failed(attachment.id, error))?;
    descriptor.download_url = Some(url);
    Ok(descriptor)
}

pub(crate) fn no_store() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers
}

/// The descriptor of a decided upload: ready or attached rows describe the
/// document, rejected rows explain why they never will.
fn finalized(attachment: &DbAttachment) -> Result<AttachmentDescriptor, ApiError> {
    if attachment.is_expired() {
        return Err(ApiError::attachment_expired());
    }
    if attachment.status.parse::<AttachmentStatus>() == Ok(AttachmentStatus::Rejected) {
        return Err(ApiError::attachment_rejected(
            attachment.scan_result.as_deref().unwrap_or("rejected"),
        ));
    }
    attachment.descriptor().ok_or_else(|| {
        tracing::error!(attachment_id = %attachment.id, status = %attachment.status, "attachment row has no commitment");
        ApiError::internal("attachment is not finalized")
    })
}

fn storage_failed(attachment_id: Uuid, error: AttachmentError) -> ApiError {
    tracing::error!(error = %error, %attachment_id, "attachment storage request failed");
    ApiError::internal("Attachment storage is unavailable")
}
