//! Payment indexer worker.
//!
//! A supervised background task that reconciles counterfactual payment addresses
//! against their required amounts and advances invoices `created -> funded`.
//!
//! Detection is by **canonical balance reconciliation**: each poll pass reads
//! the current native-token balance of every unpaid invoice's payment address
//! and, when it meets the required amount, transitions the invoice. Native
//! funding emits no log, so the balance is the source of truth (this also
//! matches the native-only create API today). The transition is an idempotent
//! compare-and-set, so re-runs, duplicate polls, and concurrent workers are
//! safe. No DB transaction is held across RPC calls.

use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::{Address, U256};
use futures::future::join_all;
use gateway_core::{ChainId, Invoice, InvoiceStatus};
use thiserror::Error;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;
use tracing::{info, warn};

use crate::chain::{ChainClient, ChainError, SweepOutcome};
use gateway_db::{CursorRepository, InvoiceRepository};

/// Maximum invoices reconciled in a single poll pass, so a large backlog cannot
/// stall one tick. Remaining invoices are picked up on subsequent passes.
const RECONCILE_BATCH_LIMIT: i64 = 500;

/// Payment addresses read per Multicall3 `aggregate` call. One `eth_call`
/// carries this many balance reads; a full pass splits its invoices into chunks
/// of this size and runs the chunks concurrently. Kept well under the point
/// where a node's `eth_call` gas/response limits start rejecting large
/// multicalls (empirically ~1k calls), leaving headroom.
const MULTICALL_CHUNK_SIZE: usize = 200;

/// Maximum invoices swept in a single pass. Sweeps are sent sequentially (one
/// signer, so nonces must be ordered), so this bounds the per-pass work.
const SWEEP_BATCH_LIMIT: i64 = 500;

/// Transient sweep failures tolerated before an invoice is blocked for manual
/// intervention. A contract revert (receiver rejects) blocks immediately and
/// does not count against this budget.
const MAX_SWEEP_ATTEMPTS: i32 = 10;

/// Exponential backoff for retrying a transient sweep failure: the nth retry
/// waits `min(BASE * 2^attempts, CAP)` seconds, so a struggling RPC is not
/// hammered while a brief blip still recovers quickly.
const SWEEP_BACKOFF_BASE_SECS: f64 = 2.0;
const SWEEP_BACKOFF_CAP_SECS: f64 = 300.0;

/// Errors that can abort a single poll pass. All are transient or indicate
/// out-of-contract data; the worker logs and retries on the next tick rather
/// than crashing the process.
#[derive(Debug, Error)]
pub enum IndexerError {
    #[error("chain error: {0}")]
    Chain(#[from] ChainError),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

/// The payment indexer. Owns its own repository handles and chain client; it is
/// not shared with the HTTP handlers.
pub struct Indexer {
    repo: InvoiceRepository,
    cursor: CursorRepository,
    chain: Arc<dyn ChainClient>,
    chain_id: ChainId,
    factory: Address,
    poll_interval: Duration,
}

impl Indexer {
    pub fn new(
        repo: InvoiceRepository,
        cursor: CursorRepository,
        chain: Arc<dyn ChainClient>,
        chain_id: ChainId,
        factory: Address,
        poll_interval: Duration,
    ) -> Self {
        Self {
            repo,
            cursor,
            chain,
            chain_id,
            factory,
            poll_interval,
        }
    }

    /// Run the poll loop until `shutdown` is set to `true`. Returns when a
    /// shutdown is signalled or the shutdown channel is dropped, so the caller
    /// can `await` the task for a clean stop.
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let mut interval = tokio::time::interval(self.poll_interval);
        // If a tick is delayed (e.g. a slow RPC pass), don't fire a burst of
        // catch-up ticks afterwards; just resume the cadence.
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        info!(
            chain_id = self.chain_id.0,
            poll_interval_ms = self.poll_interval.as_millis() as u64,
            "indexer started"
        );

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    // Funding pass: created -> funded (gated on a new block).
                    if let Err(e) = self.tick().await {
                        warn!(error = %e, "indexer poll failed; retrying next tick");
                    }
                    // Sweep pass: funded -> deploying -> fulfilled/blocked. Not
                    // gated on a new block — a funded invoice can be swept as
                    // soon as it is observed, and retries follow their own backoff.
                    if let Err(e) = self.sweep_tick().await {
                        warn!(error = %e, "indexer sweep pass failed; retrying next tick");
                    }
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
    }

    /// One reconcile pass: read the head, and if a new block has arrived, check
    /// every unpaid invoice's balance and fund those that are covered.
    async fn tick(&self) -> Result<(), IndexerError> {
        let head = self.chain.get_block_number().await?;

        // Skip work when no new block has arrived since the last pass. Funding a
        // payment address always lands in a block that advances the head, so an
        // unpaid invoice is always re-examined on the tick following its funding.
        if let Some(last) = self.cursor.get(self.chain_id.0).await?
            && head <= last
        {
            return Ok(());
        }

        let rows = self
            .repo
            .list_created(self.chain_id.0, RECONCILE_BATCH_LIMIT)
            .await?;

        // Decode rows to the domain model up front so we have the typed payment
        // address and amount before fetching balances. A single corrupt row
        // must not abort the pass, so it is logged and skipped rather than
        // propagated.
        let invoices: Vec<Invoice> = rows
            .iter()
            .filter_map(|row| match Invoice::try_from(row) {
                Ok(invoice) => Some(invoice),
                Err(e) => {
                    warn!(invoice_id = %row.id, error = %e, "skipping corrupt invoice row");
                    None
                }
            })
            .collect();

        // Read every payment address's balance, batched into Multicall calls
        // that run concurrently, then fund the covered invoices.
        let balances = self.fetch_balances(&invoices).await;
        for (invoice, balance) in invoices.iter().zip(balances) {
            let Some(balance) = balance else {
                // This invoice's batch failed this pass; it is retried next tick.
                continue;
            };
            if let Err(e) = self.fund_if_covered(invoice, balance, head).await {
                warn!(invoice_id = %invoice.id.0, error = %e, "failed to fund invoice");
            }
        }

        self.cursor.upsert(self.chain_id.0, head).await?;
        Ok(())
    }

    /// Fetch the on-chain balance of every invoice's payment address.
    ///
    /// Addresses are split into [`MULTICALL_CHUNK_SIZE`] chunks; each chunk is
    /// one Multicall `aggregate` request, and all chunks are issued
    /// concurrently and awaited together, so a full pass costs
    /// `ceil(N / chunk)` round-trips overlapped in flight rather than `N`
    /// sequential ones.
    ///
    /// Returns balances aligned with `invoices`. An entry is `None` when its
    /// chunk's request failed this pass; those invoices are simply left for the
    /// next tick, so one failed chunk never aborts the others or the pass.
    async fn fetch_balances(&self, invoices: &[Invoice]) -> Vec<Option<U256>> {
        let batches = invoices.chunks(MULTICALL_CHUNK_SIZE).map(|batch| {
            let chain = Arc::clone(&self.chain);
            let addrs: Vec<Address> = batch.iter().map(|inv| inv.payment_address.0).collect();
            async move {
                match chain.get_balances(&addrs).await {
                    // Multicall guarantees one result per input address, but
                    // guard the alignment so a malformed response can never
                    // shift balances onto the wrong invoices.
                    Ok(balances) if balances.len() == addrs.len() => {
                        balances.into_iter().map(Some).collect::<Vec<_>>()
                    }
                    Ok(balances) => {
                        warn!(
                            expected = addrs.len(),
                            got = balances.len(),
                            "multicall returned mismatched balance count; retrying next tick"
                        );
                        vec![None; addrs.len()]
                    }
                    Err(e) => {
                        warn!(
                            error = %e,
                            batch_size = addrs.len(),
                            "multicall balance batch failed; retrying next tick"
                        );
                        vec![None; addrs.len()]
                    }
                }
            }
        });

        join_all(batches)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// Fund one invoice if its observed balance meets the required amount. The
    /// transition is an idempotent compare-and-set in the repository.
    async fn fund_if_covered(
        &self,
        invoice: &Invoice,
        balance: U256,
        head: u64,
    ) -> Result<(), IndexerError> {
        if balance < invoice.amount.0 {
            return Ok(());
        }

        let funded = self
            .repo
            .mark_funded(invoice.id.0, &balance.to_string(), head)
            .await?;

        if funded {
            info!(
                invoice_id = %invoice.id.0,
                payment_address = %invoice.payment_address.0,
                balance = %balance,
                required = %invoice.amount.0,
                block = head,
                "invoice funded"
            );
        }

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
                warn!(invoice_id = %invoice.id.0, error = %e, "sweep pass error for invoice");
            }
        }

        Ok(())
    }

    /// Sweep one invoice: claim it if newly `funded`, call `execute`, and apply
    /// the resulting transition. A `deploying` invoice is already claimed (a
    /// prior attempt / crash recovery), so it is retried without re-claiming.
    async fn sweep_invoice(&self, invoice: &Invoice) -> Result<(), IndexerError> {
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
                self.factory,
                invoice.token.0,
                invoice.amount.0,
                invoice.beneficiary.0,
                invoice.salt.0,
                invoice.payment_address.0,
            )
            .await;

        match outcome {
            Ok(SweepOutcome::Executed { tx_hash, block }) => {
                if self
                    .repo
                    .mark_fulfilled(invoice.id.0, Some(tx_hash.as_slice()), Some(block))
                    .await?
                {
                    info!(
                        invoice_id = %invoice.id.0,
                        tx_hash = %tx_hash,
                        block,
                        "invoice fulfilled"
                    );
                }
            }
            Ok(SweepOutcome::AlreadyDeployed) => {
                if self.repo.mark_fulfilled(invoice.id.0, None, None).await? {
                    info!(
                        invoice_id = %invoice.id.0,
                        payment_address = %invoice.payment_address.0,
                        "invoice already executed on-chain; marked fulfilled"
                    );
                }
            }
            Ok(SweepOutcome::Blocked) => {
                if self.repo.mark_blocked(invoice.id.0, "receiver_rejected").await? {
                    warn!(
                        invoice_id = %invoice.id.0,
                        beneficiary = %invoice.beneficiary.0,
                        "invoice blocked: beneficiary rejects the payment"
                    );
                }
            }
            Err(e) => {
                // Transient failure: bump the attempt counter (with backoff). If
                // the retry budget is now exhausted the row is blocked instead.
                match self
                    .repo
                    .record_sweep_failure(invoice.id.0, MAX_SWEEP_ATTEMPTS)
                    .await?
                {
                    Some(status) if status == "blocked" => {
                        warn!(
                            invoice_id = %invoice.id.0,
                            error = %e,
                            "invoice blocked after exhausting sweep retries"
                        );
                    }
                    _ => {
                        warn!(
                            invoice_id = %invoice.id.0,
                            error = %e,
                            "transient sweep failure; will retry after backoff"
                        );
                    }
                }
            }
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
        Amount, BeneficiaryAddress, ChainId, FactoryAddress, Invoice, NATIVE_TOKEN_ADDRESS,
    };
    use sqlx::PgPool;

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

    /// In-memory [`ChainClient`] with a fixed head, per-address balances, and a
    /// scripted sweep outcome per payment address (default: `Executed`).
    struct MockChain {
        block: u64,
        balances: HashMap<Address, U256>,
        sweeps: HashMap<Address, MockSweep>,
    }

    impl MockChain {
        fn new(block: u64, balances: HashMap<Address, U256>) -> Self {
            Self {
                block,
                balances,
                sweeps: HashMap::new(),
            }
        }

        /// Script the sweep outcome for one payment address.
        fn with_sweep(mut self, addr: Address, outcome: MockSweep) -> Self {
            self.sweeps.insert(addr, outcome);
            self
        }
    }

    #[async_trait]
    impl ChainClient for MockChain {
        async fn get_block_number(&self) -> Result<u64, ChainError> {
            Ok(self.block)
        }

        async fn get_balances(&self, addrs: &[Address]) -> Result<Vec<U256>, ChainError> {
            Ok(addrs
                .iter()
                .map(|addr| self.balances.get(addr).copied().unwrap_or(U256::ZERO))
                .collect())
        }

        async fn sweep(
            &self,
            _factory: Address,
            _token: Address,
            _amount: U256,
            _receiver: Address,
            _salt: B256,
            payment_address: Address,
        ) -> Result<SweepOutcome, ChainError> {
            match self
                .sweeps
                .get(&payment_address)
                .copied()
                .unwrap_or(MockSweep::Executed)
            {
                MockSweep::Executed => Ok(SweepOutcome::Executed {
                    tx_hash: MOCK_TX_HASH,
                    block: self.block,
                }),
                MockSweep::AlreadyDeployed => Ok(SweepOutcome::AlreadyDeployed),
                MockSweep::Blocked => Ok(SweepOutcome::Blocked),
                MockSweep::Transient => {
                    Err(ChainError::Transient("mock transient failure".to_string()))
                }
            }
        }
    }

    const CHAIN_ID: u64 = 31337;

    fn factory() -> FactoryAddress {
        FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3"))
    }

    /// Build a native-token invoice for the test chain with the given base-unit amount.
    fn make_invoice(amount: u64) -> Invoice {
        Invoice::new(
            factory(),
            ChainId(CHAIN_ID),
            NATIVE_TOKEN_ADDRESS,
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01")),
            Amount(U256::from(amount)),
        )
    }

    async fn insert(pool: &PgPool, invoice: &Invoice, key: &str) {
        let repo = InvoiceRepository::new(pool.clone());
        let input = CreateInvoiceInput::from_invoice(invoice, key.to_string(), 18);
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
            factory().0,
            Duration::from_millis(10),
        )
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn funds_invoice_when_balance_meets_amount(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let mut balances = HashMap::new();
        balances.insert(invoice.payment_address.0, U256::from(100));
        let indexer = indexer_with(&pool, MockChain::new(1, balances));

        indexer.tick().await.expect("tick should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "funded");
        assert_eq!(row.observed_amount.as_deref(), Some("100"));
        assert_eq!(row.funded_at_block, Some(1));
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn overpayment_still_funds(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let mut balances = HashMap::new();
        balances.insert(invoice.payment_address.0, U256::from(150));
        let indexer = indexer_with(&pool, MockChain::new(1, balances));

        indexer.tick().await.expect("tick should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "funded");
        assert_eq!(row.observed_amount.as_deref(), Some("150"));
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn leaves_invoice_created_when_underfunded(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let mut balances = HashMap::new();
        balances.insert(invoice.payment_address.0, U256::from(99));
        let indexer = indexer_with(&pool, MockChain::new(1, balances));

        indexer.tick().await.expect("tick should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "created");
        assert_eq!(row.observed_amount, None);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn tick_is_idempotent(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let mut balances = HashMap::new();
        balances.insert(invoice.payment_address.0, U256::from(100));

        // First tick funds at block 1.
        let indexer = indexer_with(&pool, MockChain::new(1, balances.clone()));
        indexer.tick().await.expect("first tick");

        // A later tick at a higher block must not re-transition (CAS guard) and
        // must not overwrite the original funding block.
        let indexer2 = indexer_with(&pool, MockChain::new(5, balances));
        indexer2.tick().await.expect("second tick");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "funded");
        assert_eq!(row.funded_at_block, Some(1), "funding block must be stable");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn skips_when_no_new_block(pool: PgPool) {
        // Seed the cursor at block 10.
        let cursor = CursorRepository::new(pool.clone());
        cursor.upsert(CHAIN_ID, 10).await.unwrap();

        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        // Head is behind the cursor -> no reconcile even though the balance is sufficient.
        let mut balances = HashMap::new();
        balances.insert(invoice.payment_address.0, U256::from(100));
        let indexer = indexer_with(&pool, MockChain::new(10, balances));

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
            NATIVE_TOKEN_ADDRESS,
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01")),
            Amount(U256::from(100)),
        );
        insert(&pool, &invoice, "key-1").await;

        let mut balances = HashMap::new();
        balances.insert(invoice.payment_address.0, U256::from(100));
        let indexer = indexer_with(&pool, MockChain::new(1, balances));

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
        let indexer = indexer_with(&pool, MockChain::new(7, HashMap::new()));
        indexer.sweep_tick().await.expect("sweep should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "fulfilled");
        assert_eq!(row.execute_tx_hash.as_deref(), Some(MOCK_TX_HASH.as_slice()));
        assert_eq!(row.fulfilled_at_block, Some(7));
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn already_deployed_marks_fulfilled_without_tx(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(
            &pool,
            MockChain::new(7, HashMap::new())
                .with_sweep(invoice.payment_address.0, MockSweep::AlreadyDeployed),
        );
        indexer.sweep_tick().await.expect("sweep should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "fulfilled");
        // No transaction of ours landed it, so no hash/block is recorded.
        assert_eq!(row.execute_tx_hash, None);
        assert_eq!(row.fulfilled_at_block, None);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn receiver_rejection_marks_blocked(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(
            &pool,
            MockChain::new(7, HashMap::new())
                .with_sweep(invoice.payment_address.0, MockSweep::Blocked),
        );
        indexer.sweep_tick().await.expect("sweep should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "blocked");
        assert_eq!(row.blocked_reason.as_deref(), Some("receiver_rejected"));
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn transient_failure_increments_attempts_and_stays_deploying(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(
            &pool,
            MockChain::new(7, HashMap::new())
                .with_sweep(invoice.payment_address.0, MockSweep::Transient),
        );
        indexer.sweep_tick().await.expect("sweep should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "deploying", "claimed but not resolved -> retryable");
        assert_eq!(row.sweep_attempts, 1);
        assert!(row.last_attempt_at.is_some());
        assert_eq!(row.blocked_reason, None);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn sweep_ignores_created_invoices(pool: PgPool) {
        // A `created` (unfunded) invoice must not be swept.
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(&pool, MockChain::new(7, HashMap::new()));
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
            NATIVE_TOKEN_ADDRESS,
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01")),
            Amount(U256::from(100)),
        );
        insert_funded(&pool, &invoice, "key-1").await;

        let indexer = indexer_with(&pool, MockChain::new(7, HashMap::new()));
        indexer.sweep_tick().await.expect("sweep should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "funded", "other-chain invoice must be untouched");
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
    async fn transient_failures_block_after_exhausting_retries(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let repo = InvoiceRepository::new(pool.clone());
        repo.mark_deploying(invoice.id.0).await.unwrap();

        // The first MAX-1 failures stay retryable; the MAX-th blocks.
        for _ in 0..(MAX_SWEEP_ATTEMPTS - 1) {
            let status = repo
                .record_sweep_failure(invoice.id.0, MAX_SWEEP_ATTEMPTS)
                .await
                .unwrap();
            assert_eq!(status.as_deref(), Some("deploying"));
        }
        let status = repo
            .record_sweep_failure(invoice.id.0, MAX_SWEEP_ATTEMPTS)
            .await
            .unwrap();
        assert_eq!(status.as_deref(), Some("blocked"));

        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "blocked");
        assert_eq!(row.blocked_reason.as_deref(), Some("max_retries_exceeded"));
        assert_eq!(row.sweep_attempts, MAX_SWEEP_ATTEMPTS);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn backoff_gates_deploying_reselection(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1").await;

        let repo = InvoiceRepository::new(pool.clone());
        repo.mark_deploying(invoice.id.0).await.unwrap();
        // One failure just now: attempts=1, last_attempt_at=now().
        repo.record_sweep_failure(invoice.id.0, MAX_SWEEP_ATTEMPTS)
            .await
            .unwrap();

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
        let indexer = indexer_with(&pool, MockChain::new(7, HashMap::new()));
        indexer.sweep_tick().await.expect("first sweep");

        // A second pass must leave it fulfilled with the original tx hash/block
        // (list_sweepable excludes terminal states).
        let indexer2 = indexer_with(&pool, MockChain::new(9, HashMap::new()));
        indexer2.sweep_tick().await.expect("second sweep");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "fulfilled");
        assert_eq!(row.fulfilled_at_block, Some(7), "must not be re-fulfilled");
    }
}
