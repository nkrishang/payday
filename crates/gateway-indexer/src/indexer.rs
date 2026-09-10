//! Payment indexer worker.
//!
//! Two independently scheduled loops share the invoice table as a durable
//! queue. The block indexer ingests finalized USDC `Transfer` logs and moves
//! invoices `created -> funded` or `-> expired`. The sweep worker turns every
//! invoice with uncollected funds into a helper transaction and lets the
//! finalized receipt decide the outcome: `Settled` -> `fulfilled`, `Recovered`
//! -> `recovered`, `SweepRecovered` -> late funds collected, `SweepFailed` ->
//! classified into a retry or a block.
//!
//! The block indexer is a reconciler: each pass scans `cursor + 1 ..=
//! finalized` in bounded ranges, one `eth_getLogs` per range per 500 watched
//! addresses, and commits atomically. It runs on a slow timer and immediately on a wake from the
//! transfer signal (`signal.rs`), so detection latency comes from the push
//! path while the request budget is set by the range cap, not the cadence.
//! Neither path trusts the other: the signal never writes, and the timer
//! never waits for it.
//!
//! A fatal block-indexer error ends the process (finalized history no longer
//! matches; the operator must look). A fatal sweep condition only pauses the
//! sweep worker, which keeps reporting it every tick until it clears, so
//! payment detection never stops because a transaction is stuck.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alloy_primitives::{Address, U256};
use chrono::Duration as ChronoDuration;
use gateway_core::{ChainId, FinalitySource, Invoice, InvoiceStatus, PaymentBinding};
use gateway_db::{
    BatchResolution, CursorRepository, DbInvoice, IndexerCursor, InvoiceOutcome, InvoiceRepository,
    MinedBatch, PaymentObservation, RecoveredFundsInput, RecoveryReason, SweepBatch,
    WatchFingerprint,
};
use sqlx::types::chrono::Utc;
use thiserror::Error;
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};
use tracing::{error, info, warn};

use crate::chain::{
    BlockHeader, ChainClient, ChainError, FeeEstimate, PreparedSweepTransaction, SweepOutcome,
    SweepReceipt, SweepRequest, sweep_batch_gas_limit,
};
use crate::signal::{SignalState, WatchList};

/// Maximum invoices included in one helper transaction.
pub const SWEEP_BATCH_LIMIT: i64 = 20;

/// Exponential backoff for retrying a transient sweep failure: the nth retry
/// waits `min(BASE * 2^attempts, CAP)` seconds, so a struggling RPC is not
/// hammered while a brief blip still recovers quickly.
pub const SWEEP_BACKOFF_BASE_SECS: f64 = 2.0;
pub const SWEEP_BACKOFF_CAP_SECS: f64 = 300.0;

/// Warn once the cursor trails finality by this many blocks (~7 minutes on Monad).
const CURSOR_LAG_WARN_BLOCKS: u64 = 1_000;
/// Chain reads inside a range are retried with backoff instead of aborting
/// the tick: an aborted tick discards its uncommitted ranges, and during a
/// long catch-up those same ranges are then re-fetched next tick against a
/// provider whose request budget the burst already strained. The retry bound
/// keeps a permanently failing read from hanging the tick forever.
const RANGE_RETRY_ATTEMPTS: u32 = 10;
const RANGE_RETRY_BACKOFF: Duration = Duration::from_millis(500);
const RANGE_RETRY_BACKOFF_CAP: Duration = Duration::from_secs(15);
/// Warn once collectable funds have waited this long.
const SWEEP_BACKLOG_WARN_SECS: f64 = 900.0;
/// Emit sweep health telemetry (one `eth_getBalance`) at most this often.
const SWEEP_HEALTH_INTERVAL: Duration = Duration::from_secs(300);
/// How often the signal's recipient list is checked against the database.
/// The check is one indexed aggregate; the list itself loads only on change.
const WATCH_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
/// A wake names a block the signal saw a transfer in. If the boundary has
/// not reached it yet, wait the block times the boundary is behind by and
/// pass again, up to this many times, before leaving it to the timer. Monad
/// finalizes within two blocks (under a second); an L2 on `latest + N`
/// needs N blocks, which the first sleep covers in one go.
const WAKE_CATCHUP_ATTEMPTS: u32 = 16;

/// What one reconcile pass accomplished. `indexed` is the highest block whose
/// USDC activity is in the ledger; `boundary` the finality boundary the pass
/// read. `indexed < boundary` means the per-pass range budget stopped the pass
/// with finalized work left in the range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PassSummary {
    boundary: u64,
    indexed: u64,
}

impl PassSummary {
    fn backlog(&self) -> bool {
        self.indexed < self.boundary
    }
}

/// Errors that can abort a poll pass.
#[derive(Debug, Error)]
pub enum IndexerError {
    #[error("chain error: {0}")]
    Chain(#[from] ChainError),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("configuration invariant failed: {0}")]
    Configuration(String),
    /// A sweep condition an operator must resolve. The sweep worker reports it
    /// every tick and retries; block indexing is unaffected.
    #[error("{0}")]
    SweepStalled(String),
}

impl IndexerError {
    fn requires_halt(&self) -> bool {
        matches!(self, Self::Chain(ChainError::FinalityViolation(_)))
            || matches!(self, Self::Chain(error) if error.is_permanent_rpc())
            || matches!(self, Self::Configuration(_) | Self::SweepStalled(_))
    }
}

#[derive(Debug, Clone)]
pub struct IndexerConfig {
    pub chain_id: ChainId,
    pub factory: Address,
    pub batch_sweeper: Address,
    pub usdc: Address,
    pub usdc_start_block: u64,
    pub finality_source: FinalitySource,
    pub finality_confirmations: u64,
    /// The chain's block interval; paces the catch-up after a wake.
    pub block_time: Duration,
    pub log_range_size: u64,
    pub max_ranges_per_tick: u64,
    /// Reconcile cadence while the chain is watched and the transfer signal
    /// is disconnected or disabled, and the sweep worker's cadence always.
    pub poll_interval: Duration,
    /// Reconcile cadence while the chain is watched and the transfer signal
    /// is connected. Wakes from the signal run a pass immediately regardless.
    pub reconcile_interval: Duration,
    /// Reconcile cadence while nothing is watched: each pass costs two calls
    /// and only fast-forwards the cursor.
    pub idle_interval: Duration,
    /// How long after its last change a settled address stays in the watch
    /// list, which is both what the signal subscribes to and what every
    /// range scan is filtered by: a late transfer to an address outside the
    /// window is not seen (`recover(token)` returns it by hand).
    pub late_watch_window: Duration,
    pub sweep_pending_timeout: Duration,
    pub sweep_max_submissions: u32,
    pub sweep_max_attempts: u32,
    pub sweep_backoff_base_secs: f64,
    pub sweep_backoff_cap_secs: f64,
    pub signer_low_balance_wei: U256,
}

/// The payment indexer. Owns its own repository handles and chain client; it is
/// not shared with the HTTP handlers.
pub struct Indexer {
    repo: InvoiceRepository,
    cursor: CursorRepository,
    chain: Arc<dyn ChainClient>,
    cfg: IndexerConfig,
    current_log_range_size: AtomicU64,
    /// Wake-ups and health from the transfer signal; see `signal.rs`.
    signal: Arc<SignalState>,
    /// The recipient list the signal subscribes to, replaced on change.
    watch_tx: watch::Sender<WatchList>,
    watch_fingerprint: Mutex<Option<WatchFingerprint>>,
    last_health_report: Mutex<Option<Instant>>,
}

impl Indexer {
    pub fn new(
        repo: InvoiceRepository,
        cursor: CursorRepository,
        chain: Arc<dyn ChainClient>,
        cfg: IndexerConfig,
    ) -> Self {
        let (watch_tx, _) = watch::channel(WatchList::default());
        Self {
            repo,
            cursor,
            chain,
            current_log_range_size: AtomicU64::new(cfg.log_range_size),
            signal: SignalState::new(),
            watch_tx,
            watch_fingerprint: Mutex::new(None),
            last_health_report: Mutex::new(None),
            cfg,
        }
    }

    /// The seam a transfer signal drives: wake-ups and connection health.
    pub fn signal(&self) -> Arc<SignalState> {
        Arc::clone(&self.signal)
    }

    /// The recipient list a transfer signal subscribes to.
    pub fn watch_list(&self) -> watch::Receiver<WatchList> {
        self.watch_tx.subscribe()
    }

    /// Whether the chain has nothing to watch: no open bound request, no
    /// uncollected funds, nothing settled within the late window. An idle
    /// chain is neither scanned nor subscribed to.
    fn idle(&self) -> bool {
        self.watch_tx.borrow().is_empty()
    }

    /// The cadence after a pass: keep going while finalized ranges are left,
    /// crawl while idle, and otherwise follow the signal's health.
    fn cadence(&self, summary: Option<&PassSummary>) -> Duration {
        match summary {
            Some(summary) if summary.backlog() => Duration::ZERO,
            _ if self.idle() => self.cfg.idle_interval,
            _ if self.signal.is_connected() => self.cfg.reconcile_interval,
            _ => self.cfg.poll_interval,
        }
    }

    /// Run both loops until `shutdown` is set to `true`. A fatal block-indexer
    /// error is returned so the process supervisor cannot mistake a halted
    /// payment worker for a healthy one; the sweep loop never fails the process.
    pub async fn run(self, shutdown: watch::Receiver<bool>) -> Result<(), IndexerError> {
        let worker = Arc::new(self);
        let index_loop = Arc::clone(&worker).run_index_loop(shutdown.clone());
        let sweep_loop = worker.run_sweep_loop(shutdown);
        tokio::pin!(index_loop, sweep_loop);

        tokio::select! {
            result = &mut index_loop => result,
            () = &mut sweep_loop => Ok(()),
        }
    }

    async fn run_index_loop(
        self: Arc<Self>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), IndexerError> {
        info!(
            chain_id = self.cfg.chain_id.0,
            usdc = %self.cfg.usdc,
            finality_source = ?self.cfg.finality_source,
            finality_confirmations = self.cfg.finality_confirmations,
            reconcile_interval_ms = self.cfg.reconcile_interval.as_millis() as u64,
            poll_interval_ms = self.cfg.poll_interval.as_millis() as u64,
            "block indexer started"
        );

        let mut watch_refresh = tokio::time::interval(WATCH_REFRESH_INTERVAL);
        watch_refresh.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // The timer restarts after every pass rather than firing on a fixed
        // grid, so a slow pass or a wake never queues a burst of catch-up
        // passes; the cadence is simply "this long after the last pass".
        let mut next_pass = Instant::now();
        let mut signal_was_connected = false;
        let mut was_idle = false;
        loop {
            let connected = self.signal.is_connected();
            if connected != signal_was_connected {
                info!(
                    chain_id = self.cfg.chain_id.0,
                    signal_connected = connected,
                    "block indexer cadence changed with transfer signal health"
                );
                signal_was_connected = connected;
                if !connected && !self.idle() {
                    // The pass currently scheduled was sized by a healthy
                    // signal; fall back to the polling cadence now rather
                    // than waiting it out. `set_connected(false)` wakes this
                    // loop, so the adjustment is not stranded until the next
                    // scheduled event. An idle chain's signal closes on
                    // purpose and keeps the idle cadence.
                    next_pass = next_pass.min(Instant::now() + self.cfg.poll_interval);
                }
            }
            let target = tokio::select! {
                _ = tokio::time::sleep_until(next_pass) => None,
                _ = self.signal.notified() => self.signal.take_target(),
                // A healthy signal dropping is itself a scheduling event:
                // clamp the deadline now, even if the signal already
                // reconnected, because the loop-top comparison below would
                // not see a drop that was healed in between.
                _ = self.signal.health_changed() => {
                    if !self.idle() {
                        next_pass = next_pass.min(Instant::now() + self.cfg.poll_interval);
                    }
                    continue;
                }
                _ = watch_refresh.tick() => {
                    if let Err(error) = self.refresh_watch_list().await {
                        warn!(error = %error, "watch list refresh failed; retrying");
                    }
                    // Something to watch appeared on an idle chain: pass now
                    // rather than at the end of the idle interval.
                    if was_idle && !self.idle() {
                        next_pass = Instant::now();
                    }
                    was_idle = self.idle();
                    continue;
                }
                changed = shutdown.changed() => {
                    // `Err` means the sender was dropped -> treat as shutdown.
                    if changed.is_err() || *shutdown.borrow() {
                        info!("block indexer shutting down");
                        break;
                    }
                    continue;
                }
            };
            let summary = match self.reconcile(target).await {
                Ok(summary) => Some(summary),
                Err(error) if error.requires_halt() => {
                    return Err(error);
                }
                Err(error) => {
                    warn!(error = %error, "indexer pass failed; retrying next pass");
                    None
                }
            };
            // A pass with a backlog keeps passing now. Each pass is still
            // bounded by the per-pass range budget, so a catch-up backlog
            // drains as fast as the budget allows without starving the sweep
            // loop.
            was_idle = self.idle();
            next_pass = Instant::now() + self.cadence(summary.as_ref());
        }

        Ok(())
    }

    /// Refresh the signal's recipient list when the set of watched invoices
    /// changed. The fingerprint is one indexed aggregate; the address list
    /// is loaded, and the signal re-subscribed, only when it moves.
    async fn refresh_watch_list(&self) -> Result<(), IndexerError> {
        let since = Utc::now()
            - ChronoDuration::from_std(self.cfg.late_watch_window)
                .unwrap_or_else(|_| ChronoDuration::days(30));
        let fingerprint = self
            .repo
            .watch_fingerprint(self.cfg.chain_id.0, self.cfg.usdc, since)
            .await?;
        if *self
            .watch_fingerprint
            .lock()
            .expect("watch fingerprint lock")
            == Some(fingerprint)
        {
            return Ok(());
        }
        let addresses: WatchList = Arc::new(
            self.repo
                .watch_addresses(self.cfg.chain_id.0, self.cfg.usdc, since)
                .await?,
        );
        self.watch_tx.send_if_modified(|current| {
            if *current == addresses {
                return false;
            }
            *current = addresses;
            true
        });
        *self
            .watch_fingerprint
            .lock()
            .expect("watch fingerprint lock") = Some(fingerprint);
        Ok(())
    }

    async fn run_sweep_loop(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        let mut interval = tokio::time::interval(self.cfg.poll_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        info!(batch_sweeper = %self.cfg.batch_sweeper, "sweep worker started");

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    match self.sweep_tick().await {
                        Ok(()) => self.persist_sweeper_health("running").await,
                        // Logged every tick so the alarm stays raised until the
                        // condition clears; the next tick re-evaluates it.
                        Err(error) if error.requires_halt() => {
                            self.persist_sweeper_health("paused").await;
                            error!(error = %error, "sweep worker paused");
                        }
                        Err(error) => {
                            self.persist_sweeper_health("degraded").await;
                            warn!(error = %error, "sweep pass failed; retrying next tick");
                        }
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
    }

    async fn persist_sweeper_health(&self, state: &str) {
        if let Err(error) = self
            .repo
            .record_sweeper_status(self.cfg.chain_id.0, state)
            .await
        {
            warn!(%error, state, "failed to persist sweep worker health");
        }
    }

    /// Header of the highest block whose logs and state this worker treats
    /// as irreversible. With the `finalized` tag and no confirmation margin
    /// (Monad's production setting) this is a single header read.
    async fn boundary_header(
        &self,
        attempt: &mut u32,
        backoff: &mut Duration,
    ) -> Result<BlockHeader, IndexerError> {
        let margin = self.cfg.finality_confirmations;
        match self.cfg.finality_source {
            FinalitySource::Finalized => {
                let finalized = loop {
                    match self.chain.finalized_header().await {
                        Ok(header) => break header,
                        Err(error) if error.is_retryable() => {
                            self.back_off_or_fail("eth_getBlockByNumber", attempt, backoff, error)
                                .await?;
                        }
                        Err(error) => return Err(error.into()),
                    }
                };
                if margin == 0 {
                    return Ok(finalized);
                }
                self.retried_header(finalized.number.saturating_sub(margin), attempt, backoff)
                    .await
            }
            FinalitySource::Latest => {
                let latest = loop {
                    match self.chain.latest_block_number().await {
                        Ok(number) => break number,
                        Err(error) if error.is_retryable() => {
                            self.back_off_or_fail("eth_blockNumber", attempt, backoff, error)
                                .await?;
                        }
                        Err(error) => return Err(error.into()),
                    }
                };
                self.retried_header(latest.saturating_sub(margin), attempt, backoff)
                    .await
            }
        }
    }

    /// Number of the block [`Self::boundary_header`] describes.
    async fn finality_boundary(
        &self,
        attempt: &mut u32,
        backoff: &mut Duration,
    ) -> Result<u64, IndexerError> {
        self.boundary_header(attempt, backoff)
            .await
            .map(|header| header.number)
    }

    /// A `block_header` read that backs off and retries throttle-shaped
    /// failures instead of aborting the work it belongs to.
    async fn retried_header(
        &self,
        number: u64,
        attempt: &mut u32,
        backoff: &mut Duration,
    ) -> Result<BlockHeader, IndexerError> {
        loop {
            match self.chain.block_header(number).await {
                Ok(header) => return Ok(header),
                Err(error) if error.is_retryable() => {
                    self.back_off_or_fail("eth_getBlockByNumber", attempt, backoff, error)
                        .await?;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// One reconcile pass without a wake target.
    #[cfg(test)]
    async fn tick(&self) -> Result<(), IndexerError> {
        self.pass().await.map(drop)
    }

    /// Reconcile on a timer or on a wake. A wake names the block the signal
    /// saw a transfer in; it is only covered once finality reaches it *and*
    /// the cursor has ingested it — a wake consumed while the range budget
    /// left blocks unindexed would otherwise wait a full cadence for nothing.
    /// Each wait is sized by how far the boundary trails the target, so a
    /// chain that finalizes N blocks behind the tip is passed about twice,
    /// not once per block.
    async fn reconcile(&self, target: Option<u64>) -> Result<PassSummary, IndexerError> {
        let mut summary = self.pass().await?;
        let Some(target) = target else {
            return Ok(summary);
        };
        let mut attempts = 0;
        while (summary.boundary < target || summary.indexed < target)
            && attempts < WAKE_CATCHUP_ATTEMPTS
        {
            attempts += 1;
            let behind = target.saturating_sub(summary.boundary).clamp(1, 1_000);
            tokio::time::sleep(self.cfg.block_time * behind as u32).await;
            summary = self.pass().await?;
        }
        if summary.indexed < target {
            warn!(
                target,
                boundary = summary.boundary,
                indexed = summary.indexed,
                "wake target not indexed after catch-up; the timer covers it"
            );
        }
        Ok(summary)
    }

    /// Ingest finalized USDC ranges until the cursor reaches the finality
    /// boundary or the per-pass range budget is spent. Reports both the
    /// boundary and how far the cursor got, so the caller can tell a fully
    /// ingested boundary from a budget-limited backlog.
    ///
    /// The scan is demand-driven: with nothing on the watch list there is
    /// nothing a range could contain for us, so the cursor is fast-forwarded
    /// to the boundary in one commit and no `eth_getLogs` is spent. Every
    /// range fetch is filtered by that list, so the cost of a range follows
    /// the addresses Payday watches, never the chain's USDC volume. The list
    /// is (re)loaded after the boundary is read, which is what keeps the
    /// filtered scan race-free: a request bound after that read was disclosed
    /// after every block in the range was mined.
    async fn pass(&self) -> Result<PassSummary, IndexerError> {
        let mut attempt = 0u32;
        let mut backoff = RANGE_RETRY_BACKOFF;
        let boundary_header = self.boundary_header(&mut attempt, &mut backoff).await?;
        let boundary = boundary_header.number;
        self.cursor
            .record_finalized_head(
                self.cfg.chain_id.0,
                self.cfg.usdc,
                boundary,
                boundary_header.hash,
                boundary_header.timestamp,
            )
            .await?;
        let mut cursor = self.cursor.get(self.cfg.chain_id.0, self.cfg.usdc).await?;

        if let Some(cursor) = cursor {
            let canonical = self
                .retried_header(cursor.block, &mut attempt, &mut backoff)
                .await?;
            if canonical.hash != cursor.block_hash {
                return Err(ChainError::FinalityViolation(format!(
                    "finalized cursor hash mismatch at block {}; refusing to continue",
                    cursor.block
                ))
                .into());
            }
        }

        // An unbound request belongs to no chain, so any chain's finalized
        // clock may close it; the update is idempotent across chains.
        for invoice_id in self.repo.expire_unbound(boundary_header.timestamp).await? {
            info!(%invoice_id, block_timestamp = boundary_header.timestamp, "unbound invoice expired");
        }

        self.refresh_watch_list().await?;
        let watched: WatchList = self.watch_tx.borrow().clone();
        if watched.is_empty() {
            let behind = cursor.is_none_or(|cursor| cursor.block < boundary);
            if behind {
                let outcome = self
                    .repo
                    .apply_finalized_usdc_range(
                        self.cfg.chain_id.0,
                        self.cfg.usdc,
                        cursor,
                        boundary,
                        boundary_header.hash,
                        boundary_header.timestamp,
                        &[],
                    )
                    .await?;
                for invoice_id in &outcome.expired {
                    info!(%invoice_id, block = boundary, block_timestamp = boundary_header.timestamp, "invoice expired");
                }
                info!(
                    from = cursor.map_or(self.cfg.usdc_start_block, |cursor| cursor.block + 1),
                    to = boundary,
                    "nothing watched; cursor fast-forwarded without scanning"
                );
            }
            return Ok(PassSummary {
                boundary,
                indexed: boundary,
            });
        }
        let recipients = watched.as_slice();

        for _ in 0..self.cfg.max_ranges_per_tick {
            let from_block = cursor
                .map(|cursor| cursor.block.saturating_add(1))
                .unwrap_or(self.cfg.usdc_start_block);
            if from_block > boundary {
                break;
            }
            cursor = Some(
                self.index_range(cursor, from_block, boundary, recipients)
                    .await?,
            );
        }

        let indexed = cursor.map_or(self.cfg.usdc_start_block, |cursor| cursor.block);
        let lag = boundary.saturating_sub(indexed);
        if lag > CURSOR_LAG_WARN_BLOCKS {
            warn!(
                cursor_lag_blocks = lag,
                boundary, indexed, "indexer cursor lagging"
            );
        }
        Ok(PassSummary { boundary, indexed })
    }

    /// Fetch and commit one bounded range starting at `from_block`: the USDC
    /// transfers to `recipients`.
    async fn index_range(
        &self,
        cursor: Option<IndexerCursor>,
        from_block: u64,
        boundary: u64,
        recipients: &[Address],
    ) -> Result<IndexerCursor, IndexerError> {
        let mut attempt = 0u32;
        let mut backoff = RANGE_RETRY_BACKOFF;
        loop {
            let range_size = self.current_log_range_size.load(Ordering::Relaxed).max(1);
            let to_block = from_block.saturating_add(range_size - 1).min(boundary);

            // Bracket the log request with the range-end header. If a reorg happens
            // while the provider is producing the response, do not pair old logs
            // with a new canonical cursor; retry the whole uncommitted range.
            let end = self
                .retried_header(to_block, &mut attempt, &mut backoff)
                .await?;
            let mut transfers = match self
                .chain
                .usdc_transfers(self.cfg.usdc, from_block, to_block, recipients)
                .await
            {
                Ok(transfers) => transfers,
                Err(ChainError::LogRangeTooLarge(message)) if range_size > 1 => {
                    let smaller = (range_size / 2).max(1);
                    self.current_log_range_size
                        .store(smaller, Ordering::Relaxed);
                    warn!(from_block, to_block, smaller, error = %message, "provider rejected USDC log range; splitting it");
                    continue;
                }
                // A throttled or transiently failing read backs off and retries
                // the same range: aborting the tick here would discard every
                // uncommitted range of this pass, to no provider's benefit.
                Err(error) if error.is_retryable() => {
                    self.back_off_or_fail("eth_getLogs", &mut attempt, &mut backoff, error)
                        .await?;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            transfers.sort_by_key(|transfer| {
                (
                    transfer.block_number,
                    transfer.transaction_index,
                    transfer.log_index,
                )
            });
            // Every block in the range is at or below the finality boundary,
            // and the log carries its block hash and timestamp, so no
            // per-block header read is needed: the range-end header read
            // before and after the log fetch is the reorg-in-flight guard.
            let confirmed_end = self
                .retried_header(to_block, &mut attempt, &mut backoff)
                .await?;
            if confirmed_end.hash != end.hash {
                return Err(ChainError::Transient(format!(
                    "block {to_block} changed while fetching USDC logs and headers"
                ))
                .into());
            }
            let observations: Vec<PaymentObservation> = transfers
                .into_iter()
                .map(|transfer| PaymentObservation {
                    block_number: transfer.block_number,
                    block_hash: transfer.block_hash,
                    block_timestamp: transfer.block_timestamp,
                    transaction_hash: transfer.transaction_hash,
                    transaction_index: transfer.transaction_index,
                    log_index: transfer.log_index,
                    sender: transfer.sender,
                    recipient: transfer.recipient,
                    amount: transfer.amount,
                })
                .collect();
            let outcome = self
                .repo
                .apply_finalized_usdc_range(
                    self.cfg.chain_id.0,
                    self.cfg.usdc,
                    cursor,
                    to_block,
                    end.hash,
                    end.timestamp,
                    &observations,
                )
                .await?;
            for invoice_id in &outcome.funded {
                info!(%invoice_id, block = to_block, "invoice funded by finalized USDC transfers");
            }
            for invoice_id in &outcome.expired {
                info!(%invoice_id, block = to_block, block_timestamp = end.timestamp, "invoice expired");
            }

            let range_size = self.current_log_range_size.load(Ordering::Relaxed);
            self.current_log_range_size.store(
                range_size
                    .saturating_add((range_size / 4).max(1))
                    .min(self.cfg.log_range_size),
                Ordering::Relaxed,
            );
            return Ok(IndexerCursor {
                block: to_block,
                block_hash: end.hash,
                block_timestamp: Some(end.timestamp),
            });
        }
    }

    /// Sleep out one backoff step for a retryable chain-read failure inside a
    /// range, or fail the tick once the attempts are spent.
    async fn back_off_or_fail(
        &self,
        operation: &'static str,
        attempt: &mut u32,
        backoff: &mut Duration,
        error: ChainError,
    ) -> Result<(), IndexerError> {
        *attempt += 1;
        if *attempt >= RANGE_RETRY_ATTEMPTS {
            return Err(error.into());
        }
        warn!(
            operation,
            attempt = *attempt,
            backoff_ms = backoff.as_millis() as u64,
            error = %error,
            "chain read failed inside range; backing off before retrying it"
        );
        tokio::time::sleep(*backoff).await;
        *backoff = (*backoff * 2).min(RANGE_RETRY_BACKOFF_CAP);
        Ok(())
    }

    /// Reconcile the in-flight batch if there is one, otherwise claim and
    /// submit the next. The signer owns one nonce stream, so a new batch is
    /// never sent while a prior one is unresolved.
    async fn sweep_tick(&self) -> Result<(), IndexerError> {
        let health_due = self
            .last_health_report
            .lock()
            .expect("health report lock")
            .is_none_or(|at| at.elapsed() >= SWEEP_HEALTH_INTERVAL);
        if health_due {
            self.report_sweep_health().await?;
            *self.last_health_report.lock().expect("health report lock") = Some(Instant::now());
        }
        match self.repo.open_sweep_batch(self.cfg.chain_id.0).await? {
            Some(batch) if batch.broadcast_at.is_none() => self.broadcast_batch(&batch).await,
            Some(batch) => self.reconcile_batch(batch).await,
            None => self.submit_next_batch().await,
        }
    }

    async fn broadcast_prepared(
        &self,
        batch_id: uuid::Uuid,
        transaction: &PreparedSweepTransaction,
    ) -> Result<(), IndexerError> {
        self.chain.broadcast_sweep_transaction(transaction).await?;
        if !self
            .repo
            .record_batch_broadcast(batch_id, transaction.hash)
            .await?
        {
            return Err(IndexerError::Configuration(format!(
                "durable transaction {} is not the pending broadcast for batch {batch_id}",
                transaction.hash
            )));
        }
        Ok(())
    }

    async fn broadcast_batch(&self, batch: &SweepBatch) -> Result<(), IndexerError> {
        let transaction = PreparedSweepTransaction {
            hash: *batch.tx_hashes.last().ok_or_else(|| {
                IndexerError::Configuration(format!("sweep batch {} has no transaction", batch.id))
            })?,
            raw: batch.raw_transactions.last().cloned().ok_or_else(|| {
                IndexerError::Configuration(format!(
                    "sweep batch {} has no raw transaction",
                    batch.id
                ))
            })?,
        };
        self.broadcast_prepared(batch.id, &transaction).await?;
        info!(batch_id = %batch.id, tx_hash = %transaction.hash, nonce = batch.nonce, "durable helper transaction broadcast");
        Ok(())
    }

    async fn reconcile_batch(&self, batch: SweepBatch) -> Result<(), IndexerError> {
        // A replacement and the submission it replaced share a nonce; whichever
        // mined resolves the batch. Newest first: it is the likeliest.
        for &tx_hash in batch.tx_hashes.iter().rev() {
            if let Some(receipt) = self
                .chain
                .sweep_receipt(tx_hash, self.cfg.batch_sweeper)
                .await?
            {
                return self.apply_receipt(&batch, tx_hash, receipt).await;
            }
        }
        self.handle_unmined(&batch).await
    }

    async fn apply_receipt(
        &self,
        batch: &SweepBatch,
        tx_hash: alloy_primitives::B256,
        receipt: SweepReceipt,
    ) -> Result<(), IndexerError> {
        let mined = MinedBatch {
            tx_hash,
            block: receipt.block,
            block_hash: receipt.block_hash,
            transaction_index: receipt.transaction_index,
        };
        if batch.mined != Some(mined) {
            self.repo.record_batch_mined(batch.id, mined).await?;
        }
        let mut attempt = 0u32;
        let mut backoff = RANGE_RETRY_BACKOFF;
        if receipt.block > self.finality_boundary(&mut attempt, &mut backoff).await? {
            return Ok(());
        }
        let header = self
            .retried_header(receipt.block, &mut attempt, &mut backoff)
            .await?;
        if header.hash != receipt.block_hash {
            self.repo.clear_batch_mined(batch.id).await?;
            warn!(batch_id = %batch.id, %tx_hash, block = receipt.block, "helper transaction receipt is no longer canonical; waiting for a new receipt");
            return Ok(());
        }

        if !receipt.succeeded {
            if self.chain.block_header(receipt.block).await?.hash != receipt.block_hash {
                self.repo.clear_batch_mined(batch.id).await?;
                return Ok(());
            }
            let requeued = self
                .repo
                .resolve_batch(batch.id, BatchResolution::Reverted)
                .await?;
            warn!(batch_id = %batch.id, %tx_hash, requeued, "finalized helper transaction reverted; invoices returned to the queue");
            return Ok(());
        }

        let rows = self.repo.batch_invoices(batch.id).await?;
        let mut outcomes = Vec::with_capacity(rows.len());
        for row in &rows {
            let invoice = decode(row)?;
            let payment = bound(&invoice)?.payment_address.0;
            let outcome = self
                .classify(
                    &invoice,
                    row,
                    tx_hash,
                    receipt.outcomes.get(&payment),
                    &header,
                )
                .await?;
            outcomes.push((row.id, outcome));
        }
        if self.chain.block_header(receipt.block).await?.hash != receipt.block_hash {
            self.repo.clear_batch_mined(batch.id).await?;
            warn!(batch_id = %batch.id, %tx_hash, block = receipt.block, "helper transaction changed during classification; discarding results");
            return Ok(());
        }
        self.repo
            .finalize_batch(
                batch.id,
                tx_hash,
                receipt.block,
                receipt.transaction_index,
                header.timestamp,
                &outcomes,
            )
            .await?;
        for (invoice_id, outcome) in &outcomes {
            match outcome {
                InvoiceOutcome::Drained { status, .. } => {
                    info!(%invoice_id, %tx_hash, block = receipt.block, status = ?status, "sweep finalized")
                }
                InvoiceOutcome::Retry => {
                    warn!(%invoice_id, %tx_hash, "sweep item failed for a transient reason; retrying")
                }
                InvoiceOutcome::Blocked { reason } => {
                    warn!(%invoice_id, %tx_hash, reason, "sweep item blocked")
                }
            }
        }
        Ok(())
    }

    /// Translate one item's receipt outcome into the invoice's next state.
    async fn classify(
        &self,
        invoice: &Invoice,
        row: &DbInvoice,
        tx_hash: alloy_primitives::B256,
        outcome: Option<&SweepOutcome>,
        header: &BlockHeader,
    ) -> Result<InvoiceOutcome, IndexerError> {
        let binding = bound(invoice)?;
        let payment = binding.payment_address.0;
        // Only a nonzero amount is a recovery worth a ledger row.
        let recovered = |amount: U256, reason: RecoveryReason| {
            (!amount.is_zero()).then_some(RecoveredFundsInput { amount, reason })
        };
        match outcome {
            Some(SweepOutcome::Settled {
                recovered_amount, ..
            }) => Ok(InvoiceOutcome::Drained {
                status: Some(InvoiceStatus::Fulfilled),
                execute_tx: true,
                settlement_tx_hash: tx_hash,
                settlement_block: header.number,
                settlement_timestamp: header.timestamp,
                settlement_recovered: None,
                recovered: recovered(*recovered_amount, RecoveryReason::Overpayment),
            }),
            Some(SweepOutcome::Recovered { amount }) => Ok(InvoiceOutcome::Drained {
                status: Some(InvoiceStatus::Recovered),
                execute_tx: true,
                settlement_tx_hash: tx_hash,
                settlement_block: header.number,
                settlement_timestamp: header.timestamp,
                settlement_recovered: None,
                recovered: recovered(*amount, RecoveryReason::Expired),
            }),
            Some(SweepOutcome::Collected { amount }) => {
                // The contract already existed. For an open invoice that means
                // someone else deployed it; the contract recorded how it routed
                // the balance it found.
                let status = if invoice.status.is_terminal() {
                    None
                } else if self.chain.payment_settled(payment, header.number).await? {
                    Some(InvoiceStatus::Fulfilled)
                } else {
                    Some(InvoiceStatus::Recovered)
                };
                let from_block = self
                    .repo
                    .first_observed_block(row.id)
                    .await?
                    .unwrap_or(self.cfg.usdc_start_block);
                let settlement = self
                    .chain
                    .payment_settlement_tx(payment, from_block, header.number)
                    .await?;
                let settlement_header = self.chain.block_header(settlement.block_number).await?;
                if settlement_header.hash != settlement.block_hash {
                    return Err(ChainError::FinalityViolation(format!(
                        "settlement event block {} changed during classification",
                        settlement.block_number
                    ))
                    .into());
                }
                // A terminal invoice's settlement was ledgered when it resolved
                // (or predates the ledger). An open one was deployed by someone
                // else, and whatever that deployment sent to the recovery
                // wallet is known only from its own logs: the remainder of a
                // live settlement, or the whole balance once expired.
                let settlement_recovered = if invoice.status.is_terminal() {
                    None
                } else if settlement.settled.is_some() {
                    recovered(settlement.recovered, RecoveryReason::Overpayment)
                } else {
                    recovered(settlement.recovered, RecoveryReason::Expired)
                };
                Ok(InvoiceOutcome::Drained {
                    status,
                    execute_tx: false,
                    settlement_tx_hash: settlement.transaction_hash,
                    settlement_block: settlement.block_number,
                    settlement_timestamp: settlement_header.timestamp,
                    settlement_recovered,
                    recovered: recovered(*amount, RecoveryReason::LateTransfer),
                })
            }
            Some(SweepOutcome::Failed { .. }) => {
                let expired = header.timestamp > invoice.expiration_timestamp;
                let probe = self
                    .chain
                    .probe_failure(
                        binding.network.token.0,
                        payment,
                        invoice.beneficiary.0,
                        binding.recovery.0,
                        header.number,
                    )
                    .await?;
                let to_recovery = probe.code_present || expired;
                // A live deployment also forwards any balance above the
                // invoice amount to the recovery wallet in the same
                // transaction, so that leg alone can fail an overpaid item.
                let touches_recovery = to_recovery
                    || probe
                        .balance
                        .is_some_and(|balance| balance > invoice.amount.0);
                let blocked = |reason: &str| InvoiceOutcome::Blocked {
                    reason: reason.to_string(),
                };
                Ok(if probe.paused == Some(true) {
                    InvoiceOutcome::Retry
                } else if probe.payment_blacklisted == Some(true) {
                    blocked("payment_address_blacklisted")
                } else if touches_recovery && probe.recovery_blacklisted == Some(true) {
                    blocked("recovery_blacklisted")
                } else if !to_recovery && probe.receiver_blacklisted == Some(true) {
                    blocked("beneficiary_blacklisted")
                } else if !to_recovery
                    && probe
                        .balance
                        .is_some_and(|balance| balance < invoice.amount.0)
                {
                    blocked("balance_below_amount")
                } else if row.sweep_attempts.saturating_add(1) as u32 >= self.cfg.sweep_max_attempts
                {
                    blocked("retries_exhausted")
                } else {
                    InvoiceOutcome::Retry
                })
            }
            None => Err(IndexerError::SweepStalled(format!(
                "helper transaction {tx_hash} carries no outcome for invoice {}; batch cannot be finalized",
                invoice.id.0
            ))),
        }
    }

    /// No submission for the batch has a receipt yet.
    async fn handle_unmined(&self, batch: &SweepBatch) -> Result<(), IndexerError> {
        let age = Utc::now()
            .signed_duration_since(batch.submitted_at)
            .to_std()
            .unwrap_or_default();
        if age < self.cfg.sweep_pending_timeout {
            return Ok(());
        }

        // Monad reports no receipt for a transaction still in flight and forgets
        // one it dropped, so after the timeout the mined nonce is the signal: if
        // it moved past ours without a receipt, nothing we sent can mine any more.
        if self.chain.signer_nonce(false).await? > batch.nonce {
            let requeued = self
                .repo
                .resolve_batch(batch.id, BatchResolution::Abandoned)
                .await?;
            warn!(batch_id = %batch.id, nonce = batch.nonce, requeued, "sweep batch nonce was consumed without a visible receipt; invoices returned to the queue");
            return Ok(());
        }

        if batch.tx_hashes.len() as u32 >= self.cfg.sweep_max_submissions {
            return Err(IndexerError::SweepStalled(format!(
                "sweep batch {} (nonce {}) is unconfirmed after {} submissions; check the signer balance and fee market, then replace or cancel nonce {} manually",
                batch.id,
                batch.nonce,
                batch.tx_hashes.len(),
                batch.nonce
            )));
        }

        let previous = FeeEstimate {
            max_fee_per_gas: batch.max_fee_per_gas,
            max_priority_fee_per_gas: batch.max_priority_fee_per_gas,
        };
        let fees = self.chain.estimate_fees().await?.max(previous.bumped());
        let rows = self.repo.batch_invoices(batch.id).await?;
        let requests = rows
            .iter()
            .map(|row| decode(row).and_then(|invoice| sweep_request(&invoice)))
            .collect::<Result<Vec<_>, _>>()?;
        let transaction = self
            .chain
            .prepare_sweep_batch(
                self.cfg.batch_sweeper,
                &requests,
                batch.nonce,
                batch.gas_limit,
                fees,
            )
            .await?;
        if !self
            .repo
            .record_batch_replacement(
                batch.id,
                transaction.hash,
                fees.max_fee_per_gas,
                fees.max_priority_fee_per_gas,
                &transaction.raw,
            )
            .await?
        {
            return Err(IndexerError::Configuration(format!(
                "could not persist replacement for open sweep batch {}",
                batch.id
            )));
        }
        self.broadcast_prepared(batch.id, &transaction).await?;
        warn!(batch_id = %batch.id, nonce = batch.nonce, tx_hash = %transaction.hash, submission = batch.tx_hashes.len() + 1, max_fee_per_gas = fees.max_fee_per_gas, "replaced unconfirmed helper transaction");
        Ok(())
    }

    async fn submit_next_batch(&self) -> Result<(), IndexerError> {
        let claimed = self
            .repo
            .claim_sweep_batch(
                self.cfg.chain_id.0,
                SWEEP_BATCH_LIMIT,
                self.cfg.sweep_backoff_base_secs,
                self.cfg.sweep_backoff_cap_secs,
            )
            .await?;
        if claimed.is_empty() {
            return Ok(());
        }

        let mut ids = Vec::with_capacity(claimed.len());
        let mut requests = Vec::with_capacity(claimed.len());
        for row in &claimed {
            let invoice = match decode(row) {
                Ok(invoice) => invoice,
                Err(error) => {
                    self.repo.block_invoice(row.id, "corrupt_row").await?;
                    error!(invoice_id = %row.id, %error, "invoice row cannot be decoded; blocked");
                    continue;
                }
            };
            // The address commits every parameter, so a row that no longer
            // derives its own address would sweep an address nobody paid,
            // and a row bound to another network is another chain's to sweep.
            let network = invoice.network().copied();
            if network.is_none_or(|network| {
                network.chain_id != self.cfg.chain_id
                    || network.factory.0 != self.cfg.factory
                    || network.token.0 != self.cfg.usdc
            }) || !invoice.address_matches_parameters()
            {
                self.repo
                    .block_invoice(row.id, "parameters_mismatch")
                    .await?;
                error!(invoice_id = %row.id, "invoice parameters do not derive its payment address for this deployment; blocked");
                continue;
            }
            ids.push(row.id);
            requests.push(sweep_request(&invoice)?);
        }
        if ids.is_empty() {
            return Ok(());
        }

        let nonce = self.chain.signer_nonce(true).await?;
        let fees = self.chain.estimate_fees().await?;
        let gas_limit = sweep_batch_gas_limit(requests.len());
        let transaction = match self
            .chain
            .prepare_sweep_batch(self.cfg.batch_sweeper, &requests, nonce, gas_limit, fees)
            .await
        {
            Ok(transaction) => transaction,
            Err(error) => {
                self.repo.release_claim(&ids).await?;
                return Err(error.into());
            }
        };
        let batch_id = self
            .repo
            .record_batch_submission(
                self.cfg.chain_id.0,
                &ids,
                nonce,
                gas_limit,
                fees.max_fee_per_gas,
                fees.max_priority_fee_per_gas,
                transaction.hash,
                &transaction.raw,
            )
            .await?;
        self.broadcast_prepared(batch_id, &transaction).await?;
        info!(%batch_id, tx_hash = %transaction.hash, nonce, invoices = ids.len(), gas_limit, "helper transaction submitted");
        Ok(())
    }

    async fn report_sweep_health(&self) -> Result<(), IndexerError> {
        let stats = self.repo.sweep_queue_stats(self.cfg.chain_id.0).await?;
        let balance = self.chain.signer_balance().await?;
        info!(
            queued = stats.queued,
            in_flight = stats.in_flight,
            oldest_uncollected_secs = stats.oldest_uncollected_secs,
            signer_balance_wei = %balance,
            "sweep worker health"
        );
        if balance < self.cfg.signer_low_balance_wei {
            warn!(signer_balance_wei = %balance, threshold_wei = %self.cfg.signer_low_balance_wei, "sweep signer balance low");
        }
        if stats
            .oldest_uncollected_secs
            .is_some_and(|age| age > SWEEP_BACKLOG_WARN_SECS)
        {
            warn!(
                oldest_uncollected_secs = stats.oldest_uncollected_secs,
                queued = stats.queued,
                "sweep backlog stale"
            );
        }
        Ok(())
    }
}

fn decode(row: &DbInvoice) -> Result<Invoice, IndexerError> {
    Invoice::try_from(row).map_err(|error| {
        IndexerError::Configuration(format!("corrupt invoice {}: {error}", row.id))
    })
}

/// The binding a queued invoice must have: funds only reach a bound
/// address, so a queued row without one is corrupt.
fn bound(invoice: &Invoice) -> Result<&PaymentBinding, IndexerError> {
    invoice.binding.as_ref().ok_or_else(|| {
        IndexerError::Configuration(format!(
            "invoice {} is queued without a payer wallet binding",
            invoice.id.0
        ))
    })
}

fn sweep_request(invoice: &Invoice) -> Result<SweepRequest, IndexerError> {
    let binding = bound(invoice)?;
    Ok(SweepRequest {
        token: binding.network.token.0,
        amount: invoice.amount.0,
        receiver: invoice.beneficiary.0,
        expiration_timestamp: invoice.expiration_timestamp,
        recovery: binding.recovery.0,
        salt: binding.salt.0,
        chain_id: binding.network.chain_id.0,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::sync::Mutex;

    use alloy_primitives::{Address, B256, Bytes, U256, address, keccak256};
    use async_trait::async_trait;
    use gateway_core::{
        Amount, BeneficiaryAddress, CanonicalIssuanceSnapshot, ChainId, FactoryAddress, Invoice,
        NetworkTerms, Party, PayerAttestation, PayerPolicy, RecoveryAddress, TokenAddress,
        USDC_DECIMALS, sign_payer_attestation, wallet_of,
    };
    use sqlx::PgPool;

    use crate::chain::{FailureProbe, SettlementEvent, UsdcTransfer};
    use gateway_db::{
        BindPayerWallet, CreateInvoiceInput, InvoiceRepository, PAYER_SESSION_TTL,
        PayerSessionRepository,
    };

    const CHAIN_ID: u64 = 31337;
    /// Block timestamps in the mock advance ten seconds per block from here.
    const GENESIS_TIMESTAMP: u64 = 1_800_000_000;
    /// Comfortably after every block the tests mine.
    const FAR_EXPIRY: u64 = GENESIS_TIMESTAMP + 1_000_000;

    fn usdc() -> Address {
        address!("0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512")
    }

    fn factory() -> FactoryAddress {
        FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3"))
    }

    fn batch_sweeper() -> Address {
        address!("0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0")
    }

    fn block_hash(block: u64) -> B256 {
        B256::with_last_byte(block as u8)
    }

    fn block_timestamp(block: u64) -> u64 {
        GENESIS_TIMESTAMP + block * 10
    }

    /// The address a sweep item deploys to, derived the way the API does.
    fn payment_address_of(sweep: &SweepRequest) -> Address {
        gateway_core::predict_payment_address(
            factory(),
            TokenAddress(sweep.token),
            Amount(sweep.amount),
            BeneficiaryAddress(sweep.receiver),
            sweep.expiration_timestamp,
            RecoveryAddress(sweep.recovery),
            gateway_core::Salt(sweep.salt),
            ChainId(sweep.chain_id),
        )
        .0
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Submission {
        nonce: u64,
        gas_limit: u64,
        fees: FeeEstimate,
        sweeps: Vec<SweepRequest>,
        tx_hash: B256,
    }

    #[derive(Default)]
    pub(crate) struct MockState {
        pub(crate) chain_id: u64,
        latest: u64,
        finalized: u64,
        transfers: Vec<UsdcTransfer>,
        max_log_range: Option<u64>,
        /// Every `eth_getLogs` the worker made: the range and the recipient
        /// filter it carried.
        log_requests: Vec<(u64, u64, Vec<Address>)>,
        /// Hash overrides to simulate a reorg at a height.
        hashes: HashMap<u64, B256>,
        receipts: HashMap<B256, SweepReceipt>,
        /// Outcomes attached to the receipt of the next submission, keyed by
        /// payment address. Items without an entry get `Settled`.
        next_outcomes: HashMap<Address, SweepOutcome>,
        /// Block the next submission's receipt lands in; `None` leaves it unmined.
        mine_at: Option<u64>,
        next_receipt_succeeds: bool,
        prepared: HashMap<B256, Submission>,
        next_transaction_id: u8,
        submissions: Vec<Submission>,
        mined_nonce: u64,
        submit_error: Option<fn() -> ChainError>,
        fees: FeeEstimate,
        balance: U256,
        settled: HashMap<Address, bool>,
        settlement_events: HashMap<Address, SettlementEvent>,
        probes: HashMap<Address, FailureProbe>,
        header_requests: usize,
        reorg_on_header_request: Option<usize>,
        /// Number of `block_header` reads that still fail with a retryable
        /// error before succeeding; see the range-retry test.
        failing_header_reads: u32,
        /// Runtime code hashes by address; see [`mock_code_hash`] for the default.
        pub(crate) code_hashes: HashMap<Address, B256>,
        /// Factory each BatchSweeper reports; defaults to the test factory.
        pub(crate) sweeper_factories: HashMap<Address, Address>,
    }

    /// The code hash the mock reports for an address it has no override for:
    /// a stand-in for "some deployed runtime bytecode" that differs per address.
    pub(crate) fn mock_code_hash(address: Address) -> B256 {
        keccak256(address.as_slice())
    }

    pub(crate) struct MockChain {
        state: Mutex<MockState>,
    }

    impl MockChain {
        pub(crate) fn new(latest: u64) -> Self {
            Self {
                state: Mutex::new(MockState {
                    chain_id: 31337,
                    latest,
                    finalized: latest,
                    mine_at: Some(latest),
                    next_receipt_succeeds: true,
                    fees: FeeEstimate {
                        max_fee_per_gas: 100,
                        max_priority_fee_per_gas: 2,
                    },
                    balance: U256::from(10u64).pow(U256::from(18u64)),
                    ..MockState::default()
                }),
            }
        }

        pub(crate) fn with(self, apply: impl FnOnce(&mut MockState)) -> Self {
            apply(&mut self.state.lock().unwrap());
            self
        }

        fn set(&self, apply: impl FnOnce(&mut MockState)) {
            apply(&mut self.state.lock().unwrap());
        }

        fn submissions(&self) -> Vec<Submission> {
            self.state.lock().unwrap().submissions.clone()
        }
    }

    #[async_trait]
    impl ChainClient for MockChain {
        async fn get_chain_id(&self) -> Result<u64, ChainError> {
            Ok(self.state.lock().unwrap().chain_id)
        }

        async fn latest_block_number(&self) -> Result<u64, ChainError> {
            Ok(self.state.lock().unwrap().latest)
        }

        async fn finalized_header(&self) -> Result<BlockHeader, ChainError> {
            // A tag read is a header read: it counts toward `header_requests`
            // like every other `eth_getBlockByNumber`.
            let finalized = self.state.lock().unwrap().finalized;
            self.block_header(finalized).await
        }

        async fn block_header(&self, number: u64) -> Result<BlockHeader, ChainError> {
            let mut state = self.state.lock().unwrap();
            state.header_requests += 1;
            if state.failing_header_reads > 0 {
                state.failing_header_reads -= 1;
                return Err(ChainError::Transient(format!(
                    "block {number} read throttled"
                )));
            }
            if number > state.latest {
                return Err(ChainError::Transient(format!(
                    "block {number} is unavailable"
                )));
            }
            Ok(BlockHeader {
                number,
                hash: if state
                    .reorg_on_header_request
                    .is_some_and(|request| state.header_requests >= request)
                {
                    B256::repeat_byte(0xEE)
                } else {
                    state
                        .hashes
                        .get(&number)
                        .copied()
                        .unwrap_or_else(|| block_hash(number))
                },
                timestamp: block_timestamp(number),
            })
        }

        async fn usdc_transfers(
            &self,
            _token: Address,
            from_block: u64,
            to_block: u64,
            recipients: &[Address],
        ) -> Result<Vec<UsdcTransfer>, ChainError> {
            let mut state = self.state.lock().unwrap();
            let requested = to_block - from_block + 1;
            if state.max_log_range.is_some_and(|max| requested > max) {
                return Err(ChainError::LogRangeTooLarge(format!(
                    "mock limit is {} blocks",
                    state.max_log_range.unwrap()
                )));
            }
            state
                .log_requests
                .push((from_block, to_block, recipients.to_vec()));
            Ok(state
                .transfers
                .iter()
                .filter(|transfer| (from_block..=to_block).contains(&transfer.block_number))
                .filter(|transfer| recipients.contains(&transfer.recipient))
                .cloned()
                .collect())
        }

        async fn sweep_receipt(
            &self,
            tx_hash: B256,
            _batch_sweeper: Address,
        ) -> Result<Option<SweepReceipt>, ChainError> {
            Ok(self.state.lock().unwrap().receipts.get(&tx_hash).cloned())
        }

        async fn signer_nonce(&self, pending: bool) -> Result<u64, ChainError> {
            let state = self.state.lock().unwrap();
            // Monad semantics: `pending` reads the same as `latest`.
            let _ = pending;
            Ok(state.mined_nonce)
        }

        async fn signer_balance(&self) -> Result<U256, ChainError> {
            Ok(self.state.lock().unwrap().balance)
        }

        async fn estimate_fees(&self) -> Result<FeeEstimate, ChainError> {
            Ok(self.state.lock().unwrap().fees)
        }

        async fn prepare_sweep_batch(
            &self,
            _batch_sweeper: Address,
            sweeps: &[SweepRequest],
            nonce: u64,
            gas_limit: u64,
            fees: FeeEstimate,
        ) -> Result<PreparedSweepTransaction, ChainError> {
            let mut state = self.state.lock().unwrap();
            state.next_transaction_id += 1;
            let tx_hash = B256::with_last_byte(state.next_transaction_id);
            let raw = alloy_primitives::Bytes::copy_from_slice(tx_hash.as_slice());
            state.prepared.insert(
                tx_hash,
                Submission {
                    nonce,
                    gas_limit,
                    fees,
                    sweeps: sweeps.to_vec(),
                    tx_hash,
                },
            );
            Ok(PreparedSweepTransaction { hash: tx_hash, raw })
        }

        async fn broadcast_sweep_transaction(
            &self,
            transaction: &PreparedSweepTransaction,
        ) -> Result<(), ChainError> {
            let mut state = self.state.lock().unwrap();
            if let Some(error) = state.submit_error.take() {
                return Err(error());
            }
            if state
                .submissions
                .iter()
                .any(|submission| submission.tx_hash == transaction.hash)
            {
                return Ok(());
            }
            let submission = state.prepared[&transaction.hash].clone();
            let tx_hash = submission.tx_hash;
            let nonce = submission.nonce;
            let sweeps = submission.sweeps.clone();
            state.submissions.push(submission);
            if let Some(block) = state.mine_at {
                let outcomes =
                    sweeps
                        .iter()
                        .map(|sweep| {
                            let payment = payment_address_of(sweep);
                            let outcome = state.next_outcomes.remove(&payment).unwrap_or(
                                SweepOutcome::Settled {
                                    amount: sweep.amount,
                                    recovered_amount: U256::ZERO,
                                },
                            );
                            (payment, outcome)
                        })
                        .collect();
                let succeeded = state.next_receipt_succeeds;
                state.mined_nonce = nonce + 1;
                state.receipts.insert(
                    tx_hash,
                    SweepReceipt {
                        succeeded,
                        block,
                        block_hash: block_hash(block),
                        transaction_index: 1,
                        outcomes,
                    },
                );
            }
            Ok(())
        }

        async fn payment_settled(&self, payment: Address, _block: u64) -> Result<bool, ChainError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .settled
                .get(&payment)
                .copied()
                .unwrap_or(true))
        }

        async fn payment_settlement_tx(
            &self,
            payment: Address,
            from_block: u64,
            _to_block: u64,
        ) -> Result<SettlementEvent, ChainError> {
            let state = self.state.lock().unwrap();
            if let Some(event) = state.settlement_events.get(&payment) {
                return Ok(*event);
            }
            // Without an override the deployment was this worker's own exact
            // settlement, whose amount the mock finds among its submissions.
            let settled = state
                .submissions
                .iter()
                .flat_map(|submission| &submission.sweeps)
                .find(|sweep| payment_address_of(sweep) == payment)
                .map_or(U256::ZERO, |sweep| sweep.amount);
            Ok(SettlementEvent {
                transaction_hash: B256::repeat_byte(0xCC),
                block_number: from_block,
                block_hash: block_hash(from_block),
                settled: Some(settled),
                recovered: U256::ZERO,
            })
        }

        async fn probe_failure(
            &self,
            _token: Address,
            payment: Address,
            _receiver: Address,
            _recovery: Address,
            _block: u64,
        ) -> Result<FailureProbe, ChainError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .probes
                .get(&payment)
                .copied()
                .unwrap_or_default())
        }

        async fn code_hash(&self, address: Address) -> Result<B256, ChainError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .code_hashes
                .get(&address)
                .copied()
                .unwrap_or_else(|| mock_code_hash(address)))
        }

        async fn batch_sweeper_factory(
            &self,
            batch_sweeper: Address,
        ) -> Result<Address, ChainError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .sweeper_factories
                .get(&batch_sweeper)
                .copied()
                .unwrap_or(factory().0))
        }
    }

    fn config() -> IndexerConfig {
        IndexerConfig {
            chain_id: ChainId(CHAIN_ID),
            factory: factory().0,
            batch_sweeper: batch_sweeper(),
            usdc: usdc(),
            usdc_start_block: 0,
            finality_source: FinalitySource::Finalized,
            finality_confirmations: 0,
            block_time: Duration::from_millis(25),
            log_range_size: 100,
            max_ranges_per_tick: 20,
            poll_interval: Duration::from_millis(10),
            reconcile_interval: Duration::from_millis(10),
            idle_interval: Duration::from_millis(10),
            late_watch_window: Duration::from_secs(30 * 24 * 3600),
            sweep_pending_timeout: Duration::from_secs(60),
            sweep_max_submissions: 3,
            sweep_max_attempts: 3,
            sweep_backoff_base_secs: 0.0,
            sweep_backoff_cap_secs: 0.0,
            signer_low_balance_wei: U256::ZERO,
        }
    }

    fn indexer_with(pool: &PgPool, chain: Arc<MockChain>, cfg: IndexerConfig) -> Indexer {
        Indexer::new(
            InvoiceRepository::new(pool.clone()),
            CursorRepository::new(pool.clone()),
            chain,
            cfg,
        )
    }

    fn indexer(pool: &PgPool, chain: Arc<MockChain>) -> Indexer {
        indexer_with(pool, chain, config())
    }

    /// Build a USDC invoice for the test chain with the given atomic amount.
    fn make_invoice(amount: u64) -> Invoice {
        make_invoice_expiring(amount, FAR_EXPIRY)
    }

    fn make_invoice_expiring(amount: u64, expiration: u64) -> Invoice {
        issue(ChainId(CHAIN_ID), amount, expiration)
    }

    /// The wallet every test payer attests and pays from.
    const PAYER_KEY: [u8; 32] = [7u8; 32];

    fn payer_wallet() -> Address {
        wallet_of(&PAYER_KEY)
    }

    fn payment_address(invoice: &Invoice) -> Address {
        invoice.payment_address().expect("test invoice is bound").0
    }

    /// A second network every test request offers and nobody pays on.
    const OTHER_CHAIN: ChainId = ChainId(84_532);

    /// Issue with a minimal permissionless snapshot offering the test chain
    /// and another, unbound: what a request looks like before its payer
    /// signs. The document is not what the indexer is exercising.
    fn issue_unbound(amount: u64, expiration: u64) -> Invoice {
        let networks = vec![
            NetworkTerms {
                chain_id: ChainId(CHAIN_ID),
                token: TokenAddress(usdc()),
                factory: factory(),
            },
            NetworkTerms {
                chain_id: OTHER_CHAIN,
                token: TokenAddress(address!("0x036CbD53842c5426634e7929541eC2318f3dCF7e")),
                factory: factory(),
            },
        ];
        let beneficiary =
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01"));
        let amount = Amount(U256::from(amount));
        let party = |name: &str| Party {
            name: name.into(),
            email: None,
            details: None,
        };
        let snapshot = CanonicalIssuanceSnapshot::new(
            party("Acme"),
            party("Globex"),
            PayerPolicy::Permissionless,
            &networks,
            beneficiary,
            amount,
            expiration,
        );
        Invoice::issue(&networks, beneficiary, amount, expiration, snapshot).unwrap()
    }

    /// Issue and bind the test payer's wallet on `chain_id` in memory;
    /// `insert` writes the same binding to the row.
    fn issue(chain_id: ChainId, amount: u64, expiration: u64) -> Invoice {
        let mut invoice = issue_unbound(amount, expiration);
        // A fresh nonce per invoice keeps identical requests at distinct
        // addresses, as a session's challenge would.
        let nonce = keccak256(invoice.id.0.as_bytes());
        let message =
            PayerAttestation::new(invoice.attribution_hash, payer_wallet(), nonce, expiration);
        let attestation = sign_payer_attestation(&PAYER_KEY, &message, chain_id.0, factory().0);
        let binding = invoice
            .bind_payer_wallet(chain_id, attestation, "2026-09-06T00:00:00Z".into())
            .unwrap();
        invoice.binding = Some(binding);
        invoice
    }

    fn transfer(recipient: Address, amount: u64, block: u64, log_index: u64) -> UsdcTransfer {
        transfer_at(recipient, amount, block, 0, log_index)
    }

    fn transfer_at(
        recipient: Address,
        amount: u64,
        block: u64,
        transaction_index: u64,
        log_index: u64,
    ) -> UsdcTransfer {
        UsdcTransfer {
            block_number: block,
            block_hash: block_hash(block),
            block_timestamp: block_timestamp(block),
            transaction_hash: B256::from(U256::from(
                block * 1_000_000 + transaction_index * 1000 + log_index + 1,
            )),
            transaction_index,
            log_index,
            sender: payer_wallet(),
            recipient,
            amount: U256::from(amount),
        }
    }

    /// Insert an issued invoice without binding a wallet to it.
    async fn insert_unbound(pool: &PgPool, invoice: &Invoice, key: &str) {
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
            invoice.expiration_timestamp,
            format!("at:{}", invoice.expiration_timestamp),
        );
        repo.insert_issued(&input, None)
            .await
            .expect("insert should succeed");
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
            invoice.expiration_timestamp,
            format!("at:{}", invoice.expiration_timestamp),
        );
        repo.insert_issued(&input, None)
            .await
            .expect("insert should succeed");
        // The row takes the same binding the in-memory invoice carries.
        let session = PayerSessionRepository::new(pool.clone())
            .create(invoice.id.0, PAYER_SESSION_TTL)
            .await
            .unwrap();
        let bound = repo
            .bind_payer_wallet(
                invoice.id.0,
                session.id,
                invoice.binding.as_ref().expect("test invoice is bound"),
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        assert!(matches!(bound, BindPayerWallet::Bound(_)));
    }

    /// Insert an invoice and credit it through the block indexer at `block`,
    /// or at the first block the cursor has not yet passed.
    async fn insert_funded(pool: &PgPool, invoice: &Invoice, key: &str, block: u64) {
        insert(pool, invoice, key).await;
        let block = CursorRepository::new(pool.clone())
            .get(CHAIN_ID, usdc())
            .await
            .unwrap()
            .map_or(block, |cursor| block.max(cursor.block + 1));
        let chain = Arc::new(MockChain::new(block).with(|state| {
            state.transfers = vec![transfer(
                payment_address(invoice),
                invoice.amount.0.to::<u64>(),
                block,
                0,
            )]
        }));
        indexer(pool, chain).tick().await.unwrap();
        assert_eq!(fetch(pool, invoice).await.status, "funded");
    }

    async fn fetch(pool: &PgPool, invoice: &Invoice) -> DbInvoice {
        InvoiceRepository::new(pool.clone())
            .find_by_id(invoice.id.0)
            .await
            .unwrap()
            .unwrap()
    }

    async fn uncollected(pool: &PgPool, invoice: &Invoice) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM payment_observations WHERE invoice_id = $1 AND collected_at_block IS NULL AND disposition <> 'error'",
        )
        .bind(invoice.id.0)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn open_batches(pool: &PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM sweep_batches WHERE resolved_at IS NULL")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn set_submitted_at_in_the_past(pool: &PgPool) {
        sqlx::query("UPDATE sweep_batches SET submitted_at = now() - interval '1 hour'")
            .execute(pool)
            .await
            .unwrap();
    }

    /// Recovery ledger rows for an invoice: (amount, reason, tx hash, block,
    /// recovered_at as a unix timestamp), oldest first.
    async fn ledger(pool: &PgPool, invoice: &Invoice) -> Vec<(String, String, Vec<u8>, i64, i64)> {
        sqlx::query_as(
            "SELECT amount, reason, transaction_hash, block_number,
                    EXTRACT(EPOCH FROM recovered_at)::bigint
             FROM recovered_funds WHERE invoice_id = $1 ORDER BY created_at",
        )
        .bind(invoice.id.0)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    // --- Block indexer ------------------------------------------------------

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn funds_invoice_from_finalized_usdc_transfer(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let chain =
            Arc::new(MockChain::new(1).with(|state| {
                state.transfers = vec![transfer(payment_address(&invoice), 100, 1, 0)]
            }));
        indexer(&pool, chain)
            .tick()
            .await
            .expect("tick should succeed");

        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "funded");
        assert_eq!(row.observed_amount.as_deref(), Some("100"));
        assert_eq!(row.confirmed_received, "100");
        assert_eq!(row.funded_at_block, Some(1));
        assert_eq!(
            row.funded_at_block_hash.as_deref(),
            Some(block_hash(1).as_slice())
        );
        assert_eq!(row.uncollected_count, 1);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn finalized_tag_gates_crediting_not_the_latest_block(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;
        let chain = Arc::new(MockChain::new(5).with(|state| {
            state.finalized = 2;
            state.transfers = vec![transfer(payment_address(&invoice), 100, 3, 0)];
        }));
        let worker = indexer(&pool, chain.clone());

        worker.tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "created");
        assert_eq!(
            CursorRepository::new(pool.clone())
                .get(CHAIN_ID, usdc())
                .await
                .unwrap()
                .unwrap()
                .block,
            2
        );

        chain.set(|state| state.finalized = 5);
        worker.tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "funded");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn confirmation_margin_is_subtracted_from_the_finality_anchor(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;
        let chain = Arc::new(MockChain::new(4).with(|state| {
            state.transfers = vec![transfer(payment_address(&invoice), 100, 3, 0)];
        }));
        let mut cfg = config();
        cfg.finality_source = FinalitySource::Latest;
        cfg.finality_confirmations = 2;
        let worker = indexer_with(&pool, chain.clone(), cfg);

        worker.tick().await.unwrap();
        assert_eq!(
            fetch(&pool, &invoice).await.status,
            "created",
            "block 3 is inside the margin"
        );

        chain.set(|state| state.latest = 5);
        worker.tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "funded");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn one_tick_drains_many_ranges_up_to_its_budget(pool: PgPool) {
        // A bound request keeps the chain watched, so every range is scanned.
        insert(&pool, &make_invoice(100), "watched").await;
        let chain = Arc::new(MockChain::new(1_000));
        let mut cfg = config();
        cfg.log_range_size = 100;
        cfg.max_ranges_per_tick = 4;
        let worker = indexer_with(&pool, chain.clone(), cfg);
        let cursor = CursorRepository::new(pool.clone());

        worker.tick().await.unwrap();
        assert_eq!(
            cursor.get(CHAIN_ID, usdc()).await.unwrap().unwrap().block,
            399
        );
        worker.tick().await.unwrap();
        assert_eq!(
            cursor.get(CHAIN_ID, usdc()).await.unwrap().unwrap().block,
            799
        );
        worker.tick().await.unwrap();
        assert_eq!(
            cursor.get(CHAIN_ID, usdc()).await.unwrap().unwrap().block,
            1_000
        );
        let requests_before = chain.state.lock().unwrap().header_requests;
        worker.tick().await.unwrap();
        assert_eq!(
            chain.state.lock().unwrap().header_requests - requests_before,
            2,
            "an idle tick records the finalized head and verifies the cursor hash"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_budget_limited_pass_reports_its_backlog(pool: PgPool) {
        insert(&pool, &make_invoice(100), "watched").await;
        let chain = Arc::new(MockChain::new(1_000));
        let mut cfg = config();
        cfg.log_range_size = 100;
        cfg.max_ranges_per_tick = 2;
        let worker = indexer_with(&pool, chain, cfg);

        let summary = worker.pass().await.unwrap();
        assert_eq!(summary.boundary, 1_000);
        assert_eq!(summary.indexed, 199, "two of the twenty ranges were spent");
        assert!(summary.backlog(), "the pass stopped inside the boundary");

        // Successive passes drain the backlog until the cursor reaches the
        // boundary, which is what the caller's immediate-re-pass cadence
        // relies on.
        let mut summary = summary;
        while summary.backlog() {
            summary = worker.pass().await.unwrap();
        }
        assert_eq!(summary.indexed, 1_000);
        assert!(!summary.backlog());
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn transfer_bearing_blocks_cost_no_header_reads(pool: PgPool) {
        // Fifty transfers in fifty distinct blocks across ten ranges: the
        // request count must be the range-end reads plus the finality read,
        // never a read per block that carried a transfer.
        let invoice = make_invoice(10_000);
        insert(&pool, &invoice, "key-1").await;
        let chain = Arc::new(MockChain::new(1_000).with(|state| {
            state.transfers = (0..50)
                .map(|i| transfer(payment_address(&invoice), 1, 20 * i + 1, 0))
                .collect();
        }));
        let mut cfg = config();
        cfg.log_range_size = 100;
        cfg.max_ranges_per_tick = 10;
        let worker = indexer_with(&pool, chain.clone(), cfg);

        worker.tick().await.unwrap();

        assert_eq!(
            chain.state.lock().unwrap().header_requests,
            1 + 10 * 2,
            "finality read plus two range-end reads per range"
        );
        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.confirmed_received, "50");
        let timestamps: Vec<i64> = sqlx::query_scalar(
            "SELECT block_timestamp FROM payment_observations WHERE invoice_id = $1 ORDER BY block_number LIMIT 2",
        )
        .bind(invoice.id.0)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            timestamps,
            vec![block_timestamp(1) as i64, block_timestamp(21) as i64],
            "observations carry the log's own block timestamp"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn wake_catches_up_to_the_signalled_block(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;
        // The signal saw the transfer in block 5, which the node has not
        // finalized when the wake arrives; it finalizes shortly after.
        let chain = Arc::new(MockChain::new(5).with(|state| {
            state.finalized = 3;
            state.transfers = vec![transfer(payment_address(&invoice), 100, 5, 0)];
        }));
        let worker = indexer(&pool, chain.clone());
        let advance = {
            let chain = chain.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(400)).await;
                chain.set(|state| state.finalized = 5);
            })
        };

        worker.reconcile(Some(5)).await.unwrap();
        advance.await.unwrap();

        assert_eq!(
            fetch(&pool, &invoice).await.status,
            "funded",
            "the wake pass polled finality until block 5 was covered"
        );
        assert_eq!(
            CursorRepository::new(pool.clone())
                .get(CHAIN_ID, usdc())
                .await
                .unwrap()
                .unwrap()
                .block,
            5
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn wake_without_a_reachable_target_falls_back_to_the_timer(pool: PgPool) {
        // Something to watch, or the pass would not scan at all.
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;
        let chain = Arc::new(MockChain::new(5).with(|state| state.finalized = 3));
        let worker = indexer(&pool, chain.clone());
        let started = Instant::now();
        worker.reconcile(Some(5)).await.unwrap();
        // Each attempt waits the two block times the boundary trails by.
        assert!(
            started.elapsed() >= config().block_time * 2 * WAKE_CATCHUP_ATTEMPTS,
            "catch-up is bounded, then the timer takes over"
        );
        assert_eq!(
            CursorRepository::new(pool.clone())
                .get(CHAIN_ID, usdc())
                .await
                .unwrap()
                .unwrap()
                .block,
            3
        );
    }

    /// The scan is demand-driven: a chain with nothing on its watch list
    /// spends no `eth_getLogs` and fast-forwards its cursor to the boundary.
    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn idle_chain_fast_forwards_without_scanning(pool: PgPool) {
        let stranger = Address::repeat_byte(0x99);
        let chain = Arc::new(MockChain::new(500).with(|state| {
            state.transfers = vec![transfer(stranger, 1, 250, 0)];
        }));
        let worker = indexer(&pool, chain.clone());
        assert!(worker.idle(), "nothing is bound yet");

        worker.tick().await.unwrap();
        let cursor = CursorRepository::new(pool.clone())
            .get(CHAIN_ID, usdc())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cursor.block, 500, "the cursor jumps to the boundary");
        assert_eq!(cursor.block_timestamp, Some(block_timestamp(500)));
        assert!(
            chain.state.lock().unwrap().log_requests.is_empty(),
            "an idle chain fetches no logs"
        );
        assert_eq!(worker.cadence(None), config().idle_interval);

        // A bound request appears: the next pass scans from where the cursor
        // stands, and the wake-up found the transfer in the new range.
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;
        chain.set(|state| {
            state.latest = 510;
            state.finalized = 510;
            state
                .transfers
                .push(transfer(payment_address(&invoice), 100, 505, 0));
        });
        worker.tick().await.unwrap();
        assert!(!worker.idle());
        assert_eq!(fetch(&pool, &invoice).await.status, "funded");
        let requests = chain.state.lock().unwrap().log_requests.clone();
        assert_eq!(requests, vec![(501, 510, vec![payment_address(&invoice)])]);
        assert_eq!(worker.cadence(None), config().poll_interval);
    }

    /// Every range fetch names the watch list, loaded after the boundary was
    /// read, so a high-volume token costs one small call and a transfer to
    /// a stranger is never even requested.
    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn range_scan_filters_logs_by_the_watch_list(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;
        let stranger = Address::repeat_byte(0x99);
        let chain = Arc::new(MockChain::new(3).with(|state| {
            state.transfers = vec![
                transfer(stranger, 5, 1, 0),
                transfer(payment_address(&invoice), 100, 2, 0),
            ];
        }));
        let worker = indexer(&pool, chain.clone());

        worker.tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "funded");
        let requests = chain.state.lock().unwrap().log_requests.clone();
        assert_eq!(requests, vec![(0, 3, vec![payment_address(&invoice)])]);
    }

    /// A request nobody has bound belongs to no chain, so this chain's
    /// finalized clock closes it like any other.
    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn unbound_invoices_expire_on_this_chains_clock(pool: PgPool) {
        let expiring = issue_unbound(100, block_timestamp(2) + 5);
        let live = issue_unbound(100, FAR_EXPIRY);
        insert_unbound(&pool, &expiring, "a").await;
        insert_unbound(&pool, &live, "b").await;
        let chain = Arc::new(MockChain::new(2));
        let worker = indexer(&pool, chain.clone());

        worker.tick().await.unwrap();
        assert_eq!(fetch(&pool, &expiring).await.status, "created");

        chain.set(|state| {
            state.latest = 3;
            state.finalized = 3;
        });
        worker.tick().await.unwrap();
        assert_eq!(fetch(&pool, &expiring).await.status, "expired");
        assert_eq!(fetch(&pool, &live).await.status, "created");
        assert!(fetch(&pool, &expiring).await.chain_id.is_none());
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn watch_list_follows_open_and_recently_settled_invoices(pool: PgPool) {
        let open = make_invoice(100);
        let settled = make_invoice(100);
        let stale = make_invoice(100);
        insert(&pool, &open, "a").await;
        insert(&pool, &settled, "b").await;
        insert(&pool, &stale, "c").await;
        let worker = indexer(&pool, Arc::new(MockChain::new(1)));
        let mut list = worker.watch_list();

        worker.refresh_watch_list().await.unwrap();
        assert!(list.has_changed().unwrap());
        let mut expected = vec![
            payment_address(&open),
            payment_address(&settled),
            payment_address(&stale),
        ];
        expected.sort();
        assert_eq!(**list.borrow_and_update(), expected);

        // A settled address stays watched for the late window, then drops.
        sqlx::query(
            "UPDATE invoices SET status = 'fulfilled', updated_at = now() - interval '1 day' WHERE id = $1",
        )
        .bind(settled.id.0)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE invoices SET status = 'recovered', updated_at = now() - interval '40 days' WHERE id = $1",
        )
        .bind(stale.id.0)
        .execute(&pool)
        .await
        .unwrap();
        worker.refresh_watch_list().await.unwrap();
        let mut expected = vec![payment_address(&open), payment_address(&settled)];
        expected.sort();
        assert_eq!(**list.borrow_and_update(), expected);

        // An unchanged set neither reloads nor re-sends the list.
        worker.refresh_watch_list().await.unwrap();
        assert!(!list.has_changed().unwrap());
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn overpayment_still_funds(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let chain =
            Arc::new(MockChain::new(1).with(|state| {
                state.transfers = vec![transfer(payment_address(&invoice), 150, 1, 0)]
            }));
        indexer(&pool, chain).tick().await.unwrap();

        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "funded");
        assert_eq!(row.observed_amount.as_deref(), Some("150"));
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn partial_transfers_accumulate_across_blocks(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let chain = Arc::new(MockChain::new(2).with(|state| {
            state.transfers = vec![
                transfer(payment_address(&invoice), 40, 1, 0),
                transfer(payment_address(&invoice), 60, 2, 1),
            ]
        }));
        indexer(&pool, chain).tick().await.unwrap();

        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "funded");
        assert_eq!(row.confirmed_received, "100");
        assert_eq!(row.funded_at_block, Some(2));
        assert_eq!(row.uncollected_count, 2);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn tick_is_idempotent(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let logs = vec![transfer(payment_address(&invoice), 100, 1, 0)];
        let chain = Arc::new(MockChain::new(1).with(|state| state.transfers = logs.clone()));
        indexer(&pool, chain).tick().await.expect("first tick");

        let chain = Arc::new(MockChain::new(5).with(|state| state.transfers = logs));
        indexer(&pool, chain).tick().await.expect("second tick");

        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "funded");
        assert_eq!(row.funded_at_block, Some(1), "funding block must be stable");
        assert_eq!(row.confirmed_received, "100");
        assert_eq!(row.uncollected_count, 1);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn zero_value_transfer_is_an_error_observation_and_never_queued(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;

        let chain =
            Arc::new(MockChain::new(1).with(|state| {
                state.transfers = vec![transfer(payment_address(&invoice), 0, 1, 0)]
            }));
        indexer(&pool, chain).tick().await.unwrap();

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
        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "created");
        assert_eq!(row.uncollected_count, 0);
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

        let chain =
            Arc::new(MockChain::new(1).with(|state| {
                state.transfers = vec![transfer(payment_address(&invoice), 1, 1, 0)]
            }));
        let error = indexer(&pool, chain).tick().await.unwrap_err();
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
    async fn finalized_cursor_mismatch_halts_the_indexer(pool: PgPool) {
        let chain = Arc::new(MockChain::new(1));
        indexer(&pool, chain.clone()).tick().await.unwrap();

        chain.set(|state| {
            state.latest = 2;
            state.finalized = 2;
            state.hashes.insert(1, B256::repeat_byte(0xFF));
        });
        let error = indexer(&pool, chain).tick().await.unwrap_err();
        assert!(error.requires_halt());
        assert!(error.to_string().contains("cursor hash mismatch"));
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn oversized_log_ranges_are_split_and_then_grow(pool: PgPool) {
        insert(&pool, &make_invoice(100), "watched").await;
        let chain = Arc::new(MockChain::new(10).with(|state| state.max_log_range = Some(2)));
        let mut cfg = config();
        cfg.log_range_size = 8;
        cfg.max_ranges_per_tick = 1;
        let worker = indexer_with(&pool, chain, cfg);

        worker.tick().await.expect("range should split and succeed");

        let cursor = CursorRepository::new(pool)
            .get(CHAIN_ID, usdc())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cursor.block, 1, "the successful range was blocks 0..=1");
        assert_eq!(
            worker.current_log_range_size.load(Ordering::Relaxed),
            3,
            "a successful small range should cautiously grow"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn throttled_chain_reads_back_off_and_still_commit_the_range(pool: PgPool) {
        let chain = Arc::new(MockChain::new(10).with(|state| {
            // Fail the range-end header read twice before letting it through;
            // exactly the provider-throttle shape that used to abort the tick.
            state.failing_header_reads = 2;
        }));
        let worker = indexer_with(&pool, chain, config());

        worker
            .tick()
            .await
            .expect("a throttled read should back off and retry, not abort the tick");

        let cursor = CursorRepository::new(pool)
            .get(CHAIN_ID, usdc())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            cursor.block, 10,
            "the range was committed after the retries"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn ignores_invoices_on_other_chains(pool: PgPool) {
        let invoice = issue(OTHER_CHAIN, 100, FAR_EXPIRY); // not CHAIN_ID
        insert(&pool, &invoice, "key-1").await;

        let chain =
            Arc::new(MockChain::new(1).with(|state| {
                state.transfers = vec![transfer(payment_address(&invoice), 100, 1, 0)]
            }));
        indexer(&pool, chain).tick().await.unwrap();

        assert_eq!(fetch(&pool, &invoice).await.status, "created");
    }

    // --- Expiry ---------------------------------------------------------------

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn open_invoices_expire_by_block_timestamp(pool: PgPool) {
        // Expires between the timestamps of blocks 2 and 3.
        let expiration = block_timestamp(2) + 5;
        let unpaid = make_invoice_expiring(100, expiration);
        let partial = make_invoice_expiring(100, expiration);
        let funded = make_invoice_expiring(100, expiration);
        let live = make_invoice_expiring(100, FAR_EXPIRY);
        for (invoice, key) in [
            (&unpaid, "a"),
            (&partial, "b"),
            (&funded, "c"),
            (&live, "d"),
        ] {
            insert(&pool, invoice, key).await;
        }
        let chain = Arc::new(MockChain::new(2).with(|state| {
            state.transfers = vec![
                transfer(payment_address(&partial), 40, 1, 0),
                transfer(payment_address(&funded), 100, 1, 1),
            ]
        }));
        let worker = indexer(&pool, chain.clone());

        worker.tick().await.unwrap();
        assert_eq!(fetch(&pool, &partial).await.status, "created");
        assert_eq!(fetch(&pool, &funded).await.status, "funded");

        chain.set(|state| {
            state.latest = 3;
            state.finalized = 3;
        });
        worker.tick().await.unwrap();
        for invoice in [&unpaid, &partial, &funded] {
            assert_eq!(fetch(&pool, invoice).await.status, "expired");
        }
        assert_eq!(fetch(&pool, &live).await.status, "created");
        // Only balances can be recovered: the partial and funded ones queue.
        assert_eq!(fetch(&pool, &unpaid).await.uncollected_count, 0);
        assert_eq!(fetch(&pool, &partial).await.uncollected_count, 1);
        assert_eq!(fetch(&pool, &funded).await.uncollected_count, 1);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn transfer_after_expiry_is_late_and_credited_to_nothing(pool: PgPool) {
        let invoice = make_invoice_expiring(100, block_timestamp(1) + 5);
        insert(&pool, &invoice, "key-1").await;
        let chain =
            Arc::new(MockChain::new(2).with(|state| {
                state.transfers = vec![transfer(payment_address(&invoice), 100, 2, 0)]
            }));
        let worker = indexer(&pool, chain.clone());
        // The first transfer is already after the deadline even though the
        // invoice's persisted status changes later in the same range.
        worker.tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "expired");

        chain.set(|state| {
            state.latest = 3;
            state.finalized = 3;
            state
                .transfers
                .push(transfer(payment_address(&invoice), 5, 3, 0));
        });
        worker.tick().await.unwrap();
        let dispositions: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT disposition, disposition_reason FROM payment_observations WHERE invoice_id = $1 ORDER BY block_number",
        )
        .bind(invoice.id.0)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            dispositions,
            vec![
                ("late".to_string(), Some("invoice_expired".to_string())),
                ("late".to_string(), Some("invoice_expired".to_string()))
            ]
        );
        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.confirmed_received, "0");
        assert_eq!(row.uncollected_count, 2);
    }

    // --- Sweep worker ---------------------------------------------------------

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn settles_a_funded_invoice_through_a_finalized_helper_transaction(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7));
        let worker = indexer(&pool, chain.clone());

        worker.sweep_tick().await.unwrap();
        let submissions = chain.submissions();
        assert_eq!(submissions.len(), 1);
        assert_eq!(submissions[0].nonce, 0);
        assert_eq!(submissions[0].gas_limit, sweep_batch_gas_limit(1));
        assert_eq!(
            submissions[0].sweeps,
            vec![sweep_request(&invoice).unwrap()]
        );
        assert_eq!(fetch(&pool, &invoice).await.status, "deploying");
        assert_eq!(open_batches(&pool).await, 1);

        worker.sweep_tick().await.unwrap();
        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "fulfilled");
        assert_eq!(
            row.execute_tx_hash.as_deref(),
            Some(submissions[0].tx_hash.as_slice())
        );
        assert_eq!(
            row.settlement_tx_hash.as_deref(),
            Some(submissions[0].tx_hash.as_slice())
        );
        assert_eq!(row.resolved_at_block, Some(7));
        assert_eq!(row.drained_at_block, Some(7));
        assert_eq!(row.uncollected_count, 0);
        assert_eq!(row.sweep_batch_id, None);
        assert_eq!(uncollected(&pool, &invoice).await, 0);
        assert_eq!(open_batches(&pool).await, 0);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn mined_sweep_waits_for_finality_and_survives_restarts(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7).with(|state| state.finalized = 5));

        indexer(&pool, chain.clone()).sweep_tick().await.unwrap();
        indexer(&pool, chain.clone()).sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "deploying");
        let mined: Option<i64> = sqlx::query_scalar("SELECT mined_block FROM sweep_batches")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            mined,
            Some(7),
            "the receipt block is persisted while finality is pending"
        );

        chain.set(|state| state.finalized = 7);
        indexer(&pool, chain).sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "fulfilled");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn orphaned_receipt_is_forgotten_and_the_batch_keeps_waiting(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7));
        let worker = indexer(&pool, chain.clone());
        worker.sweep_tick().await.unwrap();

        chain.set(|state| {
            state.hashes.insert(7, B256::repeat_byte(0xEE));
        });
        worker.sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "deploying");
        let mined: Option<i64> = sqlx::query_scalar("SELECT mined_block FROM sweep_batches")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mined, None);
        assert_eq!(open_batches(&pool).await, 1);

        chain.set(|state| {
            state.hashes.clear();
        });
        worker.sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "fulfilled");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn reverted_helper_transaction_requeues_every_invoice(pool: PgPool) {
        let first = make_invoice(100);
        let second = make_invoice(200);
        insert_funded(&pool, &first, "key-1", 1).await;
        insert_funded(&pool, &second, "key-2", 1).await;
        let chain = Arc::new(MockChain::new(7).with(|state| {
            state.finalized = 5;
            state.next_receipt_succeeds = false;
        }));
        let worker = indexer(&pool, chain.clone());

        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        for invoice in [&first, &second] {
            assert!(fetch(&pool, invoice).await.sweep_batch_id.is_some());
        }
        assert_eq!(
            open_batches(&pool).await,
            1,
            "a non-final revert still owns its nonce"
        );

        chain.set(|state| state.finalized = 7);
        worker.sweep_tick().await.unwrap();
        for invoice in [&first, &second] {
            let row = fetch(&pool, invoice).await;
            assert_eq!(row.status, "deploying");
            assert_eq!(row.sweep_batch_id, None);
            assert_eq!(row.sweep_attempts, 1);
        }
        assert_eq!(open_batches(&pool).await, 0);

        chain.set(|state| state.next_receipt_succeeds = true);
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &first).await.status, "fulfilled");
        assert_eq!(fetch(&pool, &second).await.status, "fulfilled");
        assert_eq!(chain.submissions().len(), 2);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn reorg_during_receipt_classification_does_not_commit_outcomes(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7).with(|state| {
            // After the finality read, the first receipt-header read is
            // canonical; the pre-commit recheck changes.
            state.reorg_on_header_request = Some(3);
        }));
        let worker = indexer(&pool, chain);

        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "deploying");
        assert!(row.sweep_batch_id.is_some());
        let mined: Option<i64> = sqlx::query_scalar("SELECT mined_block FROM sweep_batches")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mined, None);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn expired_balance_is_recovered_and_reported_as_such(pool: PgPool) {
        let invoice = make_invoice_expiring(100, block_timestamp(1) + 5);
        insert(&pool, &invoice, "key-1").await;
        let chain = Arc::new(MockChain::new(2).with(|state| {
            state.transfers = vec![transfer(payment_address(&invoice), 40, 1, 0)];
        }));
        let worker = indexer(&pool, chain.clone());
        worker.tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "expired");

        chain.set(|state| {
            state.next_outcomes.insert(
                payment_address(&invoice),
                SweepOutcome::Recovered {
                    amount: U256::from(40),
                },
            );
        });
        worker.sweep_tick().await.unwrap();
        assert_eq!(
            fetch(&pool, &invoice).await.status,
            "expired",
            "no status flip while in flight"
        );
        worker.sweep_tick().await.unwrap();
        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "recovered");
        assert!(row.execute_tx_hash.is_some());
        assert_eq!(row.uncollected_count, 0);
    }

    // --- Recovery ledger ------------------------------------------------------

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn overpayment_records_only_remainder_as_recovered(pool: PgPool) {
        let invoice = make_invoice(100);
        insert(&pool, &invoice, "key-1").await;
        let chain =
            Arc::new(MockChain::new(7).with(|state| {
                state.transfers = vec![transfer(payment_address(&invoice), 150, 1, 0)]
            }));
        let worker = indexer(&pool, chain.clone());
        worker.tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "funded");

        // The contract pays the receiver exactly the invoice amount and sends
        // the remainder to the platform recovery wallet.
        chain.set(|state| {
            state.next_outcomes.insert(
                payment_address(&invoice),
                SweepOutcome::Settled {
                    amount: U256::from(100),
                    recovered_amount: U256::from(50),
                },
            );
        });
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();

        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "fulfilled");
        let tx_hash = chain.submissions()[0].tx_hash;
        assert_eq!(
            ledger(&pool, &invoice).await,
            vec![(
                "50".to_string(),
                "overpayment".to_string(),
                tx_hash.to_vec(),
                7,
                block_timestamp(7) as i64
            )]
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn expired_sweep_records_full_recovered_balance(pool: PgPool) {
        let invoice = make_invoice_expiring(100, block_timestamp(1) + 5);
        insert(&pool, &invoice, "key-1").await;
        let chain = Arc::new(MockChain::new(2).with(|state| {
            state.transfers = vec![transfer(payment_address(&invoice), 40, 1, 0)];
        }));
        let worker = indexer(&pool, chain.clone());
        worker.tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "expired");

        chain.set(|state| {
            state.next_outcomes.insert(
                payment_address(&invoice),
                SweepOutcome::Recovered {
                    amount: U256::from(40),
                },
            );
        });
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();

        assert_eq!(fetch(&pool, &invoice).await.status, "recovered");
        let tx_hash = chain.submissions()[0].tx_hash;
        assert_eq!(
            ledger(&pool, &invoice).await,
            vec![(
                "40".to_string(),
                "expired".to_string(),
                tx_hash.to_vec(),
                2,
                block_timestamp(2) as i64
            )]
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn late_collection_appends_recovery_ledger_entry(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7));
        let worker = indexer(&pool, chain.clone());
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "fulfilled");
        assert!(
            ledger(&pool, &invoice).await.is_empty(),
            "an exact settlement recovers nothing"
        );

        // A repeat payment lands after settlement; `recover()` forwards it.
        chain.set(|state| {
            state.latest = 9;
            state.finalized = 9;
            state.mine_at = Some(9);
            state
                .transfers
                .push(transfer(payment_address(&invoice), 30, 8, 0));
            state.next_outcomes.insert(
                payment_address(&invoice),
                SweepOutcome::Collected {
                    amount: U256::from(30),
                },
            );
        });
        worker.tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();

        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "fulfilled");
        assert_eq!(row.uncollected_count, 0);
        let submissions = chain.submissions();
        assert_eq!(submissions.len(), 2);
        assert_eq!(
            ledger(&pool, &invoice).await,
            vec![(
                "30".to_string(),
                "late_transfer".to_string(),
                submissions[1].tx_hash.to_vec(),
                9,
                block_timestamp(9) as i64
            )]
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn funded_invoice_swept_after_its_deadline_reports_recovered_not_fulfilled(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7).with(|state| {
            state.next_outcomes.insert(
                payment_address(&invoice),
                SweepOutcome::Recovered {
                    amount: U256::from(100),
                },
            );
        }));
        let worker = indexer(&pool, chain);
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "recovered");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn third_party_execution_is_classified_from_the_contract(pool: PgPool) {
        let settled = make_invoice(100);
        let recovered = make_invoice(200);
        insert_funded(&pool, &settled, "key-1", 1).await;
        insert_funded(&pool, &recovered, "key-2", 1).await;
        let chain = Arc::new(MockChain::new(7).with(|state| {
            state.settlement_events.insert(
                payment_address(&settled),
                SettlementEvent {
                    transaction_hash: B256::repeat_byte(0xA1),
                    block_number: 3,
                    block_hash: block_hash(3),
                    settled: Some(U256::from(100)),
                    recovered: U256::ZERO,
                },
            );
            state.settlement_events.insert(
                payment_address(&recovered),
                SettlementEvent {
                    transaction_hash: B256::repeat_byte(0xA2),
                    block_number: 4,
                    block_hash: block_hash(4),
                    settled: None,
                    recovered: U256::from(200),
                },
            );
            for invoice in [&settled, &recovered] {
                state.next_outcomes.insert(
                    payment_address(invoice),
                    SweepOutcome::Collected { amount: U256::ZERO },
                );
            }
            state.settled.insert(payment_address(&recovered), false);
        }));
        let worker = indexer(&pool, chain);
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();

        let settled_row = fetch(&pool, &settled).await;
        assert_eq!(settled_row.status, "fulfilled");
        assert_eq!(settled_row.execute_tx_hash, None, "not our transaction");
        assert_eq!(
            settled_row.settlement_tx_hash,
            Some(vec![0xA1; 32]),
            "the third party's transaction is still the settlement"
        );
        assert_eq!(settled_row.resolved_at_block, Some(3));
        assert_eq!(
            settled_row.settled_at.unwrap().timestamp(),
            (GENESIS_TIMESTAMP + 30) as i64
        );
        assert!(
            ledger(&pool, &settled).await.is_empty(),
            "an exact third-party settlement recovers nothing"
        );
        let recovered_row = fetch(&pool, &recovered).await;
        assert_eq!(recovered_row.status, "recovered");
        assert_eq!(recovered_row.execute_tx_hash, None);
        assert_eq!(recovered_row.settlement_tx_hash, Some(vec![0xA2; 32]));
        assert_eq!(recovered_row.resolved_at_block, Some(4));
        assert_eq!(
            ledger(&pool, &recovered).await,
            vec![(
                "200".to_string(),
                "expired".to_string(),
                vec![0xA2; 32],
                4,
                block_timestamp(4) as i64
            )],
            "the balance the third party's deployment recovered is ledgered under their transaction"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn third_party_overpaid_execution_ledgers_the_remainder(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        // Someone else deployed the contract while the address held 125: the
        // receiver got the invoice amount and the remainder went to the
        // recovery wallet in that same transaction, before our item ran.
        let chain = Arc::new(MockChain::new(7).with(|state| {
            state.settlement_events.insert(
                payment_address(&invoice),
                SettlementEvent {
                    transaction_hash: B256::repeat_byte(0xA3),
                    block_number: 3,
                    block_hash: block_hash(3),
                    settled: Some(U256::from(100)),
                    recovered: U256::from(25),
                },
            );
            state.next_outcomes.insert(
                payment_address(&invoice),
                SweepOutcome::Collected { amount: U256::ZERO },
            );
        }));
        let worker = indexer(&pool, chain);
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();

        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "fulfilled");
        assert_eq!(row.settlement_tx_hash, Some(vec![0xA3; 32]));
        assert_eq!(
            ledger(&pool, &invoice).await,
            vec![(
                "25".to_string(),
                "overpayment".to_string(),
                vec![0xA3; 32],
                3,
                block_timestamp(3) as i64
            )],
            "the remainder is keyed by the third party's transaction and an empty recover() adds nothing"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn third_party_expired_execution_ledgers_the_full_balance(pool: PgPool) {
        let invoice = make_invoice_expiring(100, block_timestamp(1) + 5);
        insert(&pool, &invoice, "key-1").await;
        let chain = Arc::new(MockChain::new(7).with(|state| {
            state.transfers = vec![transfer(payment_address(&invoice), 40, 1, 0)];
        }));
        let worker = indexer(&pool, chain.clone());
        worker.tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "expired");

        // Someone else deployed the expired contract, which sent the whole
        // balance to the recovery wallet.
        chain.set(|state| {
            state.settlement_events.insert(
                payment_address(&invoice),
                SettlementEvent {
                    transaction_hash: B256::repeat_byte(0xA4),
                    block_number: 4,
                    block_hash: block_hash(4),
                    settled: None,
                    recovered: U256::from(40),
                },
            );
            state.settled.insert(payment_address(&invoice), false);
            state.next_outcomes.insert(
                payment_address(&invoice),
                SweepOutcome::Collected { amount: U256::ZERO },
            );
        });
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();

        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "recovered");
        assert_eq!(row.settlement_tx_hash, Some(vec![0xA4; 32]));
        assert_eq!(
            ledger(&pool, &invoice).await,
            vec![(
                "40".to_string(),
                "expired".to_string(),
                vec![0xA4; 32],
                4,
                block_timestamp(4) as i64
            )]
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn late_funds_on_a_settled_invoice_are_collected_without_changing_its_status(
        pool: PgPool,
    ) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7));
        let worker = indexer(&pool, chain.clone());
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "fulfilled");

        // A repeat payment lands after the settlement block.
        chain.set(|state| {
            state.latest = 9;
            state.finalized = 9;
            state.mine_at = Some(9);
            state
                .transfers
                .push(transfer(payment_address(&invoice), 30, 8, 0));
            state.next_outcomes.insert(
                payment_address(&invoice),
                SweepOutcome::Collected {
                    amount: U256::from(30),
                },
            );
        });
        worker.tick().await.unwrap();
        let queued = fetch(&pool, &invoice).await;
        assert_eq!(queued.status, "fulfilled");
        assert_eq!(queued.uncollected_count, 1);
        assert_eq!(
            queued.confirmed_received, "100",
            "late funds never count toward the invoice"
        );

        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        let collected = fetch(&pool, &invoice).await;
        assert_eq!(collected.status, "fulfilled");
        assert_eq!(collected.uncollected_count, 0);
        assert_eq!(collected.drained_at_block, Some(9));
        assert_eq!(
            collected.resolved_at_block,
            Some(7),
            "the settlement block is kept"
        );
        assert_eq!(chain.submissions().len(), 2);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn transfer_indexed_after_the_drain_that_moved_it_is_never_requeued(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7).with(|state| state.mine_at = Some(5)));
        let worker = indexer(&pool, chain.clone());
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.drained_at_block, Some(5));

        // The block indexer only now reaches a transfer from block 3, which the
        // sweep at block 5 already forwarded.
        chain.set(|state| {
            state
                .transfers
                .push(transfer(payment_address(&invoice), 20, 3, 0));
        });
        worker.tick().await.unwrap();
        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.uncollected_count, 0);
        assert_eq!(uncollected(&pool, &invoice).await, 0);
        let collected_at: Option<i64> = sqlx::query_scalar(
            "SELECT collected_at_block FROM payment_observations WHERE invoice_id = $1 AND block_number = 3",
        )
        .bind(invoice.id.0)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(collected_at, Some(5));
        worker.sweep_tick().await.unwrap();
        assert_eq!(chain.submissions().len(), 1, "nothing to sweep");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn same_block_transfers_are_collected_only_when_before_the_sweep(pool: PgPool) {
        let before = make_invoice(100);
        let after = make_invoice(200);
        insert_funded(&pool, &before, "before", 1).await;
        insert_funded(&pool, &after, "after", 1).await;
        let chain = Arc::new(MockChain::new(7));
        let worker = indexer(&pool, chain.clone());

        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        assert_eq!(
            fetch(&pool, &before).await.drained_at_transaction_index,
            Some(1)
        );

        // These logs are deliberately indexed after sweep finalization. The
        // first was present when transaction 1 drained the address; the second
        // arrived too late and must remain queued for another recovery.
        chain.set(|state| {
            state.transfers = vec![
                transfer_at(payment_address(&before), 11, 7, 0, 0),
                transfer_at(payment_address(&after), 22, 7, 2, 0),
            ];
        });
        worker.tick().await.unwrap();

        assert_eq!(fetch(&pool, &before).await.uncollected_count, 0);
        assert_eq!(fetch(&pool, &after).await.uncollected_count, 1);
        let positions: Vec<(i64, Option<i64>)> = sqlx::query_as(
            "SELECT transaction_index, collected_at_transaction_index
             FROM payment_observations WHERE block_number = 7 ORDER BY transaction_index",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(positions, vec![(0, Some(1)), (2, None)]);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn failure_classification_retries_transient_and_blocks_permanent_causes(pool: PgPool) {
        let paused = make_invoice(100);
        let blacklisted = make_invoice(200);
        let underfunded = make_invoice(300);
        let unknown = make_invoice(400);
        for (invoice, key) in [
            (&paused, "a"),
            (&blacklisted, "b"),
            (&underfunded, "c"),
            (&unknown, "d"),
        ] {
            insert_funded(&pool, invoice, key, 1).await;
        }
        let failed = SweepOutcome::Failed {
            revert_data: Bytes::from_static(&[0x30, 0x11, 0x64, 0x25]),
        };
        let chain = Arc::new(MockChain::new(7).with(|state| {
            for invoice in [&paused, &blacklisted, &underfunded, &unknown] {
                state
                    .next_outcomes
                    .insert(payment_address(invoice), failed.clone());
            }
            state.probes.insert(
                payment_address(&paused),
                FailureProbe {
                    paused: Some(true),
                    ..FailureProbe::default()
                },
            );
            state.probes.insert(
                payment_address(&blacklisted),
                FailureProbe {
                    paused: Some(false),
                    receiver_blacklisted: Some(true),
                    ..FailureProbe::default()
                },
            );
            state.probes.insert(
                payment_address(&underfunded),
                FailureProbe {
                    paused: Some(false),
                    balance: Some(U256::from(299)),
                    ..FailureProbe::default()
                },
            );
        }));
        let worker = indexer(&pool, chain);
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();

        let paused_row = fetch(&pool, &paused).await;
        assert_eq!(paused_row.status, "deploying");
        assert_eq!(paused_row.sweep_attempts, 1);
        assert_eq!(paused_row.blocked_reason, None);

        let blacklisted_row = fetch(&pool, &blacklisted).await;
        assert_eq!(blacklisted_row.status, "blocked");
        assert_eq!(
            blacklisted_row.blocked_reason.as_deref(),
            Some("beneficiary_blacklisted")
        );

        let underfunded_row = fetch(&pool, &underfunded).await;
        assert_eq!(underfunded_row.status, "blocked");
        assert_eq!(
            underfunded_row.blocked_reason.as_deref(),
            Some("balance_below_amount")
        );

        let unknown_row = fetch(&pool, &unknown).await;
        assert_eq!(unknown_row.status, "deploying");
        assert_eq!(unknown_row.sweep_attempts, 1);
        assert_eq!(
            unknown_row.uncollected_count, 1,
            "funds are still at the address"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn overpaid_live_invoice_with_blacklisted_recovery_is_blocked(pool: PgPool) {
        let overpaid = make_invoice(100);
        let exact = make_invoice(200);
        insert_funded(&pool, &overpaid, "key-1", 1).await;
        insert_funded(&pool, &exact, "key-2", 1).await;
        let failed = SweepOutcome::Failed {
            revert_data: Bytes::from_static(&[0x30, 0x11, 0x64, 0x25]),
        };
        let chain = Arc::new(MockChain::new(7).with(|state| {
            for (invoice, balance) in [(&overpaid, 110), (&exact, 200)] {
                state
                    .next_outcomes
                    .insert(payment_address(invoice), failed.clone());
                state.probes.insert(
                    payment_address(invoice),
                    FailureProbe {
                        code_present: false,
                        recovery_blacklisted: Some(true),
                        balance: Some(U256::from(balance)),
                        ..FailureProbe::default()
                    },
                );
            }
        }));
        let worker = indexer(&pool, chain);
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();

        // A live deployment forwards the remainder to the recovery wallet in
        // the same transaction, so the restriction alone explains the failure.
        let overpaid_row = fetch(&pool, &overpaid).await;
        assert_eq!(overpaid_row.status, "blocked");
        assert_eq!(
            overpaid_row.blocked_reason.as_deref(),
            Some("recovery_blacklisted")
        );

        // An exact balance never touches the recovery wallet, so its failure
        // has some other, possibly transient, cause.
        let exact_row = fetch(&pool, &exact).await;
        assert_eq!(exact_row.status, "deploying");
        assert_eq!(exact_row.blocked_reason, None);
        assert_eq!(exact_row.sweep_attempts, 1);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn unknown_failures_block_once_retries_are_exhausted(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7));
        let worker = indexer(&pool, chain.clone());
        let fail = |state: &mut MockState| {
            state.next_outcomes.insert(
                payment_address(&invoice),
                SweepOutcome::Failed {
                    revert_data: Bytes::new(),
                },
            );
        };

        for attempt in 1..=3 {
            chain.set(fail);
            worker.sweep_tick().await.unwrap();
            worker.sweep_tick().await.unwrap();
            let row = fetch(&pool, &invoice).await;
            assert_eq!(row.sweep_attempts, attempt);
            if attempt < 3 {
                assert_eq!(row.status, "deploying");
            }
        }
        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "blocked");
        assert_eq!(row.blocked_reason.as_deref(), Some("retries_exhausted"));
        worker.sweep_tick().await.unwrap();
        assert_eq!(
            chain.submissions().len(),
            3,
            "blocked invoices leave the queue"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn failed_late_collection_keeps_the_terminal_status_but_leaves_the_queue(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7));
        let worker = indexer(&pool, chain.clone());
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();

        chain.set(|state| {
            state.latest = 9;
            state.finalized = 9;
            state.mine_at = Some(9);
            state
                .transfers
                .push(transfer(payment_address(&invoice), 30, 8, 0));
            state.next_outcomes.insert(
                payment_address(&invoice),
                SweepOutcome::Failed {
                    revert_data: Bytes::new(),
                },
            );
            state.probes.insert(
                payment_address(&invoice),
                FailureProbe {
                    code_present: true,
                    recovery_blacklisted: Some(true),
                    ..FailureProbe::default()
                },
            );
        });
        worker.tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();

        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "fulfilled");
        assert_eq!(row.blocked_reason.as_deref(), Some("recovery_blacklisted"));
        assert_eq!(row.uncollected_count, 1);
        worker.sweep_tick().await.unwrap();
        assert_eq!(
            chain.submissions().len(),
            2,
            "a blocked reason removes the invoice from the queue"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn unconfirmed_batch_is_replaced_with_bumped_fees_on_the_same_nonce(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7).with(|state| state.mine_at = None));
        let worker = indexer(&pool, chain.clone());
        worker.sweep_tick().await.unwrap();

        worker.sweep_tick().await.unwrap();
        assert_eq!(
            chain.submissions().len(),
            1,
            "nothing happens before the timeout"
        );

        set_submitted_at_in_the_past(&pool).await;
        worker.sweep_tick().await.unwrap();
        let submissions = chain.submissions();
        assert_eq!(submissions.len(), 2);
        assert_eq!(submissions[1].nonce, submissions[0].nonce);
        assert_eq!(submissions[1].sweeps, submissions[0].sweeps);
        assert_eq!(submissions[1].fees, submissions[0].fees.bumped());
        let hashes: Vec<Vec<u8>> = sqlx::query_scalar("SELECT tx_hashes FROM sweep_batches")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(hashes.len(), 2);

        // The replacement mines; the batch resolves through its newer hash.
        chain.set(|state| {
            let tx_hash = state.submissions[1].tx_hash;
            state.mined_nonce = 1;
            state.receipts.insert(
                tx_hash,
                SweepReceipt {
                    succeeded: true,
                    block: 7,
                    block_hash: block_hash(7),
                    transaction_index: 1,
                    outcomes: HashMap::from([(
                        payment_address(&invoice),
                        SweepOutcome::Settled {
                            amount: U256::from(100),
                            recovered_amount: U256::ZERO,
                        },
                    )]),
                },
            );
        });
        worker.sweep_tick().await.unwrap();
        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "fulfilled");
        assert_eq!(
            row.execute_tx_hash.as_deref(),
            Some(submissions[1].tx_hash.as_slice())
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn original_submission_mining_after_a_replacement_still_resolves_the_batch(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7).with(|state| state.mine_at = None));
        let worker = indexer(&pool, chain.clone());
        worker.sweep_tick().await.unwrap();
        set_submitted_at_in_the_past(&pool).await;
        worker.sweep_tick().await.unwrap();

        chain.set(|state| {
            let tx_hash = state.submissions[0].tx_hash;
            state.mined_nonce = 1;
            state.receipts.insert(
                tx_hash,
                SweepReceipt {
                    succeeded: true,
                    block: 7,
                    block_hash: block_hash(7),
                    transaction_index: 1,
                    outcomes: HashMap::from([(
                        payment_address(&invoice),
                        SweepOutcome::Settled {
                            amount: U256::from(100),
                            recovered_amount: U256::ZERO,
                        },
                    )]),
                },
            );
        });
        worker.sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "fulfilled");
        assert_eq!(open_batches(&pool).await, 0);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn batch_whose_nonce_was_consumed_elsewhere_is_abandoned_and_requeued(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7).with(|state| state.mine_at = None));
        let worker = indexer(&pool, chain.clone());
        worker.sweep_tick().await.unwrap();

        set_submitted_at_in_the_past(&pool).await;
        chain.set(|state| state.mined_nonce = 1);
        worker.sweep_tick().await.unwrap();
        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "deploying");
        assert_eq!(row.sweep_batch_id, None);
        assert_eq!(row.sweep_attempts, 1);
        let resolution: String = sqlx::query_scalar("SELECT resolution FROM sweep_batches")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(resolution, "abandoned");

        // The next batch uses the next nonce and lets the contract decide.
        chain.set(|state| state.mine_at = Some(8));
        worker.sweep_tick().await.unwrap();
        assert_eq!(chain.submissions()[1].nonce, 1);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn exhausted_replacements_pause_the_sweep_worker_without_touching_indexing(pool: PgPool) {
        let invoice = make_invoice(100);
        let later = make_invoice(200);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        insert(&pool, &later, "key-2").await;
        let chain = Arc::new(MockChain::new(7).with(|state| state.mine_at = None));
        let worker = indexer(&pool, chain.clone());
        worker.sweep_tick().await.unwrap();
        for _ in 0..2 {
            set_submitted_at_in_the_past(&pool).await;
            worker.sweep_tick().await.unwrap();
        }
        assert_eq!(chain.submissions().len(), 3);

        set_submitted_at_in_the_past(&pool).await;
        let error = worker.sweep_tick().await.unwrap_err();
        assert!(matches!(error, IndexerError::SweepStalled(_)));
        assert!(error.requires_halt());
        assert_eq!(chain.submissions().len(), 3, "no further replacements");

        // Indexing keeps going while the sweep worker is paused.
        chain.set(|state| {
            state.latest = 8;
            state.finalized = 8;
            state
                .transfers
                .push(transfer(payment_address(&later), 200, 8, 0));
        });
        worker.tick().await.unwrap();
        assert_eq!(fetch(&pool, &later).await.status, "funded");

        // ...and resumes the moment the stuck transaction is found mined.
        chain.set(|state| {
            let tx_hash = state.submissions[2].tx_hash;
            state.mined_nonce = 1;
            state.receipts.insert(
                tx_hash,
                SweepReceipt {
                    succeeded: true,
                    block: 8,
                    block_hash: block_hash(8),
                    transaction_index: 1,
                    outcomes: HashMap::from([(
                        payment_address(&invoice),
                        SweepOutcome::Settled {
                            amount: U256::from(100),
                            recovered_amount: U256::ZERO,
                        },
                    )]),
                },
            );
        });
        worker.sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "fulfilled");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn run_keeps_indexing_while_the_sweep_worker_is_paused(pool: PgPool) {
        let stuck = make_invoice(100);
        let later = make_invoice(200);
        insert_funded(&pool, &stuck, "key-1", 1).await;
        insert(&pool, &later, "key-2").await;
        let chain = Arc::new(MockChain::new(7).with(|state| state.mine_at = None));
        let mut cfg = config();
        cfg.sweep_max_submissions = 1;
        cfg.sweep_pending_timeout = Duration::ZERO;
        let worker = indexer_with(&pool, chain.clone(), cfg);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(worker.run(shutdown_rx));

        tokio::time::timeout(Duration::from_secs(2), async {
            while chain.submissions().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the sweep worker must submit the stuck batch");
        chain.set(|state| {
            state.latest = 8;
            state.finalized = 8;
            state
                .transfers
                .push(transfer(payment_address(&later), 200, 8, 0));
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if fetch(&pool, &later).await.status == "funded" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("indexing must continue while the sweep worker is paused");

        shutdown_tx.send(true).unwrap();
        task.await.unwrap().unwrap();
        assert_eq!(chain.submissions().len(), 1);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn failed_broadcast_keeps_the_signed_transaction_durable_for_retry(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7).with(|state| {
            state.submit_error = Some(|| ChainError::Transient("mock transient failure".into()))
        }));
        let worker = indexer(&pool, chain.clone());

        let error = worker.sweep_tick().await.unwrap_err();
        assert!(matches!(
            error,
            IndexerError::Chain(ChainError::Transient(_))
        ));
        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "deploying");
        assert_eq!(row.sweep_attempts, 0);
        assert!(row.sweep_batch_id.is_some());
        assert_eq!(open_batches(&pool).await, 1);
        let (raw_count, broadcast_at): (i32, Option<sqlx::types::chrono::DateTime<Utc>>) =
            sqlx::query_as("SELECT cardinality(raw_transactions), broadcast_at FROM sweep_batches")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(raw_count, 1);
        assert_eq!(broadcast_at, None);

        // The next pass broadcasts the exact durable bytes; only a subsequent
        // pass reconciles their receipt.
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "fulfilled");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_second_batch_is_never_submitted_while_one_is_in_flight(pool: PgPool) {
        let pending = make_invoice(100);
        let waiting = make_invoice(200);
        insert_funded(&pool, &pending, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7).with(|state| state.mine_at = None));
        let worker = indexer(&pool, chain.clone());
        worker.sweep_tick().await.unwrap();
        insert_funded(&pool, &waiting, "key-2", 2).await;

        worker.sweep_tick().await.unwrap();
        assert_eq!(chain.submissions().len(), 1);
        assert_eq!(fetch(&pool, &waiting).await.status, "funded");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn claimed_invoices_left_by_a_crash_are_reclaimed(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let repo = InvoiceRepository::new(pool.clone());
        // Claimed by a worker that died before recording its submission.
        let claimed = repo
            .claim_sweep_batch(CHAIN_ID, 20, 0.0, 0.0)
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].status, "deploying");

        let chain = Arc::new(MockChain::new(7));
        let worker = indexer(&pool, chain.clone());
        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "fulfilled");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn tampered_parameters_are_blocked_before_any_transaction(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        // The database refuses to change issuance fields; this test deliberately
        // bypasses that guard to prove the indexer still refuses to sweep a row
        // whose parameters no longer derive its payment address.
        sqlx::query("ALTER TABLE invoices DISABLE TRIGGER invoice_issuance_immutable")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE invoices SET beneficiary_address = $2 WHERE id = $1")
            .bind(invoice.id.0)
            .bind([9u8; 20].as_slice())
            .execute(&pool)
            .await
            .unwrap();
        let chain = Arc::new(MockChain::new(7));
        let worker = indexer(&pool, chain.clone());

        worker.sweep_tick().await.unwrap();
        assert!(chain.submissions().is_empty());
        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "blocked");
        assert_eq!(row.blocked_reason.as_deref(), Some("parameters_mismatch"));
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn sweep_ignores_invoices_that_are_not_ready(pool: PgPool) {
        let created = make_invoice(100);
        let partial = make_invoice(100);
        let other_chain = issue(OTHER_CHAIN, 100, FAR_EXPIRY);
        insert(&pool, &created, "a").await;
        insert(&pool, &partial, "b").await;
        insert(&pool, &other_chain, "c").await;
        sqlx::query("UPDATE invoices SET status = 'funded', uncollected_count = 1 WHERE id = $1")
            .bind(other_chain.id.0)
            .execute(&pool)
            .await
            .unwrap();
        let chain =
            Arc::new(MockChain::new(2).with(|state| {
                state.transfers = vec![transfer(payment_address(&partial), 40, 1, 0)]
            }));
        let worker = indexer(&pool, chain.clone());
        worker.tick().await.unwrap();

        worker.sweep_tick().await.unwrap();
        assert!(chain.submissions().is_empty());
        assert_eq!(fetch(&pool, &created).await.status, "created");
        assert_eq!(fetch(&pool, &partial).await.status, "created");
        assert_eq!(fetch(&pool, &other_chain).await.status, "funded");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_full_batch_settles_together_and_the_rest_waits(pool: PgPool) {
        let invoices: Vec<Invoice> = (1..=(SWEEP_BATCH_LIMIT as u64 + 1))
            .map(|i| make_invoice(i * 100))
            .collect();
        let chain = Arc::new(MockChain::new(7));
        for (index, invoice) in invoices.iter().enumerate() {
            insert(&pool, invoice, &format!("key-{index}")).await;
            chain.set(|state| {
                state.transfers.push(transfer(
                    payment_address(invoice),
                    invoice.amount.0.to::<u64>(),
                    1,
                    index as u64,
                ))
            });
        }
        let worker = indexer(&pool, chain.clone());
        worker.tick().await.unwrap();

        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        let submissions = chain.submissions();
        assert_eq!(submissions.len(), 1);
        assert_eq!(submissions[0].sweeps.len(), SWEEP_BATCH_LIMIT as usize);
        assert_eq!(
            submissions[0].gas_limit,
            sweep_batch_gas_limit(SWEEP_BATCH_LIMIT as usize)
        );
        let fulfilled: i64 =
            sqlx::query_scalar("SELECT count(*) FROM invoices WHERE status = 'fulfilled'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(fulfilled, SWEEP_BATCH_LIMIT);
        assert_eq!(
            fetch(&pool, invoices.last().unwrap()).await.status,
            "funded"
        );

        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        assert_eq!(
            fetch(&pool, invoices.last().unwrap()).await.status,
            "fulfilled"
        );
    }
}
