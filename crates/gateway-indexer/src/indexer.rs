//! Payment indexer worker.
//!
//! A supervised worker that ingests finalized USDC `Transfer` logs and advances
//! invoices `created -> funded -> deploying -> fulfilled`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use alloy_primitives::{Address, B256};
use gateway_core::{ChainId, Invoice};
use thiserror::Error;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;
use tracing::{info, warn};

use crate::chain::{ChainClient, ChainError, SweepReceipt, SweepRequest};
use gateway_db::{CursorRepository, InvoiceRepository, PaymentObservation};

/// Maximum invoices included in one helper transaction.
const SWEEP_BATCH_LIMIT: i64 = 20;

/// Exponential backoff for retrying a transient sweep failure: the nth retry
/// waits `min(BASE * 2^attempts, CAP)` seconds, so a struggling RPC is not
/// hammered while a brief blip still recovers quickly.
const SWEEP_BACKOFF_BASE_SECS: f64 = 2.0;
const SWEEP_BACKOFF_CAP_SECS: f64 = 300.0;
const SWEEP_PENDING_TIMEOUT: Duration = Duration::from_secs(300);

/// Errors that can abort a poll pass. Transient failures are retried; permanent
/// RPC and finality failures escape the run loop so ECS restarts and alerts
/// rather than leaving an inert process marked healthy.
#[derive(Debug, Error)]
pub enum IndexerError {
    #[error("chain error: {0}")]
    Chain(#[from] ChainError),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("configuration invariant failed: {0}")]
    Configuration(String),
    #[error("pending sweep requires operator intervention: {0}")]
    PendingSweep(String),
}

impl IndexerError {
    fn requires_halt(&self) -> bool {
        matches!(self, Self::Chain(ChainError::FinalityViolation(_)))
            || matches!(self, Self::Chain(error) if error.is_permanent_rpc())
            || matches!(self, Self::Configuration(_) | Self::PendingSweep(_))
    }
}

/// The payment indexer. Owns its own repository handles and chain client; it is
/// not shared with the HTTP handlers.
pub struct Indexer {
    repo: InvoiceRepository,
    cursor: CursorRepository,
    chain: Arc<dyn ChainClient>,
    chain_id: ChainId,
    factory: Address,
    batch_sweeper: Address,
    usdc: Address,
    usdc_start_block: u64,
    finality_confirmations: u64,
    max_log_range_size: u64,
    current_log_range_size: AtomicU64,
    poll_interval: Duration,
    pending_sweep_timeout: Duration,
}

impl Indexer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repo: InvoiceRepository,
        cursor: CursorRepository,
        chain: Arc<dyn ChainClient>,
        chain_id: ChainId,
        factory: Address,
        batch_sweeper: Address,
        usdc: Address,
        usdc_start_block: u64,
        finality_confirmations: u64,
        log_range_size: u64,
        poll_interval: Duration,
    ) -> Self {
        Self {
            repo,
            cursor,
            chain,
            chain_id,
            factory,
            batch_sweeper,
            usdc,
            usdc_start_block,
            finality_confirmations,
            max_log_range_size: log_range_size,
            current_log_range_size: AtomicU64::new(log_range_size),
            poll_interval,
            pending_sweep_timeout: SWEEP_PENDING_TIMEOUT,
        }
    }

    /// Run the poll loop until `shutdown` is set to `true`. Permanent RPC and
    /// finality errors are returned so the process supervisor cannot mistake a
    /// halted payment worker for a healthy one.
    pub async fn run(self, shutdown: watch::Receiver<bool>) -> Result<(), IndexerError> {
        let worker = Arc::new(self);
        let index_loop = Arc::clone(&worker).run_index_loop(shutdown.clone());
        let sweep_loop = worker.run_sweep_loop(shutdown);
        tokio::pin!(index_loop, sweep_loop);

        tokio::select! {
            result = &mut index_loop => result,
            result = &mut sweep_loop => result,
        }
    }

    async fn run_index_loop(
        self: Arc<Self>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), IndexerError> {
        let mut interval = tokio::time::interval(self.poll_interval);
        // If a tick is delayed (e.g. a slow RPC pass), don't fire a burst of
        // catch-up ticks afterwards; just resume the cadence.
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        info!(
            chain_id = self.chain_id.0,
            usdc = %self.usdc,
            finality_confirmations = self.finality_confirmations,
            poll_interval_ms = self.poll_interval.as_millis() as u64,
            "block indexer started"
        );

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(error) = self.tick().await {
                        if error.requires_halt() {
                            return Err(error);
                        }
                        warn!(error = %error, "indexer poll failed; retrying next tick");
                    }
                }
                changed = shutdown.changed() => {
                    // `Err` means the sender was dropped -> treat as shutdown.
                    if changed.is_err() || *shutdown.borrow() {
                        info!("block indexer shutting down");
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn run_sweep_loop(
        self: Arc<Self>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), IndexerError> {
        let mut interval = tokio::time::interval(self.poll_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        info!(batch_sweeper = %self.batch_sweeper, "sweep worker started");

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(error) = self.sweep_tick().await {
                        if error.requires_halt() {
                            return Err(error);
                        }
                        warn!(error = %error, "sweep pass failed; retrying next tick");
                    }
                }
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        info!("sweep worker shutting down");
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    /// Ingest one bounded, finalized range of USDC Transfer logs.
    async fn tick(&self) -> Result<(), IndexerError> {
        let head = self.chain.get_block_number().await?;
        if head < self.finality_confirmations {
            return Ok(());
        }
        let finalized_head = head - self.finality_confirmations;
        let cursor = self.cursor.get(self.chain_id.0, self.usdc).await?;

        if let Some(cursor) = cursor {
            let canonical_hash = self.chain.get_block_hash(cursor.block).await?;
            if canonical_hash != cursor.block_hash {
                return Err(ChainError::FinalityViolation(format!(
                    "finalized cursor hash mismatch at block {}; refusing to continue",
                    cursor.block
                ))
                .into());
            }
        }

        let from_block = cursor
            .map(|cursor| cursor.block.saturating_add(1))
            .unwrap_or(self.usdc_start_block);
        if from_block > finalized_head {
            return Ok(());
        }
        let (to_block, end_block_hash, mut transfers) = loop {
            let range_size = self.current_log_range_size.load(Ordering::Relaxed).max(1);
            let to_block = from_block
                .saturating_add(range_size - 1)
                .min(finalized_head);

            // Bracket the log request with the range-end hash. If a reorg happens
            // while the provider is producing the response, do not pair old logs
            // with a new canonical cursor; retry the whole uncommitted range.
            let end_block_hash = self.chain.get_block_hash(to_block).await?;
            match self
                .chain
                .get_usdc_transfers(self.usdc, from_block, to_block)
                .await
            {
                Ok(transfers) => break (to_block, end_block_hash, transfers),
                Err(ChainError::LogRangeTooLarge(message)) if range_size > 1 => {
                    let smaller = (range_size / 2).max(1);
                    self.current_log_range_size
                        .store(smaller, Ordering::Relaxed);
                    warn!(from_block, to_block, smaller, error = %message, "provider rejected USDC log range; splitting it");
                }
                Err(error) => return Err(error.into()),
            }
        };
        if self.chain.get_block_hash(to_block).await? != end_block_hash {
            return Err(ChainError::Transient(format!(
                "block {to_block} changed while fetching USDC logs"
            ))
            .into());
        }
        transfers.sort_by_key(|transfer| {
            (
                transfer.block_number,
                transfer.transaction_index,
                transfer.log_index,
            )
        });
        let observations: Vec<PaymentObservation> = transfers
            .into_iter()
            .map(|transfer| PaymentObservation {
                block_number: transfer.block_number,
                block_hash: transfer.block_hash,
                transaction_hash: transfer.transaction_hash,
                transaction_index: transfer.transaction_index,
                log_index: transfer.log_index,
                sender: transfer.sender,
                recipient: transfer.recipient,
                amount: transfer.amount,
            })
            .collect();
        let funded = self
            .repo
            .apply_finalized_usdc_range(
                self.chain_id.0,
                self.usdc,
                cursor,
                to_block,
                end_block_hash,
                &observations,
            )
            .await?;
        for invoice_id in funded {
            info!(%invoice_id, block = to_block, "invoice funded by finalized USDC transfers");
        }
        let range_size = self.current_log_range_size.load(Ordering::Relaxed);
        self.current_log_range_size.store(
            range_size
                .saturating_add((range_size / 4).max(1))
                .min(self.max_log_range_size),
            Ordering::Relaxed,
        );
        Ok(())
    }

    /// Reconcile prior submissions, then claim and submit at most one new batch.
    /// The database rows are the durable queue shared with the block indexer.
    async fn sweep_tick(&self) -> Result<(), IndexerError> {
        let existing = self.repo.list_sweeps_to_reconcile(self.chain_id.0).await?;
        let mut submitted: HashMap<B256, Vec<(Invoice, gateway_db::DbInvoice)>> = HashMap::new();
        for row in existing {
            let invoice = Invoice::try_from(&row).map_err(|error| {
                IndexerError::Configuration(format!("corrupt invoice {}: {error}", row.id))
            })?;
            if let Some(tx_hash) = row.execute_tx_hash.as_deref() {
                let tx_hash = B256::try_from(tx_hash)
                    .map_err(|_| sqlx::Error::Decode("invalid execute transaction hash".into()))?;
                submitted.entry(tx_hash).or_default().push((invoice, row));
            } else {
                self.reconcile_sweep(&invoice, &row).await?;
            }
        }
        let mut unresolved_submission = false;
        for (tx_hash, invoices) in submitted {
            let receipt = self
                .chain
                .get_sweep_receipt(tx_hash, self.batch_sweeper)
                .await?;
            if let Some(receipt) = &receipt {
                for (invoice, _) in invoices {
                    self.apply_batch_receipt(&invoice, tx_hash, receipt).await?;
                }
            } else if invoices
                .iter()
                .any(|(_, row)| row.sweep_detected_at_block.is_some())
            {
                for (invoice, row) in &invoices {
                    self.handle_missing_receipt(invoice, row).await?;
                }
                unresolved_submission = true;
            } else if self.submission_timed_out(&invoices)? {
                if self.chain.transaction_known(tx_hash).await? {
                    return Err(IndexerError::PendingSweep(format!(
                        "transaction {tx_hash} is still pending after {} seconds",
                        self.pending_sweep_timeout.as_secs()
                    )));
                }
                let retried = self
                    .repo
                    .retry_dropped_sweep_batch(self.chain_id.0, tx_hash.as_slice())
                    .await?;
                warn!(%tx_hash, retried, "dropped batch transaction returned to retry queue");
                unresolved_submission = true;
            } else {
                unresolved_submission = true;
            }
        }
        // The signer owns one nonce stream. Never submit a higher nonce while a
        // prior transaction has no receipt; wait, recover it, or halt first.
        if unresolved_submission {
            return Ok(());
        }

        let claimed = self
            .repo
            .claim_sweep_batch(
                self.chain_id.0,
                SWEEP_BATCH_LIMIT,
                SWEEP_BACKOFF_BASE_SECS,
                SWEEP_BACKOFF_CAP_SECS,
            )
            .await?
            .into_iter()
            .map(|row| {
                Invoice::try_from(&row).map_err(|error| {
                    IndexerError::Configuration(format!("corrupt invoice {}: {error}", row.id))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if claimed.is_empty() {
            return Ok(());
        }

        let mut batch = Vec::with_capacity(claimed.len());
        for invoice in claimed {
            if invoice.factory.0 != self.factory || invoice.token.0 != self.usdc {
                return Err(IndexerError::Configuration(format!(
                    "invoice {} does not match configured factory/token",
                    invoice.id.0
                )));
            }
            if self
                .chain
                .payment_deployed(invoice.payment_address.0)
                .await?
            {
                let block = self.chain.get_block_number().await?;
                let hash = self.chain.get_block_hash(block).await?;
                self.repo
                    .record_sweep_detection(invoice.id.0, None, block, hash.as_slice())
                    .await?;
                self.finalize_detected_sweep(&invoice, block, hash, None)
                    .await?;
            } else {
                batch.push(invoice);
            }
        }
        if batch.is_empty() {
            return Ok(());
        }

        // Covers the unavoidable crash window after RPC acceptance but before
        // the batch hash is committed to PostgreSQL.
        if self.chain.signer_has_pending_transaction().await? {
            return Err(IndexerError::PendingSweep(
                "signer has an untracked pending transaction".to_string(),
            ));
        }

        let requests = batch
            .iter()
            .map(|invoice| SweepRequest {
                token: invoice.token.0,
                amount: invoice.amount.0,
                receiver: invoice.beneficiary.0,
                expiration_timestamp: invoice.expiration_timestamp,
                recovery: invoice.recovery.0,
                salt: invoice.salt.0,
            })
            .collect::<Vec<_>>();
        let tx_hash = match self
            .chain
            .submit_sweep_batch(self.batch_sweeper, &requests)
            .await
        {
            Ok(tx_hash) => tx_hash,
            Err(error) => {
                for invoice in &batch {
                    self.repo.record_sweep_retry(invoice.id.0).await?;
                }
                return Err(error.into());
            }
        };
        let ids = batch.iter().map(|invoice| invoice.id.0).collect::<Vec<_>>();
        self.repo
            .record_batch_sweep_submission(&ids, tx_hash.as_slice())
            .await?;

        if let Some(receipt) = self
            .chain
            .get_sweep_receipt(tx_hash, self.batch_sweeper)
            .await?
        {
            for invoice in &batch {
                self.apply_batch_receipt(invoice, tx_hash, &receipt).await?;
            }
        }
        Ok(())
    }

    fn submission_timed_out(
        &self,
        invoices: &[(Invoice, gateway_db::DbInvoice)],
    ) -> Result<bool, IndexerError> {
        let submitted_at = invoices
            .iter()
            .map(|(_, row)| {
                row.sweep_submitted_at.ok_or_else(|| {
                    IndexerError::Configuration(format!(
                        "invoice {} has an execute hash without sweep_submitted_at",
                        row.id
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .min()
            .ok_or_else(|| IndexerError::Configuration("empty submitted batch".to_string()))?;
        Ok(sqlx::types::chrono::Utc::now()
            .signed_duration_since(submitted_at)
            .to_std()
            .is_ok_and(|elapsed| elapsed >= self.pending_sweep_timeout))
    }

    async fn reconcile_sweep(
        &self,
        invoice: &Invoice,
        row: &gateway_db::DbInvoice,
    ) -> Result<(), IndexerError> {
        if let (Some(block), Some(hash)) = (
            row.sweep_detected_at_block,
            row.sweep_detected_at_block_hash.as_deref(),
        ) {
            let block_hash = B256::try_from(hash)
                .map_err(|_| sqlx::Error::Decode("invalid sweep detection block hash".into()))?;
            return self
                .finalize_detected_sweep(invoice, block as u64, block_hash, None)
                .await;
        }
        Ok(())
    }

    async fn handle_missing_receipt(
        &self,
        invoice: &Invoice,
        row: &gateway_db::DbInvoice,
    ) -> Result<(), IndexerError> {
        let (Some(block), Some(hash)) = (
            row.sweep_detected_at_block,
            row.sweep_detected_at_block_hash.as_deref(),
        ) else {
            return Ok(());
        };
        let block = block as u64;
        if self.chain.get_block_number().await? < block.saturating_add(self.finality_confirmations)
        {
            return Ok(());
        }
        let detected_hash = B256::try_from(hash)
            .map_err(|_| sqlx::Error::Decode("invalid sweep detection block hash".into()))?;
        if self.chain.get_block_hash(block).await? != detected_hash {
            self.repo.clear_sweep_detection(invoice.id.0).await?;
            warn!(invoice_id = %invoice.id.0, block, "missing batch receipt was orphaned; retrying execution");
        }
        Ok(())
    }

    async fn apply_batch_receipt(
        &self,
        invoice: &Invoice,
        tx_hash: B256,
        receipt: &SweepReceipt,
    ) -> Result<(), IndexerError> {
        if !receipt.succeeded {
            self.repo.clear_sweep_submission(invoice.id.0).await?;
            self.repo.record_sweep_retry(invoice.id.0).await?;
            warn!(invoice_id = %invoice.id.0, %tx_hash, "batch transaction reverted; retrying invoice");
            return Ok(());
        }

        self.repo
            .record_sweep_detection(
                invoice.id.0,
                Some(tx_hash.as_slice()),
                receipt.block,
                receipt.block_hash.as_slice(),
            )
            .await?;
        let head = self.chain.get_block_number().await?;
        if head < receipt.block.saturating_add(self.finality_confirmations) {
            return Ok(());
        }
        if self.chain.get_block_hash(receipt.block).await? != receipt.block_hash {
            self.repo.clear_sweep_detection(invoice.id.0).await?;
            warn!(invoice_id = %invoice.id.0, "batch receipt was orphaned; retrying execution");
            return Ok(());
        }

        let matching_failures = receipt
            .failures
            .iter()
            .filter(|(payment, token)| {
                *payment == invoice.payment_address.0 && *token == invoice.token.0
            })
            .count();
        if matching_failures > 1 {
            return Err(ChainError::FinalityViolation(format!(
                "batch receipt contains duplicate failure events for {}",
                invoice.payment_address.0
            ))
            .into());
        }
        let deployed = self
            .chain
            .payment_deployed_at(invoice.payment_address.0, receipt.block_hash)
            .await?;
        match (matching_failures == 1, deployed) {
            (true, false) => {
                self.repo
                    .mark_blocked(invoice.id.0, "token_transfer_failed")
                    .await?;
                warn!(invoice_id = %invoice.id.0, "batch item failed and was marked blocked");
            }
            (true, true) => {
                self.repo
                    .mark_fulfilled(invoice.id.0, None, Some(receipt.block))
                    .await?;
                info!(invoice_id = %invoice.id.0, "batch item was already executed");
            }
            (false, true) => {
                self.repo
                    .mark_fulfilled(invoice.id.0, Some(tx_hash.as_slice()), Some(receipt.block))
                    .await?;
                info!(invoice_id = %invoice.id.0, "batched invoice sweep finalized");
            }
            (false, false) => {
                return Err(ChainError::FinalityViolation(format!(
                    "successful batch item left no code at {}",
                    invoice.payment_address.0
                ))
                .into());
            }
        }
        Ok(())
    }

    async fn finalize_detected_sweep(
        &self,
        invoice: &Invoice,
        detected_block: u64,
        detected_hash: B256,
        tx_hash: Option<&[u8]>,
    ) -> Result<(), IndexerError> {
        let head = self.chain.get_block_number().await?;
        if head < detected_block.saturating_add(self.finality_confirmations) {
            return Ok(());
        }

        let canonical_hash = self.chain.get_block_hash(detected_block).await?;
        if canonical_hash != detected_hash {
            self.repo.clear_sweep_detection(invoice.id.0).await?;
            warn!(invoice_id = %invoice.id.0, detected_block, "sweep detection was orphaned; retrying execution");
            return Ok(());
        }
        let still_deployed = self
            .chain
            .payment_deployed_at(invoice.payment_address.0, detected_hash)
            .await?;
        if !still_deployed {
            self.repo.clear_sweep_detection(invoice.id.0).await?;
            warn!(invoice_id = %invoice.id.0, detected_block, "sweep detection was orphaned; retrying execution");
            return Ok(());
        }

        if self
            .repo
            .mark_fulfilled(invoice.id.0, tx_hash, Some(detected_block))
            .await?
        {
            info!(invoice_id = %invoice.id.0, detected_block, "invoice sweep finalized");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::AtomicBool;

    use alloy_primitives::{Address, B256, U256, address};
    use async_trait::async_trait;
    use gateway_core::{
        Amount, BeneficiaryAddress, ChainId, FactoryAddress, Invoice, RecoveryAddress,
        TokenAddress, USDC_DECIMALS,
    };
    use sqlx::PgPool;
    use tokio::sync::{Mutex, Notify};

    use crate::chain::{SweepReceipt, UsdcTransfer};
    use gateway_db::{CreateInvoiceInput, InvoiceRepository};

    /// Scripted sweep result for a given payment address.
    #[derive(Clone, Copy)]
    enum MockSweep {
        Executed,
        AlreadyDeployed,
        Blocked,
        Transient,
    }

    /// Fixed transaction hash the mock reports for an `Executed` sweep.
    const MOCK_TX_HASH: B256 = B256::repeat_byte(0xAB);

    /// In-memory [`ChainClient`] with a fixed head, USDC logs, and scripted sweeps.
    struct MockChain {
        block: u64,
        transfers: Vec<UsdcTransfer>,
        sweeps: HashMap<Address, MockSweep>,
        max_log_range: Option<u64>,
        hash_mismatch: bool,
        receipt_pending: bool,
        receipt_block: Option<u64>,
        submitted: AtomicBool,
        receipt_observed: AtomicBool,
        transaction_known: bool,
        signer_pending: bool,
        latest_only_code: HashSet<Address>,
    }

    impl MockChain {
        fn new(block: u64, transfers: Vec<UsdcTransfer>) -> Self {
            Self {
                block,
                transfers,
                sweeps: HashMap::new(),
                max_log_range: None,
                hash_mismatch: false,
                receipt_pending: false,
                receipt_block: None,
                submitted: AtomicBool::new(false),
                receipt_observed: AtomicBool::new(false),
                transaction_known: true,
                signer_pending: false,
                latest_only_code: HashSet::new(),
            }
        }

        /// Script the sweep outcome for one payment address.
        fn with_sweep(mut self, addr: Address, outcome: MockSweep) -> Self {
            self.sweeps.insert(addr, outcome);
            self
        }

        fn with_max_log_range(mut self, max: u64) -> Self {
            self.max_log_range = Some(max);
            self
        }

        fn with_hash_mismatch(mut self) -> Self {
            self.hash_mismatch = true;
            self
        }

        fn with_pending_receipt(mut self) -> Self {
            self.receipt_pending = true;
            self
        }

        fn with_receipt_block(mut self, block: u64) -> Self {
            self.receipt_block = Some(block);
            self
        }

        fn with_dropped_transaction(mut self) -> Self {
            self.transaction_known = false;
            self
        }

        fn with_latest_only_code(mut self, address: Address) -> Self {
            self.latest_only_code.insert(address);
            self
        }

        fn with_pending_signer_nonce(mut self) -> Self {
            self.signer_pending = true;
            self
        }
    }

    #[async_trait]
    impl ChainClient for MockChain {
        async fn get_block_number(&self) -> Result<u64, ChainError> {
            Ok(self.block)
        }

        async fn get_block_hash(&self, block: u64) -> Result<B256, ChainError> {
            Ok(if self.hash_mismatch {
                B256::repeat_byte(0xFF)
            } else {
                block_hash(block)
            })
        }

        async fn payment_deployed(&self, address: Address) -> Result<bool, ChainError> {
            Ok(self.latest_only_code.contains(&address)
                || matches!(
                    self.sweeps
                        .get(&address)
                        .copied()
                        .unwrap_or(MockSweep::Executed),
                    MockSweep::AlreadyDeployed
                )
                || (matches!(
                    self.sweeps
                        .get(&address)
                        .copied()
                        .unwrap_or(MockSweep::Executed),
                    MockSweep::Executed
                ) && (self.submitted.load(Ordering::Relaxed)
                    || self.receipt_observed.load(Ordering::Relaxed))))
        }

        async fn payment_deployed_at(
            &self,
            address: Address,
            _block_hash: B256,
        ) -> Result<bool, ChainError> {
            if self.latest_only_code.contains(&address) {
                return Ok(false);
            }
            self.payment_deployed(address).await
        }

        async fn transaction_known(&self, _tx_hash: B256) -> Result<bool, ChainError> {
            Ok(self.transaction_known)
        }

        async fn signer_has_pending_transaction(&self) -> Result<bool, ChainError> {
            Ok(self.signer_pending)
        }

        async fn get_sweep_receipt(
            &self,
            _tx_hash: B256,
            _batch_sweeper: Address,
        ) -> Result<Option<SweepReceipt>, ChainError> {
            if self.receipt_pending {
                return Ok(None);
            }
            self.receipt_observed.store(true, Ordering::Relaxed);
            let receipt_block = self.receipt_block.unwrap_or(self.block);
            Ok(Some(SweepReceipt {
                succeeded: true,
                block: receipt_block,
                block_hash: block_hash(receipt_block),
                failures: self
                    .sweeps
                    .iter()
                    .filter(|(_, outcome)| matches!(outcome, MockSweep::Blocked))
                    .map(|(payment, _)| (*payment, usdc()))
                    .collect(),
            }))
        }

        async fn get_usdc_transfers(
            &self,
            _token: Address,
            from_block: u64,
            to_block: u64,
        ) -> Result<Vec<UsdcTransfer>, ChainError> {
            let requested = to_block - from_block + 1;
            if self.max_log_range.is_some_and(|max| requested > max) {
                return Err(ChainError::LogRangeTooLarge(format!(
                    "mock limit is {} blocks",
                    self.max_log_range.unwrap()
                )));
            }
            Ok(self
                .transfers
                .iter()
                .filter(|transfer| {
                    transfer.block_number >= from_block && transfer.block_number <= to_block
                })
                .cloned()
                .collect())
        }

        async fn submit_sweep_batch(
            &self,
            _batch_sweeper: Address,
            _sweeps: &[SweepRequest],
        ) -> Result<B256, ChainError> {
            if self
                .sweeps
                .values()
                .any(|outcome| matches!(outcome, MockSweep::Transient))
            {
                return Err(ChainError::Transient("mock transient failure".to_string()));
            }
            self.submitted.store(true, Ordering::Relaxed);
            Ok(MOCK_TX_HASH)
        }
    }

    const CHAIN_ID: u64 = 31337;

    fn usdc() -> Address {
        address!("0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512")
    }

    fn block_hash(block: u64) -> B256 {
        B256::with_last_byte(block as u8)
    }

    fn factory() -> FactoryAddress {
        FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3"))
    }

    /// Build a USDC invoice for the test chain with the given atomic amount.
    fn make_invoice(amount: u64) -> Invoice {
        Invoice::new(
            factory(),
            ChainId(CHAIN_ID),
            TokenAddress(usdc()),
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01")),
            Amount(U256::from(amount)),
            1_900_000_000,
            RecoveryAddress(address!("0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC")),
        )
    }

    fn transfer(recipient: Address, amount: u64, block: u64, log_index: u64) -> UsdcTransfer {
        UsdcTransfer {
            block_number: block,
            block_hash: block_hash(block),
            transaction_hash: B256::with_last_byte((log_index + 1) as u8),
            transaction_index: 0,
            log_index,
            sender: address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"),
            recipient,
            amount: U256::from(amount),
        }
    }

    async fn insert(pool: &PgPool, invoice: &Invoice, key: &str) {
        let repo = InvoiceRepository::new(pool.clone());
        let account_id = uuid::Uuid::from_u128(1);
        sqlx::query(
            r#"INSERT INTO accounts (id, api_key_hash, api_key_hint)
               VALUES ($1, $2, 'test') ON CONFLICT (id) DO NOTHING"#,
        )
        .bind(account_id)
        .bind([1_u8; 32].as_slice())
        .execute(pool)
        .await
        .expect("test account should be inserted");
        let input = CreateInvoiceInput::from_invoice(
            invoice,
            gateway_db::AccountId(account_id),
            key.to_string(),
            USDC_DECIMALS,
        );
        repo.insert(&input)
            .await
            .expect("insert should succeed")
            .expect("row should be inserted");
    }

    /// Insert an invoice and advance it to `funded`, the starting state for the
    /// sweep pass.
    async fn insert_funded(pool: &PgPool, invoice: &Invoice, key: &str) {
        insert(pool, invoice, key).await;
        let repo = InvoiceRepository::new(pool.clone());
        repo.mark_funded(invoice.id.0, &invoice.amount.0.to_string(), 1)
            .await
            .expect("mark_funded should succeed");
    }

    async fn insert_submitted(pool: &PgPool, invoice: &Invoice, key: &str) {
        insert_funded(pool, invoice, key).await;
        let repo = InvoiceRepository::new(pool.clone());
        let claimed = repo.claim_sweep_batch(CHAIN_ID, 1, 0.0, 0.0).await.unwrap();
        assert_eq!(claimed.len(), 1);
        repo.record_batch_sweep_submission(&[invoice.id.0], MOCK_TX_HASH.as_slice())
            .await
            .unwrap();
    }

    fn indexer_with(pool: &PgPool, chain: MockChain) -> Indexer {
        Indexer::new(
            InvoiceRepository::new(pool.clone()),
            CursorRepository::new(pool.clone()),
            Arc::new(chain),
            ChainId(CHAIN_ID),
            factory().0,
            batch_sweeper(),
            usdc(),
            0,
            0,
            100,
            Duration::from_millis(10),
        )
    }

    fn batch_sweeper() -> Address {
        address!("0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0")
    }

    struct BlockingSweepChain {
        block: AtomicU64,
        transfers: Mutex<Vec<UsdcTransfer>>,
        submit_started: Notify,
        release_submit: Notify,
    }

    #[async_trait]
    impl ChainClient for BlockingSweepChain {
        async fn get_block_number(&self) -> Result<u64, ChainError> {
            Ok(self.block.load(Ordering::Relaxed))
        }

        async fn get_block_hash(&self, block: u64) -> Result<B256, ChainError> {
            Ok(block_hash(block))
        }

        async fn payment_deployed(&self, _address: Address) -> Result<bool, ChainError> {
            Ok(false)
        }

        async fn payment_deployed_at(
            &self,
            _address: Address,
            _block_hash: B256,
        ) -> Result<bool, ChainError> {
            Ok(false)
        }

        async fn transaction_known(&self, _tx_hash: B256) -> Result<bool, ChainError> {
            Ok(true)
        }

        async fn signer_has_pending_transaction(&self) -> Result<bool, ChainError> {
            Ok(false)
        }

        async fn get_sweep_receipt(
            &self,
            _tx_hash: B256,
            _batch_sweeper: Address,
        ) -> Result<Option<SweepReceipt>, ChainError> {
            Ok(None)
        }

        async fn get_usdc_transfers(
            &self,
            _token: Address,
            from_block: u64,
            to_block: u64,
        ) -> Result<Vec<UsdcTransfer>, ChainError> {
            Ok(self
                .transfers
                .lock()
                .await
                .iter()
                .filter(|transfer| (from_block..=to_block).contains(&transfer.block_number))
                .cloned()
                .collect())
        }

        async fn submit_sweep_batch(
            &self,
            _batch_sweeper: Address,
            _sweeps: &[SweepRequest],
        ) -> Result<B256, ChainError> {
            self.submit_started.notify_one();
            self.release_submit.notified().await;
            Ok(MOCK_TX_HASH)
        }
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn block_indexing_continues_while_batch_submission_is_blocked(pool: PgPool) {
        let sweep_invoice = make_invoice(100);
        let later_invoice = make_invoice(200);
        insert_funded(&pool, &sweep_invoice, "key-sweep").await;
        insert(&pool, &later_invoice, "key-later").await;

        let chain = Arc::new(BlockingSweepChain {
            block: AtomicU64::new(1),
            transfers: Mutex::new(Vec::new()),
            submit_started: Notify::new(),
            release_submit: Notify::new(),
        });
        let worker = Indexer::new(
            InvoiceRepository::new(pool.clone()),
            CursorRepository::new(pool.clone()),
            chain.clone(),
            ChainId(CHAIN_ID),
            factory().0,
            batch_sweeper(),
            usdc(),
            1,
            0,
            100,
            Duration::from_millis(10),
        );
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(worker.run(shutdown_rx));

        chain.submit_started.notified().await;
        chain
            .transfers
            .lock()
            .await
            .push(transfer(later_invoice.payment_address.0, 200, 2, 0));
        chain.block.store(2, Ordering::Relaxed);

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let row = InvoiceRepository::new(pool.clone())
                    .find_by_id(later_invoice.id.0)
                    .await
                    .unwrap()
                    .unwrap();
                if row.status == "funded" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("index loop must not wait for the blocked sweep worker");

        chain.release_submit.notify_one();
        shutdown_tx.send(true).unwrap();
        task.await.unwrap().unwrap();
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn funds_invoice_from_usdc_transfer(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(
            &pool,
            MockChain::new(1, vec![transfer(invoice.payment_address.0, 100, 1, 0)]),
        );

        indexer.tick().await.expect("tick should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "funded");
        assert_eq!(row.observed_amount.as_deref(), Some("100"));
        assert_eq!(row.confirmed_received, "100");
        assert_eq!(row.funded_at_block, Some(1));
        assert_eq!(
            row.funded_at_block_hash.as_deref(),
            Some(block_hash(1).as_slice())
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn overpayment_still_funds(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(
            &pool,
            MockChain::new(1, vec![transfer(invoice.payment_address.0, 150, 1, 0)]),
        );

        indexer.tick().await.expect("tick should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "funded");
        assert_eq!(row.observed_amount.as_deref(), Some("150"));
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn partial_transfers_accumulate(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(
            &pool,
            MockChain::new(
                2,
                vec![
                    transfer(invoice.payment_address.0, 40, 1, 0),
                    transfer(invoice.payment_address.0, 60, 2, 1),
                ],
            ),
        );

        indexer.tick().await.expect("tick should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "funded");
        assert_eq!(row.confirmed_received, "100");
        assert_eq!(row.funded_at_block, Some(2));
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn tick_is_idempotent(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let logs = vec![transfer(invoice.payment_address.0, 100, 1, 0)];
        let indexer = indexer_with(&pool, MockChain::new(1, logs.clone()));
        indexer.tick().await.expect("first tick");

        let indexer2 = indexer_with(&pool, MockChain::new(5, logs));
        indexer2.tick().await.expect("second tick");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "funded");
        assert_eq!(row.funded_at_block, Some(1), "funding block must be stable");
        assert_eq!(row.confirmed_received, "100");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn transfers_after_funding_remain_in_the_observation_ledger(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        indexer_with(
            &pool,
            MockChain::new(1, vec![transfer(invoice.payment_address.0, 100, 1, 0)]),
        )
        .tick()
        .await
        .unwrap();
        indexer_with(
            &pool,
            MockChain::new(2, vec![transfer(invoice.payment_address.0, 50, 2, 1)]),
        )
        .tick()
        .await
        .unwrap();

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "funded");
        assert_eq!(row.confirmed_received, "150");
        let observations: i64 =
            sqlx::query_scalar("SELECT count(*) FROM payment_observations WHERE invoice_id = $1")
                .bind(invoice.id.0)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(observations, 2);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn zero_value_transfer_is_durably_marked_as_error(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        indexer_with(
            &pool,
            MockChain::new(1, vec![transfer(invoice.payment_address.0, 0, 1, 0)]),
        )
        .tick()
        .await
        .unwrap();

        let observation: (String, Option<String>) = sqlx::query_as(
            "SELECT disposition, disposition_reason FROM payment_observations WHERE invoice_id = $1",
        )
        .bind(invoice.id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            observation,
            ("error".to_string(), Some("zero_amount".to_string()))
        );

        let row = InvoiceRepository::new(pool.clone())
            .find_by_id(invoice.id.0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "created");
        assert_eq!(row.confirmed_received, "0");
        assert_eq!(
            CursorRepository::new(pool)
                .get(CHAIN_ID, usdc())
                .await
                .unwrap()
                .unwrap()
                .block,
            1,
            "error disposition and cursor must commit together"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn transfer_to_terminal_invoice_is_durably_blocked(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;
        sqlx::query("UPDATE invoices SET status = 'fulfilled' WHERE id = $1")
            .bind(invoice.id.0)
            .execute(&pool)
            .await
            .unwrap();

        indexer_with(
            &pool,
            MockChain::new(1, vec![transfer(invoice.payment_address.0, 50, 1, 0)]),
        )
        .tick()
        .await
        .unwrap();

        let observation: (String, Option<String>) = sqlx::query_as(
            "SELECT disposition, disposition_reason FROM payment_observations WHERE invoice_id = $1",
        )
        .bind(invoice.id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            observation,
            (
                "blocked".to_string(),
                Some("invoice_not_accepting_payments".to_string())
            )
        );

        let row = InvoiceRepository::new(pool)
            .find_by_id(invoice.id.0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "fulfilled");
        assert_eq!(row.confirmed_received, "0");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn accounting_error_rolls_back_observation_and_cursor(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;
        sqlx::query("UPDATE invoices SET confirmed_received = $2 WHERE id = $1")
            .bind(invoice.id.0)
            .bind(U256::MAX.to_string())
            .execute(&pool)
            .await
            .unwrap();

        let error = indexer_with(
            &pool,
            MockChain::new(1, vec![transfer(invoice.payment_address.0, 1, 1, 0)]),
        )
        .tick()
        .await
        .unwrap_err();
        assert!(error.to_string().contains("overflowed uint256"));

        let observations: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_observations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(observations, 0);
        assert!(
            CursorRepository::new(pool)
                .get(CHAIN_ID, usdc())
                .await
                .unwrap()
                .is_none(),
            "cursor must not advance past an uncommitted relevant log"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn finalized_cursor_mismatch_halts_before_sweeping(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let repo = InvoiceRepository::new(pool.clone());
        repo.apply_finalized_usdc_range(CHAIN_ID, usdc(), None, 1, block_hash(1), &[])
            .await
            .unwrap();

        let indexer = indexer_with(&pool, MockChain::new(2, vec![]).with_hash_mismatch());
        let error = indexer.tick().await.unwrap_err();

        assert!(error.requires_halt());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "funded", "halt must happen before sweep");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn oversized_log_ranges_are_split_and_then_grow(pool: PgPool) {
        let mut indexer = indexer_with(&pool, MockChain::new(10, vec![]).with_max_log_range(2));
        indexer.max_log_range_size = 8;
        indexer.current_log_range_size.store(8, Ordering::Relaxed);

        indexer
            .tick()
            .await
            .expect("range should split and succeed");

        let cursor = CursorRepository::new(pool)
            .get(CHAIN_ID, usdc())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cursor.block, 1, "the successful range was blocks 0..=1");
        assert_eq!(
            indexer.current_log_range_size.load(Ordering::Relaxed),
            3,
            "a successful small range should cautiously grow"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn confirmation_depth_gates_funding(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let mut indexer = indexer_with(
            &pool,
            MockChain::new(1, vec![transfer(invoice.payment_address.0, 100, 1, 0)]),
        );
        indexer.finality_confirmations = 1;

        indexer.tick().await.expect("tick should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "created");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn ignores_invoices_on_other_chains(pool: PgPool) {
        // An invoice on a different chain must not be reconciled by this indexer.
        let invoice = Invoice::new(
            factory(),
            ChainId(1), // not CHAIN_ID
            TokenAddress(usdc()),
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01")),
            Amount(U256::from(100)),
            1_900_000_000,
            RecoveryAddress(address!("0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC")),
        );
        insert(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(
            &pool,
            MockChain::new(1, vec![transfer(invoice.payment_address.0, 100, 1, 0)]),
        );

        indexer.tick().await.expect("tick should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "created");
    }

    // --- Sweep pass ---------------------------------------------------------

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn sweeps_funded_invoice_to_fulfilled(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        // Default mock sweep is Executed at the mock head block.
        let indexer = indexer_with(&pool, MockChain::new(7, vec![]));
        indexer.sweep_tick().await.expect("sweep should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "fulfilled");
        assert_eq!(
            row.execute_tx_hash.as_deref(),
            Some(MOCK_TX_HASH.as_slice())
        );
        assert_eq!(row.fulfilled_at_block, Some(7));
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn sweep_rejects_invoice_from_another_factory(pool: PgPool) {
        let invoice_factory =
            FactoryAddress(address!("0x0000000000000000000000000000000000000009"));
        let invoice = Invoice::new(
            invoice_factory,
            ChainId(CHAIN_ID),
            TokenAddress(usdc()),
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01")),
            Amount(U256::from(100)),
            1_900_000_000,
            RecoveryAddress(address!("0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC")),
        );
        insert_funded(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(&pool, MockChain::new(7, vec![]));
        let error = indexer.sweep_tick().await.unwrap_err();
        assert!(error.requires_halt());

        let row = InvoiceRepository::new(pool)
            .find_by_id(invoice.id.0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "deploying");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn already_deployed_marks_fulfilled_without_tx(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(
            &pool,
            MockChain::new(7, vec![])
                .with_sweep(invoice.payment_address.0, MockSweep::AlreadyDeployed),
        );
        indexer.sweep_tick().await.expect("sweep should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "fulfilled");
        // No transaction of ours landed it, so no transaction hash is recorded.
        assert_eq!(row.execute_tx_hash, None);
        assert_eq!(row.fulfilled_at_block, Some(7));
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn mined_sweep_waits_for_finality(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let mut indexer = indexer_with(&pool, MockChain::new(7, vec![]));
        indexer.finality_confirmations = 2;
        indexer
            .sweep_tick()
            .await
            .expect("submission should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "deploying");
        assert_eq!(row.sweep_detected_at_block, Some(7));

        let mut later = indexer_with(&pool, MockChain::new(9, vec![]).with_receipt_block(7));
        later.finality_confirmations = 2;
        later
            .sweep_tick()
            .await
            .expect("finalization should succeed");

        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "fulfilled");
        assert_eq!(row.fulfilled_at_block, Some(7));
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn submitted_sweep_hash_is_persisted_before_receipt(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(&pool, MockChain::new(7, vec![]).with_pending_receipt());
        indexer
            .sweep_tick()
            .await
            .expect("submission should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "deploying");
        assert_eq!(
            row.execute_tx_hash.as_deref(),
            Some(MOCK_TX_HASH.as_slice())
        );
        assert!(row.sweep_submitted_at.is_some());
        assert_eq!(row.sweep_detected_at_block, None);

        let later = indexer_with(&pool, MockChain::new(8, vec![]));
        later
            .sweep_tick()
            .await
            .expect("receipt recovery should succeed");
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "fulfilled");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn pending_batch_prevents_a_higher_nonce_submission(pool: PgPool) {
        let pending = make_invoice(100);
        let waiting = make_invoice(200);
        insert_submitted(&pool, &pending, "key-pending").await;
        insert_funded(&pool, &waiting, "key-waiting").await;

        let indexer = indexer_with(&pool, MockChain::new(7, vec![]).with_pending_receipt());
        indexer.sweep_tick().await.unwrap();

        let waiting = InvoiceRepository::new(pool)
            .find_by_id(waiting.id.0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(waiting.status, "funded");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn untracked_pending_signer_nonce_halts_before_submission(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-funded").await;
        let indexer = indexer_with(&pool, MockChain::new(7, vec![]).with_pending_signer_nonce());

        let error = indexer.sweep_tick().await.unwrap_err();
        assert!(matches!(error, IndexerError::PendingSweep(_)));
        let row = InvoiceRepository::new(pool)
            .find_by_id(invoice.id.0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.execute_tx_hash, None);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn dropped_batch_returns_all_members_to_retry_queue(pool: PgPool) {
        let first = make_invoice(100);
        let second = make_invoice(200);
        insert_funded(&pool, &first, "key-first").await;
        insert_funded(&pool, &second, "key-second").await;
        let repo = InvoiceRepository::new(pool.clone());
        let claimed = repo.claim_sweep_batch(CHAIN_ID, 2, 0.0, 0.0).await.unwrap();
        assert_eq!(claimed.len(), 2);
        repo.record_batch_sweep_submission(&[first.id.0, second.id.0], MOCK_TX_HASH.as_slice())
            .await
            .unwrap();

        let mut indexer = indexer_with(
            &pool,
            MockChain::new(7, vec![])
                .with_pending_receipt()
                .with_dropped_transaction(),
        );
        indexer.pending_sweep_timeout = Duration::ZERO;
        indexer.sweep_tick().await.unwrap();

        for id in [first.id.0, second.id.0] {
            let row = repo.find_by_id(id).await.unwrap().unwrap();
            assert_eq!(row.status, "deploying");
            assert_eq!(row.execute_tx_hash, None);
            assert_eq!(row.sweep_attempts, 1);
        }
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn stale_but_known_pending_batch_halts(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_submitted(&pool, &invoice, "key-pending").await;
        let mut indexer = indexer_with(&pool, MockChain::new(7, vec![]).with_pending_receipt());
        indexer.pending_sweep_timeout = Duration::ZERO;

        let error = indexer.sweep_tick().await.unwrap_err();
        assert!(matches!(error, IndexerError::PendingSweep(_)));
        assert!(error.requires_halt());
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn failure_classification_reads_code_at_receipt_block(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_submitted(&pool, &invoice, "key-failure").await;
        let indexer = indexer_with(
            &pool,
            MockChain::new(7, vec![])
                .with_sweep(invoice.payment_address.0, MockSweep::Blocked)
                .with_latest_only_code(invoice.payment_address.0),
        );

        indexer.sweep_tick().await.unwrap();

        let row = InvoiceRepository::new(pool)
            .find_by_id(invoice.id.0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "blocked");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn deterministic_token_failure_marks_blocked(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(
            &pool,
            MockChain::new(7, vec![]).with_sweep(invoice.payment_address.0, MockSweep::Blocked),
        );
        indexer.sweep_tick().await.expect("sweep should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "blocked");
        assert_eq!(row.blocked_reason.as_deref(), Some("token_transfer_failed"));
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn mixed_invoices_share_one_batch_hash_and_resolve_independently(pool: PgPool) {
        let successful = make_invoice(100);
        let blocked = make_invoice(200);
        insert_funded(&pool, &successful, "key-success").await;
        insert_funded(&pool, &blocked, "key-blocked").await;

        let indexer = indexer_with(
            &pool,
            MockChain::new(7, vec![]).with_sweep(blocked.payment_address.0, MockSweep::Blocked),
        );
        indexer.sweep_tick().await.expect("batch should succeed");

        let repo = InvoiceRepository::new(pool);
        let successful_row = repo.find_by_id(successful.id.0).await.unwrap().unwrap();
        let blocked_row = repo.find_by_id(blocked.id.0).await.unwrap().unwrap();
        assert_eq!(successful_row.status, "fulfilled");
        assert_eq!(blocked_row.status, "blocked");
        assert_eq!(
            successful_row.execute_tx_hash, blocked_row.execute_tx_hash,
            "both invoice outcomes must come from one helper transaction"
        );
        assert_eq!(
            successful_row.execute_tx_hash.as_deref(),
            Some(MOCK_TX_HASH.as_slice())
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn transient_failure_increments_attempts_and_stays_deploying(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(
            &pool,
            MockChain::new(7, vec![]).with_sweep(invoice.payment_address.0, MockSweep::Transient),
        );
        let error = indexer.sweep_tick().await.unwrap_err();
        assert!(matches!(
            error,
            IndexerError::Chain(ChainError::Transient(_))
        ));

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(
            row.status, "deploying",
            "claimed but not resolved -> retryable"
        );
        assert_eq!(row.sweep_attempts, 1);
        assert!(row.last_attempt_at.is_some());
        assert_eq!(row.blocked_reason, None);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn sweep_ignores_created_invoices(pool: PgPool) {
        // A `created` (unfunded) invoice must not be swept.
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(&pool, MockChain::new(7, vec![]));
        indexer.sweep_tick().await.expect("sweep should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "created");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn sweep_ignores_other_chains(pool: PgPool) {
        // A funded invoice on another chain must not be swept by this indexer.
        let invoice = Invoice::new(
            factory(),
            ChainId(1), // not CHAIN_ID
            TokenAddress(usdc()),
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01")),
            Amount(U256::from(100)),
            1_900_000_000,
            RecoveryAddress(address!("0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC")),
        );
        insert_funded(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(&pool, MockChain::new(7, vec![]));
        indexer.sweep_tick().await.expect("sweep should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(
            row.status, "funded",
            "other-chain invoice must be untouched"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn claim_is_idempotent(pool: PgPool) {
        // funded -> deploying claims exactly once; a second claim is empty.
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let repo = InvoiceRepository::new(pool.clone());
        let first = repo
            .claim_sweep_batch(CHAIN_ID, 1, 2.0, 300.0)
            .await
            .unwrap();
        let second = repo
            .claim_sweep_batch(CHAIN_ID, 1, 2.0, 300.0)
            .await
            .unwrap();
        assert_eq!(first.len(), 1);
        assert!(second.is_empty());

        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "deploying");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn retryable_failures_never_make_an_invoice_terminal(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let repo = InvoiceRepository::new(pool.clone());
        repo.claim_sweep_batch(CHAIN_ID, 1, 2.0, 300.0)
            .await
            .unwrap();

        for _ in 0..20 {
            assert!(repo.record_sweep_retry(invoice.id.0).await.unwrap());
        }

        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "deploying");
        assert_eq!(row.blocked_reason, None);
        assert_eq!(row.sweep_attempts, 20);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn backoff_gates_deploying_reselection(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let repo = InvoiceRepository::new(pool.clone());
        repo.claim_sweep_batch(CHAIN_ID, 1, 2.0, 300.0)
            .await
            .unwrap();
        // One failure just now: attempts=1, last_attempt_at=now().
        repo.record_sweep_retry(invoice.id.0).await.unwrap();

        // With a large base the backoff has not elapsed -> excluded.
        let excluded = repo
            .claim_sweep_batch(CHAIN_ID, 10, 1000.0, 100_000.0)
            .await
            .unwrap();
        assert!(
            !excluded.iter().any(|r| r.id == invoice.id.0),
            "recently-failed invoice must wait for backoff"
        );

        // With a zero base it is immediately eligible again -> included.
        let included = repo
            .claim_sweep_batch(CHAIN_ID, 10, 0.0, 0.0)
            .await
            .unwrap();
        assert!(
            included.iter().any(|r| r.id == invoice.id.0),
            "invoice must be reselected once backoff elapses"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn fulfilled_invoice_is_not_reswept(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        // First pass fulfills it.
        let indexer = indexer_with(&pool, MockChain::new(7, vec![]));
        indexer.sweep_tick().await.expect("first sweep");

        // A second pass must leave it fulfilled with the original tx hash/block
        // (list_sweepable excludes terminal states).
        let indexer2 = indexer_with(&pool, MockChain::new(9, vec![]));
        indexer2.sweep_tick().await.expect("second sweep");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "fulfilled");
        assert_eq!(row.fulfilled_at_block, Some(7), "must not be re-fulfilled");
    }
}
