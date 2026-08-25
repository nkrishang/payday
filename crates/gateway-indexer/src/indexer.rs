//! Payment indexer worker.
//!
//! A supervised worker that ingests finalized USDC `Transfer` logs and advances
//! invoices `created -> funded -> deploying -> fulfilled`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use alloy_primitives::{Address, B256};
use gateway_core::{ChainId, Invoice, InvoiceStatus};
use thiserror::Error;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;
use tracing::{info, warn};

use crate::chain::{ChainClient, ChainError, SweepOutcome};
use gateway_db::{CursorRepository, InvoiceRepository, PaymentObservation};

/// Maximum invoices swept in a single pass. Sweeps are sent sequentially (one
/// signer, so nonces must be ordered), so this bounds the per-pass work.
const SWEEP_BATCH_LIMIT: i64 = 500;

/// Exponential backoff for retrying a transient sweep failure: the nth retry
/// waits `min(BASE * 2^attempts, CAP)` seconds, so a struggling RPC is not
/// hammered while a brief blip still recovers quickly.
const SWEEP_BACKOFF_BASE_SECS: f64 = 2.0;
const SWEEP_BACKOFF_CAP_SECS: f64 = 300.0;

/// Errors that can abort a poll pass. Transient failures are retried; permanent
/// RPC and finality failures escape the run loop so ECS restarts and alerts
/// rather than leaving an inert process marked healthy.
#[derive(Debug, Error)]
pub enum IndexerError {
    #[error("chain error: {0}")]
    Chain(#[from] ChainError),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

impl IndexerError {
    fn requires_halt(&self) -> bool {
        matches!(self, Self::Chain(ChainError::FinalityViolation(_)))
            || matches!(self, Self::Chain(error) if error.is_permanent_rpc())
    }
}

/// The payment indexer. Owns its own repository handles and chain client; it is
/// not shared with the HTTP handlers.
pub struct Indexer {
    repo: InvoiceRepository,
    cursor: CursorRepository,
    chain: Arc<dyn ChainClient>,
    chain_id: ChainId,
    usdc: Address,
    usdc_start_block: u64,
    finality_confirmations: u64,
    max_log_range_size: u64,
    current_log_range_size: AtomicU64,
    poll_interval: Duration,
}

impl Indexer {
    pub fn new(
        repo: InvoiceRepository,
        cursor: CursorRepository,
        chain: Arc<dyn ChainClient>,
        chain_id: ChainId,
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
            usdc,
            usdc_start_block,
            finality_confirmations,
            max_log_range_size: log_range_size,
            current_log_range_size: AtomicU64::new(log_range_size),
            poll_interval,
        }
    }

    /// Run the poll loop until `shutdown` is set to `true`. Permanent RPC and
    /// finality errors are returned so the process supervisor cannot mistake a
    /// halted payment worker for a healthy one.
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) -> Result<(), IndexerError> {
        let mut interval = tokio::time::interval(self.poll_interval);
        // If a tick is delayed (e.g. a slow RPC pass), don't fire a burst of
        // catch-up ticks afterwards; just resume the cadence.
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        info!(
            chain_id = self.chain_id.0,
            usdc = %self.usdc,
            finality_confirmations = self.finality_confirmations,
            poll_interval_ms = self.poll_interval.as_millis() as u64,
            "indexer started"
        );

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.poll_once().await?;
                }
                changed = shutdown.changed() => {
                    // `Err` means the sender was dropped -> treat as shutdown.
                    if changed.is_err() || *shutdown.borrow() {
                        info!("indexer shutting down");
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn poll_once(&self) -> Result<(), IndexerError> {
        if let Err(error) = self.tick().await {
            if error.requires_halt() {
                warn!(error = %error, "indexer halted after a non-retryable safety or RPC error; operator intervention required");
                return Err(error);
            }
            warn!(error = %error, "indexer poll failed; retrying next tick");
        }

        // Existing finalized funding may still be swept during an ordinary RPC
        // outage, but never after canonical history has been disputed.
        if let Err(error) = self.sweep_tick().await {
            if error.requires_halt() {
                warn!(error = %error, "indexer halted after a non-retryable sweep RPC error; operator intervention required");
                return Err(error);
            } else {
                warn!(error = %error, "indexer sweep pass failed; retrying next tick");
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
            .filter(|transfer| !transfer.amount.is_zero())
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

    /// One sweep pass: claim funded invoices and execute their payments, and
    /// retry any in-flight (`deploying`) invoices whose backoff has elapsed.
    ///
    /// Invoices are processed sequentially: the sweep uses a single signing key,
    /// so its transactions must take ordered nonces. The batch is bounded by
    /// [`SWEEP_BATCH_LIMIT`]; the rest are picked up on later passes.
    async fn sweep_tick(&self) -> Result<(), IndexerError> {
        let rows = self
            .repo
            .list_sweepable(
                self.chain_id.0,
                SWEEP_BATCH_LIMIT,
                SWEEP_BACKOFF_BASE_SECS,
                SWEEP_BACKOFF_CAP_SECS,
            )
            .await?;

        for row in &rows {
            let invoice = match Invoice::try_from(row) {
                Ok(invoice) => invoice,
                Err(e) => {
                    warn!(invoice_id = %row.id, error = %e, "skipping corrupt invoice row");
                    continue;
                }
            };

            if let Err(e) = self.sweep_invoice(&invoice).await {
                if e.requires_halt() {
                    return Err(e);
                }
                warn!(invoice_id = %invoice.id.0, error = %e, "sweep pass error for invoice");
            }
        }

        Ok(())
    }

    /// Sweep one invoice: claim it if newly `funded`, call `execute`, and apply
    /// the resulting transition. A `deploying` invoice is already claimed (a
    /// prior attempt / crash recovery), so it is retried without re-claiming.
    async fn sweep_invoice(&self, invoice: &Invoice) -> Result<(), IndexerError> {
        let row = self
            .repo
            .find_by_id(invoice.id.0)
            .await?
            .ok_or_else(|| sqlx::Error::RowNotFound)?;
        if let (Some(block), Some(hash)) = (
            row.sweep_detected_at_block,
            row.sweep_detected_at_block_hash.as_deref(),
        ) {
            let block_hash = B256::try_from(hash)
                .map_err(|_| sqlx::Error::Decode("invalid sweep detection block hash".into()))?;
            return self
                .finalize_detected_sweep(
                    invoice,
                    block as u64,
                    block_hash,
                    row.execute_tx_hash.as_deref(),
                )
                .await;
        }
        if let Some(tx_hash) = row.execute_tx_hash.as_deref() {
            let tx_hash = B256::try_from(tx_hash)
                .map_err(|_| sqlx::Error::Decode("invalid execute transaction hash".into()))?;
            return self.process_submitted_sweep(invoice, tx_hash).await;
        }

        // Claim `funded -> deploying` before sending, so a concurrent worker or
        // the next tick cannot double-submit. Losing the claim means someone
        // else owns it now; skip.
        if invoice.status == InvoiceStatus::Funded {
            let claimed = self.repo.mark_deploying(invoice.id.0).await?;
            if !claimed {
                return Ok(());
            }
        }

        let outcome = self
            .chain
            .sweep(
                invoice.factory.0,
                invoice.token.0,
                invoice.amount.0,
                invoice.beneficiary.0,
                invoice.expiration_timestamp,
                invoice.recovery.0,
                invoice.salt.0,
                invoice.payment_address.0,
            )
            .await;

        match outcome {
            Ok(SweepOutcome::Submitted { tx_hash }) => {
                self.repo
                    .record_sweep_submission(invoice.id.0, tx_hash.as_slice())
                    .await?;
                self.process_submitted_sweep(invoice, tx_hash).await?;
            }
            Ok(SweepOutcome::AlreadyDeployed) => {
                let block = self.chain.get_block_number().await?;
                let block_hash = self.chain.get_block_hash(block).await?;
                self.repo
                    .record_sweep_detection(invoice.id.0, None, block, block_hash.as_slice())
                    .await?;
                self.finalize_detected_sweep(invoice, block, block_hash, None)
                    .await?;
            }
            Ok(SweepOutcome::TokenTransferFailed) => {
                if self
                    .repo
                    .mark_blocked(invoice.id.0, "token_transfer_failed")
                    .await?
                {
                    warn!(
                        invoice_id = %invoice.id.0,
                        beneficiary = %invoice.beneficiary.0,
                        "invoice blocked: USDC transfer failed deterministically"
                    );
                }
            }
            Err(e) => {
                if !e.is_retryable() {
                    return Err(e.into());
                }
                self.repo.record_sweep_retry(invoice.id.0).await?;
                warn!(invoice_id = %invoice.id.0, error = %e, "retryable sweep failure; will retry after backoff");
            }
        }

        Ok(())
    }

    async fn process_submitted_sweep(
        &self,
        invoice: &Invoice,
        tx_hash: B256,
    ) -> Result<(), IndexerError> {
        let Some(receipt) = self.chain.get_sweep_receipt(tx_hash).await? else {
            return Ok(());
        };
        if !receipt.succeeded {
            self.repo.clear_sweep_submission(invoice.id.0).await?;
            warn!(invoice_id = %invoice.id.0, %tx_hash, "execute transaction reverted; clearing submission for classification and retry");
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
        self.finalize_detected_sweep(
            invoice,
            receipt.block,
            receipt.block_hash,
            Some(tx_hash.as_slice()),
        )
        .await
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
        let still_deployed = self
            .chain
            .payment_deployed(invoice.payment_address.0)
            .await?;
        if canonical_hash != detected_hash || !still_deployed {
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

    use std::collections::HashMap;

    use alloy_primitives::{Address, B256, U256, address};
    use async_trait::async_trait;
    use gateway_core::{
        Amount, BeneficiaryAddress, ChainId, FactoryAddress, Invoice, RecoveryAddress,
        TokenAddress, USDC_DECIMALS,
    };
    use sqlx::PgPool;

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
        expected_factory: Option<Address>,
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
                expected_factory: None,
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

        fn with_expected_factory(mut self, factory: Address) -> Self {
            self.expected_factory = Some(factory);
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
            Ok(matches!(
                self.sweeps
                    .get(&address)
                    .copied()
                    .unwrap_or(MockSweep::Executed),
                MockSweep::Executed | MockSweep::AlreadyDeployed
            ))
        }

        async fn get_sweep_receipt(
            &self,
            _tx_hash: B256,
        ) -> Result<Option<SweepReceipt>, ChainError> {
            if self.receipt_pending {
                return Ok(None);
            }
            Ok(Some(SweepReceipt {
                succeeded: true,
                block: self.block,
                block_hash: block_hash(self.block),
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

        async fn sweep(
            &self,
            factory: Address,
            _token: Address,
            _amount: U256,
            _receiver: Address,
            _expiration_timestamp: u64,
            _recovery: Address,
            _salt: B256,
            payment_address: Address,
        ) -> Result<SweepOutcome, ChainError> {
            if let Some(expected) = self.expected_factory {
                assert_eq!(factory, expected, "sweep must use the invoice's factory");
            }
            match self
                .sweeps
                .get(&payment_address)
                .copied()
                .unwrap_or(MockSweep::Executed)
            {
                MockSweep::Executed => Ok(SweepOutcome::Submitted {
                    tx_hash: MOCK_TX_HASH,
                }),
                MockSweep::AlreadyDeployed => Ok(SweepOutcome::AlreadyDeployed),
                MockSweep::Blocked => Ok(SweepOutcome::TokenTransferFailed),
                MockSweep::Transient => {
                    Err(ChainError::Transient("mock transient failure".to_string()))
                }
            }
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
        let input = CreateInvoiceInput::from_invoice(invoice, key.to_string(), USDC_DECIMALS);
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

    fn indexer_with(pool: &PgPool, chain: MockChain) -> Indexer {
        Indexer::new(
            InvoiceRepository::new(pool.clone()),
            CursorRepository::new(pool.clone()),
            Arc::new(chain),
            ChainId(CHAIN_ID),
            usdc(),
            0,
            0,
            100,
            Duration::from_millis(10),
        )
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
    async fn finalized_cursor_mismatch_halts_before_sweeping(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let repo = InvoiceRepository::new(pool.clone());
        repo.apply_finalized_usdc_range(CHAIN_ID, usdc(), None, 1, block_hash(1), &[])
            .await
            .unwrap();

        let indexer = indexer_with(&pool, MockChain::new(2, vec![]).with_hash_mismatch());
        let error = indexer.poll_once().await.unwrap_err();

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
    async fn sweep_uses_factory_stored_on_invoice(pool: PgPool) {
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

        let indexer = indexer_with(
            &pool,
            MockChain::new(7, vec![]).with_expected_factory(invoice_factory.0),
        );
        indexer.sweep_tick().await.expect("sweep should succeed");

        let row = InvoiceRepository::new(pool)
            .find_by_id(invoice.id.0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "fulfilled");
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

        let mut later = indexer_with(&pool, MockChain::new(9, vec![]));
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
    async fn transient_failure_increments_attempts_and_stays_deploying(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(
            &pool,
            MockChain::new(7, vec![]).with_sweep(invoice.payment_address.0, MockSweep::Transient),
        );
        indexer.sweep_tick().await.expect("sweep should succeed");

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
        // funded -> deploying claims exactly once; a second claim returns false.
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let repo = InvoiceRepository::new(pool.clone());
        assert!(repo.mark_deploying(invoice.id.0).await.unwrap());
        assert!(!repo.mark_deploying(invoice.id.0).await.unwrap());

        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "deploying");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn retryable_failures_never_make_an_invoice_terminal(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let repo = InvoiceRepository::new(pool.clone());
        repo.mark_deploying(invoice.id.0).await.unwrap();

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
        repo.mark_deploying(invoice.id.0).await.unwrap();
        // One failure just now: attempts=1, last_attempt_at=now().
        repo.record_sweep_retry(invoice.id.0).await.unwrap();

        // With a large base the backoff has not elapsed -> excluded.
        let excluded = repo
            .list_sweepable(CHAIN_ID, 10, 1000.0, 100_000.0)
            .await
            .unwrap();
        assert!(
            !excluded.iter().any(|r| r.id == invoice.id.0),
            "recently-failed invoice must wait for backoff"
        );

        // With a zero base it is immediately eligible again -> included.
        let included = repo.list_sweepable(CHAIN_ID, 10, 0.0, 0.0).await.unwrap();
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
