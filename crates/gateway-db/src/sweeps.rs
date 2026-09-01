//! Sweep queue and helper-transaction batches.
//!
//! The invoice table is the durable queue: every invoice with uncollected
//! funds at its payment address is eligible, whatever its status, because the
//! helper contract settles, recovers, or collects as appropriate. A
//! `sweep_batches` row owns one signer nonce; replacements for that nonce
//! append their hashes so a receipt for any of them resolves the batch.

use alloy_primitives::{B256, Bytes, U256};
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

/// Why an amount left a payment address for the platform recovery wallet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryReason {
    /// The address held more than the invoice amount when it settled.
    Overpayment,
    /// The invoice expired and its whole balance was recovered.
    Expired,
    /// Funds arrived after the contract existed and `recover()` forwarded them.
    LateTransfer,
}

impl RecoveryReason {
    /// The canonical string stored in `recovered_funds.reason`. This is the
    /// single source of truth; the migration's CHECK constraint agrees with it.
    pub fn as_str(self) -> &'static str {
        match self {
            RecoveryReason::Overpayment => "overpayment",
            RecoveryReason::Expired => "expired",
            RecoveryReason::LateTransfer => "late_transfer",
        }
    }
}

/// One nonzero amount the platform recovery wallet received on an invoice's
/// behalf in the finalized batch transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredFundsInput {
    pub amount: U256,
    pub reason: RecoveryReason,
}

/// What a finalized batch receipt established for one invoice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvoiceOutcome {
    /// The payment address was emptied in this transaction. `status` moves an
    /// open invoice to its terminal state; `None` keeps a terminal status when
    /// only late funds were collected. `execute_tx` records the batch as the
    /// transaction that deployed the payment contract. `recovered` is the
    /// amount that went to the platform recovery wallet in the batch
    /// transaction, and `settlement_recovered` the amount the settlement
    /// transaction sent there when somebody else deployed the contract; both
    /// are written to the recovery ledger in the same database transaction,
    /// keyed by the chain transaction that moved them. Zero recoveries pass
    /// `None`.
    Drained {
        status: Option<InvoiceStatus>,
        execute_tx: bool,
        settlement_tx_hash: B256,
        settlement_block: u64,
        settlement_timestamp: u64,
        settlement_recovered: Option<RecoveredFundsInput>,
        recovered: Option<RecoveredFundsInput>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweeperStatus {
    pub state: String,
    pub heartbeat_at: DateTime<Utc>,
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
        let mut tx = self.pool().begin().await?;
        let account: Option<Uuid> = sqlx::query_scalar(
            r#"
            UPDATE invoices
            SET status = CASE WHEN status IN ('deploying', 'expired') THEN 'blocked' ELSE status END,
                blocked_reason = $2,
                updated_at = now()
            WHERE id = $1 AND sweep_batch_id IS NULL AND blocked_reason IS NULL
            RETURNING account_id
            "#,
        )
        .bind(id)
        .bind(reason)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(account) = account {
            sqlx::query("INSERT INTO notification_outbox(id,account_id,invoice_id,reason,email) SELECT $1,$2,$3,$4,email FROM accounts WHERE id=$2")
                .bind(Uuid::now_v7()).bind(account).bind(id).bind(reason)
                .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(account.is_some())
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
    /// funds the sweep already moved. Every nonzero recovery is appended to
    /// `recovered_funds` in the same transaction, stamped with the receipt
    /// block's `block_timestamp`, so the ledger can never disagree with the
    /// invoice state it was derived from.
    pub async fn finalize_batch(
        &self,
        batch_id: Uuid,
        tx_hash: B256,
        block: u64,
        transaction_index: u64,
        block_timestamp: u64,
        outcomes: &[(Uuid, InvoiceOutcome)],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        for (invoice_id, outcome) in outcomes {
            let updated: u64 = match outcome {
                InvoiceOutcome::Drained {
                    status,
                    execute_tx,
                    settlement_tx_hash,
                    settlement_block,
                    settlement_timestamp,
                    settlement_recovered,
                    recovered,
                } => {
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
                    let drained: Option<(i64, Vec<u8>)> = sqlx::query_as(
                        r#"
                        UPDATE invoices
                        SET status = COALESCE($4, status),
                            execute_tx_hash = CASE WHEN $5 THEN $6 ELSE execute_tx_hash END,
                            settlement_tx_hash = COALESCE(settlement_tx_hash, $8),
                            resolved_at_block = CASE WHEN $4 IS NULL THEN resolved_at_block
                                                     ELSE COALESCE(resolved_at_block, $9) END,
                            settled_at = CASE WHEN $4 IS NULL THEN settled_at
                                              ELSE COALESCE(settled_at, to_timestamp($10)) END,
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
                        RETURNING chain_id, token_address
                        "#,
                    )
                    .bind(invoice_id)
                    .bind(block as i64)
                    .bind(transaction_index as i64)
                    .bind(status.map(|status| status.as_str()))
                    .bind(execute_tx)
                    .bind(tx_hash.as_slice())
                    .bind(batch_id)
                    .bind(settlement_tx_hash.as_slice())
                    .bind(*settlement_block as i64)
                    .bind(*settlement_timestamp as f64)
                    .fetch_optional(&mut *tx)
                    .await?;
                    match drained {
                        None => 0,
                        Some((chain_id, token_address)) => {
                            // Each ledger row is keyed by the chain transaction
                            // that moved the funds: a third party's settlement
                            // first, then the batch's own recovery. A receipt
                            // is applied at most once per batch, so the
                            // conflict target only fires if the same
                            // transaction is replayed through another batch;
                            // the ledger keeps its first record.
                            let ledger_rows = [
                                settlement_recovered.as_ref().map(|recovery| {
                                    (
                                        recovery,
                                        settlement_tx_hash.as_slice(),
                                        *settlement_block,
                                        *settlement_timestamp,
                                    )
                                }),
                                recovered.as_ref().map(|recovery| {
                                    (recovery, tx_hash.as_slice(), block, block_timestamp)
                                }),
                            ];
                            for (recovery, transaction_hash, block, timestamp) in
                                ledger_rows.into_iter().flatten()
                            {
                                sqlx::query(
                                    r#"
                                    INSERT INTO recovered_funds
                                        (id, invoice_id, chain_id, token_address, transaction_hash,
                                         block_number, amount, reason, recovered_at)
                                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, to_timestamp($9))
                                    ON CONFLICT (invoice_id, transaction_hash, reason) DO NOTHING
                                    "#,
                                )
                                .bind(Uuid::now_v7())
                                .bind(invoice_id)
                                .bind(chain_id)
                                .bind(token_address.as_slice())
                                .bind(transaction_hash)
                                .bind(block as i64)
                                .bind(recovery.amount.to_string())
                                .bind(recovery.reason.as_str())
                                .bind(timestamp as f64)
                                .execute(&mut *tx)
                                .await?;
                            }
                            1
                        }
                    }
                }
                InvoiceOutcome::Retry => sqlx::query(
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
                .rows_affected(),
                InvoiceOutcome::Blocked { reason } => sqlx::query(
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
                .rows_affected(),
            };
            if updated != 1 {
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

    pub async fn record_sweeper_status(
        &self,
        chain_id: u64,
        state: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO sweeper_status (chain_id, state) VALUES ($1, $2) ON CONFLICT (chain_id) DO UPDATE SET state=EXCLUDED.state, heartbeat_at=now()")
            .bind(chain_id as i64).bind(state).execute(self.pool()).await.map(drop)
    }

    pub async fn sweeper_status(
        &self,
        chain_id: u64,
    ) -> Result<Option<SweeperStatus>, sqlx::Error> {
        sqlx::query_as::<_, (String, DateTime<Utc>)>("SELECT CASE WHEN heartbeat_at < now() - interval '30 seconds' THEN 'degraded' ELSE state END, heartbeat_at FROM sweeper_status WHERE chain_id=$1")
            .bind(chain_id as i64).fetch_optional(self.pool()).await
            .map(|row| row.map(|(state, heartbeat_at)| SweeperStatus { state, heartbeat_at }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloy_primitives::{U256, address};
    use gateway_core::{
        Amount, BeneficiaryAddress, ChainId, FactoryAddress, Invoice, RecoveryAddress,
        TokenAddress, USDC_DECIMALS,
    };
    use sqlx::PgPool;

    use crate::{AccountId, CreateInvoiceInput};

    const CHAIN_ID: u64 = 31337;
    const TX_HASH: B256 = B256::repeat_byte(0xA1);
    const BLOCK: u64 = 7;
    const BLOCK_TIMESTAMP: u64 = 1_800_000_070;
    /// A deployment somebody other than this worker mined, four blocks earlier.
    const SETTLEMENT_TX_HASH: B256 = B256::repeat_byte(0xB2);
    const SETTLEMENT_BLOCK: u64 = 3;
    const SETTLEMENT_TIMESTAMP: u64 = 1_800_000_030;

    async fn insert_invoice(pool: &PgPool, amount: u64) -> Invoice {
        let account_id = Uuid::from_u128(1);
        sqlx::query(
            r#"INSERT INTO accounts (id, api_key_hash, api_key_hint)
               VALUES ($1, $2, 'test') ON CONFLICT (id) DO NOTHING"#,
        )
        .bind(account_id)
        .bind([1_u8; 32].as_slice())
        .execute(pool)
        .await
        .unwrap();
        let invoice = Invoice::new(
            FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3")),
            ChainId(CHAIN_ID),
            TokenAddress(address!("0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512")),
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8")),
            Amount(U256::from(amount)),
            1_900_000_000,
            RecoveryAddress(address!("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc")),
        );
        let input = CreateInvoiceInput::from_invoice(
            &invoice,
            AccountId(account_id),
            invoice.id.0.to_string(),
            USDC_DECIMALS,
            None,
            3_600,
            "in:3600".into(),
        );
        InvoiceRepository::new(pool.clone())
            .insert(&input)
            .await
            .unwrap()
            .expect("row should be inserted");
        invoice
    }

    async fn open_batch(repo: &InvoiceRepository, ids: &[Uuid]) -> Uuid {
        repo.record_batch_submission(CHAIN_ID, ids, 0, 500_000, 100, 2, TX_HASH, &[0xAB])
            .await
            .unwrap()
    }

    fn overpayment(amount: u64) -> RecoveredFundsInput {
        RecoveredFundsInput {
            amount: U256::from(amount),
            reason: RecoveryReason::Overpayment,
        }
    }

    fn drained(recovered: Option<RecoveredFundsInput>) -> InvoiceOutcome {
        InvoiceOutcome::Drained {
            status: Some(InvoiceStatus::Fulfilled),
            execute_tx: true,
            settlement_tx_hash: TX_HASH,
            settlement_block: BLOCK,
            settlement_timestamp: BLOCK_TIMESTAMP,
            settlement_recovered: None,
            recovered,
        }
    }

    #[derive(sqlx::FromRow)]
    struct LedgerRow {
        chain_id: i64,
        token_address: Vec<u8>,
        transaction_hash: Vec<u8>,
        block_number: i64,
        amount: String,
        reason: String,
        recovered_at: DateTime<Utc>,
    }

    async fn ledger(pool: &PgPool, invoice_id: Uuid) -> Vec<LedgerRow> {
        sqlx::query_as(
            "SELECT chain_id, token_address, transaction_hash, block_number, amount, reason, recovered_at
             FROM recovered_funds WHERE invoice_id = $1 ORDER BY created_at",
        )
        .bind(invoice_id)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn open_batches(pool: &PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM sweep_batches WHERE resolved_at IS NULL")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn finalize_batch_writes_recovery_ledger_atomically(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let member = insert_invoice(&pool, 100).await;
        let outsider = insert_invoice(&pool, 200).await;
        let batch = open_batch(&repo, &[member.id.0]).await;

        // The member's ledger row is written before the outsider is rejected;
        // the rejection must take the ledger row down with it.
        let error = repo
            .finalize_batch(
                batch,
                TX_HASH,
                BLOCK,
                1,
                BLOCK_TIMESTAMP,
                &[
                    (member.id.0, drained(Some(overpayment(50)))),
                    (outsider.id.0, drained(None)),
                ],
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not part of sweep batch"));
        assert!(ledger(&pool, member.id.0).await.is_empty());
        assert_eq!(open_batches(&pool).await, 1);
        let row = repo.find_by_id(member.id.0).await.unwrap().unwrap();
        assert_eq!(row.sweep_batch_id, Some(batch));
        assert_eq!(row.status, "created");

        repo.finalize_batch(
            batch,
            TX_HASH,
            BLOCK,
            1,
            BLOCK_TIMESTAMP,
            &[(member.id.0, drained(Some(overpayment(50))))],
        )
        .await
        .unwrap();
        let rows = ledger(&pool, member.id.0).await;
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.chain_id, CHAIN_ID as i64);
        assert_eq!(row.token_address, member.token.0.to_vec());
        assert_eq!(row.transaction_hash, TX_HASH.to_vec());
        assert_eq!(row.block_number, BLOCK as i64);
        assert_eq!(row.amount, "50");
        assert_eq!(row.reason, "overpayment");
        assert_eq!(row.recovered_at.timestamp(), BLOCK_TIMESTAMP as i64);
        assert_eq!(open_batches(&pool).await, 0);
        assert!(ledger(&pool, outsider.id.0).await.is_empty());

        let event: (String, serde_json::Value) = sqlx::query_as(
            "SELECT event_type, payload FROM webhook_events WHERE invoice_id = $1 AND event_type = 'payment.recovered_funds'",
        )
        .bind(member.id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(event.0, "payment.recovered_funds");
        assert_eq!(event.1["data"]["recovery"]["amount"], "50");
        assert_eq!(event.1["data"]["recovery"]["reason"], "overpayment");
        assert_eq!(event.1["data"]["payment"]["status"], "settled");
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn third_party_settlement_and_late_collection_are_ledgered_together(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let member = insert_invoice(&pool, 100).await;
        let outsider = insert_invoice(&pool, 200).await;
        let batch = open_batch(&repo, &[member.id.0]).await;
        // Somebody else deployed the contract while the address held 125, so
        // their transaction sent the 25 remainder to the recovery wallet; a
        // later transfer of 30 was forwarded by this batch's `recover()`.
        let outcome = InvoiceOutcome::Drained {
            status: Some(InvoiceStatus::Fulfilled),
            execute_tx: false,
            settlement_tx_hash: SETTLEMENT_TX_HASH,
            settlement_block: SETTLEMENT_BLOCK,
            settlement_timestamp: SETTLEMENT_TIMESTAMP,
            settlement_recovered: Some(overpayment(25)),
            recovered: Some(RecoveredFundsInput {
                amount: U256::from(30),
                reason: RecoveryReason::LateTransfer,
            }),
        };

        let error = repo
            .finalize_batch(
                batch,
                TX_HASH,
                BLOCK,
                1,
                BLOCK_TIMESTAMP,
                &[
                    (member.id.0, outcome.clone()),
                    (outsider.id.0, drained(None)),
                ],
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not part of sweep batch"));
        assert!(
            ledger(&pool, member.id.0).await.is_empty(),
            "both ledger rows roll back with the batch"
        );

        repo.finalize_batch(
            batch,
            TX_HASH,
            BLOCK,
            1,
            BLOCK_TIMESTAMP,
            &[(member.id.0, outcome)],
        )
        .await
        .unwrap();
        let mut rows = ledger(&pool, member.id.0).await;
        // Both rows are written in one transaction, so `created_at` cannot
        // order them.
        rows.sort_by(|left, right| left.reason.cmp(&right.reason));
        let rows: Vec<_> = rows
            .iter()
            .map(|row| {
                (
                    row.reason.as_str(),
                    row.amount.as_str(),
                    row.transaction_hash.clone(),
                    row.block_number,
                    row.recovered_at.timestamp(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "late_transfer",
                    "30",
                    TX_HASH.to_vec(),
                    BLOCK as i64,
                    BLOCK_TIMESTAMP as i64
                ),
                (
                    "overpayment",
                    "25",
                    SETTLEMENT_TX_HASH.to_vec(),
                    SETTLEMENT_BLOCK as i64,
                    SETTLEMENT_TIMESTAMP as i64
                ),
            ]
        );
        let row = repo.find_by_id(member.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "fulfilled");
        assert_eq!(row.execute_tx_hash, None);
        assert_eq!(row.settlement_tx_hash, Some(SETTLEMENT_TX_HASH.to_vec()));
        let events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM webhook_events WHERE invoice_id = $1 AND event_type = 'payment.recovered_funds'",
        )
        .bind(member.id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(events, 2, "one recovery event per ledger row");
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn replayed_receipt_does_not_duplicate_recovery(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice(&pool, 100).await;
        let batch = open_batch(&repo, &[invoice.id.0]).await;
        let outcomes = [(invoice.id.0, drained(Some(overpayment(50))))];

        repo.finalize_batch(batch, TX_HASH, BLOCK, 1, BLOCK_TIMESTAMP, &outcomes)
            .await
            .unwrap();
        assert_eq!(ledger(&pool, invoice.id.0).await.len(), 1);

        // Re-applying the receipt to the closed batch is refused outright.
        let error = repo
            .finalize_batch(batch, TX_HASH, BLOCK, 1, BLOCK_TIMESTAMP, &outcomes)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not part of sweep batch"));
        assert_eq!(ledger(&pool, invoice.id.0).await.len(), 1);

        // Should the same transaction ever be applied through a later batch,
        // the ledger's uniqueness guard keeps the first record.
        let replay = open_batch(&repo, &[invoice.id.0]).await;
        repo.finalize_batch(replay, TX_HASH, BLOCK, 1, BLOCK_TIMESTAMP, &outcomes)
            .await
            .unwrap();
        let rows = ledger(&pool, invoice.id.0).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].amount, "50");
        assert_eq!(open_batches(&pool).await, 0);
    }
}
