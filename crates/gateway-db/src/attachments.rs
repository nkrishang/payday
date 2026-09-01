//! PDF attachments (product plan §4.2): staged by presigned upload,
//! finalized after the malware scan, and bound to exactly one invoice at
//! issuance. The bytes live in object storage; this table holds the
//! commitment (length and SHA-256) the invoice is hashed over.

use std::fmt;
use std::str::FromStr;

use alloy_primitives::{B256, hex};
use gateway_core::{AttachmentCommitment, AttachmentDescriptor, PDF_MIME_TYPE};
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use thiserror::Error;
use uuid::Uuid;

use crate::AccountId;

/// The `scan_result` of a ready upload that expired from the bucket before it
/// was attached; it is not a scanner verdict.
const EXPIRED_UPLOAD: &str = "expired";

/// Attachment lifecycle. Only `ready` can be issued; `attached` is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentStatus {
    PendingUpload,
    Scanning,
    Ready,
    Rejected,
    Attached,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("unknown attachment status: {0}")]
pub struct AttachmentStatusParseError(pub String);

impl AttachmentStatus {
    pub const ALL: [AttachmentStatus; 5] = [
        AttachmentStatus::PendingUpload,
        AttachmentStatus::Scanning,
        AttachmentStatus::Ready,
        AttachmentStatus::Rejected,
        AttachmentStatus::Attached,
    ];

    /// The canonical string stored in `invoice_attachments.status`; the
    /// migration's CHECK constraint agrees with it.
    pub fn as_str(self) -> &'static str {
        match self {
            AttachmentStatus::PendingUpload => "pending_upload",
            AttachmentStatus::Scanning => "scanning",
            AttachmentStatus::Ready => "ready",
            AttachmentStatus::Rejected => "rejected",
            AttachmentStatus::Attached => "attached",
        }
    }
}

impl fmt::Display for AttachmentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AttachmentStatus {
    type Err = AttachmentStatusParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        AttachmentStatus::ALL
            .into_iter()
            .find(|status| status.as_str() == s)
            .ok_or_else(|| AttachmentStatusParseError(s.to_string()))
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DbAttachment {
    pub id: Uuid,
    pub account_id: Uuid,
    pub invoice_id: Option<Uuid>,
    pub object_key: String,
    pub original_filename: String,
    pub mime_type: String,
    pub byte_length: Option<i64>,
    pub sha256: Option<Vec<u8>>,
    /// The bucket version the commitment was computed over; `None` before
    /// finalization and on unversioned buckets.
    pub version_id: Option<String>,
    pub status: String,
    pub scan_result: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finalized_at: Option<DateTime<Utc>>,
    pub attached_at: Option<DateTime<Utc>>,
}

impl DbAttachment {
    /// Whether the upload was rejected because its object expired from the
    /// bucket before an invoice was issued with it.
    pub fn is_expired(&self) -> bool {
        self.status == AttachmentStatus::Rejected.as_str()
            && self.scan_result.as_deref() == Some(EXPIRED_UPLOAD)
    }

    /// The commitment an invoice hashes over; `None` until finalized.
    pub fn commitment(&self) -> Option<AttachmentCommitment> {
        Some(AttachmentCommitment {
            id: self.id,
            byte_length: self.byte_length?.to_string(),
            sha256: hex::encode_prefixed(self.sha256.as_deref()?),
        })
    }

    /// The descriptor shown to whoever may see the invoice, without a
    /// download link; `None` until finalized.
    pub fn descriptor(&self) -> Option<AttachmentDescriptor> {
        let commitment = self.commitment()?;
        Some(AttachmentDescriptor {
            id: self.id,
            filename: self.original_filename.clone(),
            mime_type: self.mime_type.clone(),
            byte_length: commitment.byte_length,
            sha256: commitment.sha256,
            download_url: None,
        })
    }
}

pub struct CreateAttachmentUpload {
    pub id: Uuid,
    pub account_id: AccountId,
    pub object_key: String,
    pub original_filename: String,
}

#[derive(Debug, Error)]
pub enum AttachInvoiceError {
    #[error("attachment not found")]
    NotFound,
    #[error("attachment is not ready to be issued")]
    NotReady,
    #[error("attachment is already attached to an invoice")]
    AlreadyAttached,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Clone)]
pub struct AttachmentRepository {
    pool: PgPool,
}

impl AttachmentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Stage an upload. The object does not exist yet; the row reserves its
    /// key so finalization can find and verify it.
    pub async fn create_upload(
        &self,
        input: &CreateAttachmentUpload,
    ) -> Result<DbAttachment, sqlx::Error> {
        sqlx::query_as::<_, DbAttachment>(
            r#"INSERT INTO invoice_attachments
                 (id, account_id, object_key, original_filename, mime_type, status)
               VALUES ($1, $2, $3, $4, $5, $6)
               RETURNING *"#,
        )
        .bind(input.id)
        .bind(input.account_id.0)
        .bind(&input.object_key)
        .bind(&input.original_filename)
        .bind(PDF_MIME_TYPE)
        .bind(AttachmentStatus::PendingUpload.as_str())
        .fetch_one(&self.pool)
        .await
    }

    /// Record a clean scan and the verified content commitment, pinned to the
    /// bucket version it was computed over. Only an unfinalized upload can
    /// become ready; `scan_result` must be the clean verdict or the schema
    /// refuses the row.
    pub async fn mark_ready(
        &self,
        account: AccountId,
        id: Uuid,
        byte_length: u64,
        sha256: B256,
        version_id: Option<&str>,
        scan_result: &str,
    ) -> Result<Option<DbAttachment>, sqlx::Error> {
        sqlx::query_as::<_, DbAttachment>(
            r#"UPDATE invoice_attachments
               SET byte_length = $3, sha256 = $4, version_id = $5, scan_result = $6, status = $7,
                   finalized_at = now()
               WHERE account_id = $1 AND id = $2 AND status = ANY($8)
               RETURNING *"#,
        )
        .bind(account.0)
        .bind(id)
        .bind(byte_length as i64)
        .bind(sha256.as_slice())
        .bind(version_id)
        .bind(scan_result)
        .bind(AttachmentStatus::Ready.as_str())
        .bind(unfinalized())
        .fetch_optional(&self.pool)
        .await
    }

    /// A ready upload whose object the bucket no longer holds (the lifecycle
    /// rule expired it before an invoice was issued) can never be issued:
    /// it becomes `rejected` with [`EXPIRED_UPLOAD`] as the reason. Returns
    /// `None` when the row was not ready any more.
    pub async fn mark_expired(
        &self,
        account: AccountId,
        id: Uuid,
    ) -> Result<Option<DbAttachment>, sqlx::Error> {
        sqlx::query_as::<_, DbAttachment>(
            r#"UPDATE invoice_attachments
               SET scan_result = $3, status = $4
               WHERE account_id = $1 AND id = $2 AND status = $5
               RETURNING *"#,
        )
        .bind(account.0)
        .bind(id)
        .bind(EXPIRED_UPLOAD)
        .bind(AttachmentStatus::Rejected.as_str())
        .bind(AttachmentStatus::Ready.as_str())
        .fetch_optional(&self.pool)
        .await
    }

    /// Record a failed finalization (wrong type, size, magic bytes, or a
    /// scan verdict other than clean). A rejected upload can never be issued.
    pub async fn mark_rejected(
        &self,
        account: AccountId,
        id: Uuid,
        scan_result: &str,
    ) -> Result<Option<DbAttachment>, sqlx::Error> {
        sqlx::query_as::<_, DbAttachment>(
            r#"UPDATE invoice_attachments
               SET scan_result = $3, status = $4, finalized_at = now()
               WHERE account_id = $1 AND id = $2 AND status = ANY($5)
               RETURNING *"#,
        )
        .bind(account.0)
        .bind(id)
        .bind(scan_result)
        .bind(AttachmentStatus::Rejected.as_str())
        .bind(unfinalized())
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn get_for_account(
        &self,
        account: AccountId,
        id: Uuid,
    ) -> Result<Option<DbAttachment>, sqlx::Error> {
        sqlx::query_as::<_, DbAttachment>(
            "SELECT * FROM invoice_attachments WHERE account_id = $1 AND id = $2",
        )
        .bind(account.0)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    /// Bind a ready attachment to an invoice inside the caller's transaction,
    /// so the invoice insert and the binding commit or roll back together.
    pub async fn attach_to_invoice(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        account: AccountId,
        attachment_id: Uuid,
        invoice_id: Uuid,
    ) -> Result<DbAttachment, AttachInvoiceError> {
        attach_in_transaction(tx, account, attachment_id, invoice_id).await
    }

    pub async fn find_by_invoice(
        &self,
        invoice_id: Uuid,
    ) -> Result<Option<DbAttachment>, sqlx::Error> {
        find_by_invoice_with(&self.pool, invoice_id).await
    }
}

fn unfinalized() -> Vec<&'static str> {
    vec![
        AttachmentStatus::PendingUpload.as_str(),
        AttachmentStatus::Scanning.as_str(),
    ]
}

pub(crate) async fn find_by_invoice_with<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    invoice_id: Uuid,
) -> Result<Option<DbAttachment>, sqlx::Error> {
    sqlx::query_as::<_, DbAttachment>("SELECT * FROM invoice_attachments WHERE invoice_id = $1")
        .bind(invoice_id)
        .fetch_optional(executor)
        .await
}

/// Lock the attachment row, require `ready`, and bind it. The lock serializes
/// two issuances racing for the same document: the second sees `attached`.
pub(crate) async fn attach_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    account: AccountId,
    attachment_id: Uuid,
    invoice_id: Uuid,
) -> Result<DbAttachment, AttachInvoiceError> {
    let locked = sqlx::query_as::<_, DbAttachment>(
        "SELECT * FROM invoice_attachments WHERE account_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(account.0)
    .bind(attachment_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(AttachInvoiceError::NotFound)?;
    match locked.status.parse::<AttachmentStatus>() {
        Ok(AttachmentStatus::Ready) => {}
        Ok(AttachmentStatus::Attached) => return Err(AttachInvoiceError::AlreadyAttached),
        _ => return Err(AttachInvoiceError::NotReady),
    }
    sqlx::query_as::<_, DbAttachment>(
        r#"UPDATE invoice_attachments
           SET invoice_id = $3, status = $4, attached_at = now()
           WHERE account_id = $1 AND id = $2 AND status = $5
           RETURNING *"#,
    )
    .bind(account.0)
    .bind(attachment_id)
    .bind(invoice_id)
    .bind(AttachmentStatus::Attached.as_str())
    .bind(AttachmentStatus::Ready.as_str())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(AttachInvoiceError::NotReady)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::invoices::tests::{account, issuance_input};
    use crate::{InsertIssuedInvoiceError, InvoiceRepository};

    const CLEAN: &str = "NO_THREATS_FOUND";

    pub(crate) async fn upload(
        repo: &AttachmentRepository,
        account: AccountId,
        name: &str,
    ) -> DbAttachment {
        let id = Uuid::now_v7();
        repo.create_upload(&CreateAttachmentUpload {
            id,
            account_id: account,
            object_key: format!("{}/{id}.pdf", account.0),
            original_filename: name.into(),
        })
        .await
        .unwrap()
    }

    pub(crate) async fn ready(
        repo: &AttachmentRepository,
        account: AccountId,
        name: &str,
    ) -> DbAttachment {
        let staged = upload(repo, account, name).await;
        repo.mark_ready(
            account,
            staged.id,
            1_234,
            B256::repeat_byte(0xAB),
            Some("v1"),
            CLEAN,
        )
        .await
        .unwrap()
        .expect("a staged upload can be finalized")
    }

    #[test]
    fn status_strings_round_trip() {
        for status in AttachmentStatus::ALL {
            assert_eq!(status.as_str().parse::<AttachmentStatus>().unwrap(), status);
        }
        assert!("clean".parse::<AttachmentStatus>().is_err());
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn finalization_fixes_the_commitment_once(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let repo = AttachmentRepository::new(pool.clone());
        let staged = upload(&repo, owner, "contract.pdf").await;
        assert_eq!(staged.status, "pending_upload");
        assert!(staged.commitment().is_none());
        assert!(staged.descriptor().is_none());

        let ready = repo
            .mark_ready(
                owner,
                staged.id,
                1_234,
                B256::repeat_byte(0xAB),
                Some("version-1"),
                CLEAN,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ready.status, "ready");
        assert_eq!(ready.version_id.as_deref(), Some("version-1"));
        let commitment = ready.commitment().unwrap();
        assert_eq!(commitment.byte_length, "1234");
        assert_eq!(commitment.sha256, format!("0x{}", "ab".repeat(32)));
        let descriptor = ready.descriptor().unwrap();
        assert_eq!(descriptor.filename, "contract.pdf");
        assert_eq!(descriptor.mime_type, "application/pdf");
        assert!(descriptor.download_url.is_none());

        // Finalized rows are immutable and the other account sees nothing.
        assert!(
            repo.mark_ready(owner, staged.id, 1, B256::repeat_byte(0xCD), None, CLEAN)
                .await
                .unwrap()
                .is_none()
        );
        let repinned = sqlx::query("UPDATE invoice_attachments SET version_id = $2 WHERE id = $1")
            .bind(staged.id)
            .bind("version-2")
            .execute(&pool)
            .await;
        assert!(
            repinned.is_err(),
            "the pinned version is part of the commitment"
        );
        assert!(
            repo.mark_rejected(owner, staged.id, "THREATS_FOUND")
                .await
                .unwrap()
                .is_none()
        );
        let other = account(&pool, 2).await;
        assert!(
            repo.get_for_account(other, staged.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            repo.mark_ready(
                other,
                staged.id,
                1_234,
                B256::repeat_byte(0xAB),
                None,
                CLEAN
            )
            .await
            .unwrap()
            .is_none()
        );

        // A dirty verdict can only ever reject.
        let dirty = upload(&repo, owner, "dirty.pdf").await;
        assert!(
            repo.mark_ready(
                owner,
                dirty.id,
                10,
                B256::repeat_byte(0x01),
                None,
                "THREATS_FOUND"
            )
            .await
            .is_err(),
            "the schema refuses a ready row without a clean scan"
        );
        let rejected = repo
            .mark_rejected(owner, dirty.id, "THREATS_FOUND")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rejected.status, "rejected");
        assert!(rejected.commitment().is_none());
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn only_a_ready_upload_can_expire(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let repo = AttachmentRepository::new(pool.clone());
        let staged = upload(&repo, owner, "staged.pdf").await;
        assert!(repo.mark_expired(owner, staged.id).await.unwrap().is_none());

        let document = ready(&repo, owner, "contract.pdf").await;
        let expired = repo
            .mark_expired(owner, document.id)
            .await
            .unwrap()
            .expect("a ready upload can expire");
        assert_eq!(expired.status, "rejected");
        assert_eq!(expired.scan_result.as_deref(), Some("expired"));
        assert!(expired.is_expired());
        assert!(!document.is_expired());
        assert!(
            repo.mark_expired(owner, document.id)
                .await
                .unwrap()
                .is_none()
        );
        let issued = InvoiceRepository::new(pool.clone())
            .insert_issued(&issuance_input(owner, "expired", None), Some(document.id))
            .await
            .unwrap_err();
        assert!(matches!(
            issued,
            InsertIssuedInvoiceError::AttachmentNotReady
        ));
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn one_attachment_per_invoice(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let attachments = AttachmentRepository::new(pool.clone());
        let invoices = InvoiceRepository::new(pool.clone());
        let first = ready(&attachments, owner, "first.pdf").await;
        let second = ready(&attachments, owner, "second.pdf").await;

        let issued = invoices
            .insert_issued(&issuance_input(owner, "one", Some(&first)), Some(first.id))
            .await
            .unwrap();
        let attached = issued.attachment.unwrap();
        assert_eq!(attached.status, "attached");
        assert_eq!(attached.invoice_id, Some(issued.row.id));
        assert!(attached.attached_at.is_some());

        // A second document cannot join the same invoice.
        let mut tx = pool.begin().await.unwrap();
        let error = attachments
            .attach_to_invoice(&mut tx, owner, second.id, issued.row.id)
            .await
            .unwrap_err();
        assert!(
            matches!(error, AttachInvoiceError::Database(_)),
            "the unique invoice_id refuses it: {error}"
        );
        drop(tx);

        // Nor can the attached document join a second invoice.
        let reuse = invoices
            .insert_issued(&issuance_input(owner, "two", Some(&first)), Some(first.id))
            .await
            .unwrap_err();
        assert!(matches!(
            reuse,
            InsertIssuedInvoiceError::AttachmentAlreadyAttached
        ));
        assert_eq!(
            attachments
                .find_by_invoice(issued.row.id)
                .await
                .unwrap()
                .unwrap()
                .id,
            first.id
        );
        assert_eq!(
            attachments
                .get_for_account(owner, second.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "ready"
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn only_ready_attachment_can_be_issued(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let other = account(&pool, 2).await;
        let attachments = AttachmentRepository::new(pool.clone());
        let invoices = InvoiceRepository::new(pool.clone());
        let pending = upload(&attachments, owner, "pending.pdf").await;
        let rejected = upload(&attachments, owner, "rejected.pdf").await;
        attachments
            .mark_rejected(owner, rejected.id, "THREATS_FOUND")
            .await
            .unwrap()
            .unwrap();
        let foreign = ready(&attachments, other, "foreign.pdf").await;

        for (name, attachment, expected) in [
            ("pending", pending.id, "not ready"),
            ("rejected", rejected.id, "not ready"),
            ("foreign", foreign.id, "not found"),
            ("unknown", Uuid::now_v7(), "not found"),
        ] {
            let error = invoices
                .insert_issued(&issuance_input(owner, name, None), Some(attachment))
                .await
                .unwrap_err();
            let matched = match expected {
                "not ready" => matches!(error, InsertIssuedInvoiceError::AttachmentNotReady),
                _ => matches!(error, InsertIssuedInvoiceError::AttachmentNotFound),
            };
            assert!(matched, "{name}: {error}");
            assert!(
                invoices
                    .find_by_idempotency_key(owner, name)
                    .await
                    .unwrap()
                    .is_none(),
                "{name}: no invoice may be issued"
            );
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invoices")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn attachment_and_invoice_are_committed_atomically(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let attachments = AttachmentRepository::new(pool.clone());
        let invoices = InvoiceRepository::new(pool.clone());
        let document = ready(&attachments, owner, "contract.pdf").await;

        let issued = invoices
            .insert_issued(
                &issuance_input(owner, "atomic", Some(&document)),
                Some(document.id),
            )
            .await
            .unwrap();
        assert!(!issued.replayed);
        let row = invoices.find_by_id(issued.row.id).await.unwrap().unwrap();
        assert_eq!(
            row.issuance_snapshot.unwrap().0.attachment.unwrap().id,
            document.id
        );
        let bound = attachments
            .get_for_account(owner, document.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bound.status, "attached");
        assert_eq!(bound.invoice_id, Some(row.id));

        // When the binding fails the invoice insert rolls back with it.
        let failed = invoices
            .insert_issued(
                &issuance_input(owner, "rolled-back", Some(&document)),
                Some(document.id),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            failed,
            InsertIssuedInvoiceError::AttachmentAlreadyAttached
        ));
        assert!(
            invoices
                .find_by_idempotency_key(owner, "rolled-back")
                .await
                .unwrap()
                .is_none()
        );
    }
}
