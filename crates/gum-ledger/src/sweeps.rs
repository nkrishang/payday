//! Sweep jobs: the ledger side of the deposit-request settlement workflow.
//!
//! The invoice table is the durable queue: every request with uncollected
//! funds at its payment address is eligible, whatever its status, because
//! the helper contract settles, recovers, or collects as appropriate. The
//! scheduler ([`InvoiceRepository::schedule_sweep_job`]) turns a slice of
//! that queue into a `sweep_jobs` row plus a `SweepBatch` command on the bus
//! in one transaction (the outbox), so a job exists exactly when its command
//! does. Membership is the invoice's `sweep_job_id`; a request in a job is
//! never scheduled twice.
//!
//! `gum-signers` executes the command and reports *evidence*
//! ([`gum_contracts::ExecutionEvent`]). The `apply_*` functions here are the
//! only code that turns that evidence into lifecycle transitions, so the
//! policy (what settles, what returns to the queue, what needs an operator)
//! lives in one place, and every transition is applied inside the caller's
//! transaction together with the bus acknowledgement, which is what makes
//! at-least-once delivery safe.

use alloy_primitives::{B256, U256};
use gum_bus::Publisher;
use gum_contracts::{
    AbandonReason, CorrelationId, ExecutionCommand, SettlementEvidence, SweepBatchCommand,
    SweepFailureCause, SweepItem, SweepItemOutcome, SweepItemResult, SweepItemStatus,
};
use gum_core::{Invoice, InvoiceStatus};
use sqlx::PgConnection;
use sqlx::types::chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::invoices::{DbInvoice, InvoiceRepository};

/// Statuses whose uncollected funds the scheduler moves automatically.
/// `created` is excluded because a partial balance cannot be settled before
/// expiry.
pub const SWEEPABLE_STATUSES: [InvoiceStatus; 4] = [
    InvoiceStatus::Funded,
    InvoiceStatus::Expired,
    InvoiceStatus::Fulfilled,
    InvoiceStatus::Recovered,
];

/// The attention code a request receives when its sweep failed for an
/// unexplained reason more times than the policy allows.
pub const RETRIES_EXHAUSTED: &str = "retries_exhausted";
/// The attention code for a collected item whose third-party settlement the
/// signers could not describe; the ledger cannot decide the lifecycle
/// status without it.
pub const SETTLEMENT_UNKNOWN: &str = "settlement_unknown";

/// Scheduler tunables. The backoff gates when a request that failed before
/// may be scheduled again: `base * 2^attempts` seconds, capped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SweepPolicy {
    pub batch_size: i64,
    pub backoff_base_secs: f64,
    pub backoff_cap_secs: f64,
    /// Unexplained item failures tolerated before the request is flagged
    /// for an operator.
    pub max_attempts: u32,
}

impl Default for SweepPolicy {
    fn default() -> Self {
        Self {
            batch_size: 20,
            backoff_base_secs: 30.0,
            backoff_cap_secs: 3_600.0,
            max_attempts: 5,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SweepError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Bus(#[from] gum_bus::BusError),
    /// A queued request has no payer binding; the schema forbids this
    /// (`invoices_payment_address_requires_binding`), so it is a bug.
    #[error("invoice {0} is queued without a payer wallet binding")]
    Unbound(Uuid),
    #[error("invoice {0} cannot be decoded: {1}")]
    Decode(Uuid, String),
    /// The job is not open (already resolved or unknown). Applying evidence
    /// twice is a duplicate delivery, not an error, so callers treat this as
    /// "nothing to do".
    #[error("sweep job {0} is not open")]
    NotOpen(Uuid),
    /// Evidence names a request that is not part of the job.
    #[error("invoice {invoice_id} is not part of sweep job {job_id}")]
    ForeignItem { job_id: Uuid, invoice_id: Uuid },
    /// Evidence omits a request that is part of the job.
    #[error("{count} invoices of sweep job {job_id} have no outcome")]
    MissingOutcomes { job_id: Uuid, count: i64 },
}

/// A job the scheduler just opened. The command is already on the bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledSweepJob {
    pub job_id: Uuid,
    pub correlation_id: CorrelationId,
    pub invoice_ids: Vec<Uuid>,
}

/// One open or resolved job, as `/v1/status` and the reconciler see it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SweepJobRow {
    pub id: Uuid,
    pub chain_id: i64,
    pub correlation_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub submitted_tx_hash: Option<Vec<u8>>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub resolution: Option<String>,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// What applying a finalized receipt did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppliedSweep {
    pub settled: Vec<Uuid>,
    pub returned: Vec<Uuid>,
    pub collected: Vec<Uuid>,
    pub requeued: Vec<Uuid>,
    pub attention: Vec<(Uuid, String)>,
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
/// behalf, keyed by the chain transaction that moved it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecoveredFunds {
    amount: U256,
    reason: RecoveryReason,
    tx_hash: B256,
    block: u64,
    block_timestamp: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SweepQueueStats {
    pub queued: i64,
    pub in_flight: i64,
    /// Age of the oldest uncollected transfer that automation is responsible for.
    pub oldest_uncollected_secs: Option<f64>,
}

/// The finalized receipt of a job's helper transaction, as the signers
/// reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizedSweep<'a> {
    pub job_id: Uuid,
    pub tx_hash: B256,
    pub block: u64,
    pub block_hash: B256,
    pub block_timestamp: u64,
    pub transaction_index: u64,
    pub items: &'a [SweepItemResult],
}

/// The lifecycle effect of one item's evidence, decided by [`decide`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum ItemDecision {
    /// The payment address was emptied in this transaction. `status` moves an
    /// open invoice to its terminal state; `None` keeps a terminal status when
    /// only late funds were collected. `execute_tx` records the job's
    /// transaction as the one that deployed the payment contract.
    Drained {
        status: Option<InvoiceStatus>,
        execute_tx: bool,
        settlement_tx_hash: B256,
        settlement_block: u64,
        settlement_transaction_index: u64,
        settlement_timestamp: u64,
        recoveries: Vec<RecoveredFunds>,
        attention: Option<&'static str>,
    },
    /// The item failed for a reason that may clear; it returns to the queue
    /// behind exponential backoff.
    Retry,
    /// The item failed for a reason automation cannot fix; the request keeps
    /// its status and gains an `attention_reason` that removes it from the
    /// queue until an operator clears it.
    Attention(String),
}

/// Policy: what one item's evidence does to its request. Pure so it can be
/// tested without a database.
fn decide(
    row: &DbInvoice,
    receipt: &FinalizedSweep<'_>,
    outcome: &SweepItemOutcome,
    policy: &SweepPolicy,
) -> ItemDecision {
    let terminal = matches!(row.status.as_str(), "fulfilled" | "recovered");
    let recovered = |amount: U256, reason, tx_hash, block, block_timestamp| {
        (!amount.is_zero()).then_some(RecoveredFunds {
            amount,
            reason,
            tx_hash,
            block,
            block_timestamp,
        })
    };
    let own = |amount, reason| {
        recovered(
            amount,
            reason,
            receipt.tx_hash,
            receipt.block,
            receipt.block_timestamp,
        )
    };
    match outcome {
        SweepItemOutcome::Settled {
            overpayment_recovered,
        } => ItemDecision::Drained {
            status: Some(InvoiceStatus::Fulfilled),
            execute_tx: true,
            settlement_tx_hash: receipt.tx_hash,
            settlement_block: receipt.block,
            settlement_transaction_index: receipt.transaction_index,
            settlement_timestamp: receipt.block_timestamp,
            recoveries: own(*overpayment_recovered, RecoveryReason::Overpayment)
                .into_iter()
                .collect(),
            attention: None,
        },
        SweepItemOutcome::Returned { amount } => ItemDecision::Drained {
            status: Some(InvoiceStatus::Recovered),
            execute_tx: true,
            settlement_tx_hash: receipt.tx_hash,
            settlement_block: receipt.block,
            settlement_transaction_index: receipt.transaction_index,
            settlement_timestamp: receipt.block_timestamp,
            recoveries: own(*amount, RecoveryReason::Expired).into_iter().collect(),
            attention: None,
        },
        SweepItemOutcome::LateCollected { amount, settlement } => {
            let late = own(*amount, RecoveryReason::LateTransfer);
            match settlement {
                // A terminal request's settlement was ledgered when it
                // resolved. The receipt fields only backfill behind COALESCE,
                // so passing the job's own transaction changes nothing.
                None if terminal || row.settlement_tx_hash.is_some() => ItemDecision::Drained {
                    status: None,
                    execute_tx: false,
                    settlement_tx_hash: receipt.tx_hash,
                    settlement_block: receipt.block,
                    settlement_transaction_index: receipt.transaction_index,
                    settlement_timestamp: receipt.block_timestamp,
                    recoveries: late.into_iter().collect(),
                    attention: None,
                },
                // Somebody deployed the contract for an open request and the
                // signers could not say how it routed the balance. The funds
                // are safe (collected), but the lifecycle status is unknown.
                None => ItemDecision::Drained {
                    status: None,
                    execute_tx: false,
                    settlement_tx_hash: receipt.tx_hash,
                    settlement_block: receipt.block,
                    settlement_transaction_index: receipt.transaction_index,
                    settlement_timestamp: receipt.block_timestamp,
                    recoveries: late.into_iter().collect(),
                    attention: Some(SETTLEMENT_UNKNOWN),
                },
                // A third party deployed the contract; its constructor
                // settled (or, after expiry, returned) whatever it found.
                Some(SettlementEvidence {
                    tx_hash,
                    block,
                    transaction_index,
                    block_timestamp,
                    settled,
                    recovered: settlement_recovered,
                }) => {
                    let status = if terminal {
                        None
                    } else if settled.is_some() {
                        Some(InvoiceStatus::Fulfilled)
                    } else {
                        Some(InvoiceStatus::Recovered)
                    };
                    let reason = if settled.is_some() {
                        RecoveryReason::Overpayment
                    } else {
                        RecoveryReason::Expired
                    };
                    let settlement_recovery = (!terminal)
                        .then(|| {
                            recovered(
                                *settlement_recovered,
                                reason,
                                *tx_hash,
                                *block,
                                *block_timestamp,
                            )
                        })
                        .flatten();
                    ItemDecision::Drained {
                        status,
                        execute_tx: false,
                        settlement_tx_hash: *tx_hash,
                        settlement_block: *block,
                        settlement_transaction_index: *transaction_index,
                        settlement_timestamp: *block_timestamp,
                        recoveries: settlement_recovery.into_iter().chain(late).collect(),
                        attention: None,
                    }
                }
            }
        }
        SweepItemOutcome::Failed { cause } => match cause {
            SweepFailureCause::TokenPaused => ItemDecision::Retry,
            SweepFailureCause::PaymentAddressBlacklisted
            | SweepFailureCause::RecoveryBlacklisted
            | SweepFailureCause::BeneficiaryBlacklisted
            | SweepFailureCause::BalanceBelowAmount => {
                ItemDecision::Attention(cause.as_str().to_string())
            }
            SweepFailureCause::Unknown => {
                if (row.sweep_attempts.max(0) as u32).saturating_add(1) >= policy.max_attempts {
                    ItemDecision::Attention(RETRIES_EXHAUSTED.to_string())
                } else {
                    ItemDecision::Retry
                }
            }
        },
    }
}

fn sweep_item(row: &DbInvoice, first_observed_block: Option<u64>) -> Result<SweepItem, SweepError> {
    let invoice =
        Invoice::try_from(row).map_err(|error| SweepError::Decode(row.id, error.to_string()))?;
    let binding = invoice
        .binding
        .as_ref()
        .ok_or(SweepError::Unbound(row.id))?;
    Ok(SweepItem {
        invoice_id: row.id,
        payment_address: binding.payment_address.0,
        currency: invoice.currency(),
        token: binding.network.token.0,
        amount: invoice.amount.0,
        receiver: invoice.beneficiary.0,
        expiration_timestamp: invoice.expiration_timestamp,
        recovery: binding.recovery.0,
        salt: binding.salt.0,
        status: if invoice.status.is_terminal() {
            SweepItemStatus::Closed
        } else {
            SweepItemStatus::Open
        },
        settlement_recorded: row.settlement_tx_hash.is_some(),
        first_observed_block,
    })
}

/// The eligibility predicate shared by the scheduler and the queue stats.
/// A live funded request is eligible only when it attaches no identity
/// add-ons or its verification has completed, and only while the finalized
/// chain clock has not passed its deadline (product plan §4.8). Closed
/// requests (`expired`, `fulfilled`, `recovered`) are always eligible so
/// their balance reaches recovery whatever the verification state.
const ELIGIBLE: &str = r#"
    invoice.uncollected_count > 0
    AND invoice.sweep_job_id IS NULL
    AND invoice.attention_reason IS NULL
    AND (
        invoice.status IN ('expired', 'fulfilled', 'recovered')
        OR (
            invoice.status = 'funded'
            AND (
                (invoice.expected_email IS NULL AND invoice.payer_reference IS NULL)
                OR invoice.verification_completed_at IS NOT NULL
            )
            AND invoice.expiration_timestamp >= COALESCE(
                (SELECT last_block_timestamp FROM indexer_cursor WHERE chain_id = invoice.chain_id),
                0
            )
        )
    )
"#;

impl InvoiceRepository {
    /// Open one sweep job for the chain if any request is eligible: claim
    /// the requests, write the job and its items, and publish the
    /// `SweepBatch` command, all in one transaction. Returns `None` when the
    /// queue is empty, the chain has an open fault, or every eligible request
    /// is still in backoff.
    pub async fn schedule_sweep_job(
        &self,
        chain_id: u64,
        policy: &SweepPolicy,
        publisher: &Publisher,
    ) -> Result<Option<ScheduledSweepJob>, SweepError> {
        let mut tx = self.pool().begin().await?;
        let halted: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM chain_faults WHERE chain_id = $1 AND cleared_at IS NULL)",
        )
        .bind(chain_id as i64)
        .fetch_one(&mut *tx)
        .await?;
        if halted {
            return Ok(None);
        }
        // The job row first: the invoices' `sweep_job_id` references it. If
        // nothing is eligible the whole transaction rolls back.
        let job_id = Uuid::now_v7();
        let correlation_id = CorrelationId::new();
        sqlx::query("INSERT INTO sweep_jobs (id, chain_id, correlation_id) VALUES ($1, $2, $3)")
            .bind(job_id)
            .bind(chain_id as i64)
            .bind(correlation_id.0)
            .execute(&mut *tx)
            .await?;
        let rows = sqlx::query_as::<_, DbInvoice>(sqlx::AssertSqlSafe(format!(
            r#"
            WITH candidates AS (
                SELECT id
                FROM invoices AS invoice
                WHERE invoice.chain_id = $1
                  AND {ELIGIBLE}
                  AND (
                    invoice.last_attempt_at IS NULL
                    OR now() >= invoice.last_attempt_at
                        + make_interval(secs => LEAST($3 * pow(2, invoice.sweep_attempts), $4))
                  )
                ORDER BY invoice.id
                FOR UPDATE SKIP LOCKED
                LIMIT $2
            )
            UPDATE invoices AS invoice
            SET sweep_job_id = $5,
                last_attempt_at = now(),
                updated_at = now()
            FROM candidates
            WHERE invoice.id = candidates.id
            RETURNING invoice.*
            "#
        )))
        .bind(chain_id as i64)
        .bind(policy.batch_size)
        .bind(policy.backoff_base_secs)
        .bind(policy.backoff_cap_secs)
        .bind(job_id)
        .fetch_all(&mut *tx)
        .await?;
        if rows.is_empty() {
            tx.rollback().await?;
            return Ok(None);
        }

        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            let first_observed_block: Option<i64> = sqlx::query_scalar(
                "SELECT min(block_number) FROM payment_observations WHERE invoice_id = $1 AND disposition <> 'error'",
            )
            .bind(row.id)
            .fetch_one(&mut *tx)
            .await?;
            items.push(sweep_item(row, first_observed_block.map(|b| b as u64))?);
            sqlx::query(
                "INSERT INTO sweep_job_items (job_id, invoice_id, attempt) VALUES ($1, $2, $3)",
            )
            .bind(job_id)
            .bind(row.id)
            .bind(row.sweep_attempts)
            .execute(&mut *tx)
            .await?;
        }

        publisher
            .publish(
                &mut tx,
                &ExecutionCommand::SweepBatch(SweepBatchCommand {
                    job_id,
                    chain_id,
                    items,
                }),
                correlation_id,
                None,
            )
            .await?;
        tx.commit().await?;
        Ok(Some(ScheduledSweepJob {
            job_id,
            correlation_id,
            invoice_ids: rows.into_iter().map(|row| row.id).collect(),
        }))
    }

    /// Remove a request from automation with a reason, outside any job. The
    /// status is unchanged; the merchant is notified once (the trigger on
    /// `attention_reason` enqueues the webhook, this enqueues the email).
    pub async fn flag_attention(&self, id: Uuid, reason: &str) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let flagged = flag_attention_in(&mut tx, id, reason).await?;
        tx.commit().await?;
        Ok(flagged)
    }

    /// The signers broadcast a helper transaction for the job. Informational.
    pub async fn record_sweep_submission(
        &self,
        conn: &mut PgConnection,
        job_id: Uuid,
        tx_hash: B256,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE sweep_jobs SET submitted_tx_hash = $2, submitted_at = now() WHERE id = $1 AND resolved_at IS NULL",
        )
        .bind(job_id)
        .bind(tx_hash.as_slice())
        .execute(conn)
        .await
        .map(drop)
    }

    /// Apply a finalized helper-transaction receipt inside `conn`'s
    /// transaction. Every request of the job must have an outcome. Drained
    /// requests have observations before the receipt's exact transaction
    /// position marked collected and their queue count recomputed from the
    /// ledger, so later indexing cannot re-queue funds the sweep already
    /// moved. Every nonzero recovery is appended to `recovered_funds` in the
    /// same transaction, stamped with its own block's timestamp, so the
    /// ledger can never disagree with the invoice state it was derived from.
    ///
    /// Returns `Err(NotOpen)` when the job was already resolved: a duplicate
    /// delivery of the same event, which the caller acknowledges as done.
    pub async fn apply_sweep_finalized(
        &self,
        conn: &mut PgConnection,
        receipt: &FinalizedSweep<'_>,
        policy: &SweepPolicy,
    ) -> Result<AppliedSweep, SweepError> {
        let job_id = receipt.job_id;
        let open = sqlx::query_scalar::<_, bool>(
            "SELECT resolved_at IS NULL FROM sweep_jobs WHERE id = $1 FOR UPDATE",
        )
        .bind(job_id)
        .fetch_optional(&mut *conn)
        .await?;
        if open != Some(true) {
            return Err(SweepError::NotOpen(job_id));
        }

        let mut applied = AppliedSweep::default();
        for SweepItemResult {
            invoice_id,
            outcome,
        } in receipt.items
        {
            let invoice_id = *invoice_id;
            let row = sqlx::query_as::<_, DbInvoice>(
                "SELECT * FROM invoices WHERE id = $1 AND sweep_job_id = $2 FOR UPDATE",
            )
            .bind(invoice_id)
            .bind(job_id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(SweepError::ForeignItem { job_id, invoice_id })?;

            match decide(&row, receipt, outcome, policy) {
                ItemDecision::Drained {
                    status,
                    execute_tx,
                    settlement_tx_hash,
                    settlement_block,
                    settlement_transaction_index,
                    settlement_timestamp,
                    recoveries,
                    attention,
                } => {
                    let (classification_block, classification_transaction_index) = row
                        .resolved_at_block
                        .zip(row.settlement_transaction_index)
                        .unwrap_or((settlement_block as i64, settlement_transaction_index as i64));
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
                    .bind(receipt.block as i64)
                    .bind(receipt.transaction_index as i64)
                    .execute(&mut *conn)
                    .await?;
                    // Classification is a function of chain order, not of
                    // whether observations or execution evidence arrived at
                    // the server first. Reconcile every existing transfer to
                    // the initial settlement position before recomputing the
                    // request aggregates and proof read model.
                    sqlx::query(
                        r#"
                        UPDATE payment_observations
                        SET disposition = CASE
                                WHEN block_timestamp <= $4
                                 AND (block_number, transaction_index) < ($2, $3)
                                THEN 'credited' ELSE 'late' END,
                            disposition_reason = CASE
                                WHEN block_timestamp <= $4
                                 AND (block_number, transaction_index) < ($2, $3)
                                THEN NULL
                                WHEN block_timestamp > $4 THEN 'invoice_expired'
                                ELSE $5 END
                        WHERE invoice_id = $1 AND disposition <> 'error'
                        "#,
                    )
                    .bind(invoice_id)
                    .bind(classification_block)
                    .bind(classification_transaction_index)
                    .bind(row.expiration_timestamp)
                    .bind(status.map_or_else(
                        || format!("invoice_{}", row.status),
                        |value| format!("invoice_{}", value.as_str()),
                    ))
                    .execute(&mut *conn)
                    .await?;
                    sqlx::query(
                        r#"
                        UPDATE invoices
                        SET status = COALESCE($4, status),
                            execute_tx_hash = CASE WHEN $5 THEN $6 ELSE execute_tx_hash END,
                            settlement_tx_hash = COALESCE(settlement_tx_hash, $7),
                            settlement_transaction_index = COALESCE(settlement_transaction_index, $10),
                            resolved_at_block = CASE WHEN $4 IS NULL THEN resolved_at_block
                                                     ELSE COALESCE(resolved_at_block, $8) END,
                            settled_at = CASE WHEN $4 IS NULL THEN settled_at
                                              ELSE COALESCE(settled_at, to_timestamp($9)) END,
                            drained_at_block = $2,
                            drained_at_transaction_index = $3,
                            uncollected_count = (
                                SELECT count(*) FROM payment_observations
                                WHERE invoice_id = $1
                                  AND collected_at_block IS NULL
                                  AND disposition <> 'error'
                            ),
                            confirmed_received = COALESCE((
                                SELECT sum(amount::numeric)::text FROM payment_observations
                                WHERE invoice_id = $1 AND disposition = 'credited'
                            ), '0'),
                            sweep_job_id = NULL,
                            sweep_attempts = 0,
                            last_attempt_at = NULL,
                            updated_at = now()
                        WHERE id = $1
                        "#,
                    )
                    .bind(invoice_id)
                    .bind(receipt.block as i64)
                    .bind(receipt.transaction_index as i64)
                    .bind(status.map(|status| status.as_str()))
                    .bind(execute_tx)
                    .bind(receipt.tx_hash.as_slice())
                    .bind(settlement_tx_hash.as_slice())
                    .bind(settlement_block as i64)
                    .bind(settlement_timestamp as f64)
                    .bind(settlement_transaction_index as i64)
                    .execute(&mut *conn)
                    .await?;
                    for recovery in &recoveries {
                        // Keyed by the chain transaction that moved the
                        // funds. A receipt is applied at most once per job,
                        // so the conflict target only fires if the same
                        // transaction is replayed through another job; the
                        // ledger keeps its first record.
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
                        .bind(row.chain_id)
                        .bind(row.token_address.as_deref())
                        .bind(recovery.tx_hash.as_slice())
                        .bind(recovery.block as i64)
                        .bind(recovery.amount.to_string())
                        .bind(recovery.reason.as_str())
                        .bind(recovery.block_timestamp as f64)
                        .execute(&mut *conn)
                        .await?;
                    }
                    if let Some(reason) = attention {
                        flag_attention_in(conn, invoice_id, reason).await?;
                        applied.attention.push((invoice_id, reason.to_string()));
                    }
                    match status {
                        Some(InvoiceStatus::Fulfilled) => applied.settled.push(invoice_id),
                        Some(InvoiceStatus::Recovered) => applied.returned.push(invoice_id),
                        _ => applied.collected.push(invoice_id),
                    }
                }
                ItemDecision::Retry => {
                    requeue(&mut *conn, invoice_id).await?;
                    applied.requeued.push(invoice_id);
                }
                ItemDecision::Attention(reason) => {
                    requeue(&mut *conn, invoice_id).await?;
                    flag_attention_in(conn, invoice_id, &reason).await?;
                    applied.attention.push((invoice_id, reason));
                }
            }
            sqlx::query(
                "UPDATE sweep_job_items SET outcome = $3, outcome_detail = $4 WHERE job_id = $1 AND invoice_id = $2",
            )
            .bind(job_id)
            .bind(invoice_id)
            .bind(outcome_kind(outcome))
            .bind(serde_json::to_value(outcome).expect("outcome serializes"))
            .execute(&mut *conn)
            .await?;
        }

        let orphaned: i64 =
            sqlx::query_scalar("SELECT count(*) FROM invoices WHERE sweep_job_id = $1")
                .bind(job_id)
                .fetch_one(&mut *conn)
                .await?;
        if orphaned != 0 {
            return Err(SweepError::MissingOutcomes {
                job_id,
                count: orphaned,
            });
        }
        resolve_job(conn, job_id, "finalized", None, Some(receipt.tx_hash)).await?;
        Ok(applied)
    }

    /// The signers gave up on the job without a successful finalized
    /// receipt; every request returns to the queue behind backoff. Returns
    /// the number re-queued, or `Err(NotOpen)` on a duplicate delivery.
    pub async fn apply_sweep_abandoned(
        &self,
        conn: &mut PgConnection,
        job_id: Uuid,
        reason: &AbandonReason,
    ) -> Result<u64, SweepError> {
        let detail = serde_json::to_value(reason).expect("abandon reason serializes");
        let tx_hash = match reason {
            AbandonReason::Reverted { tx_hash } => Some(*tx_hash),
            _ => None,
        };
        self.close_without_effect(conn, job_id, "abandoned", detail, tx_hash)
            .await
    }

    /// The signers refused the command outright (a bug or a misconfiguration,
    /// never the chain). The requests return to the queue behind backoff so
    /// a fixed deployment picks them up; the reason is kept on the job.
    pub async fn apply_sweep_rejected(
        &self,
        conn: &mut PgConnection,
        job_id: Uuid,
        reason: &str,
    ) -> Result<u64, SweepError> {
        self.close_without_effect(
            conn,
            job_id,
            "rejected",
            serde_json::json!({ "reason": reason }),
            None,
        )
        .await
    }

    async fn close_without_effect(
        &self,
        conn: &mut PgConnection,
        job_id: Uuid,
        resolution: &str,
        detail: serde_json::Value,
        tx_hash: Option<B256>,
    ) -> Result<u64, SweepError> {
        let open = sqlx::query_scalar::<_, bool>(
            "SELECT resolved_at IS NULL FROM sweep_jobs WHERE id = $1 FOR UPDATE",
        )
        .bind(job_id)
        .fetch_optional(&mut *conn)
        .await?;
        if open != Some(true) {
            return Err(SweepError::NotOpen(job_id));
        }
        let requeued = sqlx::query(
            r#"
            UPDATE invoices
            SET sweep_job_id = NULL,
                sweep_attempts = sweep_attempts + 1,
                last_attempt_at = now(),
                updated_at = now()
            WHERE sweep_job_id = $1
            "#,
        )
        .bind(job_id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        sqlx::query("UPDATE sweep_job_items SET outcome = $2 WHERE job_id = $1")
            .bind(job_id)
            .bind(resolution)
            .execute(&mut *conn)
            .await?;
        resolve_job(conn, job_id, resolution, Some(detail), tx_hash).await?;
        Ok(requeued)
    }

    /// Jobs the signers have not resolved yet, oldest first.
    pub async fn open_sweep_jobs(&self, chain_id: u64) -> Result<Vec<SweepJobRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT * FROM sweep_jobs WHERE chain_id = $1 AND resolved_at IS NULL ORDER BY created_at",
        )
        .bind(chain_id as i64)
        .fetch_all(self.pool())
        .await
    }

    pub async fn sweep_job(&self, job_id: Uuid) -> Result<Option<SweepJobRow>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM sweep_jobs WHERE id = $1")
            .bind(job_id)
            .fetch_optional(self.pool())
            .await
    }

    pub async fn sweep_queue_stats(&self, chain_id: u64) -> Result<SweepQueueStats, sqlx::Error> {
        let (queued, in_flight, oldest_uncollected_secs): (i64, i64, Option<f64>) =
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                r#"
                SELECT
                    (SELECT count(*) FROM invoices AS invoice
                      WHERE invoice.chain_id = $1 AND {ELIGIBLE}),
                    (SELECT count(*) FROM invoices WHERE chain_id = $1 AND sweep_job_id IS NOT NULL),
                    (SELECT EXTRACT(EPOCH FROM now() - min(o.observed_at))::float8
                       FROM payment_observations o
                       JOIN invoices AS invoice ON invoice.id = o.invoice_id
                      WHERE invoice.chain_id = $1
                        AND o.collected_at_block IS NULL AND o.disposition <> 'error'
                        AND invoice.attention_reason IS NULL
                        AND (invoice.sweep_job_id IS NOT NULL OR ({ELIGIBLE})))
                "#
            )))
            .bind(chain_id as i64)
            .fetch_one(self.pool())
            .await?;
        Ok(SweepQueueStats {
            queued,
            in_flight,
            oldest_uncollected_secs,
        })
    }
}

fn outcome_kind(outcome: &SweepItemOutcome) -> &'static str {
    match outcome {
        SweepItemOutcome::Settled { .. } => "settled",
        SweepItemOutcome::Returned { .. } => "returned",
        SweepItemOutcome::LateCollected { .. } => "late_collected",
        SweepItemOutcome::Failed { .. } => "failed",
    }
}

async fn requeue(conn: &mut PgConnection, invoice_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE invoices
        SET sweep_job_id = NULL,
            sweep_attempts = sweep_attempts + 1,
            last_attempt_at = now(),
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(invoice_id)
    .execute(conn)
    .await
    .map(drop)
}

async fn resolve_job(
    conn: &mut PgConnection,
    job_id: Uuid,
    resolution: &str,
    detail: Option<serde_json::Value>,
    tx_hash: Option<B256>,
) -> Result<(), SweepError> {
    let closed = sqlx::query(
        r#"
        UPDATE sweep_jobs
        SET resolution = $2, resolved_at = now(), resolution_detail = $3,
            submitted_tx_hash = COALESCE($4, submitted_tx_hash)
        WHERE id = $1 AND resolved_at IS NULL
        "#,
    )
    .bind(job_id)
    .bind(resolution)
    .bind(detail)
    .bind(tx_hash.map(|hash| hash.to_vec()))
    .execute(conn)
    .await?
    .rows_affected();
    if closed != 1 {
        return Err(SweepError::NotOpen(job_id));
    }
    Ok(())
}

/// Set `attention_reason` on a request that has none and enqueue the
/// merchant email. Returns whether the row changed.
async fn flag_attention_in(
    conn: &mut PgConnection,
    id: Uuid,
    reason: &str,
) -> Result<bool, sqlx::Error> {
    let account: Option<Uuid> = sqlx::query_scalar(
        r#"
        UPDATE invoices
        SET attention_reason = $2, updated_at = now()
        WHERE id = $1 AND attention_reason IS NULL
        RETURNING account_id
        "#,
    )
    .bind(id)
    .bind(reason)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(account) = account {
        sqlx::query("INSERT INTO notification_outbox(id,account_id,invoice_id,reason,email) SELECT $1,$2,$3,$4,email FROM accounts WHERE id=$2")
            .bind(Uuid::now_v7()).bind(account).bind(id).bind(reason)
            .execute(&mut *conn).await?;
    }
    Ok(account.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloy_primitives::{Address, U256, address};
    use gum_core::{
        Amount, BeneficiaryAddress, CanonicalIssuanceSnapshot, Currency, Invoice, Party,
        PayerVerification, RecoveryAddress,
    };
    use sqlx::PgPool;

    use crate::invoices::tests::TEST_RECOVERY;
    use crate::{AccountId, CreateInvoiceInput};

    const CHAIN_ID: u64 = 31337;
    const TX_HASH: B256 = B256::repeat_byte(0xA1);
    const BLOCK: u64 = 7;
    const BLOCK_HASH: B256 = B256::repeat_byte(0x07);
    const BLOCK_TIMESTAMP: u64 = 1_800_000_070;
    /// A deployment somebody other than this service mined, four blocks earlier.
    const SETTLEMENT_TX_HASH: B256 = B256::repeat_byte(0xB2);
    const SETTLEMENT_BLOCK: u64 = 3;
    const SETTLEMENT_TIMESTAMP: u64 = 1_800_000_030;
    /// The invoice's deadline, as `insert_invoice_with` issues it.
    const EXPIRATION: u64 = 1_900_000_000;

    const PUBLISHER: Publisher = Publisher::new("test");

    fn policy() -> SweepPolicy {
        SweepPolicy {
            batch_size: 10,
            backoff_base_secs: 1.0,
            backoff_cap_secs: 60.0,
            max_attempts: 3,
        }
    }

    async fn insert_invoice(pool: &PgPool, amount: u64) -> Invoice {
        insert_invoice_with(pool, amount, PayerVerification::default()).await
    }

    async fn insert_invoice_with(
        pool: &PgPool,
        amount: u64,
        verification: PayerVerification,
    ) -> Invoice {
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
        let networks = crate::invoices::tests::test_networks(CHAIN_ID);
        let beneficiary =
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"));
        let amount = Amount(U256::from(amount));
        let party = |name: &str| Party {
            name: name.into(),
            email: None,
            details: None,
        };
        let snapshot = CanonicalIssuanceSnapshot::new(
            party("Acme"),
            party("Globex"),
            verification,
            Currency::Usdc,
            &networks,
            beneficiary,
            RecoveryAddress(TEST_RECOVERY),
            amount,
            EXPIRATION,
        );
        let invoice = Invoice::issue(
            Currency::Usdc,
            &networks,
            beneficiary,
            RecoveryAddress(TEST_RECOVERY),
            amount,
            EXPIRATION,
            snapshot,
        )
        .unwrap();
        let input = CreateInvoiceInput::from_invoice(
            &invoice,
            AccountId(account_id),
            invoice.id.0.to_string(),
            Currency::Usdc.decimals(),
            3_600,
            "in:3600".into(),
        );
        InvoiceRepository::new(pool.clone())
            .insert_issued(&input, None)
            .await
            .expect("row should be inserted");
        let bound = crate::invoices::tests::bind_for_test(
            pool,
            invoice.id.0,
            &crate::invoices::tests::TEST_PAYER_KEY,
        )
        .await;
        Invoice::try_from(&bound).unwrap()
    }

    fn gated() -> PayerVerification {
        PayerVerification {
            email: Some(gum_core::EmailVerification {
                expected_email: "alice@example.com".into(),
            }),
            merchant_auth: None,
            wallet_attestation: false,
        }
    }

    /// Finalized transfers reached the amount and are waiting to be collected.
    async fn fund(pool: &PgPool, invoice: &Invoice) {
        set_status(pool, invoice, "funded").await;
    }

    async fn set_status(pool: &PgPool, invoice: &Invoice, status: &str) {
        sqlx::query(
            "UPDATE invoices SET status = $2, confirmed_received = amount, uncollected_count = 1 WHERE id = $1",
        )
        .bind(invoice.id.0)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
    }

    /// One credited observation at block 1 so the drain watermark has
    /// something to collect.
    async fn observe(pool: &PgPool, invoice: &Invoice, block: u64) {
        sqlx::query(
            r#"
            INSERT INTO payment_observations
                (chain_id, token_address, block_number, block_hash, block_timestamp,
                 transaction_hash, transaction_index, log_index, sender_address,
                 recipient_address, invoice_id, amount, disposition)
            VALUES ($1, $2, $3, $4, $5, $6, 0, 0, $7, $8, $9, '100', 'credited')
            "#,
        )
        .bind(CHAIN_ID as i64)
        .bind(address!("0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512").as_slice())
        .bind(block as i64)
        .bind(B256::with_last_byte(block as u8).as_slice())
        .bind((EXPIRATION - 120) as i64)
        .bind(B256::repeat_byte(0x22 + block as u8).as_slice())
        .bind([0x33u8; 20].as_slice())
        .bind(invoice.payment_address().unwrap().0.as_slice())
        .bind(invoice.id.0)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("UPDATE invoices SET uncollected_count = (SELECT count(*) FROM payment_observations WHERE invoice_id = $1 AND collected_at_block IS NULL) WHERE id = $1")
            .bind(invoice.id.0)
            .execute(pool)
            .await
            .unwrap();
    }

    /// The chain clock as the indexer last recorded it.
    async fn set_finalized_clock(pool: &PgPool, timestamp: u64) {
        sqlx::query(
            r#"
            INSERT INTO indexer_cursor (chain_id, last_block, last_block_hash, last_block_timestamp)
            VALUES ($1, 1, $2, $3)
            ON CONFLICT (chain_id) DO UPDATE SET last_block_timestamp = EXCLUDED.last_block_timestamp
            "#,
        )
        .bind(CHAIN_ID as i64)
        .bind([0x11u8; 32].as_slice())
        .bind(timestamp as i64)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn complete_verification(pool: &PgPool, invoice: &Invoice) {
        sqlx::query("UPDATE invoices SET verification_completed_at = now() WHERE id = $1")
            .bind(invoice.id.0)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn schedule(repo: &InvoiceRepository) -> Option<ScheduledSweepJob> {
        repo.schedule_sweep_job(CHAIN_ID, &policy(), &PUBLISHER)
            .await
            .unwrap()
    }

    async fn scheduled_ids(repo: &InvoiceRepository) -> Vec<Uuid> {
        schedule(repo)
            .await
            .map(|job| job.invoice_ids)
            .unwrap_or_default()
    }

    async fn status(pool: &PgPool, invoice: &Invoice) -> String {
        sqlx::query_scalar("SELECT status FROM invoices WHERE id = $1")
            .bind(invoice.id.0)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn published_commands(pool: &PgPool) -> Vec<ExecutionCommand> {
        let payloads: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT payload FROM bus.messages WHERE topic = 'execution.commands' ORDER BY id",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        payloads
            .into_iter()
            .map(|payload| serde_json::from_value(payload).unwrap())
            .collect()
    }

    fn receipt<'a>(job_id: Uuid, items: &'a [SweepItemResult]) -> FinalizedSweep<'a> {
        FinalizedSweep {
            job_id,
            tx_hash: TX_HASH,
            block: BLOCK,
            block_hash: BLOCK_HASH,
            block_timestamp: BLOCK_TIMESTAMP,
            transaction_index: 1,
            items,
        }
    }

    fn result(invoice: &Invoice, outcome: SweepItemOutcome) -> SweepItemResult {
        SweepItemResult {
            invoice_id: invoice.id.0,
            outcome,
        }
    }

    fn settled(overpayment: u64) -> SweepItemOutcome {
        SweepItemOutcome::Settled {
            overpayment_recovered: U256::from(overpayment),
        }
    }

    #[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
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
             FROM recovered_funds WHERE invoice_id = $1 ORDER BY reason",
        )
        .bind(invoice_id)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn open_jobs(pool: &PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM sweep_jobs WHERE resolved_at IS NULL")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn apply(
        repo: &InvoiceRepository,
        receipt: &FinalizedSweep<'_>,
    ) -> Result<AppliedSweep, SweepError> {
        let mut tx = repo.pool().begin().await.unwrap();
        let applied = repo
            .apply_sweep_finalized(&mut tx, receipt, &policy())
            .await;
        if applied.is_ok() {
            tx.commit().await.unwrap();
        }
        applied
    }

    // ---- scheduling ------------------------------------------------------

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn scheduling_writes_job_items_and_command_atomically(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice(&pool, 100).await;
        fund(&pool, &invoice).await;
        observe(&pool, &invoice, 1).await;
        set_finalized_clock(&pool, EXPIRATION - 60).await;

        let job = schedule(&repo).await.expect("one eligible request");
        assert_eq!(job.invoice_ids, [invoice.id.0]);
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.sweep_job_id, Some(job.job_id));
        assert_eq!(
            row.status, "funded",
            "membership does not change the status"
        );

        let commands = published_commands(&pool).await;
        let [ExecutionCommand::SweepBatch(command)] = commands.as_slice() else {
            panic!("expected exactly one SweepBatch command, got {commands:?}");
        };
        assert_eq!(command.job_id, job.job_id);
        assert_eq!(command.chain_id, CHAIN_ID);
        let [item] = command.items.as_slice() else {
            panic!("one item")
        };
        assert_eq!(item.invoice_id, invoice.id.0);
        assert_eq!(item.payment_address, invoice.payment_address().unwrap().0);
        assert_eq!(item.amount, U256::from(100));
        assert_eq!(item.status, SweepItemStatus::Open);
        assert!(!item.settlement_recorded);
        assert_eq!(item.first_observed_block, Some(1));
        let correlation: Uuid = sqlx::query_scalar(
            "SELECT correlation_id FROM bus.messages WHERE topic = 'execution.commands'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(correlation, job.correlation_id.0);

        // A request in a job is not scheduled again.
        assert!(schedule(&repo).await.is_none());
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn open_chain_fault_stops_scheduling(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice(&pool, 100).await;
        fund(&pool, &invoice).await;
        set_finalized_clock(&pool, EXPIRATION - 60).await;
        sqlx::query(
            "INSERT INTO chain_faults (id, chain_id, reason) VALUES ($1, $2, 'finality violation')",
        )
        .bind(Uuid::now_v7())
        .bind(CHAIN_ID as i64)
        .execute(&pool)
        .await
        .unwrap();

        assert!(schedule(&repo).await.is_none());
        assert!(published_commands(&pool).await.is_empty());

        sqlx::query("UPDATE chain_faults SET cleared_at = now()")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(scheduled_ids(&repo).await, [invoice.id.0]);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn permissionless_live_invoice_is_eligible(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice(&pool, 100).await;
        fund(&pool, &invoice).await;
        set_finalized_clock(&pool, EXPIRATION - 60).await;

        assert_eq!(scheduled_ids(&repo).await, [invoice.id.0]);
        assert_eq!(status(&pool, &invoice).await, "funded");
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn gated_live_invoice_waits_for_verification(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice_with(&pool, 100, gated()).await;
        fund(&pool, &invoice).await;
        set_finalized_clock(&pool, EXPIRATION - 60).await;

        assert!(schedule(&repo).await.is_none());
        // The funds are queued, not quarantined: the row still counts them.
        let uncollected: i32 =
            sqlx::query_scalar("SELECT uncollected_count FROM invoices WHERE id = $1")
                .bind(invoice.id.0)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(uncollected, 1);

        complete_verification(&pool, &invoice).await;
        assert_eq!(scheduled_ids(&repo).await, [invoice.id.0]);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn queue_stats_apply_the_live_verification_gate(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice_with(&pool, 100, gated()).await;
        fund(&pool, &invoice).await;
        observe(&pool, &invoice, 1).await;
        set_finalized_clock(&pool, EXPIRATION - 60).await;

        let stats = repo.sweep_queue_stats(CHAIN_ID).await.unwrap();
        assert_eq!((stats.queued, stats.in_flight), (0, 0));
        assert_eq!(stats.oldest_uncollected_secs, None);

        complete_verification(&pool, &invoice).await;
        let stats = repo.sweep_queue_stats(CHAIN_ID).await.unwrap();
        assert_eq!((stats.queued, stats.in_flight), (1, 0));
        assert!(stats.oldest_uncollected_secs.is_some());

        schedule(&repo).await.unwrap();
        let stats = repo.sweep_queue_stats(CHAIN_ID).await.unwrap();
        assert_eq!((stats.queued, stats.in_flight), (0, 1));
        assert!(
            stats.oldest_uncollected_secs.is_some(),
            "in-flight funds still age"
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn expired_unverified_invoice_is_eligible_for_recovery(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice_with(&pool, 100, gated()).await;
        set_status(&pool, &invoice, "expired").await;
        set_finalized_clock(&pool, EXPIRATION + 60).await;

        assert_eq!(scheduled_ids(&repo).await, [invoice.id.0]);
        let commands = published_commands(&pool).await;
        let ExecutionCommand::SweepBatch(command) = &commands[0] else {
            panic!()
        };
        assert_eq!(command.items[0].status, SweepItemStatus::Open);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn verification_after_expiry_does_not_create_live_claim(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice_with(&pool, 100, gated()).await;
        fund(&pool, &invoice).await;
        // The finalized clock has passed the deadline; the indexer's expiry
        // pass may not have flipped the row yet.
        set_finalized_clock(&pool, EXPIRATION + 1).await;
        complete_verification(&pool, &invoice).await;

        assert!(schedule(&repo).await.is_none());

        // Expiry, not verification, is what makes it eligible, for recovery.
        sqlx::query("UPDATE invoices SET status = 'expired' WHERE id = $1")
            .bind(invoice.id.0)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(scheduled_ids(&repo).await, [invoice.id.0]);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn backoff_gates_rescheduling_after_a_failure(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice(&pool, 100).await;
        fund(&pool, &invoice).await;
        set_finalized_clock(&pool, EXPIRATION - 60).await;
        let job = schedule(&repo).await.unwrap();

        let mut tx = pool.begin().await.unwrap();
        let requeued = repo
            .apply_sweep_abandoned(
                &mut tx,
                job.job_id,
                &AbandonReason::NonceConsumed {
                    signer: Address::repeat_byte(0xA0),
                    nonce: 3,
                },
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(requeued, 1);
        assert_eq!(open_jobs(&pool).await, 0);
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.sweep_job_id, None);
        assert_eq!(row.sweep_attempts, 1);
        assert_eq!(row.status, "funded");

        // Inside the backoff window nothing is scheduled...
        assert!(schedule(&repo).await.is_none());
        // ...and a duplicate delivery of the abandonment is recognised.
        let mut tx = pool.begin().await.unwrap();
        assert!(matches!(
            repo.apply_sweep_abandoned(
                &mut tx,
                job.job_id,
                &AbandonReason::Rejected {
                    reason: "again".into()
                }
            )
            .await,
            Err(SweepError::NotOpen(_))
        ));
        drop(tx);

        sqlx::query(
            "UPDATE invoices SET last_attempt_at = now() - interval '1 hour' WHERE id = $1",
        )
        .bind(invoice.id.0)
        .execute(&pool)
        .await
        .unwrap();
        let retry = schedule(&repo).await.unwrap();
        assert_ne!(retry.job_id, job.job_id);
        let attempt: i32 =
            sqlx::query_scalar("SELECT attempt FROM sweep_job_items WHERE job_id = $1")
                .bind(retry.job_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(attempt, 1, "the item records which attempt it is");
    }

    // ---- applying evidence ----------------------------------------------

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn finalized_receipt_settles_and_ledgers_recovery_atomically(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let member = insert_invoice(&pool, 100).await;
        let outsider = insert_invoice(&pool, 200).await;
        fund(&pool, &member).await;
        observe(&pool, &member, 1).await;
        // A transfer indexed after the receipt position stays uncollected.
        observe(&pool, &member, BLOCK + 2).await;
        set_finalized_clock(&pool, EXPIRATION - 60).await;
        let job = schedule(&repo).await.unwrap();

        // The member's ledger row is written before the outsider is rejected;
        // the rejection must take the ledger row down with it.
        let items = [result(&member, settled(50)), result(&outsider, settled(0))];
        let error = apply(&repo, &receipt(job.job_id, &items))
            .await
            .unwrap_err();
        assert!(matches!(error, SweepError::ForeignItem { .. }), "{error}");
        assert!(ledger(&pool, member.id.0).await.is_empty());
        assert_eq!(open_jobs(&pool).await, 1);
        assert_eq!(status(&pool, &member).await, "funded");

        let items = [result(&member, settled(50))];
        let applied = apply(&repo, &receipt(job.job_id, &items)).await.unwrap();
        assert_eq!(applied.settled, [member.id.0]);
        let row = repo.find_by_id(member.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "fulfilled");
        assert_eq!(row.sweep_job_id, None);
        assert_eq!(row.sweep_attempts, 0);
        assert_eq!(row.execute_tx_hash, Some(TX_HASH.to_vec()));
        assert_eq!(row.settlement_tx_hash, Some(TX_HASH.to_vec()));
        assert_eq!(row.resolved_at_block, Some(BLOCK as i64));
        assert_eq!(row.settled_at.unwrap().timestamp(), BLOCK_TIMESTAMP as i64);
        assert_eq!(row.drained_at_block, Some(BLOCK as i64));
        assert_eq!(
            row.uncollected_count, 1,
            "the later transfer is still queued"
        );

        let rows = ledger(&pool, member.id.0).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].chain_id, CHAIN_ID as i64);
        assert_eq!(
            rows[0].token_address,
            member.network().unwrap().token.0.to_vec()
        );
        assert_eq!(rows[0].transaction_hash, TX_HASH.to_vec());
        assert_eq!(rows[0].block_number, BLOCK as i64);
        assert_eq!(rows[0].amount, "50");
        assert_eq!(rows[0].reason, "overpayment");
        assert_eq!(rows[0].recovered_at.timestamp(), BLOCK_TIMESTAMP as i64);
        assert_eq!(open_jobs(&pool).await, 0);
        assert!(ledger(&pool, outsider.id.0).await.is_empty());

        let (resolution, detail): (String, Option<serde_json::Value>) =
            sqlx::query_as("SELECT outcome, outcome_detail FROM sweep_job_items WHERE job_id = $1")
                .bind(job.job_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(resolution, "settled");
        assert_eq!(detail.unwrap()["overpayment_recovered"], "0x32");

        let event: (String, serde_json::Value) = sqlx::query_as(
            "SELECT event_type, payload FROM webhook_events WHERE invoice_id = $1 AND event_type = 'deposit_request.recovered_funds'",
        )
        .bind(member.id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(event.1["data"]["recovery"]["amount_base_units"], "50");
        assert_eq!(event.1["data"]["recovery"]["reason"], "overpayment");
        assert_eq!(event.1["data"]["deposit_request"]["status"], "settled");

        // A duplicate delivery of the same receipt is a no-op the caller can
        // acknowledge.
        let items = [result(&member, settled(50))];
        assert!(matches!(
            apply(&repo, &receipt(job.job_id, &items)).await,
            Err(SweepError::NotOpen(_))
        ));
        assert_eq!(ledger(&pool, member.id.0).await.len(), 1);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn settlement_reclassifies_observations_that_arrived_first(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice(&pool, 100).await;
        fund(&pool, &invoice).await;
        observe(&pool, &invoice, 1).await;
        set_finalized_clock(&pool, EXPIRATION - 60).await;
        let job = schedule(&repo).await.unwrap();
        // This transfer is indexed before the server consumes settlement
        // evidence, but is later on chain than the settlement transaction.
        observe(&pool, &invoice, BLOCK + 2).await;

        let items = [result(&invoice, settled(0))];
        apply(&repo, &receipt(job.job_id, &items)).await.unwrap();

        let dispositions: Vec<(i64, String)> = sqlx::query_as(
            "SELECT block_number, disposition FROM payment_observations WHERE invoice_id = $1 ORDER BY block_number",
        )
        .bind(invoice.id.0)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(dispositions, [(1, "credited".into()), (9, "late".into())]);
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.confirmed_received, "100");
        assert_eq!(row.settlement_transaction_index, Some(1));
        let proof = crate::ProofRepository::new(pool)
            .settlement_transfers(invoice.id.0)
            .await
            .unwrap();
        assert_eq!(proof.len(), 1, "returned late funds are not in the proof");
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn expired_deployment_returns_funds(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice(&pool, 100).await;
        set_status(&pool, &invoice, "expired").await;
        set_finalized_clock(&pool, EXPIRATION + 60).await;
        let job = schedule(&repo).await.unwrap();

        let items = [result(
            &invoice,
            SweepItemOutcome::Returned {
                amount: U256::from(40),
            },
        )];
        let applied = apply(&repo, &receipt(job.job_id, &items)).await.unwrap();
        assert_eq!(applied.returned, [invoice.id.0]);
        assert_eq!(status(&pool, &invoice).await, "recovered");
        let rows = ledger(&pool, invoice.id.0).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].reason.as_str(), rows[0].amount.as_str()),
            ("expired", "40")
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn third_party_settlement_and_late_collection_are_ledgered_together(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice(&pool, 100).await;
        fund(&pool, &invoice).await;
        set_finalized_clock(&pool, EXPIRATION - 60).await;
        let job = schedule(&repo).await.unwrap();
        // Somebody else deployed the contract while the address held 125, so
        // their transaction sent the 25 remainder to the recovery wallet; a
        // later transfer of 30 was forwarded by this job's `recover()`.
        let items = [result(
            &invoice,
            SweepItemOutcome::LateCollected {
                amount: U256::from(30),
                settlement: Some(SettlementEvidence {
                    tx_hash: SETTLEMENT_TX_HASH,
                    block: SETTLEMENT_BLOCK,
                    transaction_index: 0,
                    block_timestamp: SETTLEMENT_TIMESTAMP,
                    settled: Some(U256::from(100)),
                    recovered: U256::from(25),
                }),
            },
        )];

        let applied = apply(&repo, &receipt(job.job_id, &items)).await.unwrap();
        assert_eq!(applied.settled, [invoice.id.0]);
        let rows: Vec<_> = ledger(&pool, invoice.id.0)
            .await
            .iter()
            .map(|row| {
                (
                    row.reason.clone(),
                    row.amount.clone(),
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
                    "late_transfer".into(),
                    "30".into(),
                    TX_HASH.to_vec(),
                    BLOCK as i64,
                    BLOCK_TIMESTAMP as i64
                ),
                (
                    "overpayment".into(),
                    "25".into(),
                    SETTLEMENT_TX_HASH.to_vec(),
                    SETTLEMENT_BLOCK as i64,
                    SETTLEMENT_TIMESTAMP as i64
                ),
            ]
        );
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "fulfilled");
        assert_eq!(row.execute_tx_hash, None, "we did not deploy it");
        assert_eq!(row.settlement_tx_hash, Some(SETTLEMENT_TX_HASH.to_vec()));
        assert_eq!(row.resolved_at_block, Some(SETTLEMENT_BLOCK as i64));
        let events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM webhook_events WHERE invoice_id = $1 AND event_type = 'deposit_request.recovered_funds'",
        )
        .bind(invoice.id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(events, 2, "one recovery event per ledger row");
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn late_funds_on_a_settled_request_keep_its_status(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice(&pool, 100).await;
        set_status(&pool, &invoice, "fulfilled").await;
        sqlx::query(
            "UPDATE invoices SET settlement_tx_hash = $2, resolved_at_block = 2 WHERE id = $1",
        )
        .bind(invoice.id.0)
        .bind(SETTLEMENT_TX_HASH.as_slice())
        .execute(&pool)
        .await
        .unwrap();
        let job = schedule(&repo).await.unwrap();
        let commands = published_commands(&pool).await;
        let ExecutionCommand::SweepBatch(command) = &commands[0] else {
            panic!()
        };
        assert_eq!(command.items[0].status, SweepItemStatus::Closed);
        assert!(command.items[0].settlement_recorded);

        let items = [result(
            &invoice,
            SweepItemOutcome::LateCollected {
                amount: U256::from(7),
                settlement: None,
            },
        )];
        let applied = apply(&repo, &receipt(job.job_id, &items)).await.unwrap();
        assert_eq!(applied.collected, [invoice.id.0]);
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "fulfilled");
        assert_eq!(row.settlement_tx_hash, Some(SETTLEMENT_TX_HASH.to_vec()));
        assert_eq!(row.resolved_at_block, Some(2));
        assert_eq!(row.attention_reason, None);
        let rows = ledger(&pool, invoice.id.0).await;
        assert_eq!(
            (rows[0].reason.as_str(), rows[0].amount.as_str()),
            ("late_transfer", "7")
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn unexplained_collection_of_an_open_request_needs_attention(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice(&pool, 100).await;
        fund(&pool, &invoice).await;
        set_finalized_clock(&pool, EXPIRATION - 60).await;
        let job = schedule(&repo).await.unwrap();

        let items = [result(
            &invoice,
            SweepItemOutcome::LateCollected {
                amount: U256::ZERO,
                settlement: None,
            },
        )];
        let applied = apply(&repo, &receipt(job.job_id, &items)).await.unwrap();
        assert_eq!(
            applied.attention,
            [(invoice.id.0, SETTLEMENT_UNKNOWN.to_string())]
        );
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "funded");
        assert_eq!(row.attention_reason.as_deref(), Some(SETTLEMENT_UNKNOWN));
        assert!(
            ledger(&pool, invoice.id.0).await.is_empty(),
            "zero is not a recovery"
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn failures_retry_or_flag_attention_by_cause(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let paused = insert_invoice(&pool, 100).await;
        let blacklisted = insert_invoice(&pool, 200).await;
        let unknown = insert_invoice(&pool, 300).await;
        for invoice in [&paused, &blacklisted, &unknown] {
            fund(&pool, invoice).await;
        }
        set_finalized_clock(&pool, EXPIRATION - 60).await;
        let job = schedule(&repo).await.unwrap();
        assert_eq!(job.invoice_ids.len(), 3);

        let items = [
            result(
                &paused,
                SweepItemOutcome::Failed {
                    cause: SweepFailureCause::TokenPaused,
                },
            ),
            result(
                &blacklisted,
                SweepItemOutcome::Failed {
                    cause: SweepFailureCause::BeneficiaryBlacklisted,
                },
            ),
            result(
                &unknown,
                SweepItemOutcome::Failed {
                    cause: SweepFailureCause::Unknown,
                },
            ),
        ];
        let applied = apply(&repo, &receipt(job.job_id, &items)).await.unwrap();
        assert_eq!(applied.requeued, [paused.id.0, unknown.id.0]);
        assert_eq!(
            applied.attention,
            [(blacklisted.id.0, "beneficiary_blacklisted".to_string())]
        );
        for invoice in [&paused, &blacklisted, &unknown] {
            let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
            assert_eq!(
                row.status, "funded",
                "failures never change the lifecycle status"
            );
            assert_eq!(row.sweep_job_id, None);
            assert_eq!(row.sweep_attempts, 1);
        }
        let row = repo.find_by_id(blacklisted.id.0).await.unwrap().unwrap();
        assert_eq!(
            row.attention_reason.as_deref(),
            Some("beneficiary_blacklisted")
        );
        let (emails, webhooks): (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM notification_outbox WHERE invoice_id=$1), (SELECT count(*) FROM webhook_events WHERE invoice_id=$1 AND event_type='deposit_request.needs_attention')",
        )
        .bind(blacklisted.id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((emails, webhooks), (1, 1));

        // The flagged request is out of the queue; the others wait out backoff.
        sqlx::query("UPDATE invoices SET last_attempt_at = now() - interval '1 hour'")
            .execute(&pool)
            .await
            .unwrap();
        let mut retry = scheduled_ids(&repo).await;
        retry.sort();
        let mut expected = vec![paused.id.0, unknown.id.0];
        expected.sort();
        assert_eq!(retry, expected);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn unexplained_failures_exhaust_retries(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice(&pool, 100).await;
        fund(&pool, &invoice).await;
        set_finalized_clock(&pool, EXPIRATION - 60).await;
        // Two earlier attempts already failed; `max_attempts` is 3.
        sqlx::query("UPDATE invoices SET sweep_attempts = 2 WHERE id = $1")
            .bind(invoice.id.0)
            .execute(&pool)
            .await
            .unwrap();
        let job = schedule(&repo).await.unwrap();
        let items = [result(
            &invoice,
            SweepItemOutcome::Failed {
                cause: SweepFailureCause::Unknown,
            },
        )];
        let applied = apply(&repo, &receipt(job.job_id, &items)).await.unwrap();
        assert_eq!(
            applied.attention,
            [(invoice.id.0, RETRIES_EXHAUSTED.to_string())]
        );
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.attention_reason.as_deref(), Some(RETRIES_EXHAUSTED));
        assert_eq!(row.sweep_attempts, 3);

        // An operator clears it and the scheduler picks it up again.
        repo.release_attention(invoice.id.0).await.unwrap();
        assert_eq!(scheduled_ids(&repo).await, [invoice.id.0]);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn receipt_must_cover_every_item(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let first = insert_invoice(&pool, 100).await;
        let second = insert_invoice(&pool, 200).await;
        fund(&pool, &first).await;
        fund(&pool, &second).await;
        set_finalized_clock(&pool, EXPIRATION - 60).await;
        let job = schedule(&repo).await.unwrap();

        let items = [result(&first, settled(0))];
        let error = apply(&repo, &receipt(job.job_id, &items))
            .await
            .unwrap_err();
        assert!(
            matches!(error, SweepError::MissingOutcomes { count: 1, .. }),
            "{error}"
        );
        assert_eq!(status(&pool, &first).await, "funded", "rolled back");
        assert_eq!(open_jobs(&pool).await, 1);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn replayed_transaction_through_another_job_does_not_duplicate_recovery(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice(&pool, 100).await;
        fund(&pool, &invoice).await;
        observe(&pool, &invoice, 1).await;
        set_finalized_clock(&pool, EXPIRATION - 60).await;
        let job = schedule(&repo).await.unwrap();
        let items = [result(&invoice, settled(50))];
        apply(&repo, &receipt(job.job_id, &items)).await.unwrap();
        assert_eq!(ledger(&pool, invoice.id.0).await.len(), 1);

        // Late funds arrive; the settled request is scheduled again and the
        // same chain transaction is (wrongly) reported once more.
        observe(&pool, &invoice, BLOCK + 5).await;
        let replay = schedule(&repo).await.unwrap();
        let items = [result(&invoice, settled(50))];
        let mut receipt = receipt(replay.job_id, &items);
        receipt.block = BLOCK + 9;
        apply(&repo, &receipt).await.unwrap();
        let rows = ledger(&pool, invoice.id.0).await;
        assert_eq!(rows.len(), 1, "the ledger keeps its first record");
        assert_eq!(rows[0].amount, "50");
        assert_eq!(open_jobs(&pool).await, 0);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn rejected_command_requeues_and_keeps_the_reason(pool: PgPool) {
        let repo = InvoiceRepository::new(pool.clone());
        let invoice = insert_invoice(&pool, 100).await;
        fund(&pool, &invoice).await;
        set_finalized_clock(&pool, EXPIRATION - 60).await;
        let job = schedule(&repo).await.unwrap();

        let mut tx = pool.begin().await.unwrap();
        let requeued = repo
            .apply_sweep_rejected(&mut tx, job.job_id, "unknown chain 31337")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(requeued, 1);
        let row = repo.sweep_job(job.job_id).await.unwrap().unwrap();
        assert_eq!(row.resolution.as_deref(), Some("rejected"));
        let detail: serde_json::Value =
            sqlx::query_scalar("SELECT resolution_detail FROM sweep_jobs WHERE id = $1")
                .bind(job.job_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(detail["reason"], "unknown chain 31337");
        assert!(repo.open_sweep_jobs(CHAIN_ID).await.unwrap().is_empty());
    }
}
