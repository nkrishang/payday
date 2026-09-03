//! Database row type and repository for invoices.
//!
//! All conversions between domain types and DB column types happen here.
//! Handlers and application logic never see SQL or DB rows.

use std::collections::{HashMap, HashSet};

use alloy_primitives::{Address, B256, U256};
use sqlx::PgPool;
use thiserror::Error;
use uuid::Uuid;

use crate::attachments::{AttachInvoiceError, DbAttachment, attach_in_transaction};
use crate::cursor::IndexerCursor;
use crate::{AccountId, attachments};
use gateway_core::{
    Amount, BeneficiaryAddress, CanonicalIssuanceSnapshot, ChainId, ExpectedIdentity,
    FactoryAddress, Invoice, InvoiceId, InvoiceStatusParseError, Party, PayerPolicy,
    PayerPolicyMode, PaymentAddress, RecoveryAddress, Salt, TokenAddress,
};

/// Database row representing one invoice.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DbInvoice {
    pub id: Uuid,
    pub account_id: Uuid,
    pub idempotency_key: String,
    pub chain_id: i64,
    pub factory_address: Vec<u8>,
    pub token_address: Vec<u8>,
    pub token_decimals: i16,
    pub beneficiary_address: Vec<u8>,
    pub expiration_timestamp: i64,
    pub expires_in_secs: i64,
    pub expiration_intent: String,
    pub recovery_address: Vec<u8>,
    pub amount: String,
    pub salt: Vec<u8>,
    pub payment_address: Vec<u8>,
    pub status: String,
    pub created_at: sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,
    pub updated_at: sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,
    /// Block height at which credited transfers first reached `amount`.
    pub funded_at_block: Option<i64>,
    /// Canonical hash paired with `funded_at_block`.
    pub funded_at_block_hash: Option<Vec<u8>>,
    /// Cumulative credited base units at funding time (decimal string).
    pub observed_amount: Option<String>,
    /// Sum of finalized USDC transfers credited toward `amount`.
    pub confirmed_received: String,
    /// Open sweep batch this invoice is part of, if a helper transaction is in flight.
    pub sweep_batch_id: Option<Uuid>,
    /// Block of the last finalized transaction that emptied the payment address.
    pub drained_at_block: Option<i64>,
    /// Position within `drained_at_block` of the transaction that emptied it.
    pub drained_at_transaction_index: Option<i64>,
    /// Nonzero observations not yet known to have been drained; the sweep
    /// queue is exactly the invoices where this is positive.
    pub uncollected_count: i32,
    /// Hash of the transaction that deployed `Payment`, when this worker sent it.
    pub execute_tx_hash: Option<Vec<u8>>,
    /// Block at which the invoice reached `fulfilled` or `recovered`.
    pub resolved_at_block: Option<i64>,
    pub settled_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    /// Consecutive unsuccessful sweep attempts; drives the exponential backoff.
    pub sweep_attempts: i32,
    /// Most recent sweep attempt; with `sweep_attempts` it gates the next retry.
    pub last_attempt_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    /// Why automatic sweeping stopped. Non-NULL rows are excluded from the queue.
    pub blocked_reason: Option<String>,
    pub cancellation_requested_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    pub reference: Option<String>,
    pub metadata: sqlx::types::Json<serde_json::Value>,
    pub paid_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    pub expired_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    pub settlement_tx_hash: Option<Vec<u8>>,
    pub fee_amount: String,
    pub net_amount: String,
    /// Optional link to the merchant's customer record; the parties below are
    /// the snapshot taken at issuance.
    pub customer_id: Option<Uuid>,
    pub issuer: Option<sqlx::types::Json<Party>>,
    pub bill_to: Option<sqlx::types::Json<Party>>,
    pub notes: Option<String>,
    pub heading: Option<String>,
    pub payer_policy_mode: String,
    pub expected_email: Option<String>,
    pub expected_identity: Option<sqlx::types::Json<ExpectedIdentity>>,
    pub verification_completed_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    /// Proof material; nullable in the schema only until pre-release reset.
    pub issuance_snapshot: Option<sqlx::types::Json<CanonicalIssuanceSnapshot>>,
    pub attribution_version: Option<i16>,
    pub attribution_nonce: Option<Vec<u8>>,
    pub attribution_hash: Option<Vec<u8>>,
    pub likely_unsolicited_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
}

/// A stored invoice row could not be decoded into the domain model. This
/// signals corrupt or out-of-contract data in the database, not client error.
#[derive(Debug, Error)]
pub enum DbInvoiceError {
    #[error("invalid amount in DB row {id}: {value:?}")]
    InvalidAmount { id: Uuid, value: String },
    #[error("DB row {id} has no {field}")]
    MissingAttribution { id: Uuid, field: &'static str },
    #[error("invalid {field} bytes in DB row {id}: expected {expected} bytes, got {got}")]
    WrongByteLength {
        id: Uuid,
        field: &'static str,
        expected: usize,
        got: usize,
    },
    #[error("invalid status in DB row {id}: {source}")]
    InvalidStatus {
        id: Uuid,
        #[source]
        source: InvoiceStatusParseError,
    },
}

/// Decode a fixed-length address column, mapping a wrong length to a typed error
/// instead of panicking (as `Address::from_slice` would).
fn address_from_col(
    id: Uuid,
    field: &'static str,
    bytes: &[u8],
) -> Result<Address, DbInvoiceError> {
    Address::try_from(bytes).map_err(|_| DbInvoiceError::WrongByteLength {
        id,
        field,
        expected: 20,
        got: bytes.len(),
    })
}

fn word_from_col(id: Uuid, field: &'static str, bytes: &[u8]) -> Result<B256, DbInvoiceError> {
    B256::try_from(bytes).map_err(|_| DbInvoiceError::WrongByteLength {
        id,
        field,
        expected: 32,
        got: bytes.len(),
    })
}

fn units_from_col(id: Uuid, value: &str) -> Result<U256, DbInvoiceError> {
    U256::from_str_radix(value, 10).map_err(|_| DbInvoiceError::InvalidAmount {
        id,
        value: value.to_string(),
    })
}

impl TryFrom<&DbInvoice> for Invoice {
    type Error = DbInvoiceError;

    /// Convert a DB row to the domain model. Fails with a typed error (rather
    /// than panicking) when a column holds data outside the contract enforced
    /// by the schema's CHECK constraints.
    fn try_from(row: &DbInvoice) -> Result<Self, Self::Error> {
        let status = row
            .status
            .parse()
            .map_err(|source| DbInvoiceError::InvalidStatus { id: row.id, source })?;
        let attribution = match (
            row.attribution_version,
            row.attribution_nonce.as_deref(),
            row.attribution_hash.as_deref(),
            row.issuance_snapshot.as_ref(),
        ) {
            (Some(version), Some(nonce), Some(hash), Some(snapshot)) => (
                version as u16,
                word_from_col(row.id, "attribution_nonce", nonce)?,
                word_from_col(row.id, "attribution_hash", hash)?,
                snapshot.0.clone(),
            ),
            (None, None, None, None) => legacy_attribution(row)?,
            _ => {
                let field = if row.attribution_version.is_none() {
                    "attribution_version"
                } else if row.attribution_nonce.is_none() {
                    "attribution_nonce"
                } else if row.attribution_hash.is_none() {
                    "attribution_hash"
                } else {
                    "issuance_snapshot"
                };
                return Err(DbInvoiceError::MissingAttribution { id: row.id, field });
            }
        };
        let (attribution_version, attribution_nonce, attribution_hash, issuance_snapshot) =
            attribution;

        Ok(Invoice {
            id: InvoiceId(row.id),
            chain_id: ChainId(row.chain_id as u64),
            token: TokenAddress(address_from_col(
                row.id,
                "token_address",
                &row.token_address,
            )?),
            beneficiary: BeneficiaryAddress(address_from_col(
                row.id,
                "beneficiary_address",
                &row.beneficiary_address,
            )?),
            expiration_timestamp: row.expiration_timestamp as u64,
            recovery: RecoveryAddress(address_from_col(
                row.id,
                "recovery_address",
                &row.recovery_address,
            )?),
            factory: FactoryAddress(address_from_col(
                row.id,
                "factory_address",
                &row.factory_address,
            )?),
            amount: Amount(units_from_col(row.id, &row.amount)?),
            salt: Salt(word_from_col(row.id, "salt", &row.salt)?),
            payment_address: PaymentAddress(address_from_col(
                row.id,
                "payment_address",
                &row.payment_address,
            )?),
            status,
            received: Amount(units_from_col(row.id, &row.confirmed_received)?),
            execute_tx_hash: row
                .execute_tx_hash
                .as_deref()
                .map(|hash| word_from_col(row.id, "execute_tx_hash", hash))
                .transpose()?,
            resolved_at_block: row.resolved_at_block.map(|block| block as u64),
            settled_at_timestamp: row.settled_at.map(|time| time.timestamp() as u64),
            blocked_reason: row.blocked_reason.clone(),
            cancellation_requested_at: row.cancellation_requested_at.map(|time| time.to_rfc3339()),
            attribution_version,
            attribution_nonce,
            attribution_hash,
            issuance_snapshot,
        })
    }
}

/// Migration 0011 deliberately permits pre-attribution rows. Keep those
/// invoices readable, but mark the unavailable commitment as version zero so
/// it can never be mistaken for a verifiable v1 issuance.
fn legacy_attribution(
    row: &DbInvoice,
) -> Result<(u16, B256, B256, CanonicalIssuanceSnapshot), DbInvoiceError> {
    let party = |name: &str| Party {
        name: name.into(),
        email: None,
        details: None,
    };
    let mut snapshot = CanonicalIssuanceSnapshot::new(
        row.issuer
            .as_ref()
            .map(|value| value.0.clone())
            .unwrap_or_else(|| party("Legacy issuer")),
        row.bill_to
            .as_ref()
            .map(|value| value.0.clone())
            .unwrap_or_else(|| party("Legacy payer")),
        PayerPolicy::Permissionless,
        FactoryAddress(address_from_col(
            row.id,
            "factory_address",
            &row.factory_address,
        )?),
        ChainId(row.chain_id as u64),
        TokenAddress(address_from_col(
            row.id,
            "token_address",
            &row.token_address,
        )?),
        BeneficiaryAddress(address_from_col(
            row.id,
            "beneficiary_address",
            &row.beneficiary_address,
        )?),
        Amount(units_from_col(row.id, &row.amount)?),
        row.expiration_timestamp as u64,
        RecoveryAddress(address_from_col(
            row.id,
            "recovery_address",
            &row.recovery_address,
        )?),
    );
    snapshot.notes = row.notes.clone();
    snapshot.heading = row.heading.clone();
    snapshot.reference = row.reference.clone();
    Ok((0, B256::ZERO, B256::ZERO, snapshot))
}

#[derive(Clone)]
pub struct InvoiceRepository {
    pool: PgPool,
}

#[derive(Debug, thiserror::Error)]
pub enum ReleasePaymentError {
    #[error("payment not found")]
    NotFound,
    #[error("payment does not need attention")]
    NotBlocked,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// The issued invoice and, when one was bound, its attachment. `replayed`
/// means an identical request had already been issued under the key.
#[derive(Debug, Clone)]
pub struct InsertIssuedInvoice {
    pub row: DbInvoice,
    pub attachment: Option<DbAttachment>,
    pub replayed: bool,
}

#[derive(Debug, Error)]
pub enum InsertIssuedInvoiceError {
    #[error("idempotency key already used with different request parameters")]
    IdempotencyConflict,
    #[error("attachment is not ready to be issued")]
    AttachmentNotReady,
    #[error("attachment not found")]
    AttachmentNotFound,
    #[error("attachment is already attached to an invoice")]
    AttachmentAlreadyAttached,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl From<AttachInvoiceError> for InsertIssuedInvoiceError {
    fn from(error: AttachInvoiceError) -> Self {
        match error {
            AttachInvoiceError::NotFound => Self::AttachmentNotFound,
            AttachInvoiceError::NotReady => Self::AttachmentNotReady,
            AttachInvoiceError::AlreadyAttached => Self::AttachmentAlreadyAttached,
            AttachInvoiceError::Database(error) => Self::Database(error),
        }
    }
}

/// Input for creating an invoice in the DB.
pub struct CreateInvoiceInput {
    pub id: Uuid,
    pub account_id: AccountId,
    pub idempotency_key: String,
    pub customer_id: Option<Uuid>,
    pub issuer: Party,
    pub bill_to: Party,
    pub notes: Option<String>,
    pub heading: Option<String>,
    pub reference: Option<String>,
    pub metadata: serde_json::Value,
    pub payer_policy: PayerPolicy,
    pub chain_id: u64,
    pub factory_address: [u8; 20],
    pub token_address: [u8; 20],
    pub token_decimals: u8,
    pub beneficiary_address: [u8; 20],
    pub expiration_timestamp: u64,
    pub expires_in_secs: u64,
    pub expiration_intent: String,
    pub recovery_address: [u8; 20],
    pub amount: String,
    pub salt: [u8; 32],
    pub payment_address: [u8; 20],
    pub issuance_snapshot: CanonicalIssuanceSnapshot,
    pub attribution_version: u16,
    pub attribution_nonce: [u8; 32],
    pub attribution_hash: [u8; 32],
}

/// Merchant-controlled issuance fields used by both the optimistic API replay
/// check and the transaction's authoritative race check.
pub struct IssuanceRequest<'a> {
    pub chain_id: u64,
    pub factory: &'a [u8],
    pub token: &'a [u8],
    pub token_decimals: u8,
    pub beneficiary: &'a [u8],
    pub amount: U256,
    pub expiration_intent: &'a str,
    pub recovery: &'a [u8],
    pub issuer: &'a Party,
    pub bill_to: &'a Party,
    pub notes: Option<&'a str>,
    pub heading: Option<&'a str>,
    pub reference: Option<&'a str>,
    pub metadata: &'a serde_json::Value,
    pub customer_id: Option<Uuid>,
    pub payer_policy: &'a PayerPolicy,
    pub attachment_id: Option<Uuid>,
    pub attachment: Option<&'a gateway_core::AttachmentCommitment>,
}

pub fn same_issuance(existing: &DbInvoice, request: &IssuanceRequest<'_>) -> bool {
    let committed = existing
        .issuance_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.0.attachment.as_ref());
    existing.chain_id as u64 == request.chain_id
        && existing.factory_address == request.factory
        && existing.token_address == request.token
        && existing.token_decimals == request.token_decimals as i16
        && existing.beneficiary_address == request.beneficiary
        && existing.expiration_intent == request.expiration_intent
        && existing.recovery_address == request.recovery
        && U256::from_str_radix(&existing.amount, 10).is_ok_and(|amount| amount == request.amount)
        && existing.reference.as_deref() == request.reference
        && existing.metadata.0 == *request.metadata
        && existing.customer_id == request.customer_id
        && existing.issuer.as_ref().map(|party| &party.0) == Some(request.issuer)
        && existing.bill_to.as_ref().map(|party| &party.0) == Some(request.bill_to)
        && existing.notes.as_deref() == request.notes
        && existing.heading.as_deref() == request.heading
        && existing.payer_policy_mode == request.payer_policy.mode().as_str()
        && existing.expected_email.as_deref() == request.payer_policy.expected_email()
        && existing
            .expected_identity
            .as_ref()
            .map(|identity| &identity.0)
            == request.payer_policy.expected_identity()
        && committed.map(|attachment| attachment.id) == request.attachment_id
        && committed == request.attachment
}

/// One validated, finalized USDC `Transfer` log from the configured token.
#[derive(Debug, Clone)]
pub struct PaymentObservation {
    pub block_number: u64,
    pub block_hash: B256,
    pub block_timestamp: u64,
    pub transaction_hash: B256,
    pub transaction_index: u64,
    pub log_index: u64,
    pub sender: Address,
    pub recipient: Address,
    pub amount: U256,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DbInvoiceTransfer {
    pub invoice_id: Uuid,
    pub block_timestamp: i64,
    pub amount: String,
    pub sender_address: Vec<u8>,
    pub transaction_hash: Vec<u8>,
    pub block_number: i64,
    pub disposition: String,
    pub collected: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DbIndexerFreshness {
    pub chain_id: i64,
    pub token_address: Vec<u8>,
    pub last_block: i64,
    pub last_block_timestamp: Option<i64>,
    pub updated_at: sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,
    pub finalized_block: Option<i64>,
}

/// Invoices whose status changed while a finalized range was applied.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RangeOutcome {
    pub funded: Vec<Uuid>,
    pub expired: Vec<Uuid>,
}

impl CreateInvoiceInput {
    /// Build the write-path input from a freshly issued domain [`Invoice`],
    /// projecting its strongly-typed fields to DB column types. The document
    /// columns come from the issuance snapshot; the idempotency key, token
    /// decimals, and expiry intent are not part of the domain model, so they
    /// are supplied by the caller, and the customer link and metadata are set
    /// afterwards. Status is not included — the INSERT hardcodes `'created'`.
    pub fn from_invoice(
        invoice: &Invoice,
        account_id: AccountId,
        idempotency_key: String,
        token_decimals: u8,
        expires_in_secs: u64,
        expiration_intent: String,
    ) -> Self {
        let snapshot = &invoice.issuance_snapshot;
        Self {
            id: invoice.id.0,
            account_id,
            idempotency_key,
            customer_id: None,
            issuer: snapshot.issuer.clone(),
            bill_to: snapshot.bill_to.clone(),
            notes: snapshot.notes.clone(),
            heading: snapshot.heading.clone(),
            reference: snapshot.reference.clone(),
            metadata: serde_json::json!({}),
            payer_policy: snapshot.payer_policy.clone(),
            chain_id: invoice.chain_id.0,
            factory_address: invoice.factory.0.into(),
            token_address: invoice.token.0.into(),
            token_decimals,
            beneficiary_address: invoice.beneficiary.0.into(),
            expiration_timestamp: invoice.expiration_timestamp,
            expires_in_secs,
            expiration_intent,
            recovery_address: invoice.recovery.0.into(),
            amount: invoice.amount.0.to_string(),
            salt: invoice.salt.0.into(),
            payment_address: invoice.payment_address.0.into(),
            issuance_snapshot: snapshot.clone(),
            attribution_version: invoice.attribution_version,
            attribution_nonce: invoice.attribution_nonce.into(),
            attribution_hash: invoice.attribution_hash.into(),
        }
    }

    /// Whether `existing` was issued from this same request. Everything the
    /// merchant asserted is compared; what the server generated (id, salt,
    /// address, nonce, the resolved deadline of a relative expiry) is not.
    fn same_issuance(
        &self,
        existing: &DbInvoice,
        existing_attachment: Option<&DbAttachment>,
        attachment_id: Option<Uuid>,
    ) -> bool {
        same_issuance(
            existing,
            &IssuanceRequest {
                chain_id: self.chain_id,
                factory: &self.factory_address,
                token: &self.token_address,
                token_decimals: self.token_decimals,
                beneficiary: &self.beneficiary_address,
                amount: U256::from_str_radix(&self.amount, 10).unwrap_or_default(),
                expiration_intent: &self.expiration_intent,
                recovery: &self.recovery_address,
                issuer: &self.issuer,
                bill_to: &self.bill_to,
                notes: self.notes.as_deref(),
                heading: self.heading.as_deref(),
                reference: self.reference.as_deref(),
                metadata: &self.metadata,
                customer_id: self.customer_id,
                payer_policy: &self.payer_policy,
                attachment_id,
                attachment: existing_attachment
                    .and_then(DbAttachment::commitment)
                    .as_ref(),
            },
        )
    }
}

impl InvoiceRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn release_blocked(&self, id: Uuid) -> Result<DbInvoice, ReleasePaymentError> {
        let row = sqlx::query_as::<_, DbInvoice>(
            r#"UPDATE invoices SET
                 blocked_reason=NULL,
                 status=CASE WHEN status IN ('fulfilled','recovered') THEN status
                   WHEN expiration_timestamp < extract(epoch FROM now()) THEN 'expired'
                   ELSE 'deploying' END,
                 sweep_attempts=0,last_attempt_at=NULL,updated_at=now()
               WHERE id=$1 AND blocked_reason IS NOT NULL AND sweep_batch_id IS NULL
               RETURNING *"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = row {
            return Ok(row);
        }
        match sqlx::query_scalar::<_, Option<String>>(
            "SELECT blocked_reason FROM invoices WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        {
            None => Err(ReleasePaymentError::NotFound),
            Some(_) => Err(ReleasePaymentError::NotBlocked),
        }
    }

    /// Round-trip a trivial query so `/health` reflects database reachability.
    pub async fn ping(&self) -> Result<(), sqlx::Error> {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map(drop)
    }

    /// Issue an invoice under an idempotency key, binding its attachment in
    /// the same transaction. The key's existing row is locked and compared
    /// field by field before anything is written: an identical request
    /// replays the original, a different one conflicts. A losing racer whose
    /// insert hits the unique key takes the same path against the winner.
    pub async fn insert_issued(
        &self,
        input: &CreateInvoiceInput,
        attachment_id: Option<Uuid>,
    ) -> Result<InsertIssuedInvoice, InsertIssuedInvoiceError> {
        let mut tx = self.pool.begin().await?;
        if let Some(existing) = lock_by_idempotency_key(&mut tx, input).await? {
            return replay(&mut tx, input, attachment_id, existing).await;
        }

        let inserted = sqlx::query_as::<_, DbInvoice>(
            r#"
            INSERT INTO invoices
                (id, account_id, idempotency_key, customer_id, issuer, bill_to, notes, heading,
                 reference, metadata, payer_policy_mode, expected_email, expected_identity,
                 chain_id, factory_address, token_address, token_decimals, beneficiary_address,
                 expiration_timestamp, expires_in_secs, expiration_intent, recovery_address,
                 amount, net_amount, salt, payment_address, issuance_snapshot,
                 attribution_version, attribution_nonce, attribution_hash, status)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                    $18, $19, $20, $21, $22, $23, $23, $24, $25, $26, $27, $28, $29, 'created')
            ON CONFLICT (account_id, idempotency_key) DO NOTHING
            RETURNING *
            "#,
        )
        .bind(input.id)
        .bind(input.account_id.0)
        .bind(&input.idempotency_key)
        .bind(input.customer_id)
        .bind(sqlx::types::Json(&input.issuer))
        .bind(sqlx::types::Json(&input.bill_to))
        .bind(&input.notes)
        .bind(&input.heading)
        .bind(&input.reference)
        .bind(&input.metadata)
        .bind(input.payer_policy.mode().as_str())
        .bind(input.payer_policy.expected_email())
        .bind(
            input
                .payer_policy
                .expected_identity()
                .map(sqlx::types::Json),
        )
        .bind(input.chain_id as i64)
        .bind(input.factory_address)
        .bind(input.token_address)
        .bind(input.token_decimals as i16)
        .bind(input.beneficiary_address)
        .bind(input.expiration_timestamp as i64)
        .bind(input.expires_in_secs as i64)
        .bind(&input.expiration_intent)
        .bind(input.recovery_address)
        .bind(&input.amount)
        .bind(input.salt)
        .bind(input.payment_address)
        .bind(sqlx::types::Json(&input.issuance_snapshot))
        .bind(input.attribution_version as i16)
        .bind(input.attribution_nonce)
        .bind(input.attribution_hash)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(row) = inserted else {
            let existing = lock_by_idempotency_key(&mut tx, input)
                .await?
                .ok_or_else(|| {
                    sqlx::Error::Protocol("idempotency key vanished after conflict".into())
                })?;
            return replay(&mut tx, input, attachment_id, existing).await;
        };
        let attachment = match attachment_id {
            Some(attachment_id) => {
                Some(attach_in_transaction(&mut tx, input.account_id, attachment_id, row.id).await?)
            }
            None => None,
        };
        tx.commit().await?;
        Ok(InsertIssuedInvoice {
            row,
            attachment,
            replayed: false,
        })
    }

    /// Fetch an existing invoice by its idempotency key.
    pub async fn find_by_idempotency_key(
        &self,
        account: AccountId,
        key: &str,
    ) -> Result<Option<DbInvoice>, sqlx::Error> {
        sqlx::query_as::<_, DbInvoice>(
            r#"SELECT * FROM invoices WHERE account_id = $1 AND idempotency_key = $2"#,
        )
        .bind(account.0)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
    }

    /// Fetch an invoice by its ID.
    pub async fn find_by_id_for_account(
        &self,
        account: AccountId,
        id: Uuid,
    ) -> Result<Option<DbInvoice>, sqlx::Error> {
        sqlx::query_as::<_, DbInvoice>(
            r#"SELECT * FROM invoices WHERE account_id = $1 AND id = $2"#,
        )
        .bind(account.0)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    /// List an account's invoices, newest first, plus one row to signal another page.
    pub async fn list_for_account(
        &self,
        account: AccountId,
        status: Option<&str>,
        reference: Option<&str>,
        starting_after: Option<Uuid>,
        limit: u32,
    ) -> Result<Vec<DbInvoice>, sqlx::Error> {
        sqlx::query_as::<_, DbInvoice>(
            r#"SELECT candidate.* FROM invoices candidate
               LEFT JOIN invoices cursor ON cursor.account_id = $1 AND cursor.id = $4
               WHERE candidate.account_id = $1
                 AND ($2::text IS NULL OR
                    ($2 = 'awaiting_payment' AND candidate.status = 'created' AND candidate.confirmed_received = '0') OR
                    ($2 = 'partially_paid' AND candidate.status = 'created' AND candidate.confirmed_received <> '0') OR
                    ($2 = 'paid' AND candidate.status IN ('funded', 'deploying')) OR
                    ($2 = 'settled' AND candidate.status = 'fulfilled') OR
                    ($2 = 'expired' AND candidate.status = 'expired') OR
                    ($2 = 'returned' AND candidate.status = 'recovered') OR
                    ($2 = 'needs_attention' AND (candidate.status = 'blocked' OR candidate.blocked_reason IS NOT NULL)))
                 AND ($3::text IS NULL OR candidate.reference = $3)
                 AND ($4 IS NULL OR (candidate.created_at, candidate.id) < (cursor.created_at, cursor.id))
               ORDER BY candidate.created_at DESC, candidate.id DESC LIMIT $5"#,
        )
        .bind(account.0)
        .bind(status)
        .bind(reference)
        .bind(starting_after)
        .bind(i64::from(limit) + 1)
        .fetch_all(&self.pool)
        .await
    }

    /// Fetch response metadata in two bounded, account-scoped queries.
    pub async fn response_metadata_for_account(
        &self,
        account: AccountId,
        invoice_ids: &[Uuid],
    ) -> Result<(Vec<DbInvoiceTransfer>, Vec<DbIndexerFreshness>), sqlx::Error> {
        if invoice_ids.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let transfers = sqlx::query_as::<_, DbInvoiceTransfer>(
            r#"SELECT o.invoice_id, o.block_timestamp, o.amount, o.sender_address,
                      o.transaction_hash, o.block_number, o.disposition,
                      o.collected_at_block IS NOT NULL AS collected
               FROM payment_observations o
               JOIN invoices i ON i.id = o.invoice_id
               WHERE i.account_id = $1 AND i.id = ANY($2::uuid[])
               ORDER BY o.block_number, o.transaction_index, o.log_index"#,
        )
        .bind(account.0)
        .bind(invoice_ids)
        .fetch_all(&self.pool)
        .await?;
        let freshness = sqlx::query_as::<_, DbIndexerFreshness>(
            r#"SELECT DISTINCT c.chain_id, c.token_address, c.last_block, c.last_block_timestamp,
                      c.updated_at, s.finalized_block
               FROM indexer_cursor c
               LEFT JOIN indexer_status s ON s.chain_id = c.chain_id
                    AND s.token_address = c.token_address
               JOIN invoices i ON i.chain_id = c.chain_id AND i.token_address = c.token_address
               WHERE i.account_id = $1 AND i.id = ANY($2::uuid[])"#,
        )
        .bind(account.0)
        .bind(invoice_ids)
        .fetch_all(&self.pool)
        .await?;
        Ok((transfers, freshness))
    }

    pub async fn find_by_payment_address_for_account(
        &self,
        account: AccountId,
        address: &[u8],
    ) -> Result<Option<DbInvoice>, sqlx::Error> {
        sqlx::query_as::<_, DbInvoice>(
            "SELECT * FROM invoices WHERE account_id = $1 AND payment_address = $2",
        )
        .bind(account.0)
        .bind(address)
        .fetch_optional(&self.pool)
        .await
    }

    /// Record customer intent only. No lifecycle status or contract parameter
    /// is changed, and repeated requests are idempotent.
    pub async fn request_cancellation(
        &self,
        account: AccountId,
        id: Uuid,
    ) -> Result<Option<DbInvoice>, sqlx::Error> {
        sqlx::query_as::<_, DbInvoice>(
            r#"UPDATE invoices
               SET cancellation_requested_at = COALESCE(cancellation_requested_at, now()),
                   updated_at = CASE WHEN cancellation_requested_at IS NULL THEN now() ELSE updated_at END
               WHERE account_id = $1 AND id = $2
               RETURNING *"#,
        )
        .bind(account.0)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    /// Operational lookup used by the indexer, whose payment processing is
    /// intentionally account-agnostic. Customer API handlers must use
    /// `find_by_id_for_account` instead.
    pub async fn find_by_id(&self, id: Uuid) -> Result<Option<DbInvoice>, sqlx::Error> {
        sqlx::query_as::<_, DbInvoice>(r#"SELECT * FROM invoices WHERE id = $1"#)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn first_observed_block(&self, invoice_id: Uuid) -> Result<Option<u64>, sqlx::Error> {
        let block: Option<i64> = sqlx::query_scalar(
            "SELECT min(block_number) FROM payment_observations WHERE invoice_id=$1 AND disposition <> 'error'",
        )
        .bind(invoice_id)
        .fetch_one(self.pool())
        .await?;
        Ok(block.map(|value| value as u64))
    }

    /// Atomically apply one contiguous finalized log range and advance its
    /// canonical cursor.
    ///
    /// Every transfer to a known invoice address is retained. Transfers to an
    /// open invoice are `credited` toward its amount; transfers to any other
    /// status are `late` and queued for recovery; zero-value transfers are
    /// `error`. A transfer whose block precedes the invoice's last drain is
    /// recorded as already collected, so a lagging cursor never re-queues funds
    /// a finalized sweep already moved. Open invoices whose deadline lies
    /// before the range's end-block timestamp become `expired`. Replays are
    /// idempotent by transaction hash + log index.
    #[allow(clippy::too_many_arguments)]
    pub async fn apply_finalized_usdc_range(
        &self,
        chain_id: u64,
        token: Address,
        expected_cursor: Option<IndexerCursor>,
        end_block: u64,
        end_block_hash: B256,
        end_block_timestamp: u64,
        observations: &[PaymentObservation],
    ) -> Result<RangeOutcome, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        type StoredCursor = (Vec<u8>, i64, Vec<u8>, Option<i64>);
        let stored: Option<StoredCursor> = sqlx::query_as(
            r#"
            SELECT token_address, last_block, last_block_hash, last_block_timestamp
            FROM indexer_cursor
            WHERE chain_id = $1
            FOR UPDATE
            "#,
        )
        .bind(chain_id as i64)
        .fetch_optional(&mut *tx)
        .await?;

        let stored_cursor = stored
            .map(|(stored_token, block, hash, timestamp)| {
                if stored_token.as_slice() != token.as_slice() {
                    return Err(sqlx::Error::Protocol(
                        "indexer cursor belongs to a different token".into(),
                    ));
                }
                let block_hash = B256::try_from(hash.as_slice()).map_err(|_| {
                    sqlx::Error::Decode("invalid indexer cursor block hash length".into())
                })?;
                Ok(IndexerCursor {
                    block: block as u64,
                    block_hash,
                    block_timestamp: timestamp.map(|value| value as u64),
                })
            })
            .transpose()?;

        if stored_cursor != expected_cursor {
            return Err(sqlx::Error::Protocol(
                "indexer cursor changed while fetching logs".into(),
            ));
        }

        let recipients: Vec<Vec<u8>> = observations
            .iter()
            .map(|observation| observation.recipient.as_slice().to_vec())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        let rows = if recipients.is_empty() {
            Vec::new()
        } else {
            sqlx::query_as::<_, DbInvoice>(
                r#"
                SELECT * FROM invoices
                WHERE chain_id = $1
                  AND token_address = $2
                  AND payment_address = ANY($3::bytea[])
                FOR UPDATE
                "#,
            )
            .bind(chain_id as i64)
            .bind(token.as_slice())
            .bind(&recipients)
            .fetch_all(&mut *tx)
            .await?
        };

        struct InvoiceCredit {
            id: Uuid,
            status: String,
            expiration_timestamp: u64,
            required: U256,
            received: U256,
            drained_at: Option<(i64, i64)>,
            uncollected: i32,
            touched: bool,
            /// Chain time of the earliest new nonzero transfer in this range;
            /// if verification was still outstanding then, the funds are
            /// likely unsolicited (product plan §4.9).
            first_funding_timestamp: Option<u64>,
            crossing: Option<(U256, u64, B256, u64)>,
        }

        let mut credits = HashMap::<Vec<u8>, InvoiceCredit>::new();
        for row in rows {
            let required = U256::from_str_radix(&row.amount, 10).map_err(|_| {
                sqlx::Error::Decode(format!("invalid invoice amount for {}", row.id).into())
            })?;
            let received = U256::from_str_radix(&row.confirmed_received, 10).map_err(|_| {
                sqlx::Error::Decode(format!("invalid confirmed_received for {}", row.id).into())
            })?;
            credits.insert(
                row.payment_address,
                InvoiceCredit {
                    id: row.id,
                    status: row.status,
                    expiration_timestamp: row.expiration_timestamp as u64,
                    required,
                    received,
                    drained_at: row.drained_at_block.zip(row.drained_at_transaction_index),
                    uncollected: row.uncollected_count,
                    touched: false,
                    first_funding_timestamp: None,
                    crossing: None,
                },
            );
        }

        for observation in observations {
            let Some(credit) = credits.get_mut(observation.recipient.as_slice()) else {
                continue;
            };

            let zero = observation.amount.is_zero();
            let open = matches!(credit.status.as_str(), "created" | "funded");
            let accepting = open && observation.block_timestamp <= credit.expiration_timestamp;
            let (disposition, disposition_reason) = if zero {
                ("error", Some("zero_amount".to_string()))
            } else if accepting {
                ("credited", None)
            } else if open {
                ("late", Some("invoice_expired".to_string()))
            } else {
                ("late", Some(format!("invoice_{}", credit.status)))
            };
            let collected_at = (!zero).then_some(credit.drained_at).flatten().filter(
                |&(block, transaction_index)| {
                    (
                        observation.block_number as i64,
                        observation.transaction_index as i64,
                    ) < (block, transaction_index)
                },
            );

            let inserted = sqlx::query(
                r#"
                INSERT INTO payment_observations
                    (chain_id, token_address, block_number, block_hash, block_timestamp,
                     transaction_hash, transaction_index, log_index,
                     sender_address, recipient_address, invoice_id, amount,
                     disposition, disposition_reason, collected_at_block,
                     collected_at_transaction_index)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(chain_id as i64)
            .bind(token.as_slice())
            .bind(observation.block_number as i64)
            .bind(observation.block_hash.as_slice())
            .bind(observation.block_timestamp as i64)
            .bind(observation.transaction_hash.as_slice())
            .bind(observation.transaction_index as i64)
            .bind(observation.log_index as i64)
            .bind(observation.sender.as_slice())
            .bind(observation.recipient.as_slice())
            .bind(credit.id)
            .bind(observation.amount.to_string())
            .bind(disposition)
            .bind(disposition_reason)
            .bind(collected_at.map(|position| position.0))
            .bind(collected_at.map(|position| position.1))
            .execute(&mut *tx)
            .await?
            .rows_affected()
                > 0;

            if !inserted || zero {
                continue;
            }
            credit.touched = true;
            credit.first_funding_timestamp = Some(
                credit
                    .first_funding_timestamp
                    .map_or(observation.block_timestamp, |first| {
                        first.min(observation.block_timestamp)
                    }),
            );
            if collected_at.is_none() {
                credit.uncollected += 1;
            }
            if disposition != "credited" {
                continue;
            }

            credit.received = credit
                .received
                .checked_add(observation.amount)
                .ok_or_else(|| {
                    sqlx::Error::Decode("USDC observation total overflowed uint256".into())
                })?;
            if credit.status == "created"
                && credit.crossing.is_none()
                && credit.received >= credit.required
            {
                credit.crossing = Some((
                    credit.received,
                    observation.block_number,
                    observation.block_hash,
                    observation.block_timestamp,
                ));
            }
        }

        // Funds that arrive while a gated invoice's verification is still
        // outstanding are recorded like any other and flagged once, at the
        // chain time they first appeared. Nothing about the invoice changes
        // otherwise: same status transitions, same address, no quarantine.
        let mut outcome = RangeOutcome::default();
        for credit in credits.values().filter(|credit| credit.touched) {
            let first_funding = credit
                .first_funding_timestamp
                .map(|timestamp| timestamp as f64);
            if let Some((observed, block, block_hash, block_timestamp)) = credit.crossing {
                let result = sqlx::query(
                    r#"
                    UPDATE invoices
                    SET confirmed_received = $2,
                        uncollected_count = $3,
                        status = 'funded',
                        observed_amount = $4,
                        funded_at_block = $5,
                        funded_at_block_hash = $6,
                        paid_at = to_timestamp($7),
                        likely_unsolicited_at = CASE
                            WHEN payer_policy_mode <> $9
                             AND verification_completed_at IS NULL
                             AND likely_unsolicited_at IS NULL
                            THEN to_timestamp($8)
                            ELSE likely_unsolicited_at
                        END,
                        updated_at = now()
                    WHERE id = $1 AND status = 'created'
                    "#,
                )
                .bind(credit.id)
                .bind(credit.received.to_string())
                .bind(credit.uncollected)
                .bind(observed.to_string())
                .bind(block as i64)
                .bind(block_hash.as_slice())
                .bind(block_timestamp as f64)
                .bind(first_funding)
                .bind(PayerPolicyMode::Permissionless.as_str())
                .execute(&mut *tx)
                .await?;
                if result.rows_affected() > 0 {
                    outcome.funded.push(credit.id);
                }
            } else {
                sqlx::query(
                    r#"
                    UPDATE invoices
                    SET confirmed_received = $2,
                        uncollected_count = $3,
                        likely_unsolicited_at = CASE
                            WHEN payer_policy_mode <> $5
                             AND verification_completed_at IS NULL
                             AND likely_unsolicited_at IS NULL
                            THEN to_timestamp($4)
                            ELSE likely_unsolicited_at
                        END,
                        updated_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(credit.id)
                .bind(credit.received.to_string())
                .bind(credit.uncollected)
                .bind(first_funding)
                .bind(PayerPolicyMode::Permissionless.as_str())
                .execute(&mut *tx)
                .await?;
            }
        }

        // The contract routes to recovery once `block.timestamp > expiration`;
        // the range's end block is the latest finalized clock reading.
        outcome.expired = sqlx::query_scalar(
            r#"
            UPDATE invoices
            SET status = 'expired', expired_at = to_timestamp($3), updated_at = now()
            WHERE chain_id = $1
              AND token_address = $2
              AND status IN ('created', 'funded')
              AND expiration_timestamp < $3
            RETURNING id
            "#,
        )
        .bind(chain_id as i64)
        .bind(token.as_slice())
        .bind(end_block_timestamp as i64)
        .fetch_all(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO indexer_cursor
                (chain_id, token_address, last_block, last_block_hash, last_block_timestamp, updated_at)
            VALUES ($1, $2, $3, $4, $5, now())
            ON CONFLICT (chain_id) DO UPDATE
            SET token_address = EXCLUDED.token_address,
                last_block = EXCLUDED.last_block,
                last_block_hash = EXCLUDED.last_block_hash,
                last_block_timestamp = EXCLUDED.last_block_timestamp,
                updated_at = now()
            "#,
        )
        .bind(chain_id as i64)
        .bind(token.as_slice())
        .bind(end_block as i64)
        .bind(end_block_hash.as_slice())
        .bind(end_block_timestamp as i64)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(outcome)
    }
}

async fn lock_by_idempotency_key(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: &CreateInvoiceInput,
) -> Result<Option<DbInvoice>, sqlx::Error> {
    sqlx::query_as::<_, DbInvoice>(
        "SELECT * FROM invoices WHERE account_id = $1 AND idempotency_key = $2 FOR UPDATE",
    )
    .bind(input.account_id.0)
    .bind(&input.idempotency_key)
    .fetch_optional(&mut **tx)
    .await
}

/// Resolve a request against the row already issued under its key.
async fn replay(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: &CreateInvoiceInput,
    attachment_id: Option<Uuid>,
    existing: DbInvoice,
) -> Result<InsertIssuedInvoice, InsertIssuedInvoiceError> {
    let attachment = attachments::find_by_invoice_with(&mut **tx, existing.id).await?;
    if !input.same_issuance(&existing, attachment.as_ref(), attachment_id) {
        return Err(InsertIssuedInvoiceError::IdempotencyConflict);
    }
    Ok(InsertIssuedInvoice {
        row: existing,
        attachment,
        replayed: true,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use alloy_primitives::address;
    use gateway_core::{AttachmentCommitment, ExpectedIdentity, PayerPolicyMode};
    use sqlx::types::chrono::{DateTime, Utc};

    fn epoch() -> DateTime<Utc> {
        DateTime::from_timestamp(0, 0).unwrap()
    }

    pub(crate) fn party(name: &str) -> Party {
        Party {
            name: name.into(),
            email: None,
            details: None,
        }
    }

    /// A minimal permissionless snapshot for the given terms.
    fn snapshot() -> CanonicalIssuanceSnapshot {
        CanonicalIssuanceSnapshot::new(
            party("Acme"),
            party("Globex"),
            PayerPolicy::Permissionless,
            FactoryAddress(Address::repeat_byte(1)),
            ChainId(1),
            TokenAddress(Address::repeat_byte(2)),
            BeneficiaryAddress(Address::repeat_byte(3)),
            Amount(U256::from(100)),
            1_900_000_000,
            RecoveryAddress(Address::repeat_byte(6)),
        )
    }

    pub(crate) async fn account(pool: &PgPool, id: u128) -> AccountId {
        let id = Uuid::from_u128(id);
        sqlx::query(
            "INSERT INTO accounts (id, api_key_hash, api_key_hint) VALUES ($1, $2, 'hint') ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(id.as_bytes().repeat(2))
        .execute(pool)
        .await
        .unwrap();
        AccountId(id)
    }

    /// Issue a fresh domain invoice (new id, nonce, salt, and address every
    /// call, as a retried request would produce) and project it for the DB.
    pub(crate) fn issuance_input(
        owner: AccountId,
        key: &str,
        attachment: Option<&DbAttachment>,
    ) -> CreateInvoiceInput {
        let factory = FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3"));
        let token = TokenAddress(address!("0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512"));
        let beneficiary =
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"));
        let recovery = RecoveryAddress(address!("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"));
        let amount = Amount(U256::from(1_000_000));
        let mut snapshot = CanonicalIssuanceSnapshot::new(
            party("Acme"),
            party("Globex"),
            PayerPolicy::Permissionless,
            factory,
            ChainId(1),
            token,
            beneficiary,
            amount,
            4_000_000_000,
            recovery,
        );
        snapshot.attachment = attachment.and_then(DbAttachment::commitment);
        let invoice = Invoice::issue(
            factory,
            ChainId(1),
            token,
            beneficiary,
            amount,
            4_000_000_000,
            recovery,
            snapshot,
        )
        .unwrap();
        CreateInvoiceInput::from_invoice(&invoice, owner, key.into(), 6, 3_600, "in:3600".into())
    }

    /// A DB row that decodes cleanly; individual tests corrupt one field.
    fn valid_row() -> DbInvoice {
        DbInvoice {
            id: Uuid::now_v7(),
            account_id: Uuid::from_u128(1),
            idempotency_key: "key".to_string(),
            chain_id: 1,
            factory_address: vec![1u8; 20],
            token_address: vec![2u8; 20],
            token_decimals: 18,
            beneficiary_address: vec![3u8; 20],
            expiration_timestamp: 1_900_000_000,
            expires_in_secs: 3_600,
            expiration_intent: "at:1900000000".into(),
            recovery_address: vec![6u8; 20],
            amount: "100".to_string(),
            salt: vec![4u8; 32],
            payment_address: vec![5u8; 20],
            status: "created".to_string(),
            created_at: epoch(),
            updated_at: epoch(),
            funded_at_block: None,
            funded_at_block_hash: None,
            observed_amount: None,
            confirmed_received: "0".to_string(),
            sweep_batch_id: None,
            drained_at_block: None,
            drained_at_transaction_index: None,
            uncollected_count: 0,
            execute_tx_hash: None,
            resolved_at_block: None,
            settled_at: None,
            sweep_attempts: 0,
            last_attempt_at: None,
            blocked_reason: None,
            cancellation_requested_at: None,
            reference: None,
            metadata: sqlx::types::Json(serde_json::json!({})),
            paid_at: None,
            expired_at: None,
            settlement_tx_hash: None,
            fee_amount: "0".into(),
            net_amount: "100".into(),
            customer_id: None,
            issuer: Some(sqlx::types::Json(party("Acme"))),
            bill_to: Some(sqlx::types::Json(party("Globex"))),
            notes: None,
            heading: None,
            payer_policy_mode: "permissionless".into(),
            expected_email: None,
            expected_identity: None,
            verification_completed_at: None,
            issuance_snapshot: Some(sqlx::types::Json(snapshot())),
            attribution_version: Some(1),
            attribution_nonce: Some(vec![7u8; 32]),
            attribution_hash: Some(vec![8u8; 32]),
            likely_unsolicited_at: None,
        }
    }

    #[test]
    fn valid_row_decodes() {
        let invoice = Invoice::try_from(&valid_row()).expect("valid row must decode");
        assert_eq!(invoice.chain_id.0, 1);
        assert_eq!(invoice.amount.0.to_string(), "100");
        assert_eq!(invoice.expiration_timestamp, 1_900_000_000);
        assert_eq!(invoice.recovery.0, Address::repeat_byte(6));
        assert_eq!(invoice.received.0, U256::ZERO);
        assert_eq!(invoice.execute_tx_hash, None);
        assert_eq!(invoice.attribution_version, 1);
        assert_eq!(invoice.attribution_nonce, B256::repeat_byte(7));
        assert_eq!(invoice.attribution_hash, B256::repeat_byte(8));
        assert_eq!(invoice.issuance_snapshot.issuer.name, "Acme");
    }

    #[test]
    fn settlement_columns_decode() {
        let mut row = valid_row();
        row.status = "fulfilled".to_string();
        row.confirmed_received = "150".to_string();
        row.execute_tx_hash = Some(vec![0xAB; 32]);
        row.resolved_at_block = Some(7);
        row.blocked_reason = Some("destination_blacklisted".to_string());
        let invoice = Invoice::try_from(&row).unwrap();
        assert_eq!(invoice.received.0, U256::from(150));
        assert_eq!(invoice.execute_tx_hash, Some(B256::repeat_byte(0xAB)));
        assert_eq!(invoice.resolved_at_block, Some(7));
        assert_eq!(
            invoice.blocked_reason.as_deref(),
            Some("destination_blacklisted")
        );
    }

    #[test]
    fn bad_amount_is_typed_error_not_panic() {
        let mut row = valid_row();
        row.amount = "not-a-number".to_string();
        assert!(matches!(
            Invoice::try_from(&row),
            Err(DbInvoiceError::InvalidAmount { .. })
        ));
    }

    #[test]
    fn unknown_status_is_typed_error_not_panic() {
        let mut row = valid_row();
        row.status = "failed".to_string();
        assert!(matches!(
            Invoice::try_from(&row),
            Err(DbInvoiceError::InvalidStatus { .. })
        ));
    }

    #[test]
    fn wrong_length_columns_are_typed_errors_not_panics() {
        let mut row = valid_row();
        row.token_address = vec![2u8; 19]; // one byte short
        assert!(matches!(
            Invoice::try_from(&row),
            Err(DbInvoiceError::WrongByteLength {
                field: "token_address",
                ..
            })
        ));

        let mut row = valid_row();
        row.salt = vec![4u8; 31];
        assert!(matches!(
            Invoice::try_from(&row),
            Err(DbInvoiceError::WrongByteLength { field: "salt", .. })
        ));

        let mut row = valid_row();
        row.execute_tx_hash = Some(vec![0xAB; 31]);
        assert!(matches!(
            Invoice::try_from(&row),
            Err(DbInvoiceError::WrongByteLength {
                field: "execute_tx_hash",
                ..
            })
        ));

        let mut row = valid_row();
        row.attribution_nonce = Some(vec![7u8; 16]);
        assert!(matches!(
            Invoice::try_from(&row),
            Err(DbInvoiceError::WrongByteLength {
                field: "attribution_nonce",
                ..
            })
        ));
    }

    #[test]
    fn legacy_rows_without_attribution_material_remain_readable() {
        let mut row = valid_row();
        row.issuance_snapshot = None;
        row.attribution_version = None;
        row.attribution_nonce = None;
        row.attribution_hash = None;
        row.issuer = None;
        row.bill_to = None;
        let invoice = Invoice::try_from(&row).expect("legacy rows must not halt readers");
        assert_eq!(invoice.attribution_version, 0);
        assert_eq!(invoice.attribution_nonce, B256::ZERO);
        assert_eq!(invoice.attribution_hash, B256::ZERO);
        assert_eq!(invoice.issuance_snapshot.issuer.name, "Legacy issuer");
    }

    #[test]
    fn partially_missing_attribution_is_still_rejected() {
        let mut row = valid_row();
        row.attribution_hash = None;
        assert!(matches!(
            Invoice::try_from(&row),
            Err(DbInvoiceError::MissingAttribution {
                field: "attribution_hash",
                ..
            })
        ));
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn customer_queries_are_scoped_and_cancellation_is_advisory(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let other = account(&pool, 2).await;
        let repo = InvoiceRepository::new(pool);
        let first = repo
            .insert_issued(&issuance_input(owner, "one", None), None)
            .await
            .unwrap()
            .row;
        let mut second_input = issuance_input(owner, "two", None);
        second_input.notes = Some("customer reference".into());
        second_input.reference = Some("order-2".into());
        let second = repo.insert_issued(&second_input, None).await.unwrap().row;
        repo.insert_issued(&issuance_input(other, "other", None), None)
            .await
            .unwrap();

        assert_eq!(
            repo.list_for_account(owner, None, None, None, 20)
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            repo.list_for_account(owner, Some("awaiting_payment"), None, None, 1)
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            repo.list_for_account(owner, None, Some("order-2"), None, 20)
                .await
                .unwrap()
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            [second.id]
        );
        assert_eq!(
            repo.find_by_id_for_account(owner, second.id)
                .await
                .unwrap()
                .unwrap()
                .notes
                .as_deref(),
            Some("customer reference")
        );
        assert_eq!(
            repo.find_by_payment_address_for_account(owner, &first.payment_address)
                .await
                .unwrap()
                .unwrap()
                .id,
            first.id
        );
        assert!(
            repo.find_by_payment_address_for_account(other, &first.payment_address)
                .await
                .unwrap()
                .is_none()
        );

        let cancelled = repo
            .request_cancellation(owner, first.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cancelled.status, "created");
        let requested_at = cancelled.cancellation_requested_at.unwrap();
        let repeated = repo
            .request_cancellation(owner, first.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(repeated.cancellation_requested_at.unwrap(), requested_at);
        assert!(
            repo.request_cancellation(other, first.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn insert_issued_replays_identical_requests_and_rejects_different_ones(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let repo = InvoiceRepository::new(pool.clone());
        let customer = Uuid::now_v7();
        sqlx::query("INSERT INTO customers (id, account_id, name) VALUES ($1, $2, 'Globex')")
            .bind(customer)
            .bind(owner.0)
            .execute(&pool)
            .await
            .unwrap();
        let attachments = crate::AttachmentRepository::new(pool.clone());
        let document = crate::attachments::tests::ready(&attachments, owner, "contract.pdf").await;

        let mut original = issuance_input(owner, "replay", Some(&document));
        original.customer_id = Some(customer);
        original.notes = Some("Net 30".into());
        original.heading = Some("March retainer".into());
        original.reference = Some("INV-1".into());
        original.metadata = serde_json::json!({"po": "42"});
        original.payer_policy = PayerPolicy::VerifiedIdentity {
            expected_email: "alice@example.com".into(),
            expected_identity: ExpectedIdentity {
                first_name: "Alice".into(),
                last_name: "Smith".into(),
            },
        };
        let issued = repo
            .insert_issued(&original, Some(document.id))
            .await
            .unwrap();
        assert!(!issued.replayed);
        assert_eq!(issued.row.customer_id, Some(customer));
        assert_eq!(issued.row.payer_policy_mode, "verified_identity");
        assert_eq!(
            issued.row.expected_email.as_deref(),
            Some("alice@example.com")
        );
        assert_eq!(
            issued.row.expected_identity.as_ref().unwrap().0.last_name,
            "Smith"
        );
        assert_eq!(issued.row.attribution_version, Some(1));
        assert_eq!(
            issued.row.attribution_hash.as_deref(),
            Some(original.attribution_hash.as_slice())
        );

        // A retry re-derives id, nonce, salt, and address; only the merchant's
        // request is compared, so the original row comes back.
        let mut retry = issuance_input(owner, "replay", Some(&document));
        retry.customer_id = original.customer_id;
        retry.notes = original.notes.clone();
        retry.heading = original.heading.clone();
        retry.reference = original.reference.clone();
        retry.metadata = original.metadata.clone();
        retry.payer_policy = original.payer_policy.clone();
        assert_ne!(retry.salt, original.salt);
        let replayed = repo.insert_issued(&retry, Some(document.id)).await.unwrap();
        assert!(replayed.replayed);
        assert_eq!(replayed.row.id, issued.row.id);
        assert_eq!(replayed.row.salt, issued.row.salt);
        assert_eq!(replayed.attachment.unwrap().id, document.id);

        type Conflict = (
            &'static str,
            Box<dyn Fn(&mut CreateInvoiceInput)>,
            Option<Uuid>,
        );
        let conflicts: Vec<Conflict> = vec![
            (
                "notes",
                Box::new(|i| i.notes = Some("Net 60".into())),
                Some(document.id),
            ),
            ("heading", Box::new(|i| i.heading = None), Some(document.id)),
            (
                "reference",
                Box::new(|i| i.reference = Some("INV-2".into())),
                Some(document.id),
            ),
            (
                "metadata",
                Box::new(|i| i.metadata = serde_json::json!({"po": "43"})),
                Some(document.id),
            ),
            (
                "customer",
                Box::new(|i| i.customer_id = None),
                Some(document.id),
            ),
            (
                "issuer",
                Box::new(|i| i.issuer.email = Some("ap@acme.example".into())),
                Some(document.id),
            ),
            (
                "bill_to",
                Box::new(|i| i.bill_to = party("Initech")),
                Some(document.id),
            ),
            (
                "policy mode",
                Box::new(|i| {
                    i.payer_policy = PayerPolicy::VerifiedEmail {
                        expected_email: "alice@example.com".into(),
                    }
                }),
                Some(document.id),
            ),
            (
                "expected identity",
                Box::new(|i| {
                    i.payer_policy = PayerPolicy::VerifiedIdentity {
                        expected_email: "alice@example.com".into(),
                        expected_identity: ExpectedIdentity {
                            first_name: "Alicia".into(),
                            last_name: "Smith".into(),
                        },
                    }
                }),
                Some(document.id),
            ),
            (
                "amount",
                Box::new(|i| i.amount = "1000001".into()),
                Some(document.id),
            ),
            (
                "expiration intent",
                Box::new(|i| i.expiration_intent = "in:7200".into()),
                Some(document.id),
            ),
            ("attachment", Box::new(|_| {}), None),
        ];
        for (name, mutate, attachment) in conflicts {
            let mut conflicting = issuance_input(owner, "replay", Some(&document));
            conflicting.customer_id = original.customer_id;
            conflicting.notes = original.notes.clone();
            conflicting.heading = original.heading.clone();
            conflicting.reference = original.reference.clone();
            conflicting.metadata = original.metadata.clone();
            conflicting.payer_policy = original.payer_policy.clone();
            mutate(&mut conflicting);
            let error = repo
                .insert_issued(&conflicting, attachment)
                .await
                .unwrap_err();
            assert!(
                matches!(error, InsertIssuedInvoiceError::IdempotencyConflict),
                "{name}: {error}"
            );
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invoices")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "conflicts issue nothing");
        assert_eq!(
            issued
                .row
                .payer_policy_mode
                .parse::<PayerPolicyMode>()
                .unwrap(),
            PayerPolicyMode::VerifiedIdentity
        );
        assert_eq!(
            issued
                .row
                .issuance_snapshot
                .unwrap()
                .0
                .attachment
                .map(|commitment: AttachmentCommitment| commitment.id),
            Some(document.id)
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn issuance_fields_are_immutable_while_lifecycle_columns_stay_writable(pool: PgPool) {
        let owner = account(&pool, 1).await;
        // A customer in the same account, so the only thing refusing the link
        // change is the immutability trigger, not the foreign key.
        let customer = Uuid::now_v7();
        sqlx::query("INSERT INTO customers (id, account_id, name) VALUES ($1, $2, 'Acme')")
            .bind(customer)
            .bind(owner.0)
            .execute(&pool)
            .await
            .unwrap();
        let repo = InvoiceRepository::new(pool.clone());
        let mut input = issuance_input(owner, "immutable", None);
        input.notes = Some("as issued".into());
        let id = repo.insert_issued(&input, None).await.unwrap().row.id;

        // The notes are part of the hashed document, the customer is the
        // party link, and the rest commit the payment address.
        let immutable = [
            "notes = 'edited'".to_string(),
            format!("customer_id = '{customer}'"),
            "amount = '2000000'".to_string(),
            r"beneficiary_address = '\x0909090909090909090909090909090909090909'".to_string(),
            "payer_policy_mode = 'verified_email', expected_email = 'a@b.co'".to_string(),
            r"attribution_nonce = '\x0909090909090909090909090909090909090909090909090909090909090909'".to_string(),
        ];
        // Test-owned literals only, so the assembled statements are safe.
        let update = |assignment: &str| {
            sqlx::AssertSqlSafe(format!("UPDATE invoices SET {assignment} WHERE id = $1"))
        };
        for assignment in &immutable {
            let error = sqlx::query(update(assignment))
                .bind(id)
                .execute(&pool)
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains("issuance fields are immutable"),
                "{assignment}: {error}"
            );
        }
        for assignment in [
            "status = 'funded'",
            "confirmed_received = '1000000'",
            "blocked_reason = 'retries_exhausted'",
            "verification_completed_at = now()",
        ] {
            sqlx::query(update(assignment))
                .bind(id)
                .execute(&pool)
                .await
                .unwrap_or_else(|error| panic!("{assignment}: {error}"));
        }
        let row = repo.find_by_id(id).await.unwrap().unwrap();
        assert_eq!(row.notes.as_deref(), Some("as issued"));
        assert_eq!(row.amount, "1000000");
        assert_eq!(row.beneficiary_address, input.beneficiary_address);
        assert_eq!(row.status, "funded");
        assert_eq!(row.confirmed_received, "1000000");
        assert_eq!(row.blocked_reason.as_deref(), Some("retries_exhausted"));
        assert!(row.verification_completed_at.is_some());
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn webhook_payloads_expose_only_the_allowlisted_payment_fields(pool: PgPool) {
        use std::collections::BTreeSet;

        let account = Uuid::from_u128(1);
        sqlx::query(
            "INSERT INTO accounts (id, api_key_hash, api_key_hint) VALUES ($1, $2, 'hint')",
        )
        .bind(account)
        .bind(account.as_bytes().repeat(2))
        .execute(&pool)
        .await
        .unwrap();
        // Inserted directly: the policy columns are issuance fields, so no
        // repository write path can set them once the row exists.
        let id = Uuid::now_v7();
        sqlx::query(
            r#"INSERT INTO invoices
                 (id, account_id, idempotency_key, chain_id, factory_address, token_address,
                  token_decimals, beneficiary_address, expiration_timestamp, expires_in_secs,
                  expiration_intent, recovery_address, amount, net_amount, salt, payment_address,
                  status, reference, metadata, payer_policy_mode, expected_email, expected_identity)
               VALUES ($1, $2, 'allowlist', 1, $3, $3, 6, $3, 4000000000, 3600, 'at:4000000000',
                  $3, '1000000', '1000000', $4, $3, 'created', 'order-7', '{"source":"checkout"}',
                  'verified_identity', 'alice@example.com',
                  '{"first_name":"Alice","last_name":"Smith"}')"#,
        )
        .bind(id)
        .bind(account)
        .bind([1u8; 20].as_slice())
        .bind([2u8; 32].as_slice())
        .execute(&pool)
        .await
        .unwrap();

        // Every transition that enqueues an invoice event, in lifecycle order.
        for statement in [
            "UPDATE invoices SET status = 'funded', paid_at = now() WHERE id = $1",
            "UPDATE invoices SET verification_completed_at = now() WHERE id = $1",
            "UPDATE invoices SET likely_unsolicited_at = now() WHERE id = $1",
            "UPDATE invoices SET status = 'fulfilled', settled_at = now() WHERE id = $1",
            "UPDATE invoices SET blocked_reason = 'recovery_blacklisted' WHERE id = $1",
        ] {
            sqlx::query(statement)
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query(
            r#"INSERT INTO recovered_funds
                 (id, invoice_id, chain_id, token_address, transaction_hash, block_number,
                  amount, reason, recovered_at)
               VALUES ($1, $2, 1, $3, $4, 7, '25', 'overpayment', now())"#,
        )
        .bind(Uuid::now_v7())
        .bind(id)
        .bind([1u8; 20].as_slice())
        .bind([0xA1u8; 32].as_slice())
        .execute(&pool)
        .await
        .unwrap();

        let events: Vec<(String, serde_json::Value)> = sqlx::query_as(
            "SELECT event_type, payload FROM webhook_events WHERE invoice_id = $1 ORDER BY event_type",
        )
        .bind(id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            events
                .iter()
                .map(|(kind, _)| kind.as_str())
                .collect::<Vec<_>>(),
            [
                "payment.likely_unsolicited",
                "payment.needs_attention",
                "payment.paid",
                "payment.recovered_funds",
                "payment.settled",
                "verification.approved",
            ]
        );
        let allowlist: BTreeSet<&str> = [
            "id",
            "status",
            "amount",
            "received",
            "reference",
            "metadata",
            "payer_policy_mode",
            "verification_completed_at",
            "likely_unsolicited_at",
        ]
        .into_iter()
        .collect();
        for (kind, payload) in &events {
            let serialized = payload.to_string();
            for secret in ["alice@example.com", "Alice", "Smith"] {
                assert!(
                    !serialized.contains(secret),
                    "{kind} carries {secret}: {serialized}"
                );
            }
            let payment = payload["data"]["payment"]
                .as_object()
                .unwrap_or_else(|| panic!("{kind} has no payment object: {serialized}"));
            let mut keys: BTreeSet<&str> = payment.keys().map(String::as_str).collect();
            assert_eq!(
                keys.remove("attention"),
                kind == "payment.needs_attention",
                "{kind}: only the attention event carries the attention object"
            );
            assert_eq!(keys, allowlist, "{kind}");
        }
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn attention_transition_notifies_once_and_release_is_guarded(pool: PgPool) {
        let account = AccountId(Uuid::now_v7());
        sqlx::query("INSERT INTO accounts(id,api_key_hash,api_key_hint,email) VALUES($1,$2,'hint','merchant@example.com')")
            .bind(account.0).bind(account.0.as_bytes().repeat(2)).execute(&pool).await.unwrap();
        let repo = InvoiceRepository::new(pool.clone());
        let mut input = issuance_input(account, "attention", None);
        input.reference = Some("order-42".into());
        input.metadata = serde_json::json!({"source":"checkout"});
        let id = repo.insert_issued(&input, None).await.unwrap().row.id;
        sqlx::query("UPDATE invoices SET status='deploying' WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();

        assert!(repo.block_invoice(id, "retries_exhausted").await.unwrap());
        assert!(!repo.block_invoice(id, "retries_exhausted").await.unwrap());
        let (email_count, webhook_count): (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM notification_outbox WHERE invoice_id=$1), (SELECT count(*) FROM webhook_events WHERE invoice_id=$1 AND event_type='payment.needs_attention')",
        ).bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!((email_count, webhook_count), (1, 1));
        let payload: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM webhook_events WHERE invoice_id=$1 AND event_type='payment.needs_attention'",
        ).bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(
            payload["data"]["payment"]["attention"]["reason_code"],
            "retries_exhausted"
        );
        assert!(
            payload["data"]["payment"]["attention"]["message"]
                .as_str()
                .unwrap()
                .contains("Funds remain safe")
        );

        let released = repo.release_blocked(id).await.unwrap();
        assert_eq!(released.status, "deploying");
        assert!(released.blocked_reason.is_none());
        assert!(matches!(
            repo.release_blocked(id).await,
            Err(ReleasePaymentError::NotBlocked)
        ));

        sqlx::query("DELETE FROM webhook_events WHERE invoice_id=$1 AND event_type='payment.needs_attention'")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE invoices SET status='fulfilled' WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE notification_outbox SET delivered_at=now() WHERE invoice_id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE accounts SET email=NULL WHERE id=$1")
            .bind(account.0)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            repo.block_invoice(id, "recovery_blacklisted")
                .await
                .unwrap()
        );
        let terminal = repo.release_blocked(id).await.unwrap();
        assert_eq!(terminal.status, "fulfilled");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM webhook_events WHERE invoice_id=$1 AND event_type='payment.needs_attention'")
                .bind(id).fetch_one(&pool).await.unwrap(),
            1
        );
        let notifications = crate::NotificationRepository::new(pool.clone());
        let missing = notifications.claim().await.unwrap().unwrap();
        assert!(missing.email.is_none());
        assert!(
            notifications
                .report_missing_email(missing.id)
                .await
                .unwrap()
        );
        assert!(
            !notifications
                .report_missing_email(missing.id)
                .await
                .unwrap()
        );
        assert!(notifications.claim().await.unwrap().is_none());

        assert!(matches!(
            repo.release_blocked(Uuid::now_v7()).await,
            Err(ReleasePaymentError::NotFound)
        ));
    }

    fn transfer(recipient: Address, amount: u64, block: u64, timestamp: u64) -> PaymentObservation {
        PaymentObservation {
            block_number: block,
            block_hash: B256::repeat_byte(block as u8),
            block_timestamp: timestamp,
            transaction_hash: B256::repeat_byte(0x80 + block as u8),
            transaction_index: 0,
            log_index: 0,
            sender: Address::repeat_byte(0x99),
            recipient,
            amount: U256::from(amount),
        }
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn funding_before_verification_is_flagged_once_and_never_quarantined(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let repo = InvoiceRepository::new(pool.clone());
        let gated = PayerPolicy::VerifiedEmail {
            expected_email: "alice@example.com".into(),
        };
        let mut unverified = issuance_input(owner, "unverified", None);
        unverified.issuance_snapshot.payer_policy = gated.clone();
        unverified.payer_policy = gated.clone();
        let mut verified = issuance_input(owner, "verified", None);
        verified.issuance_snapshot.payer_policy = gated.clone();
        verified.payer_policy = gated;
        let open = issuance_input(owner, "open", None);
        let unverified = repo.insert_issued(&unverified, None).await.unwrap().row;
        let verified = repo.insert_issued(&verified, None).await.unwrap().row;
        let open = repo.insert_issued(&open, None).await.unwrap().row;
        sqlx::query("UPDATE invoices SET verification_completed_at = now() WHERE id = $1")
            .bind(verified.id)
            .execute(&pool)
            .await
            .unwrap();
        let address = |row: &DbInvoice| Address::from_slice(&row.payment_address);
        let token = Address::from_slice(&unverified.token_address);

        // A partial transfer to each, all before the deadline.
        let outcome = repo
            .apply_finalized_usdc_range(
                1,
                token,
                None,
                10,
                B256::repeat_byte(10),
                1_000,
                &[
                    transfer(address(&unverified), 400_000, 10, 1_000),
                    transfer(address(&verified), 400_000, 10, 1_000),
                    transfer(address(&open), 400_000, 10, 1_000),
                ],
            )
            .await
            .unwrap();
        assert!(outcome.funded.is_empty());
        let flagged = repo.find_by_id(unverified.id).await.unwrap().unwrap();
        assert_eq!(
            flagged.likely_unsolicited_at.map(|at| at.timestamp()),
            Some(1_000)
        );
        assert_eq!(flagged.status, "created");
        assert_eq!(flagged.confirmed_received, "400000");
        assert_eq!(flagged.payment_address, unverified.payment_address);
        for row in [&verified, &open] {
            let row = repo.find_by_id(row.id).await.unwrap().unwrap();
            assert!(row.likely_unsolicited_at.is_none());
        }

        // Completing the amount later keeps the first timestamp and funds the
        // invoice like any other; the address is unchanged and nothing is set
        // aside.
        let cursor = Some(IndexerCursor {
            block: 10,
            block_hash: B256::repeat_byte(10),
            block_timestamp: Some(1_000),
        });
        let outcome = repo
            .apply_finalized_usdc_range(
                1,
                token,
                cursor,
                11,
                B256::repeat_byte(11),
                1_100,
                &[transfer(address(&unverified), 600_000, 11, 1_100)],
            )
            .await
            .unwrap();
        assert_eq!(outcome.funded, [unverified.id]);
        let funded = repo.find_by_id(unverified.id).await.unwrap().unwrap();
        assert_eq!(funded.status, "funded");
        assert_eq!(
            funded.likely_unsolicited_at.map(|at| at.timestamp()),
            Some(1_000)
        );
        assert_eq!(funded.payment_address, unverified.payment_address);
        assert_eq!(funded.uncollected_count, 2);
        let statuses: Vec<String> = sqlx::query_scalar("SELECT DISTINCT status FROM invoices")
            .fetch_all(&pool)
            .await
            .unwrap();
        for status in statuses {
            assert!(
                status.parse::<gateway_core::InvoiceStatus>().is_ok(),
                "unexpected status {status}"
            );
        }
    }
}
