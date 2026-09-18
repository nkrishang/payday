//! One chain's executor: turns accepted jobs into transactions and
//! transactions into events.
//!
//! # Execution model
//!
//! A pass has two halves and runs on a timer or when a command arrives:
//!
//! 1. **Reconcile** every open slot against the chain: look for a receipt
//!    of any attempt, wait for finality, verify the block hash, then
//!    classify and resolve; or, unmined past the pending timeout, decide
//!    between "the nonce was consumed by something else" (abandon) and
//!    "replace with higher fees" (bounded by `max_submissions`).
//! 2. **Start** queued jobs on free signers while the chain is not halted.
//!
//! Every decision is made from rows in the `execution` schema and facts
//! read from the chain; nothing lives only in memory, so a process killed
//! anywhere resumes by simply running the next pass.
//!
//! # Crash boundaries
//!
//! * before `begin_transaction` commits: nothing was signed that the chain
//!   could have; the job is still `queued` and is started again.
//! * after `begin_transaction`, before broadcast: the signed bytes are in
//!   `transaction_attempts`; reconcile finds the newest attempt without a
//!   `broadcast_at` and resends exactly those bytes.
//! * after broadcast, before `mark_broadcast`: same as above; the node
//!   answers "already known" and the resend is a no-op.
//! * after a receipt, before resolution: the receipt is looked up again.
//! * resolution and event publication commit in one transaction: either
//!   both exist or neither.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::{Address, B256, U256};
use chrono::Utc;
use gum_bus::{BackoffPolicy, Publisher};
use gum_chain::{
    BlockHeader, ChainError, ChainExecutor, FeeEstimate, PreparedSweepTransaction, SweepReceipt,
    SweepRequest, TransactionOutcome, finality_boundary, sweep_batch_gas_limit,
};
use gum_contracts::{
    AbandonReason, CorrelationId, ExecutionCommand, ExecutionEvent, StepResult, SweepBatchCommand,
    WithdrawalStepCommand,
};
use gum_core::ChainConfig;
use sqlx::PgPool;
use tokio::sync::{Notify, watch};
use tracing::{Instrument, error, info, info_span, warn};

use crate::step::{self, Precondition};
use crate::store::{self, Attempt, Job, JobState, Mined, NewTransaction, OpenTransaction, TxState};
use crate::sweep;

/// Operational policy shared by every chain worker.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    /// How long an attempt may stay unmined before it is replaced.
    pub pending_timeout: Duration,
    /// Attempts per slot before the lane stops raising fees and alerts.
    pub max_submissions: u32,
    /// Transient failures before a transaction exists that a job survives.
    pub max_attempts: u32,
    /// Delay between those failures.
    pub retry_backoff: BackoffPolicy,
    /// Native balance under which a signer is reported low.
    pub low_balance_wei: U256,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error(transparent)]
    Chain(#[from] ChainError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("bus error: {0}")]
    Bus(#[from] gum_bus::BusError),
    /// A command that cannot be executed and never will be.
    #[error("rejected: {0}")]
    Rejected(String),
}

impl ExecutorError {
    fn is_retryable(&self) -> bool {
        match self {
            Self::Chain(error) => error.is_retryable(),
            Self::Database(_) | Self::Bus(_) => true,
            Self::Rejected(_) => false,
        }
    }
}

/// What one pass did, for the heartbeat and tests.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PassReport {
    pub reconciled: usize,
    pub started: usize,
    pub resolved: usize,
    pub errors: Vec<String>,
}

/// A fee estimate read from the node at most once per pass.
#[derive(Default)]
struct FeeCache(tokio::sync::OnceCell<FeeEstimate>);

impl FeeCache {
    async fn get(&self, chain: &dyn ChainExecutor) -> Result<FeeEstimate, ChainError> {
        self.0
            .get_or_try_init(|| chain.estimate_fees())
            .await
            .copied()
    }
}

pub struct ChainWorker {
    chain: Arc<dyn ChainExecutor>,
    config: ChainConfig,
    pool: PgPool,
    policy: Policy,
    publisher: Publisher,
    wake: Arc<Notify>,
}

impl ChainWorker {
    pub fn new(
        chain: Arc<dyn ChainExecutor>,
        config: ChainConfig,
        pool: PgPool,
        policy: Policy,
        wake: Arc<Notify>,
    ) -> Self {
        Self {
            chain,
            config,
            pool,
            policy,
            publisher: Publisher::new("gum-signers"),
            wake,
        }
    }

    pub fn chain_id(&self) -> u64 {
        self.config.chain_id
    }

    /// Run passes until `shutdown` turns true.
    pub async fn run(self, poll_interval: Duration, mut shutdown: watch::Receiver<bool>) {
        info!(
            chain_id = self.config.chain_id,
            signers = self.chain.signers().len(),
            "chain executor started"
        );
        loop {
            if *shutdown.borrow() {
                break;
            }
            let report = self
                .pass()
                .instrument(info_span!("executor.pass", chain_id = self.config.chain_id))
                .await;
            let (state, detail) = if report.errors.is_empty() {
                ("running", None)
            } else {
                ("failing", Some(report.errors.join("; ")))
            };
            if let Err(error) = store::upsert_executor_status(
                &self.pool,
                self.config.chain_id,
                state,
                detail.as_deref(),
            )
            .await
            {
                warn!(chain_id = self.config.chain_id, error = %error, "executor heartbeat failed");
            }
            tokio::select! {
                _ = tokio::time::sleep(poll_interval) => {}
                _ = self.wake.notified() => {}
                _ = shutdown.changed() => {}
            }
        }
        info!(chain_id = self.config.chain_id, "chain executor stopped");
    }

    /// One complete pass. Errors are collected per lane, never fatal: the
    /// next pass tries again.
    pub async fn pass(&self) -> PassReport {
        let mut report = PassReport::default();
        let fees = FeeCache::default();

        let open = match store::open_transactions(&self.pool, self.config.chain_id).await {
            Ok(open) => open,
            Err(error) => {
                report.errors.push(error.to_string());
                return report;
            }
        };
        for transaction in open {
            let span = info_span!(
                "executor.reconcile",
                job_id = %transaction.job.id,
                correlation_id = %transaction.job.correlation_id,
                signer = %transaction.signer,
                nonce = transaction.nonce,
            );
            report.reconciled += 1;
            match self.reconcile(&transaction, &fees).instrument(span).await {
                Ok(true) => report.resolved += 1,
                Ok(false) => {}
                Err(error) => {
                    warn!(job_id = %transaction.job.id, error = %error, "reconciling transaction failed");
                    report.errors.push(error.to_string());
                }
            }
        }

        let halted = match store::is_halted(&self.pool, self.config.chain_id).await {
            Ok(halted) => halted,
            Err(error) => {
                report.errors.push(error.to_string());
                return report;
            }
        };
        if halted {
            return report;
        }

        let queued = match store::queued_jobs(&self.pool, self.config.chain_id).await {
            Ok(queued) => queued,
            Err(error) => {
                report.errors.push(error.to_string());
                return report;
            }
        };
        if queued.is_empty() {
            return report;
        }
        let busy: HashSet<Address> =
            match store::busy_signers(&self.pool, self.config.chain_id).await {
                Ok(busy) => busy.into_iter().collect(),
                Err(error) => {
                    report.errors.push(error.to_string());
                    return report;
                }
            };
        let mut free = self.free_signers(&busy).await;
        for job in queued {
            let span = info_span!(
                "executor.start",
                job_id = %job.id,
                correlation_id = %job.correlation_id,
                message_type = job.command.kind_name(),
            );
            match self.start(&job, &mut free, &fees).instrument(span).await {
                Ok(Started::Transaction) => report.started += 1,
                Ok(Started::Resolved) => report.resolved += 1,
                Ok(Started::Deferred) => {}
                Ok(Started::NoSigner) => break,
                Err(error) => {
                    warn!(job_id = %job.id, error = %error, "starting job failed");
                    report.errors.push(error.to_string());
                }
            }
        }
        report
    }

    /// Pool signers without an open lane, richest first, with their
    /// balances recorded for the status endpoint.
    async fn free_signers(&self, busy: &HashSet<Address>) -> Vec<Address> {
        let mut ranked: Vec<(U256, Address)> = Vec::new();
        for signer in self.chain.signers() {
            let balance = match self.chain.signer_balance(signer).await {
                Ok(balance) => {
                    let low = balance < self.policy.low_balance_wei;
                    if low {
                        warn!(chain_id = self.config.chain_id, %signer, balance = %balance, "signer balance is low");
                    }
                    Ok((balance, low))
                }
                Err(error) => {
                    warn!(chain_id = self.config.chain_id, %signer, error = %error, "signer balance read failed");
                    Err(error.to_string())
                }
            };
            if let Err(error) = store::upsert_signer_status(
                &self.pool,
                self.config.chain_id,
                signer,
                balance.as_ref().map(|b| *b).map_err(String::as_str),
            )
            .await
            {
                warn!(error = %error, "recording signer status failed");
            }
            if busy.contains(&signer) {
                continue;
            }
            // A signer whose balance cannot be read is still offered: the
            // node answers the nonce read or the submission fails the same
            // way, and the job is deferred rather than lost.
            ranked.push((balance.map(|(b, _)| b).unwrap_or(U256::ZERO), signer));
        }
        ranked.sort_by_key(|(balance, _)| std::cmp::Reverse(*balance));
        ranked.into_iter().map(|(_, signer)| signer).collect()
    }

    // -----------------------------------------------------------------------
    // Reconciliation of an open slot
    // -----------------------------------------------------------------------

    /// Advance one open slot. `Ok(true)` when it resolved.
    async fn reconcile(
        &self,
        transaction: &OpenTransaction,
        fees: &FeeCache,
    ) -> Result<bool, ExecutorError> {
        // A replacement and what it replaced share a nonce; whichever mined
        // resolves the slot. Newest first: it is the likeliest.
        for attempt in transaction.attempts.iter().rev() {
            if let Some(receipt) = self.receipt(transaction, attempt.tx_hash).await? {
                return self.on_mined(transaction, attempt.tx_hash, receipt).await;
            }
        }

        // Signed but not known to have left the node: resend the exact bytes.
        let newest = transaction.newest_attempt();
        if newest.broadcast_at.is_none() {
            self.broadcast(transaction, newest).await?;
            return Ok(false);
        }

        let age = Utc::now()
            .signed_duration_since(transaction.submitted_at)
            .to_std()
            .unwrap_or_default();
        if age < self.policy.pending_timeout {
            return Ok(false);
        }

        // Past the timeout the mined nonce is the signal: if it moved past
        // ours without a receipt for any attempt, nothing we sent can mine.
        let mined_count = self.chain.signer_nonce(transaction.signer, false).await?;
        if mined_count > transaction.nonce {
            for attempt in transaction.attempts.iter().rev() {
                if let Some(receipt) = self.receipt(transaction, attempt.tx_hash).await? {
                    return self.on_mined(transaction, attempt.tx_hash, receipt).await;
                }
            }
            let reason = AbandonReason::NonceConsumed {
                signer: transaction.signer,
                nonce: transaction.nonce,
            };
            warn!(tx_hash = %newest.tx_hash, "nonce consumed by a foreign transaction; abandoning");
            self.abandon(transaction, reason).await?;
            return Ok(true);
        }

        if transaction.attempts.len() as u32 >= self.policy.max_submissions {
            if transaction.stall_reported_at.is_none() {
                let mut tx = self.pool.begin().await?;
                let event = ExecutionEvent::ExecutionStalled {
                    job_id: transaction.job.id,
                    chain_id: self.config.chain_id,
                    signer: transaction.signer,
                    submissions: transaction.attempts.len() as u32,
                };
                self.publish(&mut tx, &transaction.job, &event).await?;
                store::mark_stall_reported(&mut tx, transaction.id).await?;
                tx.commit().await?;
                error!(
                    submissions = transaction.attempts.len(),
                    "transaction unconfirmed after the replacement limit; fees are no longer raised"
                );
            }
            // Keep the newest attempt alive in the mempool without raising
            // fees any further; a mined nonce or an operator resolves it.
            self.broadcast(transaction, newest).await?;
            return Ok(false);
        }

        self.replace(transaction, newest, fees).await?;
        Ok(false)
    }

    async fn receipt(
        &self,
        transaction: &OpenTransaction,
        tx_hash: B256,
    ) -> Result<Option<Receipt>, ExecutorError> {
        Ok(match &transaction.job.command {
            ExecutionCommand::SweepBatch(_) => self
                .chain
                .sweep_receipt(tx_hash, self.config.batch_sweeper)
                .await?
                .map(Receipt::Sweep),
            ExecutionCommand::WithdrawalStep(_) => self
                .chain
                .transaction_receipt(tx_hash)
                .await?
                .map(Receipt::Step),
        })
    }

    /// A receipt exists for `tx_hash`. Record it, wait for finality, verify
    /// the block, then conclude the job.
    async fn on_mined(
        &self,
        transaction: &OpenTransaction,
        tx_hash: B256,
        receipt: Receipt,
    ) -> Result<bool, ExecutorError> {
        let outcome = receipt.outcome();
        let mined = Mined {
            tx_hash,
            block: outcome.block,
            block_hash: outcome.block_hash,
            transaction_index: receipt.transaction_index(),
            succeeded: outcome.succeeded,
        };
        if transaction.mined != Some(mined) {
            let mut conn = self.pool.acquire().await?;
            store::record_mined(&mut conn, transaction.id, mined).await?;
            info!(%tx_hash, block = mined.block, succeeded = mined.succeeded, "transaction mined; awaiting finality");
        }
        let boundary = self.boundary().await?;
        if outcome.block > boundary.number {
            return Ok(false);
        }
        let header = self.chain.block_header(outcome.block).await?;
        if header.hash != outcome.block_hash {
            let mut conn = self.pool.acquire().await?;
            store::clear_mined(&mut conn, transaction.id).await?;
            warn!(%tx_hash, block = outcome.block, "receipt block is no longer canonical; waiting for a new receipt");
            return Ok(false);
        }

        match (&transaction.job.command, receipt) {
            (ExecutionCommand::SweepBatch(command), Receipt::Sweep(receipt)) => {
                self.finalize_sweep(transaction, command, tx_hash, &receipt, &header)
                    .await
            }
            (ExecutionCommand::WithdrawalStep(command), Receipt::Step(outcome)) => {
                self.finalize_step(transaction, command, tx_hash, outcome, &boundary)
                    .await
            }
            _ => unreachable!("receipt kind follows the command kind"),
        }
    }

    async fn finalize_sweep(
        &self,
        transaction: &OpenTransaction,
        command: &SweepBatchCommand,
        tx_hash: B256,
        receipt: &SweepReceipt,
        header: &BlockHeader,
    ) -> Result<bool, ExecutorError> {
        if !receipt.succeeded {
            warn!(%tx_hash, "helper transaction reverted as a whole");
            let event = ExecutionEvent::SweepAbandoned {
                job_id: transaction.job.id,
                chain_id: self.config.chain_id,
                reason: AbandonReason::Reverted { tx_hash },
            };
            self.resolve(transaction, JobState::Abandoned, TxState::Reverted, &event)
                .await?;
            return Ok(true);
        }
        let items = sweep::classify_items(
            self.chain.as_ref(),
            &self.config,
            &command.items,
            receipt,
            header,
        )
        .await?;
        // The classification read state pinned at the receipt block; a
        // block that changed meanwhile invalidates every read.
        if self.chain.block_header(receipt.block).await?.hash != receipt.block_hash {
            let mut conn = self.pool.acquire().await?;
            store::clear_mined(&mut conn, transaction.id).await?;
            warn!(%tx_hash, block = receipt.block, "receipt block changed during classification; discarding results");
            return Ok(false);
        }
        let event = ExecutionEvent::SweepFinalized {
            job_id: transaction.job.id,
            chain_id: self.config.chain_id,
            tx_hash,
            block: receipt.block,
            block_hash: receipt.block_hash,
            block_timestamp: header.timestamp,
            transaction_index: receipt.transaction_index,
            items,
        };
        self.resolve(transaction, JobState::Finalized, TxState::Finalized, &event)
            .await?;
        info!(%tx_hash, block = receipt.block, items = command.items.len(), "sweep finalized");
        Ok(true)
    }

    async fn finalize_step(
        &self,
        transaction: &OpenTransaction,
        command: &WithdrawalStepCommand,
        tx_hash: B256,
        outcome: TransactionOutcome,
        boundary: &BlockHeader,
    ) -> Result<bool, ExecutorError> {
        let (result, tx_state) = if outcome.succeeded {
            (
                StepResult::Succeeded {
                    tx_hash,
                    block: outcome.block,
                    block_hash: outcome.block_hash,
                },
                TxState::Finalized,
            )
        } else {
            // Reverted: either somebody else did the step first, or the
            // obligation still stands and the server decides on a retry.
            let executed_elsewhere = match &command.precondition {
                Some(precondition) => {
                    step::check(
                        self.chain.as_ref(),
                        self.config.log_range_size,
                        precondition,
                        boundary,
                    )
                    .await?
                }
                None => Precondition::Holds,
            };
            match executed_elsewhere {
                Precondition::Violated { tx_hash: other } => (
                    StepResult::ExecutedElsewhere { tx_hash: other },
                    TxState::Reverted,
                ),
                Precondition::Holds => (StepResult::Reverted { tx_hash }, TxState::Reverted),
            }
        };
        info!(%tx_hash, step = ?command.step, result = ?result, "withdrawal step finalized");
        let event = ExecutionEvent::WithdrawalStepFinalized {
            job_id: transaction.job.id,
            leg_id: command.leg_id,
            chain_id: self.config.chain_id,
            step: command.step,
            result,
        };
        self.resolve(transaction, JobState::Finalized, tx_state, &event)
            .await?;
        Ok(true)
    }

    /// Resend an attempt's exact signed bytes and record that they left.
    async fn broadcast(
        &self,
        transaction: &OpenTransaction,
        attempt: &Attempt,
    ) -> Result<(), ExecutorError> {
        let prepared = PreparedSweepTransaction {
            hash: attempt.tx_hash,
            raw: attempt.raw_transaction.clone(),
        };
        self.chain.broadcast_sweep_transaction(&prepared).await?;
        let mut tx = self.pool.begin().await?;
        store::mark_broadcast(&mut tx, transaction.id, attempt.replacement_number).await?;
        if attempt.broadcast_at.is_none()
            && let ExecutionCommand::SweepBatch(_) = &transaction.job.command
        {
            let event = ExecutionEvent::SweepSubmitted {
                job_id: transaction.job.id,
                chain_id: self.config.chain_id,
                signer: transaction.signer,
                nonce: transaction.nonce,
                tx_hash: attempt.tx_hash,
                replacement: attempt.replacement_number,
            };
            self.publish(&mut tx, &transaction.job, &event).await?;
        }
        tx.commit().await?;
        info!(tx_hash = %attempt.tx_hash, replacement = attempt.replacement_number, "transaction broadcast");
        Ok(())
    }

    /// Sign a same-nonce replacement with raised fees, persist it, then
    /// broadcast it.
    async fn replace(
        &self,
        transaction: &OpenTransaction,
        previous: &Attempt,
        fees: &FeeCache,
    ) -> Result<(), ExecutorError> {
        let floor = FeeEstimate {
            max_fee_per_gas: previous.max_fee_per_gas,
            max_priority_fee_per_gas: previous.max_priority_fee_per_gas,
        }
        .bumped();
        let fees = fees.get(self.chain.as_ref()).await?.max(floor);
        let prepared = match &transaction.job.command {
            ExecutionCommand::SweepBatch(command) => {
                self.chain
                    .prepare_sweep_batch(
                        transaction.signer,
                        self.config.batch_sweeper,
                        &sweep_requests(command, self.config.chain_id),
                        transaction.nonce,
                        transaction.gas_limit,
                        fees,
                    )
                    .await?
            }
            ExecutionCommand::WithdrawalStep(command) => {
                self.chain
                    .prepare_call(
                        transaction.signer,
                        command.to,
                        command.calldata.clone(),
                        transaction.nonce,
                        transaction.gas_limit,
                        fees,
                    )
                    .await?
            }
        };
        let attempt = Attempt {
            replacement_number: previous.replacement_number + 1,
            tx_hash: prepared.hash,
            raw_transaction: prepared.raw,
            max_fee_per_gas: fees.max_fee_per_gas,
            max_priority_fee_per_gas: fees.max_priority_fee_per_gas,
            broadcast_at: None,
        };
        {
            let mut conn = self.pool.acquire().await?;
            store::insert_attempt(&mut conn, transaction.id, &attempt).await?;
        }
        warn!(tx_hash = %attempt.tx_hash, replacement = attempt.replacement_number, max_fee_per_gas = fees.max_fee_per_gas, "replacing unconfirmed transaction with raised fees");
        self.broadcast(transaction, &attempt).await
    }

    async fn abandon(
        &self,
        transaction: &OpenTransaction,
        reason: AbandonReason,
    ) -> Result<(), ExecutorError> {
        let event = match &transaction.job.command {
            ExecutionCommand::SweepBatch(_) => ExecutionEvent::SweepAbandoned {
                job_id: transaction.job.id,
                chain_id: self.config.chain_id,
                reason,
            },
            ExecutionCommand::WithdrawalStep(command) => ExecutionEvent::WithdrawalStepFinalized {
                job_id: transaction.job.id,
                leg_id: command.leg_id,
                chain_id: self.config.chain_id,
                step: command.step,
                result: StepResult::Abandoned { reason },
            },
        };
        self.resolve(transaction, JobState::Abandoned, TxState::Abandoned, &event)
            .await
    }

    /// Close the slot and the job and publish the conclusion, atomically.
    async fn resolve(
        &self,
        transaction: &OpenTransaction,
        job_state: JobState,
        tx_state: TxState,
        event: &ExecutionEvent,
    ) -> Result<(), ExecutorError> {
        let mut tx = self.pool.begin().await?;
        store::resolve_transaction(&mut tx, transaction.id, tx_state).await?;
        if store::resolve_job(&mut tx, transaction.job.id, job_state, event).await? {
            self.publish(&mut tx, &transaction.job, event).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Starting queued jobs
    // -----------------------------------------------------------------------

    async fn start(
        &self,
        job: &Job,
        free: &mut Vec<Address>,
        fees: &FeeCache,
    ) -> Result<Started, ExecutorError> {
        match self.try_start(job, free, fees).await {
            Ok(started) => Ok(started),
            Err(error) if error.is_retryable() => {
                let mut conn = self.pool.acquire().await?;
                let attempts = job.attempts + 1;
                if attempts >= self.policy.max_attempts {
                    drop(conn);
                    warn!(attempts, error = %error, "job failed transiently once too often; abandoning");
                    self.reject_job(
                        job,
                        AbandonReason::Rejected {
                            reason: error.to_string(),
                        },
                    )
                    .await?;
                    return Ok(Started::Resolved);
                }
                let delay = self.policy.retry_backoff.delay(attempts);
                let next = Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default();
                store::defer_job(&mut conn, job.id, next, &error.to_string()).await?;
                warn!(attempt = attempts, retry_at = %next, error = %error, "job deferred after a transient failure");
                Ok(Started::Deferred)
            }
            Err(error) => {
                warn!(error = %error, "job rejected permanently");
                self.reject_job(
                    job,
                    AbandonReason::Rejected {
                        reason: error.to_string(),
                    },
                )
                .await?;
                Ok(Started::Resolved)
            }
        }
    }

    async fn try_start(
        &self,
        job: &Job,
        free: &mut Vec<Address>,
        fees: &FeeCache,
    ) -> Result<Started, ExecutorError> {
        // Nonce-free conclusions first: they need no signer at all.
        if let ExecutionCommand::WithdrawalStep(command) = &job.command {
            let now = Utc::now().timestamp().max(0) as u64;
            if command.not_after.is_some_and(|deadline| now >= deadline) {
                self.conclude_step(job, command, StepResult::Expired)
                    .await?;
                return Ok(Started::Resolved);
            }
            if let Some(precondition) = &command.precondition {
                let boundary = self.boundary().await?;
                if let Precondition::Violated { tx_hash } = step::check(
                    self.chain.as_ref(),
                    self.config.log_range_size,
                    precondition,
                    &boundary,
                )
                .await?
                {
                    info!(?tx_hash, step = ?command.step, "step already executed elsewhere");
                    self.conclude_step(job, command, StepResult::ExecutedElsewhere { tx_hash })
                        .await?;
                    return Ok(Started::Resolved);
                }
            }
        }

        let Some(signer) = free.first().copied() else {
            return Ok(Started::NoSigner);
        };

        let mined = self.chain.signer_nonce(signer, false).await?;
        let pending = self.chain.signer_nonce(signer, true).await?;
        if pending > mined {
            // Something outside the pool is using this key. Never sign on
            // top of it; another signer may still take the job.
            warn!(%signer, mined, pending, "signer has transactions in flight that are not ours; skipping it");
            free.remove(0);
            return Ok(if free.is_empty() {
                Started::NoSigner
            } else {
                Box::pin(self.try_start(job, free, fees)).await?
            });
        }
        let fees = fees.get(self.chain.as_ref()).await?;

        let (prepared, target, calldata, gas_limit) = match &job.command {
            ExecutionCommand::SweepBatch(command) => {
                if command.items.is_empty() {
                    return Err(ExecutorError::Rejected("sweep batch has no items".into()));
                }
                let requests = sweep_requests(command, self.config.chain_id);
                let gas_limit = sweep_batch_gas_limit(requests.len());
                let prepared = self
                    .chain
                    .prepare_sweep_batch(
                        signer,
                        self.config.batch_sweeper,
                        &requests,
                        mined,
                        gas_limit,
                        fees,
                    )
                    .await?;
                (
                    prepared,
                    self.config.batch_sweeper,
                    alloy_primitives::Bytes::new(),
                    gas_limit,
                )
            }
            ExecutionCommand::WithdrawalStep(command) => {
                let prepared = self
                    .chain
                    .prepare_call(
                        signer,
                        command.to,
                        command.calldata.clone(),
                        mined,
                        command.gas_limit,
                        fees,
                    )
                    .await?;
                (
                    prepared,
                    command.to,
                    command.calldata.clone(),
                    command.gas_limit,
                )
            }
        };
        let attempt = Attempt {
            replacement_number: 0,
            tx_hash: prepared.hash,
            raw_transaction: prepared.raw,
            max_fee_per_gas: fees.max_fee_per_gas,
            max_priority_fee_per_gas: fees.max_priority_fee_per_gas,
            broadcast_at: None,
        };
        let transaction_id = {
            let mut tx = self.pool.begin().await?;
            let id = store::begin_transaction(
                &mut tx,
                NewTransaction {
                    job_id: job.id,
                    chain_id: self.config.chain_id,
                    signer,
                    nonce: mined,
                    target,
                    calldata: &calldata,
                    gas_limit,
                    attempt: &attempt,
                },
            )
            .await?;
            tx.commit().await?;
            id
        };
        free.remove(0);
        info!(%signer, nonce = mined, tx_hash = %attempt.tx_hash, "transaction signed and persisted");

        // From here on the slot exists; a failure is recovered by the next
        // pass's reconcile, which resends the persisted bytes.
        let open = OpenTransaction {
            id: transaction_id,
            job: job.clone(),
            signer,
            nonce: mined,
            target,
            calldata,
            gas_limit,
            state: TxState::Prepared,
            submitted_at: Utc::now(),
            mined: None,
            stall_reported_at: None,
            attempts: vec![attempt.clone()],
        };
        if let Err(error) = self.broadcast(&open, &attempt).await {
            warn!(error = %error, tx_hash = %attempt.tx_hash, "broadcast failed; the signed transaction stays durable for the next pass");
        }
        Ok(Started::Transaction)
    }

    /// Conclude a step job without any transaction.
    async fn conclude_step(
        &self,
        job: &Job,
        command: &WithdrawalStepCommand,
        result: StepResult,
    ) -> Result<(), ExecutorError> {
        let event = ExecutionEvent::WithdrawalStepFinalized {
            job_id: job.id,
            leg_id: command.leg_id,
            chain_id: self.config.chain_id,
            step: command.step,
            result,
        };
        let mut tx = self.pool.begin().await?;
        if store::resolve_job(&mut tx, job.id, JobState::Finalized, &event).await? {
            self.publish(&mut tx, job, &event).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// A job that will never produce a transaction.
    async fn reject_job(&self, job: &Job, reason: AbandonReason) -> Result<(), ExecutorError> {
        let event = match &job.command {
            ExecutionCommand::SweepBatch(_) => ExecutionEvent::SweepAbandoned {
                job_id: job.id,
                chain_id: self.config.chain_id,
                reason,
            },
            ExecutionCommand::WithdrawalStep(command) => ExecutionEvent::WithdrawalStepFinalized {
                job_id: job.id,
                leg_id: command.leg_id,
                chain_id: self.config.chain_id,
                step: command.step,
                result: StepResult::Abandoned { reason },
            },
        };
        let mut tx = self.pool.begin().await?;
        if store::resolve_job(&mut tx, job.id, JobState::Abandoned, &event).await? {
            self.publish(&mut tx, job, &event).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn publish(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        job: &Job,
        event: &ExecutionEvent,
    ) -> Result<(), ExecutorError> {
        self.publisher
            .publish(
                tx,
                event,
                CorrelationId(job.correlation_id),
                job.command_message_id,
            )
            .await?;
        Ok(())
    }

    async fn boundary(&self) -> Result<BlockHeader, ChainError> {
        finality_boundary(
            self.chain.as_ref(),
            self.config.finality_source,
            self.config.finality_confirmations,
        )
        .await
    }
}

enum Started {
    Transaction,
    Resolved,
    Deferred,
    NoSigner,
}

enum Receipt {
    Sweep(SweepReceipt),
    Step(TransactionOutcome),
}

impl Receipt {
    fn outcome(&self) -> TransactionOutcome {
        match self {
            Self::Sweep(receipt) => TransactionOutcome {
                succeeded: receipt.succeeded,
                block: receipt.block,
                block_hash: receipt.block_hash,
            },
            Self::Step(outcome) => *outcome,
        }
    }

    fn transaction_index(&self) -> u64 {
        match self {
            Self::Sweep(receipt) => receipt.transaction_index,
            Self::Step(_) => 0,
        }
    }
}

fn sweep_requests(command: &SweepBatchCommand, chain_id: u64) -> Vec<SweepRequest> {
    command
        .items
        .iter()
        .map(|item| SweepRequest {
            token: item.token,
            amount: item.amount,
            receiver: item.receiver,
            expiration_timestamp: item.expiration_timestamp,
            recovery: item.recovery,
            salt: item.salt,
            chain_id,
        })
        .collect()
}

trait KindName {
    fn kind_name(&self) -> &'static str;
}

impl KindName for ExecutionCommand {
    fn kind_name(&self) -> &'static str {
        gum_contracts::BusMessage::kind(self)
    }
}
