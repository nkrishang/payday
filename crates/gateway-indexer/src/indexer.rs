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
//! The sweep worker signs with a pool of keys: every signer keeps at most
//! one helper transaction in flight, the pool is walked in rotation, and a
//! withdrawal relay step and a sweep batch can run side by side on different
//! signers. A fatal block-indexer error ends the process (finalized history
//! no longer matches; the operator must look). A fatal sweep condition only
//! pauses the sweep worker, which keeps reporting it every tick until it
//! clears, so payment detection never stops because a transaction is stuck.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alloy_primitives::{Address, U256};
use chrono::Duration as ChronoDuration;
use futures::future::join_all;
use gateway_core::{
    CctpConfig, ChainId, ChainRegistry, FinalitySource, Invoice, InvoiceStatus, PaymentBinding,
};
use gateway_db::{
    BatchResolution, CursorRepository, DbInvoice, DbWithdrawalLeg, IndexerCursor, InvoiceOutcome,
    InvoiceRepository, MinedBatch, PaymentObservation, RecoveredFundsInput, RecoveryReason,
    SweepBatch, WatchFingerprint, WithdrawalRepository,
};
use sqlx::types::chrono::Utc;
use sqlx::types::Uuid;
use thiserror::Error;
use tokio::sync::{Notify, OnceCell, watch};
use tokio::task::JoinSet;
use tokio::time::{Instant, MissedTickBehavior};
use tracing::{debug, error, info, warn};

use crate::chain::{
    BlockHeader, BroadcastOutcome, ChainClient, ChainError, FeeEstimate,
    PreparedSweepTransaction, SettlementEvent, SweepOutcome, SweepReceipt, SweepRequest,
    TransactionOutcome, sweep_batch_gas_limit,
};
use crate::iris::AttestationSource;
use crate::relay::MIN_AUTHORIZATION_VALIDITY;
use crate::signal::{SignalState, WatchList};
use gateway_db::RelayIntentRepository;
use gateway_relay::RelayApi;

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
pub(crate) const RANGE_RETRY_BACKOFF: Duration = Duration::from_millis(500);
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

/// Whether a lane's row is still in flight after this pass's work on it, and
/// what a broadcast performed during the lane's own work taught the
/// coordinator about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaneOutcome {
    /// The row stays open; its next observation runs on the receipt-poll
    /// cadence.
    Open,
    /// The lane broadcast during its work (a first submission's bytes, a
    /// replay, or a replacement): the flow says what the wire taught us, and
    /// the coordinator schedules the row's next observation from it exactly
    /// as it would for a fresh submission.
    Broadcast(BroadcastFlow),
    /// The row left the worker's hands (settled, requeued, or cancelled).
    Resolved,
}

/// One pool signer's native balance against the alarm level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SignerHealth {
    pub(crate) signer: Address,
    pub(crate) balance: U256,
    pub(crate) low: bool,
}

/// What one lane job reports when it finishes. Every variant names its signer
/// so the coordinator can free exactly that lane.
enum LaneDone {
    /// An already-durable row was reconciled against the chain.
    Reconciled {
        signer: Address,
        outcome: Result<LaneOutcome, IndexerError>,
    },
    /// A claimed set of invoices was submitted (or definitively failed before
    /// persistence). `reserved` lists the claimed ids the durable state does
    /// not own: they were all blocked, or the failure happened before the
    /// batch existed and the claim was released.
    Batch {
        signer: Address,
        reserved: Vec<Uuid>,
        outcome: Result<BatchSubmission, IndexerError>,
    },
    /// A withdrawal leg's step was submitted (or the leg needed nothing).
    Step {
        signer: Address,
        leg_id: Uuid,
        outcome: Result<StepSubmission, IndexerError>,
    },
}

/// The durable result of a batch submission.
enum BatchSubmission {
    /// The batch row now owns its invoices; `flow` says what the broadcast
    /// taught us about the transaction.
    Submitted { flow: BroadcastFlow },
    /// Every claimed row was blocked before signing: no batch exists, and the
    /// queue may hold more work behind the blocked rows.
    Blocked,
}

/// The durable result of a step submission.
pub(crate) enum StepSubmission {
    Submitted { flow: BroadcastFlow },
    /// The leg left the queue, was already minted by another party, or was
    /// cancelled between the read and the write.
    NotNeeded,
}

/// What the wire said about a durable transaction's broadcast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BroadcastFlow {
    /// Accepted (or already known): the receipt is a poll's business.
    Sent,
    /// The synchronous send returned the transaction's outcome.
    Included(TransactionOutcome),
    /// Neither accepted nor rejected: the node may still have the bytes.
    Unknown,
}

/// Chain reads shared by every lane job of one dispatch round: one fee
/// estimate and one finality-boundary read, however many lanes consume them.
/// A failed read is cached too, so a struggling RPC costs one round trip per
/// round rather than one per lane.
#[derive(Default)]
pub(crate) struct SweepReads {
    pub(crate) fees: OnceCell<Result<FeeEstimate, ChainError>>,
    pub(crate) boundary: OnceCell<Result<BlockHeader, ChainError>>,
}

/// How a signer's last lane failure classifies for the health report: a
/// degraded condition an operator should look at, or one that requires the
/// worker to pause. The failure itself lives in the tick report's log; this
/// is the retained classification that keeps the report honest until the
/// lane actually recovers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LaneHealth {
    Degraded,
    Paused,
}

/// The sweep coordinator's state across dispatches: its lane jobs, the
/// signers those jobs occupy, the claims made before their ownership became
/// durable, each signer's next observation deadline, the recovery cadence's
/// absolute due time, and the health each signer's last lane failure left.
struct Coordinator {
    jobs: JoinSet<LaneDone>,
    /// Signers with a running job. A signer owns at most one in-flight
    /// transaction, so at most one job runs on it.
    running: HashSet<Address>,
    /// When each deferred signer's next chain observation (receipt poll,
    /// rebroadcast, retry) may run. A deadline is consumed the moment a
    /// dispatch acts on it, so only signers waiting for the future appear
    /// here; `running` covers the jobs in flight.
    observe_at: HashMap<Address, Instant>,
    /// When the next recovery dispatch is due: the backstop cadence for lost
    /// pokes, API-side eligibility changes, and backoff expiries. Advanced
    /// only when a wake services it, so notifications and completions can
    /// never postpone a retry.
    recovery_at: Instant,
    /// Claimed invoice ids whose durable batch does not exist yet. Claims are
    /// made sequentially in dispatch, so this set is what keeps a concurrent
    /// claim from re-grabbing another job's rows while they sign.
    reserved: HashSet<Uuid>,
    /// Same, for withdrawal legs under submission.
    reserved_legs: HashSet<Uuid>,
    /// A configuration error paused the worker; only a dispatch tick clears it.
    halted: bool,
    /// Each signer's last lane failure, retained until that lane completes
    /// work that confirms recovery. A pass that dispatches successfully
    /// without the failing lane must not report the worker healthy again.
    lane_health: HashMap<Address, LaneHealth>,
}

impl Coordinator {
    fn new(recovery_at: Instant) -> Self {
        Self {
            jobs: JoinSet::new(),
            running: HashSet::new(),
            observe_at: HashMap::new(),
            recovery_at,
            reserved: HashSet::new(),
            reserved_legs: HashSet::new(),
            halted: false,
            lane_health: HashMap::new(),
        }
    }

    /// When the coordinator should next dispatch on the clock: the earliest
    /// observation deadline among deferred signers, else the recovery
    /// cadence.
    fn next_dispatch(&self) -> Instant {
        self.observe_at
            .values()
            .copied()
            .min()
            .unwrap_or(self.recovery_at)
            .min(self.recovery_at)
    }

    /// Service the recovery clock on a wake: if the wake is (also) the
    /// recovery tick coming due, schedule the next one. Wakes that only
    /// deliver an observation deadline or a poke leave it alone.
    fn service_recovery(&mut self, now: Instant, poll_interval: Duration) {
        if self.recovery_at <= now {
            self.recovery_at = now + poll_interval;
        }
    }

    /// Whether a signer may take work right now: no job of its own is
    /// running, and it is not deferring an observation to the future.
    fn signer_free(&self, signer: Address, now: Instant) -> bool {
        !self.running.contains(&signer)
            && self.observe_at.get(&signer).is_none_or(|at| *at <= now)
    }

    /// A lane finished work that confirms its signer has recovered.
    fn note_lane_recovered(&mut self, signer: Address) {
        self.lane_health.remove(&signer);
    }

    /// A lane failed: retain the failure's classification so the worker's
    /// health stays honest until this lane recovers, however many
    /// successful dispatches for other signers happen meanwhile.
    fn note_lane_failure(&mut self, signer: Address, error: &IndexerError) {
        let health = if error.requires_halt() {
            LaneHealth::Paused
        } else {
            LaneHealth::Degraded
        };
        self.lane_health.insert(signer, health);
    }

    /// The worst health any lane's unrecovered failure implies.
    fn worst_lane_health(&self) -> Option<LaneHealth> {
        self.lane_health.values().copied().max()
    }

    /// The reserved invoice ids, in a stable order, for the claim query.
    fn reserved_list(&self) -> Vec<Uuid> {
        let mut ids: Vec<Uuid> = self.reserved.iter().copied().collect();
        ids.sort();
        ids
    }

    /// The reserved leg ids, in a stable order, for the selection query.
    fn reserved_legs_list(&self) -> Vec<Uuid> {
        let mut ids: Vec<Uuid> = self.reserved_legs.iter().copied().collect();
        ids.sort();
        ids
    }
}

/// Every error one sweep pass produced, by lane (`None` is chain-wide work).
/// A pass never stops at the first failing lane: the others still reconcile
/// and submit, and the report collapses to its most severe error for the
/// loop's health state.
#[derive(Debug, Default)]
pub(crate) struct TickReport {
    pub(crate) errors: Vec<(Option<Address>, IndexerError)>,
}

impl TickReport {
    fn push(&mut self, signer: Option<Address>, error: IndexerError) {
        self.errors.push((signer, error));
    }

    /// The first error that halts the worker, else the first error at all.
    pub(crate) fn into_result(self) -> Result<(), IndexerError> {
        let mut errors = self.errors.into_iter().map(|(_, error)| error);
        let Some(first) = errors.next() else {
            return Ok(());
        };
        if first.requires_halt() {
            return Err(first);
        }
        Err(errors.find(IndexerError::requires_halt).unwrap_or(first))
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
    /// Active observation cadence for broadcast helper transactions: the
    /// recovery timer stays the backstop, but a settled sweep should not wait
    /// for it to be noticed.
    pub sweep_receipt_poll_interval: Duration,
    pub sweep_max_submissions: u32,
    pub sweep_max_attempts: u32,
    pub sweep_backoff_base_secs: f64,
    pub sweep_backoff_cap_secs: f64,
    pub signer_low_balance_wei: U256,
    /// CCTP on this chain, for withdrawal legs that bridge from or to it.
    pub cctp: Option<CctpConfig>,
    /// How often a burn from this chain is checked for Circle's attestation:
    /// seconds on a finality-tagged chain, a minute on an L2 whose burns
    /// take ~15 minutes to be attested.
    pub attestation_poll: Duration,
}

/// The payment indexer. Owns its own repository handles and chain client; it is
/// not shared with the HTTP handlers.
pub struct Indexer {
    pub(crate) repo: InvoiceRepository,
    cursor: CursorRepository,
    pub(crate) chain: Arc<dyn ChainClient>,
    pub(crate) cfg: IndexerConfig,
    /// The withdrawal legs this chain's signers relay; see `relay.rs`.
    pub(crate) withdrawals: WithdrawalRepository,
    pub(crate) iris: Arc<dyn AttestationSource>,
    /// Every chain, for the CCTP domain a bridge leg's destination has.
    pub(crate) registry: Arc<ChainRegistry>,
    /// Cross-chain payments into this chain's addresses; see
    /// `relay_intents.rs`. `None` when the deployment has no Relay key.
    pub(crate) relay: Option<Arc<dyn RelayApi>>,
    pub(crate) relay_intents: RelayIntentRepository,
    /// Every chain's client, by chain id, for reading a cross-chain
    /// payment's origin transaction when its chain is one we serve.
    pub(crate) peers: Arc<HashMap<u64, Arc<dyn ChainClient>>>,
    current_log_range_size: AtomicU64,
    /// Wake-ups and health from the transfer signal; see `signal.rs`.
    signal: Arc<SignalState>,
    /// Pokes the sweep coordinator once a pass's observations have committed:
    /// new funded invoices, expiries, recoveries. Notifications are hints the
    /// durable claim query still verifies; the recovery timer covers a lost
    /// poke. Deliberately not the transfer signal's own notify, whose single
    /// permit belongs to the indexing loop and precedes the DB commit.
    sweep_wake: Notify,
    /// Where the next assignment starts walking the signer pool, so
    /// consecutive submissions rotate through the signers and gas spend
    /// spreads evenly. Lives on the indexer, not the coordinator, so the
    /// rotation survives across rounds (each test round builds a fresh
    /// coordinator) — a signer that just failed to sign is not offered the
    /// queue's head again.
    sweep_cursor: AtomicUsize,
    /// The recipient list the signal subscribes to, replaced on change.
    watch_tx: watch::Sender<WatchList>,
    watch_fingerprint: Mutex<Option<WatchFingerprint>>,
}

impl Indexer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repo: InvoiceRepository,
        cursor: CursorRepository,
        chain: Arc<dyn ChainClient>,
        cfg: IndexerConfig,
        iris: Arc<dyn AttestationSource>,
        registry: Arc<ChainRegistry>,
        relay: Option<Arc<dyn RelayApi>>,
        peers: Arc<HashMap<u64, Arc<dyn ChainClient>>>,
    ) -> Self {
        let (watch_tx, _) = watch::channel(WatchList::default());
        let withdrawals = WithdrawalRepository::new(repo.pool().clone());
        let relay_intents = RelayIntentRepository::new(repo.pool().clone());
        Self {
            repo,
            cursor,
            chain,
            withdrawals,
            iris,
            registry,
            relay,
            relay_intents,
            peers,
            current_log_range_size: AtomicU64::new(cfg.log_range_size),
            signal: SignalState::new(),
            sweep_wake: Notify::new(),
            sweep_cursor: AtomicUsize::new(0),
            watch_tx,
            watch_fingerprint: Mutex::new(None),
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

    /// Run every loop until `shutdown` is set to `true`. A fatal block-indexer
    /// error is returned so the process supervisor cannot mistake a halted
    /// payment worker for a healthy one; the sweep loop never fails the process.
    pub async fn run(self: Arc<Self>, shutdown: watch::Receiver<bool>) -> Result<(), IndexerError> {
        let worker = self;
        let index_loop = Arc::clone(&worker).run_index_loop(shutdown.clone());
        let sweep_loop = Arc::clone(&worker).run_sweep_loop(shutdown.clone());
        let relay_loop = Arc::clone(&worker).run_relay_intent_loop(shutdown.clone());
        // Nonce-free withdrawal housekeeping (expiries, Circle attestations,
        // whose requests can block for seconds) and sweep health telemetry
        // run on their own clocks: neither may sit in front of a sweep in
        // the coordinator's turn.
        let housekeeping_loop = Arc::clone(&worker).run_relay_housekeeping_loop();
        let health_loop = Arc::clone(&worker).run_sweep_health_loop();
        tokio::pin!(index_loop, sweep_loop, relay_loop, housekeeping_loop, health_loop);

        tokio::select! {
            result = &mut index_loop => result,
            () = &mut sweep_loop => Ok(()),
            () = &mut relay_loop => Ok(()),
            () = &mut housekeeping_loop => Ok(()),
            () = &mut health_loop => Ok(()),
        }
    }

    /// Follow cross-chain payments on this chain, on a clock of its own:
    /// Relay's answers can take seconds each, and a payment poller's slowness
    /// must never delay a sweep. The loop runs even without a Relay key, so
    /// database cleanup (expiring quotes, failing stale intents, unparking)
    /// keeps going after the API is gone.
    async fn run_relay_intent_loop(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        let mut interval = tokio::time::interval(self.cfg.poll_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        info!("relay intent worker started");

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    // One pass at a time; every error stays inside, the way
                    // the sweep loop treats its own recoverable failures.
                    self.relay_intent_housekeeping().await;
                }
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        info!("relay intent worker shutting down");
                        break;
                    }
                }
            }
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
        info!(batch_sweeper = %self.cfg.batch_sweeper, "sweep worker started");
        let mut coordinator = Coordinator::new(Instant::now() + self.cfg.poll_interval);
        // One dispatch clock for the whole worker: an observation deadline
        // while helper transactions are in flight (the latency path), else
        // the recovery cadence, which is the backstop for lost pokes,
        // API-side eligibility changes, and backoff expiries — never the
        // latency path.
        let wake = tokio::time::sleep_until(coordinator.next_dispatch());
        tokio::pin!(wake);
        // The sweeper status last written. Health is persisted when it
        // changes, or whenever a dispatch ran (the heartbeat cadence), so a
        // recovery is visible on the very completion that produced it.
        let mut last_health: Option<&'static str> = None;

        loop {
            let mut report = TickReport::default();
            let mut dispatched = false;
            tokio::select! {
                _ = &mut wake => {
                    coordinator.halted = false;
                    coordinator.service_recovery(Instant::now(), self.cfg.poll_interval);
                    dispatched = true;
                }
                _ = self.sweep_wake.notified() => {
                    if !coordinator.halted {
                        dispatched = true;
                    }
                }
                done = coordinator.jobs.join_next(), if !coordinator.jobs.is_empty() => {
                    // The set cannot be empty between the guard and here.
                    if let Some(done) = done.map(|done| done.expect("a lane job does not panic")) {
                        // A completion that leaves its signer runnable
                        // dispatches at once, so a synchronously-sent
                        // transaction is reconciled in the round that
                        // submitted it; a deferred, unacknowledged, or
                        // failed lane waits for the clock it installed.
                        let runnable = self
                            .handle_lane_done(&mut coordinator, done, &mut report)
                            .await;
                        if runnable && !coordinator.halted {
                            dispatched = true;
                        }
                    }
                }
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        info!("sweep worker shutting down");
                        // Jobs die with the coordinator: nothing signed is
                        // broadcast before its durable commit, so an
                        // interrupted job leaves either a claim that expires
                        // by backoff or a durable row the next start finds.
                        break;
                    }
                    continue;
                }
            }
            if dispatched {
                self.dispatch(&mut coordinator, &mut report, Instant::now())
                    .await;
            }
            // Health from this pass's errors plus every lane failure not yet
            // recovered: a successful dispatch for other signers does not
            // report a failing lane healthy again, and a failing lane does
            // not stay degraded after work that confirms its recovery.
            let state = match report.into_result() {
                // Logged every pass so the alarm stays raised until the
                // condition clears; the clock's retry re-evaluates it.
                Err(error) if error.requires_halt() => {
                    coordinator.halted = true;
                    error!(error = %error, "sweep worker paused");
                    "paused"
                }
                // Logged every pass for the same reason.
                Err(error) => {
                    warn!(error = %error, "sweep pass failed; retrying next pass");
                    "degraded"
                }
                Ok(()) if coordinator.halted => "paused",
                Ok(()) => match coordinator.worst_lane_health() {
                    Some(LaneHealth::Paused) => "paused",
                    Some(LaneHealth::Degraded) => "degraded",
                    None => "running",
                },
            };
            if last_health != Some(state) || dispatched {
                self.persist_sweeper_health(state).await;
                last_health = Some(state);
            }
            // Every scheduling-state change re-arms the clock: completions
            // install or clear observation deadlines, and the wake serviced
            // the recovery tick.
            wake.as_mut().reset(coordinator.next_dispatch());
        }
    }

    /// Withdrawal housekeeping on a clock of its own: expiry and Circle's
    /// attestation polling are nonce-free and can each block for seconds (the
    /// attestation service's timeout), so they must never delay a sweep lane.
    async fn run_relay_housekeeping_loop(self: Arc<Self>) {
        let mut interval = tokio::time::interval(self.cfg.poll_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(error) = self.relay_housekeeping().await {
                        warn!(error = %error, "withdrawal housekeeping failed; retrying next interval");
                    }
                }
                // No shutdown branch: dropped with the other loops when
                // `run`'s select finishes.
                _ = tokio::time::sleep(Duration::MAX) => unreachable!(),
            }
        }
    }

    /// Sweep health telemetry on a clock of its own: one `eth_getBalance` per
    /// signer, at most every `SWEEP_HEALTH_INTERVAL`.
    async fn run_sweep_health_loop(self: Arc<Self>) {
        let mut interval = tokio::time::interval(SWEEP_HEALTH_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(error) = self.report_sweep_health().await {
                        warn!(error = %error, "sweep health report failed");
                    }
                }
                _ = tokio::time::sleep(Duration::MAX) => unreachable!(),
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


    /// A `block_header` read that backs off and retries throttle-shaped
    /// failures instead of aborting the work it belongs to.
    pub(crate) async fn retried_header(
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

            // The pass verified the cursor before its first range, but a reorg
            // may have replaced it since. Re-read that block only after the
            // range-end header above: both reads are then taken from the same
            // branch, so if the cursor block no longer carries the hash the
            // cursor committed, this chain's history has diverged underneath a
            // committed cursor and must halt, not silently adopt the new
            // branch.
            if let Some(cursor) = cursor {
                let previous = self
                    .retried_header(cursor.block, &mut attempt, &mut backoff)
                    .await?;
                if previous.hash != cursor.block_hash {
                    return Err(ChainError::FinalityViolation(format!(
                        "cursor block {} changed while scanning from it; refusing to continue",
                        cursor.block
                    ))
                    .into());
                }
            }
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
            // Any committed observation can have changed sweep eligibility:
            // funding is the obvious case, but an expiration makes an
            // invoice recoverable, and a late transfer can give a fulfilled
            // or recovered invoice uncollected funds. The claim query
            // re-checks eligibility, so a wake that finds nothing costs one
            // dispatch; missing one costs a recovery cadence of latency.
            // Coalesced: one permit dispatches, and the dispatch reaps the
            // whole queue.
            if !outcome.funded.is_empty()
                || !outcome.expired.is_empty()
                || !observations.is_empty()
            {
                self.sweep_wake.notify_one();
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

    /// One full round, collapsed to its most severe error: the tests' entry
    /// point. The production loop drives [`Self::dispatch`] directly.
    #[cfg(test)]
    pub(crate) async fn sweep_tick(self: Arc<Self>) -> Result<(), IndexerError> {
        // One full round on the same machinery the production loop runs:
        // dispatch, then handle completions as they finish, dispatching again
        // so a resolved lane's signer takes its next piece of work at once.
        // Deferrals are respected, never cleared: an open lane (an unmined
        // helper transaction, a failed submission) stays busy for the rest of
        // the round, as it did when a pass reconciled before it submitted —
        // re-running it immediately would spin, and the production loop is
        // what spaces observations on the clock.
        // The production loop runs withdrawal housekeeping (expiry and the
        // attestation poller) on a clock of its own; a test round has no such
        // loop, so the round runs it where the old pass did — before any
        // lane — exactly as the base pass did.
        let mut report = TickReport::default();
        if let Err(error) = self.relay_housekeeping().await {
            report.push(None, error);
            return report.into_result();
        }
        // The pass's clock freezes for the whole round: every deferral
        // `handle_lane_done` makes outlives it, so a signer whose observation
        // was deferred mid-round cannot sneak back in while the round's later
        // dispatches run, however fast the database answers.
        let now = Instant::now();
        let mut coordinator = Coordinator::new(now);
        self.dispatch(&mut coordinator, &mut report, now).await;
        while !coordinator.jobs.is_empty() {
            let done = coordinator
                .jobs
                .join_next()
                .await
                .expect("the job set is not empty")
                .expect("a lane job does not panic");
            // The helper dispatches after every completion regardless of
            // runnability; the production loop is what gates a deferred
            // signer's dispatch on the clock.
            let _runnable = self.handle_lane_done(&mut coordinator, done, &mut report).await;
            self.dispatch(&mut coordinator, &mut report, now).await;
        }
        report.into_result()
    }

    /// One dispatch: start an observation job for every open row whose signer
    /// is free, then walk the pool in rotation handing every free signer its
    /// next piece of work — a withdrawal leg to relay first, else a batch of
    /// sweeps. Claims are made sequentially here (so a short queue becomes
    /// one full batch rather than one batch per signer; Monad bills the gas
    /// limit) but signing, persisting and broadcasting run concurrently in
    /// each lane's own job, and no dispatch ever awaits a lane. `now` decides
    /// which deferred signers are free; the production loop passes a fresh
    /// instant per dispatch, while a test round freezes one instant for the
    /// whole round so a signer deferred mid-round stays busy until it ends.
    async fn dispatch(
        self: &Arc<Self>,
        coordinator: &mut Coordinator,
        report: &mut TickReport,
        now: Instant,
    ) {
        // Consume every due observation deadline up front, before any
        // fallible read: a deadline whose dispatch finds no work — the queue
        // emptied, the rows are under database backoff, the read failed —
        // must not linger in the past and spin the loop at database speed.
        // Completion handlers install any later deferral.
        coordinator.observe_at.retain(|_, at| *at > now);
        let chain_id = self.cfg.chain_id.0;
        let pool = self.chain.signers();
        let (batches, steps) = match tokio::try_join!(
            self.repo.open_sweep_batches(chain_id),
            self.withdrawals.open_steps(chain_id)
        ) {
            Ok(open) => open,
            Err(error) => {
                report.push(None, error.into());
                return;
            }
        };
        if let Err(error) = lane_signers(&pool, &batches, &steps) {
            report.push(None, error);
            coordinator.halted = true;
            return;
        }
        // One fee estimate and one finality boundary per round, taken lazily
        // by the first lane that needs either.
        let reads = Arc::new(SweepReads::default());
        for batch in batches {
            if !coordinator.signer_free(batch.signer, now) {
                continue;
            }
            coordinator.running.insert(batch.signer);
            coordinator.jobs.spawn(self.clone().observe_batch(batch, Arc::clone(&reads)));
        }
        for leg in steps {
            let Some(signer) = leg.step.as_ref().map(|step| step.signer) else {
                // lane_signers validated every open step has a signer.
                continue;
            };
            if !coordinator.signer_free(signer, now) {
                continue;
            }
            coordinator.running.insert(signer);
            coordinator.jobs.spawn(self.clone().observe_step(leg, Arc::clone(&reads)));
        }

        let start = self.sweep_cursor.load(Ordering::Relaxed) % pool.len();
        for offset in 0..pool.len() {
            let index = (start + offset) % pool.len();
            let signer = pool[index];
            if !coordinator.signer_free(signer, now) {
                continue;
            }
            // A withdrawal leg to relay has priority over a new sweep batch,
            // as it did when one signer did both.
            match self
                .withdrawals
                .next_relayable(chain_id, MIN_AUTHORIZATION_VALIDITY, &coordinator.reserved_legs_list())
                .await
            {
                Ok(Some(leg)) => {
                    coordinator.reserved_legs.insert(leg.id);
                    coordinator.running.insert(signer);
                    self.sweep_cursor.store(index + 1, Ordering::Relaxed);
                    coordinator
                        .jobs
                        .spawn(self.clone().submit_step_on(signer, leg, Arc::clone(&reads)));
                    // This signer now owns the leg's transaction; the next
                    // free signer in the rotation looks at the queue.
                    continue;
                }
                Ok(None) => {}
                Err(error) => {
                    report.push(Some(signer), error.into());
                    continue;
                }
            }
            match self
                .repo
                .claim_sweep_batch(
                    chain_id,
                    SWEEP_BATCH_LIMIT,
                    self.cfg.sweep_backoff_base_secs,
                    self.cfg.sweep_backoff_cap_secs,
                    &coordinator.reserved_list(),
                )
                .await
            {
                Ok(claimed) if claimed.is_empty() => break,
                Ok(claimed) => {
                    let ids: Vec<Uuid> = claimed.iter().map(|row| row.id).collect();
                    coordinator.reserved.extend(ids.iter().copied());
                    coordinator.running.insert(signer);
                    self.sweep_cursor.store(index + 1, Ordering::Relaxed);
                    coordinator
                        .jobs
                        .spawn(self.clone().submit_batch_on(signer, claimed, ids, Arc::clone(&reads)));
                }
                Err(error) => {
                    report.push(Some(signer), error.into());
                }
            }
        }
    }

    /// Fold a finished lane back into the coordinator: free or re-defer its
    /// signer, drain its reservations, update the signer's retained health,
    /// and schedule the next observation. Returns whether the signer is
    /// runnable right now — the production loop dispatches immediately on a
    /// runnable completion instead of waiting out the clock.
    async fn handle_lane_done(
        &self,
        coordinator: &mut Coordinator,
        done: LaneDone,
        report: &mut TickReport,
    ) -> bool {
        match done {
            LaneDone::Reconciled { signer, outcome } => {
                coordinator.running.remove(&signer);
                match outcome {
                    Ok(LaneOutcome::Open) => {
                        coordinator.note_lane_recovered(signer);
                        coordinator.observe_at.insert(
                            signer,
                            Instant::now() + self.cfg.sweep_receipt_poll_interval,
                        );
                        false
                    }
                    Ok(LaneOutcome::Broadcast(flow)) => {
                        self.schedule_flow(coordinator, signer, flow)
                    }
                    Ok(LaneOutcome::Resolved) => {
                        coordinator.note_lane_recovered(signer);
                        true
                    }
                    Err(error) => {
                        warn!(signer = %signer, error = %error, "sweep lane failed");
                        // The row's state is unknown; keep it off the chain
                        // until the recovery cadence, and keep the failure in
                        // the worker's health until the lane recovers.
                        coordinator.note_lane_failure(signer, &error);
                        coordinator
                            .observe_at
                            .insert(signer, Instant::now() + self.cfg.poll_interval);
                        report.push(Some(signer), error);
                        false
                    }
                }
            }
            LaneDone::Batch {
                signer,
                reserved,
                outcome,
            } => {
                coordinator.running.remove(&signer);
                for id in &reserved {
                    coordinator.reserved.remove(id);
                }
                match outcome {
                    Ok(BatchSubmission::Submitted { flow }) => {
                        self.schedule_flow(coordinator, signer, flow)
                    }
                    // The signer is free now; a runnable completion reaps the
                    // queue behind the blocked rows.
                    Ok(BatchSubmission::Blocked) => {
                        coordinator.note_lane_recovered(signer);
                        true
                    }
                    Err(error) => {
                        warn!(signer = %signer, error = %error, "sweep submission failed");
                        // The submission's rows may or may not be durable;
                        // either way this signer can sign again on the
                        // recovery cadence, and the failure is reported.
                        coordinator.note_lane_failure(signer, &error);
                        coordinator
                            .observe_at
                            .insert(signer, Instant::now() + self.cfg.poll_interval);
                        report.push(Some(signer), error);
                        false
                    }
                }
            }
            LaneDone::Step {
                signer,
                leg_id,
                outcome,
            } => {
                coordinator.running.remove(&signer);
                coordinator.reserved_legs.remove(&leg_id);
                match outcome {
                    Ok(StepSubmission::Submitted { flow }) => {
                        self.schedule_flow(coordinator, signer, flow)
                    }
                    Ok(StepSubmission::NotNeeded) => {
                        coordinator.note_lane_recovered(signer);
                        true
                    }
                    Err(error) => {
                        warn!(signer = %signer, leg_id = %leg_id, error = %error, "withdrawal step submission failed");
                        // The step's rows may or may not be durable; either
                        // way this signer can sign again on the recovery
                        // cadence, and the failure is reported.
                        coordinator.note_lane_failure(signer, &error);
                        coordinator
                            .observe_at
                            .insert(signer, Instant::now() + self.cfg.poll_interval);
                        report.push(Some(signer), error);
                        false
                    }
                }
            }
        }
    }

    /// Schedule a signer's next observation from what its broadcast taught
    /// the coordinator, and say whether the signer is runnable right now: an
    /// included receipt rides the next dispatch, which a runnable completion
    /// triggers immediately; an acknowledged broadcast waits for a receipt
    /// poll; an unknown outcome re-observes no faster than the recovery
    /// cadence, and its row's receipt is checked first.
    fn schedule_flow(
        &self,
        coordinator: &mut Coordinator,
        signer: Address,
        flow: BroadcastFlow,
    ) -> bool {
        match flow {
            BroadcastFlow::Included(outcome) => {
                debug!(signer = %signer, block = outcome.block, succeeded = outcome.succeeded, "receipt came with the broadcast; settling on the next dispatch");
                coordinator.note_lane_recovered(signer);
                coordinator.observe_at.remove(&signer);
                true
            }
            BroadcastFlow::Sent => {
                coordinator.note_lane_recovered(signer);
                coordinator.observe_at.insert(
                    signer,
                    Instant::now() + self.cfg.sweep_receipt_poll_interval,
                );
                false
            }
            BroadcastFlow::Unknown => {
                // Not a confirmed recovery: keep any retained failure.
                coordinator
                    .observe_at
                    .insert(signer, Instant::now() + self.cfg.poll_interval);
                false
            }
        }
    }

    /// The pass's shared fee estimate, read from the node once. A failed read
    /// is cached like a success, so concurrent lanes do not each retry a
    /// struggling RPC; the next dispatch's fresh `SweepReads` retries it.
    pub(crate) async fn tick_fees(
        &self,
        cache: &OnceCell<Result<FeeEstimate, ChainError>>,
    ) -> Result<FeeEstimate, IndexerError> {
        cache
            .get_or_init(|| self.chain.estimate_fees())
            .await
            .clone()
            .map_err(IndexerError::from)
    }

    /// The round's shared finality boundary, read once no matter how many
    /// lanes ask, failures cached like successes. A single attempt: the
    /// dispatch clock retries after a failure, so the range reader's
    /// backoff-and-retry loop has no place on the sweep path.
    pub(crate) async fn shared_boundary(&self, reads: &SweepReads) -> Result<BlockHeader, ChainError> {
        reads
            .boundary
            .get_or_init(|| self.read_sweep_boundary())
            .await
            .clone()
    }

    /// One attempt at the finality boundary, with the same semantics as
    /// [`Self::boundary_header`]: the `finalized` tag (less any confirmation
    /// margin) or `latest` minus the margin.
    async fn read_sweep_boundary(&self) -> Result<BlockHeader, ChainError> {
        let margin = self.cfg.finality_confirmations;
        match self.cfg.finality_source {
            FinalitySource::Finalized => {
                let finalized = self.chain.finalized_header().await?;
                if margin == 0 {
                    return Ok(finalized);
                }
                self.chain
                    .block_header(finalized.number.saturating_sub(margin))
                    .await
            }
            FinalitySource::Latest => {
                let latest = self.chain.latest_block_number().await?;
                self.chain.block_header(latest.saturating_sub(margin)).await
            }
        }
    }

    /// A lane job: reconcile one open batch against the chain, or put its
    /// durable-but-unacknowledged transaction on the wire. Never fails the
    /// coordinator; errors travel inside the report.
    async fn observe_batch(self: Arc<Self>, batch: SweepBatch, reads: Arc<SweepReads>) -> LaneDone {
        let signer = batch.signer;
        let outcome = self.advance_batch(batch, reads).await;
        LaneDone::Reconciled {
            signer,
            outcome,
        }
    }

    /// A lane job: reconcile one open withdrawal step. Never fails the
    /// coordinator; errors travel inside the report.
    async fn observe_step(self: Arc<Self>, leg: DbWithdrawalLeg, reads: Arc<SweepReads>) -> LaneDone {
        let signer = leg
            .step
            .as_ref()
            .map(|step| step.signer)
            .expect("an open step has a signer");
        let outcome = self.advance_step(leg, reads).await;
        LaneDone::Reconciled { signer, outcome }
    }

    /// A lane job: validate, sign, durably persist and broadcast a claimed
    /// batch of sweeps. `reserved` travels back so the coordinator can drain
    /// its reservation of these ids whichever way the job ends.
    async fn submit_batch_on(
        self: Arc<Self>,
        signer: Address,
        claimed: Vec<DbInvoice>,
        reserved: Vec<Uuid>,
        reads: Arc<SweepReads>,
    ) -> LaneDone {
        let outcome = self.prepare_and_submit_batch(signer, claimed, &reads).await;
        LaneDone::Batch {
            signer,
            reserved,
            outcome,
        }
    }

    /// A lane job: sign, durably persist and broadcast one withdrawal leg's
    /// step.
    async fn submit_step_on(
        self: Arc<Self>,
        signer: Address,
        leg: DbWithdrawalLeg,
        reads: Arc<SweepReads>,
    ) -> LaneDone {
        let leg_id = leg.id;
        let outcome = self.prepare_and_submit_step(signer, leg, &reads).await;
        LaneDone::Step {
            signer,
            leg_id,
            outcome,
        }
    }

    /// Advance one open batch: reconcile it against the chain, or put its
    /// durable-but-unacknowledged bytes on the wire — looking for its receipt
    /// first, because the broadcast's acknowledgment may have been lost while
    /// the transaction landed anyway.
    async fn advance_batch(
        &self,
        batch: SweepBatch,
        reads: Arc<SweepReads>,
    ) -> Result<LaneOutcome, IndexerError> {
        if batch.broadcast_at.is_none() {
            for &tx_hash in batch.tx_hashes.iter().rev() {
                if let Some(receipt) = self
                    .chain
                    .sweep_receipt(tx_hash, self.cfg.batch_sweeper)
                    .await?
                {
                    info!(batch_id = %batch.id, %tx_hash, "unacknowledged helper transaction has a receipt; reconciling it instead of broadcasting again");
                    return self.apply_receipt(&batch, tx_hash, receipt, &reads).await;
                }
            }
            // Re-sending the exact bytes is safe, but not forever: past the
            // pending timeout the replacement and stalled machinery takes
            // over, so a persistently rejected or invisible transaction ends
            // up reported instead of retried at the recovery cadence
            // indefinitely.
            let age = Utc::now()
                .signed_duration_since(batch.submitted_at)
                .to_std()
                .unwrap_or_default();
            if age >= self.cfg.sweep_pending_timeout {
                warn!(batch_id = %batch.id, signer = %batch.signer, "unacknowledged helper transaction is past the pending timeout without a receipt or an acknowledgment; handing it to the replacement machinery");
                return self.handle_unmined(&batch, &reads).await;
            }
            let flow = self.broadcast_batch(&batch).await?;
            // An included receipt rides the next dispatch: a runnable
            // completion observes a synchronously-sent transaction
            // immediately, without waiting out a receipt poll.
            return Ok(LaneOutcome::Broadcast(flow));
        }
        self.reconcile_batch(batch, reads).await
    }

    async fn broadcast_prepared(
        &self,
        batch_id: uuid::Uuid,
        transaction: &PreparedSweepTransaction,
    ) -> Result<BroadcastFlow, IndexerError> {
        match self.chain.broadcast_sweep_transaction(transaction).await {
            // The acknowledgment is recorded before anything else learns the
            // broadcast happened: a crash between the two leaves an
            // unacknowledged durable transaction, which the next observation
            // reconciles or re-sends by exact bytes.
            Ok(BroadcastOutcome::Accepted) => {
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
                Ok(BroadcastFlow::Sent)
            }
            Ok(BroadcastOutcome::Included(outcome)) => {
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
                Ok(BroadcastFlow::Included(outcome))
            }
            Err(ChainError::BroadcastUnknown(reason)) => {
                // Neither accepted nor rejected: leave the acknowledgment
                // unset and let observation decide. The next look at this row
                // checks the receipt before re-sending the identical bytes.
                warn!(batch_id = %batch_id, tx_hash = %transaction.hash, %reason, "broadcast outcome is unknown; reconciling instead of re-signing");
                Ok(BroadcastFlow::Unknown)
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn broadcast_batch(
        &self,
        batch: &SweepBatch,
    ) -> Result<BroadcastFlow, IndexerError> {
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
        let flow = self.broadcast_prepared(batch.id, &transaction).await?;
        info!(batch_id = %batch.id, tx_hash = %transaction.hash, signer = %batch.signer, nonce = batch.nonce, "durable helper transaction broadcast");
        Ok(flow)
    }

    async fn reconcile_batch(
        &self,
        batch: SweepBatch,
        reads: Arc<SweepReads>,
    ) -> Result<LaneOutcome, IndexerError> {
        // A replacement and the submission it replaced share a nonce; whichever
        // mined resolves the batch. Newest first: it is the likeliest.
        for &tx_hash in batch.tx_hashes.iter().rev() {
            if let Some(receipt) = self
                .chain
                .sweep_receipt(tx_hash, self.cfg.batch_sweeper)
                .await?
            {
                return self.apply_receipt(&batch, tx_hash, receipt, &reads).await;
            }
        }
        self.handle_unmined(&batch, &reads).await
    }

    async fn apply_receipt(
        &self,
        batch: &SweepBatch,
        tx_hash: alloy_primitives::B256,
        receipt: SweepReceipt,
        reads: &SweepReads,
    ) -> Result<LaneOutcome, IndexerError> {
        let mined = MinedBatch {
            tx_hash,
            block: receipt.block,
            block_hash: receipt.block_hash,
            transaction_index: receipt.transaction_index,
        };
        if batch.mined != Some(mined) {
            self.repo.record_batch_mined(batch.id, mined).await?;
        }
        if receipt.block > self.shared_boundary(reads).await?.number {
            return Ok(LaneOutcome::Open);
        }
        let mut attempt = 0u32;
        let mut backoff = RANGE_RETRY_BACKOFF;
        let header = self
            .retried_header(receipt.block, &mut attempt, &mut backoff)
            .await?;
        if header.hash != receipt.block_hash {
            self.repo.clear_batch_mined(batch.id).await?;
            warn!(batch_id = %batch.id, %tx_hash, block = receipt.block, "helper transaction receipt is no longer canonical; waiting for a new receipt");
            return Ok(LaneOutcome::Open);
        }

        if !receipt.succeeded {
            if self.chain.block_header(receipt.block).await?.hash != receipt.block_hash {
                self.repo.clear_batch_mined(batch.id).await?;
                return Ok(LaneOutcome::Open);
            }
            let requeued = self
                .repo
                .resolve_batch(batch.id, BatchResolution::Reverted)
                .await?;
            warn!(batch_id = %batch.id, %tx_hash, requeued, "finalized helper transaction reverted; invoices returned to the queue");
            return Ok(LaneOutcome::Resolved);
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
            return Ok(LaneOutcome::Open);
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
        Ok(LaneOutcome::Resolved)
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
                let (settlement, settlement_timestamp) = if invoice.status.is_terminal()
                    && row.settlement_tx_hash.is_some()
                {
                    // The settlement was ledgered when the invoice resolved:
                    // `finalize_batch` only backfills these fields behind
                    // COALESCE, and this branch discards the discovered
                    // recovery amount for a terminal invoice anyway. Searching
                    // logs for them would reach from the first observation to
                    // now — arbitrarily far past any provider's range cap —
                    // so the stored settlement stands. A legacy row without a
                    // stored hash still goes through the discovery below
                    // rather than mislabeling this drain as the settlement.
                    let settlement = SettlementEvent {
                        transaction_hash: tx_hash,
                        block_number: header.number,
                        block_hash: header.hash,
                        settled: None,
                        recovered: U256::ZERO,
                    };
                    (settlement, header.timestamp)
                } else {
                    // A third party deployed the contract, and its settlement
                    // can sit anywhere between the first observation and now:
                    // search in provider-sized chunks, never one unbounded
                    // `eth_getLogs`. A range the provider rejects splits in
                    // half, exactly like the scan ranges do.
                    let mut found: Option<SettlementEvent> = None;
                    let mut start = from_block;
                    while found.is_none() && start <= header.number {
                        let chunk = self.current_log_range_size.load(Ordering::Relaxed).max(1);
                        let end = start.saturating_add(chunk - 1).min(header.number);
                        found = match self
                            .chain
                            .payment_settlement_tx(payment, self.cfg.usdc, start, end)
                            .await
                        {
                            Ok(found) => found,
                            Err(ChainError::LogRangeTooLarge(message)) if chunk > 1 => {
                                let smaller = (chunk / 2).max(1);
                                self.current_log_range_size
                                    .store(smaller, Ordering::Relaxed);
                                warn!(
                                    from_block = start,
                                    to_block = end,
                                    smaller,
                                    error = %message,
                                    "provider rejected settlement log range; splitting it"
                                );
                                continue;
                            }
                            Err(error) => return Err(error.into()),
                        };
                        start = end.saturating_add(1);
                    }
                    let settlement = found.ok_or_else(|| {
                        ChainError::Transient(format!(
                            "payment {payment} is deployed but has no finalized settlement event in blocks {from_block}..={}",
                            header.number
                        ))
                    })?;
                    let settlement_header =
                        self.chain.block_header(settlement.block_number).await?;
                    if settlement_header.hash != settlement.block_hash {
                        return Err(ChainError::FinalityViolation(format!(
                            "settlement event block {} changed during classification",
                            settlement.block_number
                        ))
                        .into());
                    }
                    (settlement, settlement_header.timestamp)
                };
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
                    settlement_timestamp,
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
    async fn handle_unmined(
        &self,
        batch: &SweepBatch,
        reads: &SweepReads,
    ) -> Result<LaneOutcome, IndexerError> {
        let age = Utc::now()
            .signed_duration_since(batch.submitted_at)
            .to_std()
            .unwrap_or_default();
        if age < self.cfg.sweep_pending_timeout {
            return Ok(LaneOutcome::Open);
        }

        // Monad reports no receipt for a transaction still in flight and forgets
        // one it dropped, so after the timeout the mined nonce is the signal: if
        // it moved past ours without a receipt, nothing we sent can mine any more.
        if self.chain.signer_nonce(batch.signer, false).await? > batch.nonce {
            let requeued = self
                .repo
                .resolve_batch(batch.id, BatchResolution::Abandoned)
                .await?;
            warn!(batch_id = %batch.id, signer = %batch.signer, nonce = batch.nonce, requeued, "sweep batch nonce was consumed without a visible receipt; invoices returned to the queue");
            return Ok(LaneOutcome::Resolved);
        }

        if batch.tx_hashes.len() as u32 >= self.cfg.sweep_max_submissions {
            return Err(IndexerError::SweepStalled(format!(
                "sweep batch {} (signer {}, nonce {}) is unconfirmed after {} submissions; check the signer balance and fee market, then replace or cancel nonce {} manually",
                batch.id,
                batch.signer,
                batch.nonce,
                batch.tx_hashes.len(),
                batch.nonce
            )));
        }

        let previous = FeeEstimate {
            max_fee_per_gas: batch.max_fee_per_gas,
            max_priority_fee_per_gas: batch.max_priority_fee_per_gas,
        };
        let fees = self.tick_fees(&reads.fees).await?.max(previous.bumped());
        let rows = self.repo.batch_invoices(batch.id).await?;
        let requests = rows
            .iter()
            .map(|row| decode(row).and_then(|invoice| sweep_request(&invoice)))
            .collect::<Result<Vec<_>, _>>()?;
        let transaction = self
            .chain
            .prepare_sweep_batch(
                batch.signer,
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
        let flow = self.broadcast_prepared(batch.id, &transaction).await?;
        warn!(batch_id = %batch.id, signer = %batch.signer, nonce = batch.nonce, tx_hash = %transaction.hash, submission = batch.tx_hashes.len() + 1, max_fee_per_gas = fees.max_fee_per_gas, "replaced unconfirmed helper transaction");
        // The replacement's broadcast outcome schedules the observation like
        // a first submission's: an included receipt is settled on the next
        // dispatch, which a runnable completion runs at once.
        Ok(LaneOutcome::Broadcast(flow))
    }

    /// Sign and broadcast a claimed batch of sweeps from `signer`. The claim
    /// was made in dispatch; everything from the nonce read onward runs in
    /// this lane's own job, concurrently with the other lanes'.
    async fn prepare_and_submit_batch(
        &self,
        signer: Address,
        claimed: Vec<DbInvoice>,
        reads: &SweepReads,
    ) -> Result<BatchSubmission, IndexerError> {
        let started = Instant::now();
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
            return Ok(BatchSubmission::Blocked);
        }

        // The nonce and the fee estimate are independent reads; they wait in
        // parallel, and signing waits for both.
        let (nonce, fees) = match tokio::try_join!(
            async { Ok(self.chain.signer_nonce(signer, true).await?) },
            self.tick_fees(&reads.fees),
        ) {
            Ok(reads_done) => reads_done,
            // Definite failure before anything durable: the claim is released
            // so the rows' backoff applies and they return to the queue.
            Err(error) => {
                self.repo.release_claim(&ids).await?;
                return Err(error);
            }
        };
        let gas_limit = sweep_batch_gas_limit(requests.len());
        let transaction = match self
            .chain
            .prepare_sweep_batch(
                signer,
                self.cfg.batch_sweeper,
                &requests,
                nonce,
                gas_limit,
                fees,
            )
            .await
        {
            Ok(transaction) => transaction,
            Err(error) => {
                self.repo.release_claim(&ids).await?;
                return Err(error.into());
            }
        };
        let signed_at = started.elapsed();
        let batch_id = self
            .repo
            .record_batch_submission(
                self.cfg.chain_id.0,
                signer,
                &ids,
                nonce,
                gas_limit,
                fees.max_fee_per_gas,
                fees.max_priority_fee_per_gas,
                transaction.hash,
                &transaction.raw,
            )
            .await?;
        let durable_at = started.elapsed();
        // Past this point the durable batch owns the invoices: no failure
        // releases their claim, and an unknown broadcast outcome is
        // observation's business, never a re-sign.
        let flow = self.broadcast_prepared(batch_id, &transaction).await?;
        info!(
            %batch_id, tx_hash = %transaction.hash, %signer, nonce, invoices = ids.len(), gas_limit,
            prepare_ms = signed_at.as_millis() as u64,
            durable_ms = (durable_at - signed_at).as_millis() as u64,
            broadcast_ms = (started.elapsed() - durable_at).as_millis() as u64,
            "helper transaction submitted"
        );
        Ok(BatchSubmission::Submitted { flow })
    }

    /// Queue depth, relay state, and every pool signer's balance: one
    /// `eth_getBalance` per signer, at most every `SWEEP_HEALTH_INTERVAL`.
    /// A balance read that fails is logged and costs only its own signer's
    /// entry; the pass proceeds on the rest.
    pub(crate) async fn report_sweep_health(&self) -> Result<Vec<SignerHealth>, IndexerError> {
        let stats = self.repo.sweep_queue_stats(self.cfg.chain_id.0).await?;
        let relay = self.withdrawals.relay_stats(self.cfg.chain_id.0).await?;
        let relay_intents_pending = self
            .relay_intents
            .pending_count(self.cfg.chain_id.0)
            .await?;
        let pool = self.chain.signers();
        let balances = join_all(pool.iter().map(|&signer| self.chain.signer_balance(signer))).await;
        let mut health = Vec::with_capacity(pool.len());
        for (signer, balance) in pool.into_iter().zip(balances) {
            match balance {
                Ok(balance) => health.push(SignerHealth {
                    signer,
                    balance,
                    low: balance < self.cfg.signer_low_balance_wei,
                }),
                Err(error) => {
                    error!(%signer, %error, "failed to read sweep signer balance");
                }
            }
        }
        let balances = health
            .iter()
            .map(|entry| format!("{}={}", entry.signer, entry.balance))
            .collect::<Vec<_>>()
            .join(",");
        info!(
            queued = stats.queued,
            in_flight = stats.in_flight,
            oldest_uncollected_secs = stats.oldest_uncollected_secs,
            withdrawal_legs_authorized = relay.authorized,
            withdrawal_legs_awaiting_attestation = relay.awaiting_attestation,
            withdrawal_legs_attested = relay.attested,
            withdrawal_step_in_flight = relay.in_flight,
            relay_intents_pending,
            signers = health.len(),
            signer_balances_wei = %balances,
            "sweep worker health"
        );
        for entry in health.iter().filter(|entry| entry.low) {
            warn!(signer = %entry.signer, signer_balance_wei = %entry.balance, threshold_wei = %self.cfg.signer_low_balance_wei, "sweep signer balance low");
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
        Ok(health)
    }
}

/// Every open row's signer must be in the pool, and no signer may own more
/// than one open helper transaction (the database keeps batches and steps
/// unique per signer separately; a signer with one of each is a bug).
fn lane_signers(
    pool: &[Address],
    batches: &[SweepBatch],
    steps: &[DbWithdrawalLeg],
) -> Result<(), IndexerError> {
    if pool.is_empty() {
        return Err(IndexerError::Configuration(
            "the signer pool is empty".to_string(),
        ));
    }
    let mut seen = HashSet::new();
    let owned = batches
        .iter()
        .map(|batch| (Some(batch.signer), format!("sweep batch {}", batch.id)))
        .chain(steps.iter().map(|leg| {
            (
                leg.step.as_ref().map(|step| step.signer),
                format!("withdrawal leg {}", leg.id),
            )
        }));
    for (signer, row) in owned {
        let Some(signer) = signer else {
            return Err(IndexerError::Configuration(format!(
                "open {row} has no relay step"
            )));
        };
        if !pool.contains(&signer) {
            return Err(IndexerError::Configuration(format!(
                "open {row} was signed by {signer}, which is not in the signer pool; restore its key or resolve the row by hand"
            )));
        }
        if !seen.insert(signer) {
            return Err(IndexerError::Configuration(format!(
                "signer {signer} owns more than one open helper transaction, including {row}"
            )));
        }
    }
    Ok(())
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

    use std::collections::{HashMap, HashSet};
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

    pub(crate) const CHAIN_ID: u64 = 31337;
    /// Block timestamps in the mock advance ten seconds per block from here.
    const GENESIS_TIMESTAMP: u64 = 1_800_000_000;
    /// Comfortably after every block the tests mine.
    const FAR_EXPIRY: u64 = GENESIS_TIMESTAMP + 1_000_000;

    pub(crate) fn usdc() -> Address {
        address!("0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512")
    }

    fn factory() -> FactoryAddress {
        FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3"))
    }

    pub(crate) fn batch_sweeper() -> Address {
        address!("0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0")
    }

    /// The k-th signer of the mock's pool; the default pool is `[mock_signer(0)]`.
    pub(crate) fn mock_signer(index: u8) -> Address {
        Address::repeat_byte(0xA0 + index)
    }

    pub(crate) fn block_hash(block: u64) -> B256 {
        B256::with_last_byte(block as u8)
    }

    fn block_timestamp(block: u64) -> u64 {
        GENESIS_TIMESTAMP + block * 10
    }

    /// The transaction hash the mock reports for a consumed authorization:
    /// derived from the nonce, so tests can predict it.
    pub(crate) fn mock_authorization_tx_hash(nonce: B256) -> B256 {
        let mut hash = nonce;
        hash[31] = hash[31].wrapping_add(1);
        hash
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
    pub(crate) struct Submission {
        /// The pool signer the transaction was signed from.
        pub(crate) signer: Address,
        pub(crate) nonce: u64,
        pub(crate) gas_limit: u64,
        pub(crate) fees: FeeEstimate,
        pub(crate) sweeps: Vec<SweepRequest>,
        pub(crate) tx_hash: B256,
        /// Set for generic calls (withdrawal steps); sweeps leave it empty.
        pub(crate) to: Option<Address>,
        pub(crate) calldata: alloy_primitives::Bytes,
    }

    /// Circle's attestation service as a test controls it: answers by burn
    /// transaction hash, `NotIndexed` for anything else.
    #[derive(Default)]
    pub(crate) struct FakeIris {
        pub(crate) answers: Mutex<HashMap<B256, crate::iris::AttestationStatus>>,
        pub(crate) polls: Mutex<Vec<(u32, B256)>>,
    }

    #[async_trait]
    impl crate::iris::AttestationSource for FakeIris {
        async fn attestation(
            &self,
            source_domain: u32,
            burn_tx_hash: B256,
        ) -> Result<crate::iris::AttestationStatus, crate::iris::IrisError> {
            self.polls
                .lock()
                .unwrap()
                .push((source_domain, burn_tx_hash));
            Ok(self
                .answers
                .lock()
                .unwrap()
                .get(&burn_tx_hash)
                .cloned()
                .unwrap_or(crate::iris::AttestationStatus::NotIndexed))
        }
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
        pub(crate) receipts: HashMap<B256, SweepReceipt>,
        /// Outcomes attached to the receipt of the next submission, keyed by
        /// payment address. Items without an entry get `Settled`.
        next_outcomes: HashMap<Address, SweepOutcome>,
        /// Block the next submission's receipt lands in; `None` leaves it unmined.
        pub(crate) mine_at: Option<u64>,
        /// Whether the mock emulates a sync-send chain (`eth_sendRawTransactionSync`):
        /// the broadcast itself carries the receipt. Production enables this per
        /// chain (`PAYDAY_SWEEP_SYNC_SEND_CHAIN_IDS`); a synchronous broadcast
        /// lets the coordinator observe and settle the transaction in the same
        /// round it submitted it. Default `false`: the broadcast is asynchronous
        /// (`Accepted`), and only a later observation sees the receipt.
        pub(crate) sync_send: bool,
        pub(crate) next_receipt_succeeds: bool,
        prepared: HashMap<B256, Submission>,
        next_transaction_id: u8,
        submissions: Vec<Submission>,
        /// The pool, in rotation order. `MockChain::new` seeds one signer.
        pub(crate) signers: Vec<Address>,
        /// Mined transaction count per signer; a missing entry is 0.
        pub(crate) mined_nonces: HashMap<Address, u64>,
        submit_error: Option<fn() -> ChainError>,
        /// Definite broadcast failures still owed per signer: a signer with a
        /// positive count fails its next submissions with a definite RPC
        /// error that many times before succeeding, for health-retention and
        /// unacknowledged-row tests.
        pub(crate) failing_submissions: HashMap<Address, u32>,
        /// `estimate_fees` reads that still fail with a retryable error
        /// before succeeding; see the fee-cache test.
        failing_fee_reads: u32,
        pub(crate) fees: FeeEstimate,
        /// `estimate_fees` calls, for the once-per-pass assertion.
        pub(crate) fee_requests: usize,
        /// Balance overrides per signer; a missing entry is one native token.
        pub(crate) balances: HashMap<Address, U256>,
        /// Signers whose `eth_getBalance` fails, for health isolation tests.
        pub(crate) balance_errors: HashSet<Address>,
        /// Signers whose signing fails, for rotation isolation tests.
        pub(crate) prepare_errors: HashSet<Address>,
        settled: HashMap<Address, bool>,
        settlement_events: HashMap<Address, SettlementEvent>,
        /// Blocks from which each payment's settlement event exists: ranges
        /// ending before the block return no settlement, so classification
        /// has to page through chunks to find it.
        settlement_appears_at: HashMap<Address, u64>,
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
        /// `eth_call` answers by (contract, calldata); anything else is 32 zero bytes.
        pub(crate) view_results:
            HashMap<(Address, alloy_primitives::Bytes), alloy_primitives::Bytes>,
        /// EIP-3009 authorizations the token has consumed, by (authorizer, nonce).
        pub(crate) consumed_authorizations: HashSet<(Address, B256)>,
        /// The block each consumed authorization's `AuthorizationUsed` event
        /// appears in, so ranges ending before it find nothing.
        pub(crate) authorization_events: HashMap<B256, u64>,
        /// Token payment receipts the node answers `token_payment_receipt`
        /// with, by hash: execution plus the `Transfer` logs one token left.
        pub(crate) token_receipts: HashMap<B256, crate::chain::TokenPaymentReceipt>,
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
                    signers: vec![mock_signer(0)],
                    ..MockState::default()
                }),
            }
        }

        /// A pool of `count` signers, `mock_signer(0)..mock_signer(count)`.
        pub(crate) fn with_signers(self, count: u8) -> Self {
            self.with(|state| state.signers = (0..count).map(mock_signer).collect())
        }

        pub(crate) fn with(self, apply: impl FnOnce(&mut MockState)) -> Self {
            apply(&mut self.state.lock().unwrap());
            self
        }

        pub(crate) fn set(&self, apply: impl FnOnce(&mut MockState)) {
            apply(&mut self.state.lock().unwrap());
        }

        pub(crate) fn submissions(&self) -> Vec<Submission> {
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

        fn signers(&self) -> Vec<Address> {
            self.state.lock().unwrap().signers.clone()
        }

        async fn signer_nonce(&self, signer: Address, pending: bool) -> Result<u64, ChainError> {
            let state = self.state.lock().unwrap();
            // Monad semantics: `pending` reads the same as `latest`.
            let _ = pending;
            Ok(state.mined_nonces.get(&signer).copied().unwrap_or(0))
        }

        async fn signer_balance(&self, signer: Address) -> Result<U256, ChainError> {
            if self.state.lock().unwrap().balance_errors.contains(&signer) {
                return Err(ChainError::Transient(format!(
                    "balance read throttled for {signer}"
                )));
            }
            Ok(self
                .state
                .lock()
                .unwrap()
                .balances
                .get(&signer)
                .copied()
                .unwrap_or(U256::from(10u64).pow(U256::from(18u64))))
        }

        async fn estimate_fees(&self) -> Result<FeeEstimate, ChainError> {
            let mut state = self.state.lock().unwrap();
            state.fee_requests += 1;
            if state.failing_fee_reads > 0 {
                state.failing_fee_reads -= 1;
                return Err(ChainError::Transient("fee read throttled".to_string()));
            }
            Ok(state.fees)
        }

        async fn prepare_sweep_batch(
            &self,
            signer: Address,
            _batch_sweeper: Address,
            sweeps: &[SweepRequest],
            nonce: u64,
            gas_limit: u64,
            fees: FeeEstimate,
        ) -> Result<PreparedSweepTransaction, ChainError> {
            let mut state = self.state.lock().unwrap();
            if !state.signers.contains(&signer) {
                return Err(ChainError::Transient(format!("no key for signer {signer}")));
            }
            if state.prepare_errors.contains(&signer) {
                return Err(ChainError::Transient(format!(
                    "signing failed for {signer}"
                )));
            }
            state.next_transaction_id += 1;
            let tx_hash = B256::with_last_byte(state.next_transaction_id);
            let raw = alloy_primitives::Bytes::copy_from_slice(tx_hash.as_slice());
            state.prepared.insert(
                tx_hash,
                Submission {
                    signer,
                    nonce,
                    gas_limit,
                    fees,
                    sweeps: sweeps.to_vec(),
                    tx_hash,
                    to: None,
                    calldata: alloy_primitives::Bytes::new(),
                },
            );
            Ok(PreparedSweepTransaction { hash: tx_hash, raw })
        }

        async fn prepare_call(
            &self,
            signer: Address,
            to: Address,
            calldata: alloy_primitives::Bytes,
            nonce: u64,
            gas_limit: u64,
            fees: FeeEstimate,
        ) -> Result<PreparedSweepTransaction, ChainError> {
            let mut state = self.state.lock().unwrap();
            if !state.signers.contains(&signer) {
                return Err(ChainError::Transient(format!("no key for signer {signer}")));
            }
            if state.prepare_errors.contains(&signer) {
                return Err(ChainError::Transient(format!(
                    "signing failed for {signer}"
                )));
            }
            state.next_transaction_id += 1;
            let tx_hash = B256::with_last_byte(state.next_transaction_id);
            let raw = alloy_primitives::Bytes::copy_from_slice(tx_hash.as_slice());
            state.prepared.insert(
                tx_hash,
                Submission {
                    signer,
                    nonce,
                    gas_limit,
                    fees,
                    sweeps: Vec::new(),
                    tx_hash,
                    to: Some(to),
                    calldata,
                },
            );
            Ok(PreparedSweepTransaction { hash: tx_hash, raw })
        }

        async fn token_payment_receipt(
            &self,
            tx_hash: B256,
            _token: Address,
        ) -> Result<Option<crate::chain::TokenPaymentReceipt>, ChainError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .token_receipts
                .get(&tx_hash)
                .cloned())
        }

        async fn transaction_receipt(
            &self,
            tx_hash: B256,
        ) -> Result<Option<crate::chain::TransactionOutcome>, ChainError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .receipts
                .get(&tx_hash)
                .map(|receipt| crate::chain::TransactionOutcome {
                    succeeded: receipt.succeeded,
                    block: receipt.block,
                    block_hash: receipt.block_hash,
                }))
        }

        /// The mock's world has one final history, so reading through
        /// finality changes nothing.
        async fn finalized_view_call(
            &self,
            to: Address,
            calldata: alloy_primitives::Bytes,
        ) -> Result<alloy_primitives::Bytes, ChainError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .view_results
                .get(&(to, calldata))
                .cloned()
                .unwrap_or_else(|| alloy_primitives::Bytes::from(vec![0u8; 32])))
        }

        async fn authorization_state(
            &self,
            token: Address,
            authorizer: Address,
            nonce: B256,
            at_block: u64,
        ) -> Result<bool, ChainError> {
            let _ = token;
            let _ = at_block;
            Ok(self
                .state
                .lock()
                .unwrap()
                .consumed_authorizations
                .contains(&(authorizer, nonce)))
        }

        async fn authorization_used_tx(
            &self,
            token: Address,
            authorizer: Address,
            nonce: B256,
            from_block: u64,
            to_block: u64,
        ) -> Result<Option<(B256, crate::chain::TransactionOutcome)>, ChainError> {
            let _ = token;
            let _ = authorizer;
            let state = self.state.lock().unwrap();
            let Some(at_block) = state.authorization_events.get(&nonce).copied() else {
                return Ok(None);
            };
            if at_block < from_block || at_block > to_block {
                return Ok(None);
            }
            Ok(Some((
                mock_authorization_tx_hash(nonce),
                crate::chain::TransactionOutcome {
                    succeeded: true,
                    block: at_block,
                    block_hash: B256::with_last_byte(at_block as u8),
                },
            )))
        }

        async fn broadcast_sweep_transaction(
            &self,
            transaction: &PreparedSweepTransaction,
        ) -> Result<BroadcastOutcome, ChainError> {
            let mut state = self.state.lock().unwrap();
            if let Some(error) = state.submit_error.take() {
                return Err(error());
            }
            let submission = state.prepared[&transaction.hash].clone();
            let tx_hash = submission.tx_hash;
            let signer = submission.signer;
            // A definite refusal still owed to this signer: the signed bytes
            // are already durable (the submission was recorded), so this
            // models the node answering and rejecting.
            if state
                .failing_submissions
                .get(&signer)
                .is_some_and(|count| *count > 0)
            {
                *state.failing_submissions.get_mut(&signer).unwrap() -= 1;
                return Err(ChainError::Transient(format!(
                    "signer {signer}'s broadcast was refused"
                )));
            }
            // Re-sending the exact signed bytes is idempotent: the mock
            // acknowledges them without recording a second submission.
            if state
                .submissions
                .iter()
                .any(|submission| submission.tx_hash == transaction.hash)
            {
                return Ok(BroadcastOutcome::Accepted);
            }
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
                state.mined_nonces.insert(signer, nonce + 1);
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
                // The receipt rides the broadcast only on a sync-send chain;
                // an asynchronous broadcast is acknowledged without one, and
                // the next observation finds the receipt by hash.
                if state.sync_send {
                    return Ok(BroadcastOutcome::Included(TransactionOutcome {
                        succeeded,
                        block,
                        block_hash: block_hash(block),
                    }));
                }
            }
            Ok(BroadcastOutcome::Accepted)
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
            _token: Address,
            from_block: u64,
            to_block: u64,
        ) -> Result<Option<SettlementEvent>, ChainError> {
            let state = self.state.lock().unwrap();
            // A settlement the mock says exists only from some block onward is
            // invisible to ranges that end below it, exactly like a real
            // provider answering for a range without the deployment.
            if state
                .settlement_appears_at
                .get(&payment)
                .is_some_and(|appears_at| to_block < *appears_at)
            {
                return Ok(None);
            }
            if let Some(event) = state.settlement_events.get(&payment) {
                return Ok(Some(*event));
            }
            // Without an override the deployment was this worker's own exact
            // settlement, whose amount the mock finds among its submissions.
            let settled = state
                .submissions
                .iter()
                .flat_map(|submission| &submission.sweeps)
                .find(|sweep| payment_address_of(sweep) == payment)
                .map_or(U256::ZERO, |sweep| sweep.amount);
            Ok(Some(SettlementEvent {
                transaction_hash: B256::repeat_byte(0xCC),
                block_number: from_block,
                block_hash: block_hash(from_block),
                settled: Some(settled),
                recovered: U256::ZERO,
            }))
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

    pub(crate) fn config() -> IndexerConfig {
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
            sweep_receipt_poll_interval: Duration::from_millis(10),
            sweep_pending_timeout: Duration::from_secs(60),
            sweep_max_submissions: 3,
            sweep_max_attempts: 3,
            sweep_backoff_base_secs: 0.0,
            sweep_backoff_cap_secs: 0.0,
            signer_low_balance_wei: U256::ZERO,
            cctp: None,
            attestation_poll: Duration::from_millis(10),
        }
    }

    /// The other chain a bridge leg in the relay tests crosses to.
    pub(crate) const OTHER_CHAIN_ID: u64 = 8453;

    /// The registry the test deployment runs: this chain alone without CCTP,
    /// or, once a test configures CCTP, this chain and [`OTHER_CHAIN_ID`]
    /// with CCTP on both (domains 15 and 6, as Monad and Base have).
    pub(crate) fn test_registry(cctp: Option<CctpConfig>) -> Arc<ChainRegistry> {
        let chain = |chain_id: u64, cctp: Option<CctpConfig>| gateway_core::ChainConfig {
            chain_id,
            usdc: usdc(),
            factory: factory().0,
            batch_sweeper: batch_sweeper(),
            factory_code_hash: mock_code_hash(factory().0),
            batch_sweeper_code_hash: mock_code_hash(batch_sweeper()),
            usdc_start_block: 0,
            finality_source: FinalitySource::Finalized,
            finality_confirmations: 0,
            block_time_ms: 25,
            log_range_size: 100,
            signer_low_balance_wei: None,
            explorer_base_url: None,
            cctp,
        };
        let chains = match cctp {
            None => vec![chain(CHAIN_ID, None)],
            Some(cctp) => vec![
                chain(
                    CHAIN_ID,
                    Some(CctpConfig {
                        domain: 15,
                        ..cctp.clone()
                    }),
                ),
                chain(OTHER_CHAIN_ID, Some(CctpConfig { domain: 6, ..cctp })),
            ],
        };
        Arc::new(ChainRegistry::new(chains).unwrap())
    }

    fn indexer_with(pool: &PgPool, chain: Arc<MockChain>, cfg: IndexerConfig) -> Arc<Indexer> {
        indexer_with_relay(pool, chain, cfg, Arc::new(FakeIris::default()))
    }

    pub(crate) fn indexer_with_relay(
        pool: &PgPool,
        chain: Arc<MockChain>,
        cfg: IndexerConfig,
        iris: Arc<FakeIris>,
    ) -> Arc<Indexer> {
        let registry = test_registry(cfg.cctp.clone());
        Arc::new(Indexer::new(
            InvoiceRepository::new(pool.clone()),
            CursorRepository::new(pool.clone()),
            chain,
            cfg,
            iris,
            registry,
            None,
            Arc::new(HashMap::new()),
        ))
    }

    fn indexer(pool: &PgPool, chain: Arc<MockChain>) -> Arc<Indexer> {
        indexer_with(pool, chain, config())
    }

    /// Build a USDC invoice for the test chain with the given atomic amount.
    pub(crate) fn make_invoice(amount: u64) -> Invoice {
        make_invoice_expiring(amount, FAR_EXPIRY)
    }

    fn make_invoice_expiring(amount: u64, expiration: u64) -> Invoice {
        issue(ChainId(CHAIN_ID), amount, expiration)
    }

    /// The wallet every test payer attests and pays from.
    const PAYER_KEY: [u8; 32] = [7u8; 32];

    pub(crate) fn payer_wallet() -> Address {
        wallet_of(&PAYER_KEY)
    }

    pub(crate) fn payment_address(invoice: &Invoice) -> Address {
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
    pub(crate) fn issue(chain_id: ChainId, amount: u64, expiration: u64) -> Invoice {
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

    pub(crate) async fn insert(pool: &PgPool, invoice: &Invoice, key: &str) {
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
    pub(crate) async fn insert_funded(pool: &PgPool, invoice: &Invoice, key: &str, block: u64) {
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
            1 + 10 * 2 + 9,
            "finality read plus two range-end reads per range, plus one cursor re-check per range after the first (the first range has no previous cursor)"
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

        worker.clone().sweep_tick().await.unwrap();
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

        worker.clone().sweep_tick().await.unwrap();
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
        worker.clone().sweep_tick().await.unwrap();

        chain.set(|state| {
            state.hashes.insert(7, B256::repeat_byte(0xEE));
        });
        worker.clone().sweep_tick().await.unwrap();
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
        worker.clone().sweep_tick().await.unwrap();
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

        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();
        for invoice in [&first, &second] {
            assert!(fetch(&pool, invoice).await.sweep_batch_id.is_some());
        }
        assert_eq!(
            open_batches(&pool).await,
            1,
            "a non-final revert still owns its nonce"
        );

        // The final revert returns both invoices to the queue behind their
        // attempt count, and the signer it freed picks them up again in the
        // same pass (no backoff in tests), so a second batch is open at once.
        chain.set(|state| state.finalized = 7);
        worker.clone().sweep_tick().await.unwrap();
        for invoice in [&first, &second] {
            let row = fetch(&pool, invoice).await;
            assert_eq!(row.status, "deploying");
            assert!(row.sweep_batch_id.is_some());
            assert_eq!(row.sweep_attempts, 1);
        }
        assert_eq!(open_batches(&pool).await, 1);
        assert_eq!(chain.submissions().len(), 2);

        // That batch reverts too (the mock decided at broadcast); the third
        // one succeeds.
        chain.set(|state| state.next_receipt_succeeds = true);
        worker.clone().sweep_tick().await.unwrap();
        assert_eq!(chain.submissions().len(), 3);
        worker.clone().sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &first).await.status, "fulfilled");
        assert_eq!(fetch(&pool, &second).await.status, "fulfilled");
        assert_eq!(chain.submissions().len(), 3);
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

        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();
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
        worker.clone().sweep_tick().await.unwrap();
        assert_eq!(
            fetch(&pool, &invoice).await.status,
            "expired",
            "no status flip while in flight"
        );
        worker.clone().sweep_tick().await.unwrap();
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
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();

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
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();

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
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();
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
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();

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
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();
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
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();

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
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();

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
    async fn third_party_settlement_is_found_by_paging_through_chunks(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
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
            // The settlement exists from block 3; ranges ending below it
            // answer empty, so a scan that stopped at the first empty chunk
            // would miss it entirely.
            state
                .settlement_appears_at
                .insert(payment_address(&invoice), 3);
            state.next_outcomes.insert(
                payment_address(&invoice),
                SweepOutcome::Collected { amount: U256::ZERO },
            );
        }));
        let mut cfg = config();
        cfg.log_range_size = 2;
        let worker = indexer_with(&pool, chain, cfg);
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();

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
            "the discovery paged 1..=2, 3..=4 and found the settlement in the second chunk"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn terminal_row_without_a_stored_settlement_still_finds_its_settlement(pool: PgPool) {
        // A legacy row whose settlement predates the ledger carries no stored
        // hash: the late drain transaction must not stand in for it, so its
        // classification still runs the settlement discovery.
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7));
        let worker = indexer(&pool, chain.clone());
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "fulfilled");

        sqlx::query("UPDATE invoices SET settlement_tx_hash = NULL WHERE id = $1")
            .bind(invoice.id.0)
            .execute(&pool)
            .await
            .unwrap();

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
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();

        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "fulfilled");
        assert_eq!(
            row.settlement_tx_hash,
            Some(vec![0xCC; 32]),
            "the discovered settlement tx backfills the legacy row, never the late drain"
        );
        assert_eq!(
            ledger(&pool, &invoice).await,
            vec![(
                "30".to_string(),
                "late_transfer".to_string(),
                chain.submissions()[1].tx_hash.to_vec(),
                9,
                block_timestamp(9) as i64
            )]
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
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();

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
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();
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

        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();
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
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();
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
        worker.clone().sweep_tick().await.unwrap();
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

        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();
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
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();

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
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();

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

        // The mock fixes an item's outcome when its batch is broadcast, and
        // the pass that classifies a retry resubmits the item on the freed
        // signer, so the failure is armed before every pass.
        chain.set(fail);
        worker.clone().sweep_tick().await.unwrap();
        for attempt in 1..=2 {
            chain.set(fail);
            worker.clone().sweep_tick().await.unwrap();
            let row = fetch(&pool, &invoice).await;
            assert_eq!(row.sweep_attempts, attempt);
            assert_eq!(row.status, "deploying");
            assert_eq!(chain.submissions().len(), attempt as usize + 1);
        }
        worker.clone().sweep_tick().await.unwrap();
        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "blocked");
        assert_eq!(row.blocked_reason.as_deref(), Some("retries_exhausted"));
        worker.clone().sweep_tick().await.unwrap();
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
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();

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
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();

        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "fulfilled");
        assert_eq!(row.blocked_reason.as_deref(), Some("recovery_blacklisted"));
        assert_eq!(row.uncollected_count, 1);
        worker.clone().sweep_tick().await.unwrap();
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
        worker.clone().sweep_tick().await.unwrap();

        worker.clone().sweep_tick().await.unwrap();
        assert_eq!(
            chain.submissions().len(),
            1,
            "nothing happens before the timeout"
        );

        set_submitted_at_in_the_past(&pool).await;
        worker.clone().sweep_tick().await.unwrap();
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
            state.mined_nonces.insert(mock_signer(0), 1);
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
        worker.clone().sweep_tick().await.unwrap();
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
        worker.clone().sweep_tick().await.unwrap();
        set_submitted_at_in_the_past(&pool).await;
        worker.clone().sweep_tick().await.unwrap();

        chain.set(|state| {
            let tx_hash = state.submissions[0].tx_hash;
            state.mined_nonces.insert(mock_signer(0), 1);
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
        worker.clone().sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "fulfilled");
        assert_eq!(open_batches(&pool).await, 0);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn batch_whose_nonce_was_consumed_elsewhere_is_abandoned_and_requeued(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7).with(|state| state.mine_at = None));
        let worker = indexer(&pool, chain.clone());
        worker.clone().sweep_tick().await.unwrap();

        set_submitted_at_in_the_past(&pool).await;
        chain.set(|state| {
            state.mined_nonces.insert(mock_signer(0), 1);
        });
        worker.clone().sweep_tick().await.unwrap();
        let resolution: String = sqlx::query_scalar(
            "SELECT resolution FROM sweep_batches WHERE resolved_at IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(resolution, "abandoned");
        // The invoice went back to the queue behind its attempt count and the
        // freed signer claimed it again in the same pass: the next batch uses
        // the next nonce and lets the contract decide.
        let row = fetch(&pool, &invoice).await;
        assert_eq!(row.status, "deploying");
        assert!(row.sweep_batch_id.is_some());
        assert_eq!(row.sweep_attempts, 1);
        assert_eq!(open_batches(&pool).await, 1);
        let submissions = chain.submissions();
        assert_eq!(submissions.len(), 2);
        assert_eq!(submissions[1].nonce, 1);
        assert_eq!(submissions[1].signer, mock_signer(0));
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn exhausted_replacements_pause_the_sweep_worker_without_touching_indexing(pool: PgPool) {
        let invoice = make_invoice(100);
        let later = make_invoice(200);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        insert(&pool, &later, "key-2").await;
        let chain = Arc::new(MockChain::new(7).with(|state| state.mine_at = None));
        let worker = indexer(&pool, chain.clone());
        worker.clone().sweep_tick().await.unwrap();
        for _ in 0..2 {
            set_submitted_at_in_the_past(&pool).await;
            worker.clone().sweep_tick().await.unwrap();
        }
        assert_eq!(chain.submissions().len(), 3);

        set_submitted_at_in_the_past(&pool).await;
        let error = worker.clone().sweep_tick().await.unwrap_err();
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
            state.mined_nonces.insert(mock_signer(0), 1);
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
        worker.clone().sweep_tick().await.unwrap();
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
        let task = tokio::spawn(worker.clone().run(shutdown_rx));

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

        let error = worker.clone().sweep_tick().await.unwrap_err();
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
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();
        assert_eq!(fetch(&pool, &invoice).await.status, "fulfilled");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_second_batch_is_never_submitted_while_one_is_in_flight_on_the_same_signer(
        pool: PgPool,
    ) {
        let pending = make_invoice(100);
        let waiting = make_invoice(200);
        insert_funded(&pool, &pending, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7).with(|state| state.mine_at = None));
        let worker = indexer(&pool, chain.clone());
        worker.clone().sweep_tick().await.unwrap();
        insert_funded(&pool, &waiting, "key-2", 2).await;

        worker.clone().sweep_tick().await.unwrap();
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
            .claim_sweep_batch(CHAIN_ID, 20, 0.0, 0.0, &[])
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].status, "deploying");

        let chain = Arc::new(MockChain::new(7));
        let worker = indexer(&pool, chain.clone());
        worker.clone().sweep_tick().await.unwrap();
        worker.clone().sweep_tick().await.unwrap();
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

        worker.clone().sweep_tick().await.unwrap();
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

        worker.clone().sweep_tick().await.unwrap();
        assert!(chain.submissions().is_empty());
        assert_eq!(fetch(&pool, &created).await.status, "created");
        assert_eq!(fetch(&pool, &partial).await.status, "created");
        assert_eq!(fetch(&pool, &other_chain).await.status, "funded");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_full_batch_settles_together_and_the_rest_follows_once_the_signer_frees(
        pool: PgPool,
    ) {
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

        worker.clone().sweep_tick().await.unwrap();
        let submissions = chain.submissions();
        assert_eq!(submissions.len(), 1, "one signer, one batch at a time");
        assert_eq!(submissions[0].sweeps.len(), SWEEP_BATCH_LIMIT as usize);
        assert_eq!(
            submissions[0].gas_limit,
            sweep_batch_gas_limit(SWEEP_BATCH_LIMIT as usize)
        );
        assert_eq!(
            fetch(&pool, invoices.last().unwrap()).await.status,
            "funded"
        );

        // The pass that finalizes the full batch frees its signer, which
        // takes the one left over before the pass ends.
        worker.clone().sweep_tick().await.unwrap();
        let fulfilled: i64 =
            sqlx::query_scalar("SELECT count(*) FROM invoices WHERE status = 'fulfilled'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(fulfilled, SWEEP_BATCH_LIMIT);
        let submissions = chain.submissions();
        assert_eq!(submissions.len(), 2);
        assert_eq!(submissions[1].sweeps.len(), 1);
        assert_eq!(
            fetch(&pool, invoices.last().unwrap()).await.status,
            "deploying"
        );

        worker.clone().sweep_tick().await.unwrap();
        assert_eq!(
            fetch(&pool, invoices.last().unwrap()).await.status,
            "fulfilled"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_synchronous_broadcast_settles_in_the_round_that_submits(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        // A sync-send chain's broadcast carries the receipt itself, so the
        // round that submits the batch observes it and settles the sweeps
        // without waiting out a receipt poll.
        let chain = Arc::new(MockChain::new(7).with(|state| state.sync_send = true));
        let worker = indexer(&pool, chain.clone());

        worker.clone().sweep_tick().await.unwrap();
        let submissions = chain.submissions();
        assert_eq!(submissions.len(), 1);
        assert_eq!(
            fetch(&pool, &invoice).await.status,
            "fulfilled",
            "the receipt that rode the broadcast settles the sweep this round"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn the_production_loop_settles_a_synchronous_broadcast_on_its_completion(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7).with(|state| state.sync_send = true));
        // A recovery cadence the test could never wait out, and no receipt
        // poll either: only the funding wake and the completion dispatches
        // can settle the sweep.
        let mut cfg = config();
        cfg.poll_interval = Duration::from_secs(3600);
        cfg.sweep_receipt_poll_interval = Duration::from_secs(3600);
        let worker = indexer_with(&pool, chain.clone(), cfg);
        let (shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(worker.clone().run_sweep_loop(shutdown_rx));

        // One funding-style poke: eligibility just committed durably.
        worker.sweep_wake.notify_one();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while fetch(&pool, &invoice).await.status != "fulfilled" {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the synchronous broadcast's sweep did not settle without the recovery timer"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(chain.submissions().len(), 1);
        shutdown.send(true).ok();
        let _ = task.await;
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_past_deadline_does_not_survive_a_dispatch_that_finds_no_work(pool: PgPool) {
        let chain = Arc::new(MockChain::new(7));
        let worker = indexer(&pool, chain.clone());
        let mut coordinator = Coordinator::new(Instant::now());
        let mut report = TickReport::default();
        // A signer deferred into the past whose queue has since emptied.
        coordinator
            .observe_at
            .insert(mock_signer(0), Instant::now() - Duration::from_secs(1));
        worker
            .dispatch(&mut coordinator, &mut report, Instant::now())
            .await;
        assert!(report.errors.is_empty());
        assert!(
            coordinator.observe_at.is_empty(),
            "the past deadline is consumed even though no work was claimable"
        );
        // The clock falls back to the recovery cadence instead of firing
        // again on the expired deadline.
        assert_eq!(coordinator.next_dispatch(), coordinator.recovery_at);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_failing_signer_keeps_the_worker_degraded_while_the_other_signer_works(pool: PgPool)
    {
        let failing = make_invoice(100);
        let working = make_invoice(200);
        insert_funded(&pool, &failing, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7).with_signers(2).with(|state| {
            state.failing_submissions.insert(mock_signer(0), 10_000);
        }));
        let mut cfg = config();
        cfg.poll_interval = Duration::from_millis(25);
        cfg.sweep_receipt_poll_interval = Duration::from_millis(25);
        let worker = indexer_with(&pool, chain.clone(), cfg);
        let (shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(worker.clone().run_sweep_loop(shutdown_rx));
        async fn sweeper_state(worker: &Indexer) -> String {
            worker
                .repo
                .sweeper_status(CHAIN_ID)
                .await
                .unwrap()
                .map(|status| status.state)
                .unwrap_or_else(|| "unwritten".to_string())
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);

        // The failing signer's batch is claimed and refused: the worker goes
        // degraded…
        loop {
            if sweeper_state(&worker).await == "degraded" {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the failing signer's lane never showed up in the health"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // …a second invoice lands in the queue and the rotation hands it to
        // the other signer, whose settlement must not report the worker
        // healthy again…
        insert_funded(&pool, &working, "key-2", 2).await;
        while fetch(&pool, &working).await.status != "fulfilled" {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the healthy signer's sweep never settled"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            sweeper_state(&worker).await,
            "degraded",
            "a successful dispatch for another signer is not a recovery"
        );
        // …and once the refusal is lifted, the failing lane recovers and the
        // worker's health follows it.
        chain.set(|state| {
            state.failing_submissions.clear();
        });
        loop {
            if sweeper_state(&worker).await == "running"
                && fetch(&pool, &failing).await.status == "fulfilled"
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the recovered lane never restored the worker's health"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        shutdown.send(true).ok();
        let _ = task.await;
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn an_overdue_unacknowledged_row_is_replaced_instead_of_rebroadcast_forever(
        pool: PgPool,
    ) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7).with(|state| {
            state.mine_at = None;
            state.failing_submissions.insert(mock_signer(0), 1);
        }));
        let worker = indexer(&pool, chain.clone());
        // The node refuses the broadcast after the batch went durable: the
        // row keeps its claim, unacknowledged, and the tick reports it.
        worker.clone().sweep_tick().await.unwrap_err();
        assert_eq!(open_batches(&pool).await, 1);
        // Past the pending timeout, the exact bytes are not re-sent
        // forever: the replacement machinery takes the row over (its
        // receipts were checked first, and there are none).
        set_submitted_at_in_the_past(&pool).await;
        worker.clone().sweep_tick().await.unwrap();
        assert_eq!(
            chain.submissions().len(),
            1,
            "the replacement, not a replay of the refused bytes"
        );
        assert_eq!(open_batches(&pool).await, 1);
        assert_eq!(fetch(&pool, &invoice).await.status, "deploying");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_failed_fee_read_is_cached_for_the_whole_dispatch(pool: PgPool) {
        let chain = Arc::new(
            MockChain::new(7).with(|state| {
                state.failing_fee_reads = 1;
            }),
        );
        let worker = indexer(&pool, chain.clone());
        // Two lanes of one dispatch ask for the fees concurrently: the failed
        // read is cached like a success, so a struggling RPC costs one round
        // trip per dispatch rather than one per lane.
        let reads = SweepReads::default();
        let (first, second) = tokio::join!(
            worker.tick_fees(&reads.fees),
            worker.tick_fees(&reads.fees),
        );
        assert!(first.is_err() && second.is_err());
        assert_eq!(
            chain.state.lock().unwrap().fee_requests,
            1,
            "one fee read shared by every lane of the dispatch"
        );
        // A fresh dispatch's fresh reads retry the read.
        let reads = SweepReads::default();
        worker.tick_fees(&reads.fees).await.unwrap();
        assert_eq!(chain.state.lock().unwrap().fee_requests, 2);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn two_signers_carry_two_batches_concurrently(pool: PgPool) {
        let pending = make_invoice(100);
        let waiting = make_invoice(200);
        insert_funded(&pool, &pending, "key-1", 1).await;
        let chain = Arc::new(
            MockChain::new(7)
                .with(|state| state.mine_at = None)
                .with_signers(2),
        );
        let worker = indexer(&pool, chain.clone());
        worker.clone().sweep_tick().await.unwrap();
        insert_funded(&pool, &waiting, "key-2", 2).await;
        worker.clone().sweep_tick().await.unwrap();

        let submissions = chain.submissions();
        assert_eq!(submissions.len(), 2);
        assert_eq!(submissions[0].signer, mock_signer(0));
        assert_eq!(submissions[1].signer, mock_signer(1));
        assert!(
            submissions.iter().all(|submission| submission.nonce == 0),
            "each signer has its own nonce stream"
        );
        assert_eq!(open_batches(&pool).await, 2);
        assert_eq!(fetch(&pool, &waiting).await.status, "deploying");
        assert_eq!(
            worker
                .repo
                .sweep_queue_stats(CHAIN_ID)
                .await
                .unwrap()
                .in_flight,
            2
        );
        worker.clone().sweep_tick().await.unwrap();
        assert_eq!(chain.submissions().len(), 2, "both signers are busy");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_stalled_signer_does_not_stop_the_other_signer(pool: PgPool) {
        let stuck = make_invoice(100);
        let fresh = make_invoice(200);
        insert_funded(&pool, &stuck, "key-1", 1).await;
        let chain = Arc::new(
            MockChain::new(7)
                .with(|state| state.mine_at = None)
                .with_signers(2),
        );
        let worker = indexer(&pool, chain.clone());
        worker.clone().sweep_tick().await.unwrap();
        for _ in 0..2 {
            set_submitted_at_in_the_past(&pool).await;
            worker.clone().sweep_tick().await.unwrap();
        }
        assert_eq!(chain.submissions().len(), 3);
        assert!(
            chain
                .submissions()
                .iter()
                .all(|submission| submission.signer == mock_signer(0)),
            "replacements stay on the batch's own signer"
        );

        insert_funded(&pool, &fresh, "key-2", 2).await;
        set_submitted_at_in_the_past(&pool).await;
        let error = worker.clone().sweep_tick().await.unwrap_err();
        assert!(matches!(error, IndexerError::SweepStalled(_)));
        assert!(error.requires_halt());
        let submissions = chain.submissions();
        assert_eq!(submissions.len(), 4, "the other signer still submits");
        assert_eq!(submissions[3].signer, mock_signer(1));
        assert_eq!(fetch(&pool, &fresh).await.status, "deploying");
        assert_eq!(open_batches(&pool).await, 2);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn fill_first_claiming_keeps_batches_full(pool: PgPool) {
        let invoices: Vec<Invoice> = (1..=(SWEEP_BATCH_LIMIT as u64 + 1))
            .map(|i| make_invoice(i * 100))
            .collect();
        for (index, invoice) in invoices.iter().enumerate() {
            insert_funded(&pool, invoice, &format!("key-{index}"), 1).await;
        }
        let chain = Arc::new(MockChain::new(7).with_signers(2));
        let worker = indexer(&pool, chain.clone());
        worker.clone().sweep_tick().await.unwrap();

        let submissions = chain.submissions();
        assert_eq!(submissions.len(), 2);
        // The lanes broadcast concurrently, so whichever finishes first lands
        // first in the mock; fill-first claiming is what pins the split.
        let mut submissions = submissions;
        submissions.sort_by_key(|submission| std::cmp::Reverse(submission.sweeps.len()));
        assert_eq!(submissions[0].sweeps.len(), SWEEP_BATCH_LIMIT as usize);
        assert_eq!(submissions[1].sweeps.len(), 1);
        assert_ne!(submissions[0].signer, submissions[1].signer);
        assert_eq!(
            chain.state.lock().unwrap().fee_requests,
            1,
            "one fee estimate per pass"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn submissions_rotate_across_the_pool(pool: PgPool) {
        let chain = Arc::new(MockChain::new(7).with_signers(3));
        let worker = indexer(&pool, chain.clone());
        for index in 0..4u64 {
            let invoice = make_invoice((index + 1) * 100);
            insert_funded(&pool, &invoice, &format!("key-{index}"), 1).await;
            worker.clone().sweep_tick().await.unwrap();
        }
        let signers: Vec<Address> = chain
            .submissions()
            .iter()
            .map(|submission| submission.signer)
            .collect();
        assert_eq!(
            signers,
            vec![
                mock_signer(0),
                mock_signer(1),
                mock_signer(2),
                mock_signer(0)
            ]
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn an_open_batch_signed_outside_the_pool_halts_the_sweep_worker(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7).with(|state| state.mine_at = None));
        let worker = indexer(&pool, chain.clone());
        worker.clone().sweep_tick().await.unwrap();

        chain.set(|state| state.signers = vec![mock_signer(1)]);
        let error = worker.clone().sweep_tick().await.unwrap_err();
        assert!(matches!(error, IndexerError::Configuration(_)));
        assert!(error.requires_halt());
        assert!(error.to_string().contains(&mock_signer(0).to_string()));
        assert_eq!(
            chain.submissions().len(),
            1,
            "nothing is signed for a stray row"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn low_balance_warning_is_per_signer(pool: PgPool) {
        let chain = Arc::new(MockChain::new(7).with_signers(2).with(|state| {
            state.balances.insert(mock_signer(1), U256::ZERO);
        }));
        let worker = indexer_with(
            &pool,
            chain,
            IndexerConfig {
                signer_low_balance_wei: U256::from(1),
                ..config()
            },
        );
        let health = worker.report_sweep_health().await.unwrap();
        assert_eq!(
            health,
            vec![
                SignerHealth {
                    signer: mock_signer(0),
                    balance: U256::from(10u64).pow(U256::from(18u64)),
                    low: false,
                },
                SignerHealth {
                    signer: mock_signer(1),
                    balance: U256::ZERO,
                    low: true,
                },
            ]
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_failing_signer_does_not_hold_the_head_of_the_queue(pool: PgPool) {
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(MockChain::new(7).with_signers(2).with(|state| {
            state.prepare_errors.insert(mock_signer(0));
        }));
        let worker = indexer_with(
            &pool,
            chain.clone(),
            IndexerConfig {
                // Real deployments back off in hours; a zero backoff would
                // hand the row straight back within the same pass and hide
                // the rotation bug this test pins.
                sweep_backoff_base_secs: 3600.0,
                sweep_backoff_cap_secs: 3600.0,
                ..config()
            },
        );

        // The head of the rotation claims the only invoice and fails to sign
        // it; the released row backs off, so the healthy signer idles.
        assert!(worker.clone().sweep_tick().await.is_err());
        assert!(chain.submissions().is_empty());

        // Once the backoff lapses, the pass must start behind the failed
        // signer: the healthy one takes the work.
        sqlx::query("UPDATE invoices SET last_attempt_at = now() - interval '12 hours'")
            .execute(&pool)
            .await
            .unwrap();
        worker.sweep_tick().await.unwrap();
        let submissions = chain.submissions();
        assert_eq!(submissions.len(), 1);
        assert_eq!(
            submissions[0].signer,
            mock_signer(1),
            "the queue moves to the healthy signer, not back to the failed one"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_failed_balance_read_does_not_hide_the_other_signers(pool: PgPool) {
        let chain = Arc::new(MockChain::new(7).with_signers(2).with(|state| {
            state.balance_errors.insert(mock_signer(0));
            state.balances.insert(mock_signer(1), U256::ZERO);
        }));
        let worker = indexer_with(
            &pool,
            chain,
            IndexerConfig {
                signer_low_balance_wei: U256::from(1),
                ..config()
            },
        );
        let health = worker.report_sweep_health().await.unwrap();
        assert_eq!(
            health,
            vec![SignerHealth {
                signer: mock_signer(1),
                balance: U256::ZERO,
                low: true,
            }],
            "the failed read costs only its own signer's entry"
        );
    }
}
