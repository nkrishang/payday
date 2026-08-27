//! Sweep queue and helper-transaction batches.
//!
//! The invoice table is the durable queue: every invoice with uncollected
//! funds at its payment address is eligible, whatever its status, because the
//! helper contract settles, recovers, or collects as appropriate. A
//! `sweep_batches` row owns one signer nonce; replacements for that nonce
//! append their hashes so a receipt for any of them resolves the batch.

use alloy_primitives::{B256, Bytes};
use gateway_core::InvoiceStatus;
use sqlx::types::chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::invoices::{DbInvoice, InvoiceRepository};

/// Statuses whose uncollected funds the worker moves automatically. `created`
/// is excluded because a partial balance cannot be settled before expiry, and
/// `blocked` because it means an operator must look first.
pub const SWEEPABLE_STATUSES: [InvoiceStatus; 5] = [
    InvoiceStatus::Funded,
    InvoiceStatus::Deploying,
    InvoiceStatus::Expired,
    InvoiceStatus::Fulfilled,
    InvoiceStatus::Recovered,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepBatch {
    pub id: Uuid,
    pub chain_id: u64,
    pub nonce: u64,
    pub gas_limit: u64,
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
    /// Every signed transaction hash prepared for this nonce, oldest first.
    pub tx_hashes: Vec<B256>,
    /// Signed EIP-2718 bytes corresponding one-for-one with `tx_hashes`.
    pub raw_transactions: Vec<Bytes>,
    /// None means the newest signed transaction is durable but not yet known
    /// to have been broadcast.
    pub broadcast_at: Option<DateTime<Utc>>,
    /// When the newest transaction was broadcast.
    pub submitted_at: DateTime<Utc>,
    /// Receipt seen for one of the submissions, awaiting finality.
    pub mined: Option<MinedBatch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinedBatch {
    pub tx_hash: B256,
    pub block: u64,
    pub block_hash: B256,
    pub transaction_index: u64,
}

/// How a batch ended without finalizing its invoices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchResolution {
    /// The helper transaction mined with status 0.
    Reverted,
    /// No submission for the nonce can mine any more.
    Abandoned,
}

impl BatchResolution {
    fn as_str(self) -> &'static str {
        match self {
            BatchResolution::Reverted => "reverted",
            BatchResolution::Abandoned => "abandoned",
        }
    }
}

/// What a finalized batch receipt established for one invoice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvoiceOutcome {
    /// The payment address was emptied in this transaction. `status` moves an
    /// open invoice to its terminal state; `None` keeps a terminal status when
    /// only late funds were collected. `execute_tx` records the batch as the
    /// transaction that deployed the payment contract.
    Drained {
        status: Option<InvoiceStatus>,
        execute_tx: bool,
    },
    /// The item failed for a reason that may clear; it returns to the queue
    /// behind exponential backoff.
    Retry,
    /// The item failed for a reason automation cannot fix. An open invoice
    /// becomes `blocked`; a terminal one keeps its status. Either way the
    /// reason removes it from the queue until an operator clears it.
    Blocked { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SweepQueueStats {
    pub queued: i64,
    pub in_flight: i64,
    /// Age of the oldest uncollected transfer that automation is responsible for.
    pub oldest_uncollected_secs: Option<f64>,
}

#[derive(sqlx::FromRow)]
struct SweepBatchRow {
    id: Uuid,
    chain_id: i64,
    nonce: i64,
    gas_limit: i64,
    max_fee_per_gas: String,
    max_priority_fee_per_gas: String,
    tx_hashes: Vec<Vec<u8>>,
    raw_transactions: Vec<Vec<u8>>,
    submitted_at: DateTime<Utc>,
    broadcast_at: Option<DateTime<Utc>>,
    mined_tx_hash: Option<Vec<u8>>,
    mined_block: Option<i64>,
    mined_block_hash: Option<Vec<u8>>,
    mined_transaction_index: Option<i64>,
}

impl TryFrom<SweepBatchRow> for SweepBatch {
    type Error = sqlx::Error;

    fn try_from(row: SweepBatchRow) -> Result<Self, Self::Error> {
        let decode = |name: &str, value: Option<Vec<u8>>| {
            value
                .map(|bytes| {
                    B256::try_from(bytes.as_slice()).map_err(|_| {
                        sqlx::Error::Decode(format!("invalid {name} length in sweep batch").into())
                    })
                })
                .transpose()
        };
        let fee = |name: &str, value: &str| {
            value
                .parse::<u128>()
                .map_err(|_| sqlx::Error::Decode(format!("invalid {name} in sweep batch").into()))
        };
        let mined = match (
            decode("mined_tx_hash", row.mined_tx_hash)?,
            row.mined_block,
            decode("mined_block_hash", row.mined_block_hash)?,
            row.mined_transaction_index,
        ) {
            (Some(tx_hash), Some(block), Some(block_hash), Some(transaction_index)) => {
                Some(MinedBatch {
                    tx_hash,
                    block: block as u64,
                    block_hash,
                    transaction_index: transaction_index as u64,
                })
            }
            _ => None,
        };
        Ok(SweepBatch {
            id: row.id,
            chain_id: row.chain_id as u64,
            nonce: row.nonce as u64,
            gas_limit: row.gas_limit as u64,
            max_fee_per_gas: fee("max_fee_per_gas", &row.max_fee_per_gas)?,
            max_priority_fee_per_gas: fee(
                "max_priority_fee_per_gas",
                &row.max_priority_fee_per_gas,
            )?,
            tx_hashes: row
                .tx_hashes
                .into_iter()
                .map(|hash| decode("tx_hash", Some(hash)).map(Option::unwrap))
                .collect::<Result<_, _>>()?,
            raw_transactions: row.raw_transactions.into_iter().map(Bytes::from).collect(),
            submitted_at: row.submitted_at,
            broadcast_at: row.broadcast_at,
            mined,
        })
    }
}

fn sweepable_statuses() -> Vec<&'static str> {
    SWEEPABLE_STATUSES
        .iter()
        .map(InvoiceStatus::as_str)
        .collect()
}

impl InvoiceRepository {
    /// Atomically claim the next invoices with uncollected funds. Newly funded
    /// rows transition to `deploying`; every claimed row's `last_attempt_at`
    /// gates its reclaim after a crash or a failed submission. Row locks are
    /// released before any chain RPC begins.
    pub async fn claim_sweep_batch(
        &self,
        chain_id: u64,
        limit: i64,
        backoff_base_secs: f64,
        backoff_cap_secs: f64,
    ) -> Result<Vec<DbInvoice>, sqlx::Error> {
        sqlx::query_as::<_, DbInvoice>(
            r#"
            WITH candidates AS (
                SELECT id
                FROM invoices
                WHERE chain_id = $1
                  AND uncollected_count > 0
                  AND sweep_batch_id IS NULL
                  AND blocked_reason IS NULL
                  AND status = ANY($5)
                  AND (
                    last_attempt_at IS NULL
                    OR now() >= last_attempt_at
                        + make_interval(secs => LEAST($3 * pow(2, sweep_attempts), $4))
                  )
                ORDER BY id
                FOR UPDATE SKIP LOCKED
                LIMIT $2
            )
            UPDATE invoices AS invoice
            SET status = CASE WHEN invoice.status = 'funded' THEN 'deploying' ELSE invoice.status END,
                last_attempt_at = now(),
                updated_at = now()
            FROM candidates
            WHERE invoice.id = candidates.id
            RETURNING invoice.*
            "#,
        )
        .bind(chain_id as i64)
        .bind(limit)
        .bind(backoff_base_secs)
        .bind(backoff_cap_secs)
        .bind(sweepable_statuses())
        .fetch_all(self.pool())
        .await
    }

    /// A claimed set could not be submitted; count the attempt so the backoff
    /// applies before the rows are reclaimed.
    pub async fn release_claim(&self, ids: &[Uuid]) -> Result<u64, sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE invoices
            SET sweep_attempts = sweep_attempts + 1, last_attempt_at = now(), updated_at = now()
            WHERE id = ANY($1) AND sweep_batch_id IS NULL
            "#,
        )
        .bind(ids)
        .execute(self.pool())
        .await
        .map(|result| result.rows_affected())
    }

    /// Remove an invoice from automation with a reason, outside any batch.
    pub async fn block_invoice(&self, id: Uuid, reason: &str) -> Result<bool, sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE invoices
            SET status = CASE WHEN status IN ('deploying', 'expired') THEN 'blocked' ELSE status END,
                blocked_reason = $2,
                updated_at = now()
            WHERE id = $1 AND sweep_batch_id IS NULL
            "#,
        )
        .bind(id)
        .bind(reason)
        .execute(self.pool())
        .await
        .map(|result| result.rows_affected() > 0)
    }

    /// Open a batch for a signed helper transaction and attach every
    /// claimed invoice to it. The partial unique index on open batches makes a
    /// second in-flight batch per chain impossible; a partial attachment rolls
    /// back so batch membership is never ambiguous.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_batch_submission(
        &self,
        chain_id: u64,
        invoice_ids: &[Uuid],
        nonce: u64,
        gas_limit: u64,
        max_fee_per_gas: u128,
        max_priority_fee_per_gas: u128,
        tx_hash: B256,
        raw_transaction: &[u8],
    ) -> Result<Uuid, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO sweep_batches
                (id, chain_id, nonce, gas_limit, max_fee_per_gas, max_priority_fee_per_gas,
                 tx_hashes, raw_transactions)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(id)
        .bind(chain_id as i64)
        .bind(nonce as i64)
        .bind(gas_limit as i64)
        .bind(max_fee_per_gas.to_string())
        .bind(max_priority_fee_per_gas.to_string())
        .bind(vec![tx_hash.to_vec()])
        .bind(vec![raw_transaction])
        .execute(&mut *tx)
        .await?;

        let attached = sqlx::query(
            r#"
            UPDATE invoices
            SET sweep_batch_id = $1, updated_at = now()
            WHERE id = ANY($2) AND chain_id = $3 AND sweep_batch_id IS NULL
            "#,
        )
        .bind(id)
        .bind(invoice_ids)
        .bind(chain_id as i64)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if attached != invoice_ids.len() as u64 {
            return Err(sqlx::Error::Protocol(format!(
                "attached {attached} of {} invoices to sweep batch",
                invoice_ids.len()
            )));
        }
        tx.commit().await?;
        Ok(id)
    }

    /// The chain's single in-flight batch, if any.
    pub async fn open_sweep_batch(&self, chain_id: u64) -> Result<Option<SweepBatch>, sqlx::Error> {
        sqlx::query_as::<_, SweepBatchRow>(
            r#"SELECT * FROM sweep_batches WHERE chain_id = $1 AND resolved_at IS NULL"#,
        )
        .bind(chain_id as i64)
        .fetch_optional(self.pool())
        .await?
        .map(SweepBatch::try_from)
        .transpose()
    }

    pub async fn batch_invoices(&self, batch_id: Uuid) -> Result<Vec<DbInvoice>, sqlx::Error> {
        sqlx::query_as::<_, DbInvoice>(
            r#"SELECT * FROM invoices WHERE sweep_batch_id = $1 ORDER BY id"#,
        )
        .bind(batch_id)
        .fetch_all(self.pool())
        .await
    }

    /// Persist a signed same-nonce replacement before broadcasting it.
    pub async fn record_batch_replacement(
        &self,
        batch_id: Uuid,
        tx_hash: B256,
        max_fee_per_gas: u128,
        max_priority_fee_per_gas: u128,
        raw_transaction: &[u8],
    ) -> Result<bool, sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE sweep_batches
            SET tx_hashes = array_append(tx_hashes, $2),
                raw_transactions = array_append(raw_transactions, $3),
                max_fee_per_gas = $4,
                max_priority_fee_per_gas = $5,
                broadcast_at = NULL
            WHERE id = $1 AND resolved_at IS NULL
            "#,
        )
        .bind(batch_id)
        .bind(tx_hash.as_slice())
        .bind(raw_transaction)
        .bind(max_fee_per_gas.to_string())
        .bind(max_priority_fee_per_gas.to_string())
        .execute(self.pool())
        .await
        .map(|result| result.rows_affected() > 0)
    }

    /// Mark the newest durable transaction as broadcast. Re-sending the same
    /// signed bytes before this update is safe and closes the RPC/DB crash gap.
    pub async fn record_batch_broadcast(
        &self,
        batch_id: Uuid,
        tx_hash: B256,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE sweep_batches
            SET broadcast_at = now(), submitted_at = now()
            WHERE id = $1 AND resolved_at IS NULL AND broadcast_at IS NULL
              AND tx_hashes[cardinality(tx_hashes)] = $2
            "#,
        )
        .bind(batch_id)
        .bind(tx_hash.as_slice())
        .execute(self.pool())
        .await
        .map(|result| result.rows_affected() > 0)
    }

    /// Persist the receipt block so finality is tracked across restarts.
    pub async fn record_batch_mined(
        &self,
        batch_id: Uuid,
        mined: MinedBatch,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE sweep_batches
            SET mined_tx_hash = $2, mined_block = $3, mined_block_hash = $4,
                mined_transaction_index = $5
            WHERE id = $1 AND resolved_at IS NULL
            "#,
        )
        .bind(batch_id)
        .bind(mined.tx_hash.as_slice())
        .bind(mined.block as i64)
        .bind(mined.block_hash.as_slice())
        .bind(mined.transaction_index as i64)
        .execute(self.pool())
        .await
        .map(|result| result.rows_affected() > 0)
    }

    /// The recorded receipt block is no longer canonical; wait for a new receipt.
    pub async fn clear_batch_mined(&self, batch_id: Uuid) -> Result<bool, sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE sweep_batches
            SET mined_tx_hash = NULL, mined_block = NULL, mined_block_hash = NULL,
                mined_transaction_index = NULL, submitted_at = now()
            WHERE id = $1 AND resolved_at IS NULL
            "#,
        )
        .bind(batch_id)
        .execute(self.pool())
        .await
        .map(|result| result.rows_affected() > 0)
    }

    /// Close a batch that did nothing for its invoices and return them to the
    /// queue behind backoff. Returns the number of invoices re-queued.
    pub async fn resolve_batch(
        &self,
        batch_id: Uuid,
        resolution: BatchResolution,
    ) -> Result<u64, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let closed = sqlx::query(
            r#"
            UPDATE sweep_batches
            SET resolution = $2, resolved_at = now()
            WHERE id = $1 AND resolved_at IS NULL
            "#,
        )
        .bind(batch_id)
        .bind(resolution.as_str())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if closed == 0 {
            return Err(sqlx::Error::Protocol("sweep batch is not open".into()));
        }
        let requeued = sqlx::query(
            r#"
            UPDATE invoices
            SET sweep_batch_id = NULL,
                sweep_attempts = sweep_attempts + 1,
                last_attempt_at = now(),
                updated_at = now()
            WHERE sweep_batch_id = $1
            "#,
        )
        .bind(batch_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(requeued)
    }

    /// Apply a finalized helper-transaction receipt: every invoice of the batch
    /// must have an outcome. Drained invoices have observations before the
    /// receipt's exact transaction position marked collected and their queue
    /// count recomputed from the ledger, so later indexing cannot re-queue
    /// funds the sweep already moved.
    pub async fn finalize_batch(
        &self,
        batch_id: Uuid,
        tx_hash: B256,
        block: u64,
        block_timestamp: u64,
        transaction_index: u64,
        outcomes: &[(Uuid, InvoiceOutcome)],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        for (invoice_id, outcome) in outcomes {
            let updated = match outcome {
                InvoiceOutcome::Drained { status, execute_tx } => {
                    sqlx::query(
                        r#"
                        UPDATE payment_observations
                        SET collected_at_block = $2,
                            collected_at_transaction_index = $3
                        WHERE invoice_id = $1
                          AND collected_at_block IS NULL
                          AND disposition <> 'error'
                          AND (block_number, transaction_index) < ($2, $3)
                        "#,
                    )
                    .bind(invoice_id)
                    .bind(block as i64)
                    .bind(transaction_index as i64)
                    .execute(&mut *tx)
                    .await?;
                    sqlx::query(
                        r#"
                        UPDATE invoices
                        SET status = COALESCE($4, status),
                            execute_tx_hash = CASE WHEN $5 THEN $6 ELSE execute_tx_hash END,
                            resolved_at_block = CASE WHEN $4 IS NULL THEN resolved_at_block
                                                     ELSE COALESCE(resolved_at_block, $2) END,
                            settled_at = CASE WHEN $4 IS NULL THEN settled_at
                                              ELSE COALESCE(settled_at, to_timestamp($8)) END,
                            drained_at_block = $2,
                            drained_at_transaction_index = $3,
                            uncollected_count = (
                                SELECT count(*) FROM payment_observations
                                WHERE invoice_id = $1
                                  AND collected_at_block IS NULL
                                  AND disposition <> 'error'
                            ),
                            sweep_batch_id = NULL,
                            sweep_attempts = 0,
                            updated_at = now()
                        WHERE id = $1 AND sweep_batch_id = $7
                        "#,
                    )
                    .bind(invoice_id)
                    .bind(block as i64)
                    .bind(transaction_index as i64)
                    .bind(status.map(|status| status.as_str()))
                    .bind(execute_tx)
                    .bind(tx_hash.as_slice())
                    .bind(batch_id)
                    .bind(block_timestamp as f64)
                    .execute(&mut *tx)
                    .await?
                }
                InvoiceOutcome::Retry => {
                    sqlx::query(
                        r#"
                        UPDATE invoices
                        SET sweep_batch_id = NULL,
                            sweep_attempts = sweep_attempts + 1,
                            last_attempt_at = now(),
                            updated_at = now()
                        WHERE id = $1 AND sweep_batch_id = $2
                        "#,
                    )
                    .bind(invoice_id)
                    .bind(batch_id)
                    .execute(&mut *tx)
                    .await?
                }
                InvoiceOutcome::Blocked { reason } => {
                    sqlx::query(
                        r#"
                        UPDATE invoices
                        SET status = CASE WHEN status IN ('deploying', 'expired') THEN 'blocked' ELSE status END,
                            blocked_reason = $3,
                            sweep_batch_id = NULL,
                            sweep_attempts = sweep_attempts + 1,
                            last_attempt_at = now(),
                            updated_at = now()
                        WHERE id = $1 AND sweep_batch_id = $2
                        "#,
                    )
                    .bind(invoice_id)
                    .bind(batch_id)
                    .bind(reason)
                    .execute(&mut *tx)
                    .await?
                }
            };
            if updated.rows_affected() != 1 {
                return Err(sqlx::Error::Protocol(format!(
                    "invoice {invoice_id} is not part of sweep batch {batch_id}"
                )));
            }
        }

        let orphaned: i64 =
            sqlx::query_scalar(r#"SELECT count(*) FROM invoices WHERE sweep_batch_id = $1"#)
                .bind(batch_id)
                .fetch_one(&mut *tx)
                .await?;
        if orphaned != 0 {
            return Err(sqlx::Error::Protocol(format!(
                "{orphaned} invoices of sweep batch {batch_id} have no outcome"
            )));
        }

        let closed = sqlx::query(
            r#"
            UPDATE sweep_batches
            SET resolution = 'finalized', resolved_at = now()
            WHERE id = $1 AND resolved_at IS NULL
            "#,
        )
        .bind(batch_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if closed != 1 {
            return Err(sqlx::Error::Protocol("sweep batch is not open".into()));
        }
        tx.commit().await
    }

    pub async fn sweep_queue_stats(&self, chain_id: u64) -> Result<SweepQueueStats, sqlx::Error> {
        let (queued, in_flight, oldest_uncollected_secs): (i64, i64, Option<f64>) = sqlx::query_as(
            r#"
            SELECT
                (SELECT count(*) FROM invoices
                  WHERE chain_id = $1 AND uncollected_count > 0 AND sweep_batch_id IS NULL
                    AND blocked_reason IS NULL AND status = ANY($2)),
                (SELECT count(*) FROM invoices WHERE chain_id = $1 AND sweep_batch_id IS NOT NULL),
                (SELECT EXTRACT(EPOCH FROM now() - min(o.observed_at))::float8
                   FROM payment_observations o
                   JOIN invoices i ON i.id = o.invoice_id
                  WHERE i.chain_id = $1 AND o.collected_at_block IS NULL AND o.disposition <> 'error'
                    AND i.blocked_reason IS NULL AND i.status = ANY($2))
            "#,
        )
        .bind(chain_id as i64)
        .bind(sweepable_statuses())
        .fetch_one(self.pool())
        .await?;
        Ok(SweepQueueStats {
            queued,
            in_flight,
            oldest_uncollected_secs,
        })
    }
}
