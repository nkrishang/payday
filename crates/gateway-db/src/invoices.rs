//! Database row type and repository for invoices.
//!
//! All conversions between domain types and DB column types happen here.
//! Handlers and application logic never see SQL or DB rows.

use std::collections::{HashMap, HashSet};

use alloy_primitives::{Address, B256, U256};
use sqlx::PgPool;
use thiserror::Error;
use uuid::Uuid;

use crate::cursor::IndexerCursor;
use gateway_core::{
    Amount, BeneficiaryAddress, ChainId, FactoryAddress, Invoice, InvoiceId,
    InvoiceStatusParseError, PaymentAddress, RecoveryAddress, Salt, TokenAddress,
};

/// Database row representing one invoice.
#[allow(dead_code)] // some fields not used in responses yet
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DbInvoice {
    pub id: Uuid,
    pub idempotency_key: String,
    pub chain_id: i64,
    pub factory_address: Vec<u8>,
    pub token_address: Vec<u8>,
    pub token_decimals: i16,
    pub beneficiary_address: Vec<u8>,
    pub expiration_timestamp: i64,
    pub recovery_address: Vec<u8>,
    pub amount: String,
    pub salt: Vec<u8>,
    pub payment_address: Vec<u8>,
    pub status: String,
    pub created_at: sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,
    pub updated_at: sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,
    /// Block height at which the balance was first observed to be sufficient.
    /// Internal indexer bookkeeping; NULL until the invoice is funded.
    pub funded_at_block: Option<i64>,
    /// Canonical base-unit balance observed at funding time (decimal string).
    /// Internal indexer bookkeeping; NULL until the invoice is funded.
    pub observed_amount: Option<String>,
    /// Sum of finalized USDC Transfer observations attributed to this invoice.
    pub confirmed_received: String,
    /// Canonical hash paired with `funded_at_block`.
    pub funded_at_block_hash: Option<Vec<u8>>,
    /// Time at which `execute_tx_hash` was submitted. NULL when no transaction
    /// from this worker is currently pending or known-mined.
    pub sweep_submitted_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    /// Block where deployed Payment code was first observed. A deploying invoice
    /// remains non-terminal until this block reaches the finality boundary.
    pub sweep_detected_at_block: Option<i64>,
    /// Canonical hash paired with `sweep_detected_at_block`.
    pub sweep_detected_at_block_hash: Option<Vec<u8>>,
    /// Hash of an `execute` transaction submitted by this worker (32 bytes).
    /// NULL when execution was discovered from code without our transaction.
    pub execute_tx_hash: Option<Vec<u8>>,
    /// Block height at which the sweep was confirmed. NULL until fulfilled.
    pub fulfilled_at_block: Option<i64>,
    /// Count of transient (non-contract) sweep failures so far. Drives the
    /// exponential backoff and the retry ceiling.
    pub sweep_attempts: i32,
    /// Timestamp of the most recent sweep attempt; with `sweep_attempts` it
    /// gates when the next retry becomes eligible. NULL before the first attempt.
    pub last_attempt_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    /// Why the invoice was blocked (`token_transfer_failed`).
    /// NULL unless status is `blocked`.
    pub blocked_reason: Option<String>,
}

/// A stored invoice row could not be decoded into the domain model. This
/// signals corrupt or out-of-contract data in the database, not client error.
#[derive(Debug, Error)]
pub enum DbInvoiceError {
    #[error("invalid amount in DB row {id}: {value:?}")]
    InvalidAmount { id: Uuid, value: String },
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

impl TryFrom<&DbInvoice> for Invoice {
    type Error = DbInvoiceError;

    /// Convert a DB row to the domain model. Fails with a typed error (rather
    /// than panicking) when a column holds data outside the contract enforced
    /// by the schema's CHECK constraints.
    fn try_from(row: &DbInvoice) -> Result<Self, Self::Error> {
        let amount =
            U256::from_str_radix(&row.amount, 10).map_err(|_| DbInvoiceError::InvalidAmount {
                id: row.id,
                value: row.amount.clone(),
            })?;

        let salt =
            B256::try_from(row.salt.as_slice()).map_err(|_| DbInvoiceError::WrongByteLength {
                id: row.id,
                field: "salt",
                expected: 32,
                got: row.salt.len(),
            })?;

        let status = row
            .status
            .parse()
            .map_err(|source| DbInvoiceError::InvalidStatus { id: row.id, source })?;

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
            amount: Amount(amount),
            salt: Salt(salt),
            payment_address: PaymentAddress(address_from_col(
                row.id,
                "payment_address",
                &row.payment_address,
            )?),
            status,
        })
    }
}

#[derive(Clone)]
pub struct InvoiceRepository {
    pool: PgPool,
}

/// Input for creating an invoice in the DB.
pub struct CreateInvoiceInput {
    pub id: Uuid,
    pub idempotency_key: String,
    pub chain_id: u64,
    pub factory_address: [u8; 20],
    pub token_address: [u8; 20],
    pub token_decimals: u8,
    pub beneficiary_address: [u8; 20],
    pub expiration_timestamp: u64,
    pub recovery_address: [u8; 20],
    pub amount: String,
    pub salt: [u8; 32],
    pub payment_address: [u8; 20],
}

/// One validated, finalized USDC `Transfer` log from the configured token.
#[derive(Debug, Clone)]
pub struct PaymentObservation {
    pub block_number: u64,
    pub block_hash: B256,
    pub transaction_hash: B256,
    pub transaction_index: u64,
    pub log_index: u64,
    pub sender: Address,
    pub recipient: Address,
    pub amount: U256,
}

impl CreateInvoiceInput {
    /// Build the write-path input from a freshly created domain [`Invoice`],
    /// projecting its strongly-typed fields to DB column types. The
    /// idempotency key and token decimals are not part of the domain model, so
    /// they are supplied by the caller. Status is not included — the INSERT
    /// hardcodes `'created'`.
    pub fn from_invoice(invoice: &Invoice, idempotency_key: String, token_decimals: u8) -> Self {
        Self {
            id: invoice.id.0,
            idempotency_key,
            chain_id: invoice.chain_id.0,
            factory_address: invoice.factory.0.into(),
            token_address: invoice.token.0.into(),
            token_decimals,
            beneficiary_address: invoice.beneficiary.0.into(),
            expiration_timestamp: invoice.expiration_timestamp,
            recovery_address: invoice.recovery.0.into(),
            amount: invoice.amount.0.to_string(),
            salt: invoice.salt.0.into(),
            payment_address: invoice.payment_address.0.into(),
        }
    }
}

impl InvoiceRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert a new invoice. Returns the row if inserted, or None if a row with
    /// the same idempotency_key already exists (caller must handle the conflict).
    pub async fn insert(
        &self,
        input: &CreateInvoiceInput,
    ) -> Result<Option<DbInvoice>, sqlx::Error> {
        let row = sqlx::query_as::<_, DbInvoice>(
            r#"
            INSERT INTO invoices
                (id, idempotency_key, chain_id, factory_address, token_address,
                 token_decimals, beneficiary_address, expiration_timestamp,
                 recovery_address, amount, salt, payment_address, status)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 'created')
            ON CONFLICT (idempotency_key) DO NOTHING
            RETURNING *
            "#,
        )
        .bind(input.id)
        .bind(&input.idempotency_key)
        .bind(input.chain_id as i64)
        .bind(&input.factory_address)
        .bind(&input.token_address)
        .bind(input.token_decimals as i16)
        .bind(&input.beneficiary_address)
        .bind(input.expiration_timestamp as i64)
        .bind(&input.recovery_address)
        .bind(&input.amount)
        .bind(&input.salt)
        .bind(&input.payment_address)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row)
    }

    /// Fetch an existing invoice by its idempotency key.
    pub async fn find_by_idempotency_key(
        &self,
        key: &str,
    ) -> Result<Option<DbInvoice>, sqlx::Error> {
        sqlx::query_as::<_, DbInvoice>(r#"SELECT * FROM invoices WHERE idempotency_key = $1"#)
            .bind(key)
            .fetch_optional(&self.pool)
            .await
    }

    /// Fetch an invoice by its ID.
    pub async fn find_by_id(&self, id: Uuid) -> Result<Option<DbInvoice>, sqlx::Error> {
        sqlx::query_as::<_, DbInvoice>(r#"SELECT * FROM invoices WHERE id = $1"#)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    /// Atomically apply one contiguous finalized log range and advance its
    /// canonical cursor. Observations for every known invoice address are
    /// persisted, including late transfers. Replays are idempotent by
    /// transaction hash + log index.
    pub async fn apply_finalized_usdc_range(
        &self,
        chain_id: u64,
        token: Address,
        expected_cursor: Option<IndexerCursor>,
        end_block: u64,
        end_block_hash: B256,
        observations: &[PaymentObservation],
    ) -> Result<Vec<Uuid>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        let stored: Option<(Vec<u8>, i64, Vec<u8>)> = sqlx::query_as(
            r#"
            SELECT token_address, last_block, last_block_hash
            FROM indexer_cursor
            WHERE chain_id = $1
            FOR UPDATE
            "#,
        )
        .bind(chain_id as i64)
        .fetch_optional(&mut *tx)
        .await?;

        let stored_cursor = stored
            .map(|(stored_token, block, hash)| {
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
            is_created: bool,
            required: U256,
            received: U256,
            crossing: Option<(U256, u64, B256)>,
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
                    is_created: row.status == "created",
                    required,
                    received,
                    crossing: None,
                },
            );
        }

        for observation in observations {
            let Some(credit) = credits.get_mut(observation.recipient.as_slice()) else {
                continue;
            };

            let inserted = sqlx::query(
                r#"
                INSERT INTO payment_observations
                    (chain_id, token_address, block_number, block_hash,
                     transaction_hash, transaction_index, log_index,
                     sender_address, recipient_address, invoice_id, amount)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(chain_id as i64)
            .bind(token.as_slice())
            .bind(observation.block_number as i64)
            .bind(observation.block_hash.as_slice())
            .bind(observation.transaction_hash.as_slice())
            .bind(observation.transaction_index as i64)
            .bind(observation.log_index as i64)
            .bind(observation.sender.as_slice())
            .bind(observation.recipient.as_slice())
            .bind(credit.id)
            .bind(observation.amount.to_string())
            .execute(&mut *tx)
            .await?
            .rows_affected()
                > 0;

            if !inserted {
                continue;
            }

            credit.received = credit
                .received
                .checked_add(observation.amount)
                .ok_or_else(|| {
                    sqlx::Error::Decode("USDC observation total overflowed uint256".into())
                })?;
            if credit.is_created && credit.crossing.is_none() && credit.received >= credit.required
            {
                credit.crossing = Some((
                    credit.received,
                    observation.block_number,
                    observation.block_hash,
                ));
            }
        }

        let mut funded = Vec::new();
        for credit in credits.values() {
            if let Some((observed, block, block_hash)) = credit.crossing {
                let result = sqlx::query(
                    r#"
                    UPDATE invoices
                    SET confirmed_received = $2,
                        status = 'funded',
                        observed_amount = $3,
                        funded_at_block = $4,
                        funded_at_block_hash = $5,
                        updated_at = now()
                    WHERE id = $1 AND status = 'created'
                    "#,
                )
                .bind(credit.id)
                .bind(credit.received.to_string())
                .bind(observed.to_string())
                .bind(block as i64)
                .bind(block_hash.as_slice())
                .execute(&mut *tx)
                .await?;
                if result.rows_affected() > 0 {
                    funded.push(credit.id);
                }
            } else if credit.received != U256::ZERO {
                sqlx::query(
                    r#"
                    UPDATE invoices
                    SET confirmed_received = $2, updated_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(credit.id)
                .bind(credit.received.to_string())
                .execute(&mut *tx)
                .await?;
            }
        }

        sqlx::query(
            r#"
            INSERT INTO indexer_cursor
                (chain_id, token_address, last_block, last_block_hash, updated_at)
            VALUES ($1, $2, $3, $4, now())
            ON CONFLICT (chain_id) DO UPDATE
            SET token_address = EXCLUDED.token_address,
                last_block = EXCLUDED.last_block,
                last_block_hash = EXCLUDED.last_block_hash,
                updated_at = now()
            "#,
        )
        .bind(chain_id as i64)
        .bind(token.as_slice())
        .bind(end_block as i64)
        .bind(end_block_hash.as_slice())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(funded)
    }

    /// List invoices still awaiting payment on a given chain, oldest first.
    ///
    /// These are the invoices the indexer must reconcile against on-chain
    /// balances. `limit` bounds the batch size so a large backlog cannot stall a
    /// single poll pass. UUIDv7 ordering makes the scan deterministic and
    /// fair (oldest invoices are checked first).
    pub async fn list_created(
        &self,
        chain_id: u64,
        limit: i64,
    ) -> Result<Vec<DbInvoice>, sqlx::Error> {
        sqlx::query_as::<_, DbInvoice>(
            r#"
            SELECT * FROM invoices
            WHERE status = 'created' AND chain_id = $1
            ORDER BY id
            LIMIT $2
            "#,
        )
        .bind(chain_id as i64)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Transition an invoice `created -> funded`, recording the observed balance
    /// and block height. Idempotent compare-and-set: the `WHERE status =
    /// 'created'` guard means a re-run, a duplicate delivery, or a concurrent
    /// worker updates zero rows and returns `false` — the transition happens at
    /// most once.
    ///
    /// `observed_amount` is the canonical base-unit balance (decimal string).
    pub async fn mark_funded(
        &self,
        id: Uuid,
        observed_amount: &str,
        block: u64,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE invoices
            SET status = 'funded',
                observed_amount = $2,
                funded_at_block = $3,
                updated_at = now()
            WHERE id = $1 AND status = 'created'
            "#,
        )
        .bind(id)
        .bind(observed_amount)
        .bind(block as i64)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// List invoices the sweep worker should act on for a given chain: those
    /// awaiting a sweep (`funded`) plus those already in flight (`deploying`)
    /// whose exponential backoff has elapsed. `funded` rows are always eligible;
    /// a `deploying` row is eligible only when
    /// `now - last_attempt_at >= min(base * 2^sweep_attempts, cap)` seconds
    /// (or it has never been attempted). Oldest first (UUIDv7), bounded by `limit`.
    pub async fn list_sweepable(
        &self,
        chain_id: u64,
        limit: i64,
        backoff_base_secs: f64,
        backoff_cap_secs: f64,
    ) -> Result<Vec<DbInvoice>, sqlx::Error> {
        sqlx::query_as::<_, DbInvoice>(
            r#"
            SELECT * FROM invoices
            WHERE chain_id = $1
              AND (
                status = 'funded'
                OR (
                    status = 'deploying'
                    AND (
                        last_attempt_at IS NULL
                        OR now() >= last_attempt_at
                            + make_interval(secs => LEAST($3 * pow(2, sweep_attempts), $4))
                    )
                )
              )
            ORDER BY id
            LIMIT $2
            "#,
        )
        .bind(chain_id as i64)
        .bind(limit)
        .bind(backoff_base_secs)
        .bind(backoff_cap_secs)
        .fetch_all(&self.pool)
        .await
    }

    /// Claim an invoice for sweeping, transitioning `funded -> deploying`.
    /// Idempotent compare-and-set: the `WHERE status = 'funded'` guard means a
    /// concurrent worker (or a re-run) claims each invoice at most once and gets
    /// `false`. Returns `true` only for the caller that won the claim.
    pub async fn mark_deploying(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE invoices
            SET status = 'deploying',
                updated_at = now()
            WHERE id = $1 AND status = 'funded'
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Transition an in-flight invoice `deploying -> fulfilled`, preserving the
    /// sweep transaction (if we sent one) and recording its inclusion block. `tx_hash` is
    /// `None` when the payment was found already-executed on-chain rather than
    /// swept by a transaction we sent. Idempotent CAS on `status = 'deploying'`.
    pub async fn mark_fulfilled(
        &self,
        id: Uuid,
        tx_hash: Option<&[u8]>,
        block: Option<u64>,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE invoices
            SET status = 'fulfilled',
                execute_tx_hash = $2,
                fulfilled_at_block = $3,
                updated_at = now()
            WHERE id = $1 AND status = 'deploying'
            "#,
        )
        .bind(id)
        .bind(tx_hash)
        .bind(block.map(|b| b as i64))
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Persist the canonical block where deployed Payment code was observed.
    /// This survives restarts while the sweep waits for finality.
    pub async fn record_sweep_detection(
        &self,
        id: Uuid,
        tx_hash: Option<&[u8]>,
        block: u64,
        block_hash: &[u8],
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE invoices
            SET execute_tx_hash = COALESCE($2, execute_tx_hash),
                sweep_detected_at_block = $3,
                sweep_detected_at_block_hash = $4,
                updated_at = now()
            WHERE id = $1 AND status = 'deploying'
            "#,
        )
        .bind(id)
        .bind(tx_hash)
        .bind(block as i64)
        .bind(block_hash)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Persist an execute transaction immediately after submission, before
    /// waiting for its receipt, so restart recovery does not blindly resubmit.
    pub async fn record_sweep_submission(
        &self,
        id: Uuid,
        tx_hash: &[u8],
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE invoices
            SET execute_tx_hash = $2,
                sweep_submitted_at = now(),
                updated_at = now()
            WHERE id = $1 AND status = 'deploying'
            "#,
        )
        .bind(id)
        .bind(tx_hash)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Clear a transaction that mined unsuccessfully so execution can be
    /// simulated and retried on a later pass.
    pub async fn clear_sweep_submission(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE invoices
            SET execute_tx_hash = NULL,
                sweep_submitted_at = NULL,
                updated_at = now()
            WHERE id = $1 AND status = 'deploying'
              AND sweep_detected_at_block IS NULL
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Clear an orphaned sweep detection so the next pass can recover or
    /// resubmit the permissionless execute transaction.
    pub async fn clear_sweep_detection(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE invoices
            SET execute_tx_hash = NULL,
                sweep_submitted_at = NULL,
                sweep_detected_at_block = NULL,
                sweep_detected_at_block_hash = NULL,
                updated_at = now()
            WHERE id = $1 AND status = 'deploying'
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Transition an in-flight invoice `deploying -> blocked`, recording why.
    /// Used when the payment cannot be delivered on-chain (the beneficiary
    /// rejected the transfer). Idempotent CAS on `status = 'deploying'`.
    pub async fn mark_blocked(&self, id: Uuid, reason: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE invoices
            SET status = 'blocked',
                blocked_reason = $2,
                updated_at = now()
            WHERE id = $1 AND status = 'deploying'
            "#,
        )
        .bind(id)
        .bind(reason)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Record a retryable sweep failure without turning an RPC outage into a
    /// terminal invoice state. The attempt count drives exponential backoff.
    pub async fn record_sweep_retry(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE invoices
            SET sweep_attempts = sweep_attempts + 1,
                last_attempt_at = now(),
                updated_at = now()
            WHERE id = $1 AND status = 'deploying'
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::types::chrono::{DateTime, Utc};

    fn epoch() -> DateTime<Utc> {
        DateTime::from_timestamp(0, 0).unwrap()
    }

    /// A DB row that decodes cleanly; individual tests corrupt one field.
    fn valid_row() -> DbInvoice {
        DbInvoice {
            id: Uuid::now_v7(),
            idempotency_key: "key".to_string(),
            chain_id: 1,
            factory_address: vec![1u8; 20],
            token_address: vec![2u8; 20],
            token_decimals: 18,
            beneficiary_address: vec![3u8; 20],
            expiration_timestamp: 1_900_000_000,
            recovery_address: vec![6u8; 20],
            amount: "100".to_string(),
            salt: vec![4u8; 32],
            payment_address: vec![5u8; 20],
            status: "created".to_string(),
            created_at: epoch(),
            updated_at: epoch(),
            funded_at_block: None,
            observed_amount: None,
            confirmed_received: "0".to_string(),
            funded_at_block_hash: None,
            sweep_submitted_at: None,
            sweep_detected_at_block: None,
            sweep_detected_at_block_hash: None,
            execute_tx_hash: None,
            fulfilled_at_block: None,
            sweep_attempts: 0,
            last_attempt_at: None,
            blocked_reason: None,
        }
    }

    #[test]
    fn valid_row_decodes() {
        let invoice = Invoice::try_from(&valid_row()).expect("valid row must decode");
        assert_eq!(invoice.chain_id.0, 1);
        assert_eq!(invoice.amount.0.to_string(), "100");
        assert_eq!(invoice.expiration_timestamp, 1_900_000_000);
        assert_eq!(invoice.recovery.0, Address::repeat_byte(6));
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
        row.status = "bogus".to_string();
        assert!(matches!(
            Invoice::try_from(&row),
            Err(DbInvoiceError::InvalidStatus { .. })
        ));
    }

    #[test]
    fn wrong_length_address_is_typed_error_not_panic() {
        let mut row = valid_row();
        row.token_address = vec![2u8; 19]; // one byte short
        assert!(matches!(
            Invoice::try_from(&row),
            Err(DbInvoiceError::WrongByteLength {
                field: "token_address",
                ..
            })
        ));
    }

    #[test]
    fn wrong_length_salt_is_typed_error_not_panic() {
        let mut row = valid_row();
        row.salt = vec![4u8; 31];
        assert!(matches!(
            Invoice::try_from(&row),
            Err(DbInvoiceError::WrongByteLength { field: "salt", .. })
        ));
    }

    #[test]
    fn wrong_length_recovery_is_typed_error_not_panic() {
        let mut row = valid_row();
        row.recovery_address = vec![6u8; 19];
        assert!(matches!(
            Invoice::try_from(&row),
            Err(DbInvoiceError::WrongByteLength {
                field: "recovery_address",
                ..
            })
        ));
    }
}
