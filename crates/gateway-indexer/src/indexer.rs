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

use gateway_core::{ChainId, Invoice};
use thiserror::Error;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;
use tracing::{info, warn};

use crate::chain::{ChainClient, ChainError};
use gateway_db::{CursorRepository, DbInvoiceError, InvoiceRepository};

/// Maximum invoices reconciled in a single poll pass, so a large backlog cannot
/// stall one tick. Remaining invoices are picked up on subsequent passes.
const RECONCILE_BATCH_LIMIT: i64 = 500;

/// Errors that can abort a single poll pass. All are transient or indicate
/// out-of-contract data; the worker logs and retries on the next tick rather
/// than crashing the process.
#[derive(Debug, Error)]
pub enum IndexerError {
    #[error("chain error: {0}")]
    Chain(#[from] ChainError),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("corrupt invoice row: {0}")]
    Row(#[from] DbInvoiceError),
}

/// The payment indexer. Owns its own repository handles and chain client; it is
/// not shared with the HTTP handlers.
pub struct Indexer {
    repo: InvoiceRepository,
    cursor: CursorRepository,
    chain: Arc<dyn ChainClient>,
    chain_id: ChainId,
    poll_interval: Duration,
}

impl Indexer {
    pub fn new(
        repo: InvoiceRepository,
        cursor: CursorRepository,
        chain: Arc<dyn ChainClient>,
        chain_id: ChainId,
        poll_interval: Duration,
    ) -> Self {
        Self {
            repo,
            cursor,
            chain,
            chain_id,
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
                    if let Err(e) = self.tick().await {
                        warn!(error = %e, "indexer poll failed; retrying next tick");
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

        for row in &rows {
            // A single corrupt row or transient per-address RPC error must not
            // abort the whole pass or the cursor advance.
            if let Err(e) = self.reconcile(row, head).await {
                warn!(invoice_id = %row.id, error = %e, "failed to reconcile invoice");
            }
        }

        self.cursor.upsert(self.chain_id.0, head).await?;
        Ok(())
    }

    /// Reconcile one invoice against its canonical on-chain balance, funding it
    /// if the balance meets the required amount.
    async fn reconcile(&self, row: &gateway_db::DbInvoice, head: u64) -> Result<(), IndexerError> {
        // Route through the domain model so payment address and amount are the
        // exact typed values, reusing the row->domain contract and its errors.
        let invoice = Invoice::try_from(row)?;

        let balance = self.chain.get_balance(invoice.payment_address.0).await?;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    use alloy_primitives::{Address, U256, address};
    use async_trait::async_trait;
    use gateway_core::{
        Amount, BeneficiaryAddress, ChainId, FactoryAddress, Invoice, NATIVE_TOKEN_ADDRESS,
    };
    use sqlx::PgPool;

    use gateway_db::{CreateInvoiceInput, InvoiceRepository};

    /// In-memory [`ChainClient`] with a fixed head and per-address balances.
    struct MockChain {
        block: u64,
        balances: HashMap<Address, U256>,
    }

    #[async_trait]
    impl ChainClient for MockChain {
        async fn get_block_number(&self) -> Result<u64, ChainError> {
            Ok(self.block)
        }

        async fn get_balance(&self, addr: Address) -> Result<U256, ChainError> {
            Ok(self.balances.get(&addr).copied().unwrap_or(U256::ZERO))
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

    fn indexer_with(pool: &PgPool, chain: MockChain) -> Indexer {
        Indexer::new(
            InvoiceRepository::new(pool.clone()),
            CursorRepository::new(pool.clone()),
            Arc::new(chain),
            ChainId(CHAIN_ID),
            Duration::from_millis(10),
        )
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn funds_invoice_when_balance_meets_amount(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let mut balances = HashMap::new();
        balances.insert(invoice.payment_address.0, U256::from(100));
        let indexer = indexer_with(&pool, MockChain { block: 1, balances });

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
        let indexer = indexer_with(&pool, MockChain { block: 1, balances });

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
        let indexer = indexer_with(&pool, MockChain { block: 1, balances });

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
        let indexer = indexer_with(
            &pool,
            MockChain {
                block: 1,
                balances: balances.clone(),
            },
        );
        indexer.tick().await.expect("first tick");

        // A later tick at a higher block must not re-transition (CAS guard) and
        // must not overwrite the original funding block.
        let indexer2 = indexer_with(&pool, MockChain { block: 5, balances });
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
        let indexer = indexer_with(
            &pool,
            MockChain {
                block: 10,
                balances,
            },
        );

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
        let indexer = indexer_with(&pool, MockChain { block: 1, balances });

        indexer.tick().await.expect("tick should succeed");

        let repo = InvoiceRepository::new(pool.clone());
        let row = repo.find_by_id(invoice.id.0).await.unwrap().unwrap();
        assert_eq!(row.status, "created");
    }
}
