//! One chain's observer.
//!
//! The indexer is a pure function of two external facts: the ledger's
//! cursor for the chain (the last finalized block it has fully accounted
//! for) and the chain itself. It holds no state of its own that matters:
//! the watch list is a cache keyed by the ledger's fingerprint, and the
//! cursor it scans from is whatever the ledger says at the start of every
//! pass. Killing it and starting it again five minutes later resumes at the
//! same block; running two of them is safe because every range it reports
//! is a compare-and-set on that cursor ([`RangeApplied::CursorMismatch`]
//! means somebody else advanced it, and the pass starts over).
//!
//! A pass:
//!
//! 1. Re-read the cursor block's header. A different hash means a block
//!    this deployment already treated as final was replaced: a finality
//!    violation. The chain is reported halted; the pass stops. The chain
//!    is re-checked every pass thereafter, so nothing needs a restart once
//!    an operator has repaired the cursor and resumed the chain.
//! 2. Find the finality boundary and report it (the ledger's chain clock,
//!    which expires unpaid requests).
//! 3. Refresh the watch list when its fingerprint changed.
//! 4. Scan `[cursor + 1, boundary]` in `log_range_size` windows, at most
//!    `max_ranges` per pass, reporting each window with its observations as
//!    one finalized range. The ledger applies a range atomically, so a
//!    crash between two windows loses nothing.
//!
//! Errors are retried on the next pass with an exponential backoff, which
//! covers RPC providers and the server being unavailable alike.

use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::{Address, B256};
use chrono::Utc;
use gum_bus::BackoffPolicy;
use gum_chain::{ChainError, ChainReader};
use gum_contracts::rpc::{
    ChainFaultReport, FinalizedHeadReport, IndexerCursor, PaymentObservation, RangeApplied,
    RecordFinalizedRange, WatchListQuery,
};
use gum_core::ChainConfig;
use tokio::sync::watch;
use tracing::{Instrument, debug, error, info, info_span, warn};

use crate::ledger::{IndexerLedger, LedgerError};
use crate::signal::{SignalState, WatchList};

/// Everything that decides how often the worker looks at the chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cadence {
    /// Between passes while addresses are watched and no signal is connected.
    pub poll: Duration,
    /// Between passes while the WebSocket signal is connected (the signal
    /// wakes the worker early; this is the safety net).
    pub reconcile: Duration,
    /// Between passes while nothing is watched.
    pub idle: Duration,
    /// After a failed pass; grows per consecutive failure.
    pub failure_backoff: BackoffPolicy,
}

pub struct Indexer {
    ledger: Arc<dyn IndexerLedger>,
    chain: Arc<dyn ChainReader>,
    config: ChainConfig,
    /// How far back a bound-but-unpaid request stays on the watch list.
    late_watch: Duration,
    /// Windows scanned per pass, so one pass never monopolises the RPC
    /// budget when catching up.
    max_ranges: u64,
    cadence: Cadence,
    signal: Arc<SignalState>,
    watch_tx: watch::Sender<WatchList>,
}

/// What a pass established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// Up to date, or scanned `max_ranges` windows and will continue.
    Scanned { ranges: u64, observations: usize },
    /// The ledger's cursor moved under this worker (another replica, an
    /// operator); the next pass re-reads it.
    Restart,
    /// The chain is halted on a finality violation. Nothing is scanned
    /// until an operator resumes it.
    Halted,
}

impl Indexer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ledger: Arc<dyn IndexerLedger>,
        chain: Arc<dyn ChainReader>,
        config: ChainConfig,
        late_watch: Duration,
        max_ranges: u64,
        cadence: Cadence,
        signal: Arc<SignalState>,
        watch_tx: watch::Sender<WatchList>,
    ) -> Self {
        Self {
            ledger,
            chain,
            config,
            late_watch,
            max_ranges,
            cadence,
            signal,
            watch_tx,
        }
    }

    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let chain_id = self.config.chain_id;
        info!(
            chain_id,
            start_block = self.config.start_block,
            "chain worker started"
        );
        let mut cache = WatchCache::default();
        let mut failures: u32 = 0;
        let mut signal_target: Option<u64> = None;
        loop {
            if *shutdown.borrow() {
                info!(chain_id, "chain worker stopped");
                return;
            }
            let span = info_span!("indexer.pass", chain_id);
            let outcome = self.pass(&mut cache).instrument(span).await;
            let wait = match outcome {
                Ok(Pass::Restart) => {
                    failures = 0;
                    continue;
                }
                Ok(Pass::Scanned { ranges, .. }) if ranges >= self.max_ranges => {
                    // Still catching up: go straight to the next window.
                    failures = 0;
                    continue;
                }
                Ok(Pass::Scanned { .. }) => {
                    failures = 0;
                    if let Some(target) = signal_target {
                        match self.ledger.cursor(chain_id).await {
                            Ok(Some(cursor)) if cursor.block >= target => {
                                signal_target = None;
                                self.cadence_for(&cache)
                            }
                            Ok(cursor) => {
                                debug!(target, indexed = ?cursor.map(|cursor| cursor.block), "signal target is not finalized and indexed yet; following up");
                                Duration::from_millis(self.config.block_time_ms.max(1))
                            }
                            Err(error) => {
                                warn!(%error, target, "could not check signal catch-up progress");
                                self.cadence.failure_backoff.delay(1)
                            }
                        }
                    } else {
                        self.cadence_for(&cache)
                    }
                }
                Ok(Pass::Halted) => {
                    failures = 0;
                    self.cadence.idle
                }
                Err(error) => {
                    failures = failures.saturating_add(1);
                    let wait = self.cadence.failure_backoff.delay(failures);
                    warn!(
                        chain_id,
                        error = %error,
                        retryable = error.retryable(),
                        attempt = failures,
                        retry_in_ms = wait.as_millis() as u64,
                        "indexer pass failed; retrying"
                    );
                    wait
                }
            };
            let deadline = tokio::time::sleep(wait);
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    _ = &mut deadline => break,
                    _ = tokio::time::sleep(self.cadence.poll) => {
                        if let Err(error) = self.refresh_watch_list(&mut cache).await {
                            warn!(chain_id, %error, "watch list refresh failed; retrying");
                        }
                    }
                    _ = self.signal.notified() => {
                        if let Some(target) = self.signal.take_target() {
                            signal_target = Some(signal_target.map_or(target, |pending| pending.max(target)));
                        }
                        break;
                    }
                    _ = self.signal.health_changed() => break,
                    _ = shutdown.changed() => break,
                }
            }
        }
    }

    fn cadence_for(&self, cache: &WatchCache) -> Duration {
        if cache.addresses.is_empty() {
            self.cadence.idle
        } else if self.signal.is_connected() {
            self.cadence.reconcile
        } else {
            self.cadence.poll
        }
    }

    /// One pass, as described at the top of the module.
    pub async fn pass(&self, cache: &mut WatchCache) -> Result<Pass, PassError> {
        let chain_id = self.config.chain_id;
        let mut cursor = self.ledger.cursor(chain_id).await?;
        // Read the boundary before validating continuity. That puts even an
        // empty-watch fast-forward inside a before/after ancestry window:
        // adopting a replacement finalized branch cannot erase the evidence
        // that our stored cursor belonged to the old one.
        let boundary = gum_chain::finality_boundary(
            self.chain.as_ref(),
            self.config.finality_source,
            self.config.finality_confirmations,
        )
        .await?;

        if let Some(stored) = cursor {
            let header = self.chain.block_header(stored.block).await?;
            if header.hash != stored.block_hash {
                let reason = format!(
                    "finalized block {} changed hash from {} to {}",
                    stored.block, stored.block_hash, header.hash
                );
                self.ledger
                    .report_fault(
                        chain_id,
                        ChainFaultReport {
                            reason: reason.clone(),
                            block: Some(stored.block),
                        },
                    )
                    .await?;
                error!(chain_id, block = stored.block, error = %reason, "finality violation; chain halted");
                return Ok(Pass::Halted);
            }
        }

        self.ledger
            .report_finalized_head(
                chain_id,
                FinalizedHeadReport {
                    block: boundary.number,
                    block_hash: boundary.hash,
                    block_timestamp: boundary.timestamp,
                },
            )
            .await?;

        self.refresh_watch_list(cache).await?;

        let start = cursor.map_or(self.config.start_block, |c| c.block.saturating_add(1));
        if start > boundary.number {
            return Ok(Pass::Scanned {
                ranges: 0,
                observations: 0,
            });
        }

        // Nothing to watch: the whole span is one empty range, so the
        // cursor keeps up with finality and the first payment address ever
        // bound does not trigger a scan from `start_block`.
        if cache.addresses.is_empty() {
            info!(
                chain_id,
                from_block = start,
                to_block = boundary.number,
                "nothing watched; cursor fast-forwarded without scanning"
            );
            return Ok(
                match self
                    .apply(
                        cursor,
                        boundary.number,
                        boundary.hash,
                        boundary.timestamp,
                        Vec::new(),
                    )
                    .await?
                {
                    Pass::Restart => Pass::Restart,
                    _ => Pass::Scanned {
                        ranges: 1,
                        observations: 0,
                    },
                },
            );
        }

        let tokens = self.config.token_addresses();
        let mut from = start;
        let mut range_size = self.config.log_range_size.max(1);
        let mut ranges = 0;
        let mut observed = 0;
        while from <= boundary.number && ranges < self.max_ranges {
            let to = boundary.number.min(from.saturating_add(range_size - 1));
            debug!(
                chain_id,
                from_block = from,
                to_block = to,
                "scanning finalized range"
            );
            // Put the cursor continuity read inside the same before/after
            // window as the logs. A finalized reorg between a separate
            // cursor check and this first end-header read must not let both
            // end reads agree on the new branch and silently advance us.
            let before = self.chain.block_header(to).await?;
            if let Some(expected) = cursor {
                let preceding = self.chain.block_header(expected.block).await?;
                if preceding.hash != expected.block_hash {
                    let reason =
                        format!("finalized block {} changed while scanning", expected.block);
                    self.ledger
                        .report_fault(
                            chain_id,
                            ChainFaultReport {
                                reason,
                                block: Some(expected.block),
                            },
                        )
                        .await?;
                    return Ok(Pass::Halted);
                }
            }
            let transfers = match self
                .chain
                .token_transfers(&tokens, from, to, &cache.addresses)
                .await
            {
                Ok(transfers) => transfers,
                Err(ChainError::LogRangeTooLarge(message)) if range_size > 1 => {
                    range_size = (range_size / 2).max(1);
                    warn!(from_block = from, to_block = to, smaller = range_size, error = %message, "provider rejected the log range; splitting it");
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let header = self.chain.block_header(to).await?;
            if before.hash != header.hash {
                warn!(block = to, before = %before.hash, after = %header.hash, "range end changed while fetching logs; retrying the uncommitted range");
                continue;
            }
            let observations: Vec<PaymentObservation> = transfers
                .into_iter()
                .map(|transfer| {
                    info!(
                        chain_id,
                        payment_address = %transfer.recipient,
                        tx_hash = %transfer.transaction_hash,
                        block = transfer.block_number,
                        amount = %transfer.amount,
                        "payment observed"
                    );
                    PaymentObservation {
                        token: transfer.token,
                        block_number: transfer.block_number,
                        block_hash: transfer.block_hash,
                        block_timestamp: transfer.block_timestamp,
                        transaction_hash: transfer.transaction_hash,
                        transaction_index: transfer.transaction_index,
                        log_index: transfer.log_index,
                        sender: transfer.sender,
                        recipient: transfer.recipient,
                        amount: transfer.amount,
                    }
                })
                .collect();
            observed += observations.len();
            if let Pass::Restart = self
                .apply(cursor, to, header.hash, header.timestamp, observations)
                .await?
            {
                return Ok(Pass::Restart);
            }
            cursor = Some(IndexerCursor {
                block: to,
                block_hash: header.hash,
                block_timestamp: Some(header.timestamp),
            });
            from = to + 1;
            ranges += 1;
        }
        Ok(Pass::Scanned {
            ranges,
            observations: observed,
        })
    }

    async fn refresh_watch_list(&self, cache: &mut WatchCache) -> Result<(), LedgerError> {
        let chain_id = self.config.chain_id;
        let recent_since =
            Utc::now() - chrono::Duration::from_std(self.late_watch).unwrap_or_default();
        let list = self
            .ledger
            .watch_list(
                chain_id,
                WatchListQuery {
                    recent_since,
                    known_fingerprint: cache.fingerprint.clone(),
                },
            )
            .await?;
        cache.fingerprint = Some(list.fingerprint);
        if let Some(addresses) = list.addresses {
            debug!(chain_id, watched = addresses.len(), "watch list refreshed");
            cache.addresses = Arc::new(addresses);
            let _ = self.watch_tx.send(cache.addresses.clone());
        }
        Ok(())
    }

    async fn apply(
        &self,
        expected: Option<IndexerCursor>,
        end_block: u64,
        end_block_hash: B256,
        end_block_timestamp: u64,
        observations: Vec<PaymentObservation>,
    ) -> Result<Pass, PassError> {
        let chain_id = self.config.chain_id;
        let applied = self
            .ledger
            .record_finalized_range(
                chain_id,
                RecordFinalizedRange {
                    expected_cursor: expected,
                    end_block,
                    end_block_hash,
                    end_block_timestamp,
                    observations,
                },
            )
            .await?;
        match applied {
            RangeApplied::Applied {
                funded,
                expired,
                cursor,
            } => {
                if !funded.is_empty() || !expired.is_empty() {
                    info!(
                        chain_id,
                        block = cursor.block,
                        funded = funded.len(),
                        expired = expired.len(),
                        "finalized range applied"
                    );
                }
                Ok(Pass::Scanned {
                    ranges: 1,
                    observations: 0,
                })
            }
            RangeApplied::AlreadyApplied { cursor } => {
                // A replay of our own request whose response we never saw.
                info!(
                    chain_id,
                    block = cursor.block,
                    "finalized range already applied"
                );
                Ok(Pass::Scanned {
                    ranges: 1,
                    observations: 0,
                })
            }
            RangeApplied::CursorMismatch { cursor } => {
                warn!(
                    chain_id,
                    expected = ?expected.map(|c| c.block),
                    stored = ?cursor.map(|c| c.block),
                    "ledger cursor moved under this worker; restarting the pass"
                );
                Ok(Pass::Restart)
            }
        }
    }
}

/// The watch list as last fetched, and the fingerprint that lets the
/// server skip sending it again. Process memory only; rebuilt on the
/// first pass after a restart.
#[derive(Default)]
pub struct WatchCache {
    fingerprint: Option<String>,
    addresses: Arc<Vec<Address>>,
}

#[derive(Debug, thiserror::Error)]
pub enum PassError {
    #[error(transparent)]
    Chain(#[from] ChainError),
    #[error(transparent)]
    Ledger(#[from] LedgerError),
}

impl PassError {
    pub fn retryable(&self) -> bool {
        match self {
            Self::Chain(error) => error.is_retryable(),
            Self::Ledger(error) => error.is_retryable(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use alloy_primitives::{U256, address};
    use async_trait::async_trait;
    use gum_chain::WatchedTransfer;
    use gum_chain::mock::{MockChain, block_hash};
    use gum_contracts::rpc::{RpcError, RpcErrorCode, WatchList as RpcWatchList};
    use gum_core::{Currency, FinalitySource, TokenConfig};

    const CHAIN: u64 = 31337;
    const TOKEN: Address = address!("0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512");
    const PAID: Address = address!("0x1000000000000000000000000000000000000001");
    const START: u64 = 100;

    fn config(log_range_size: u64) -> ChainConfig {
        ChainConfig {
            chain_id: CHAIN,
            tokens: vec![TokenConfig {
                currency: Currency::Usdc,
                address: TOKEN,
            }],
            factory: Address::repeat_byte(0xfa),
            batch_sweeper: Address::repeat_byte(0x55),
            factory_code_hash: B256::ZERO,
            batch_sweeper_code_hash: B256::ZERO,
            start_block: START,
            finality_source: FinalitySource::Finalized,
            finality_confirmations: 0,
            block_time_ms: 1000,
            log_range_size,
            signer_low_balance_wei: None,
            explorer_base_url: None,
            cctp: None,
            block_gas_limit: None,
            transaction_gas_limit: None,
            sweep_batch_size: None,
        }
    }

    fn transfer(block: u64, recipient: Address, amount: u64) -> WatchedTransfer {
        WatchedTransfer {
            token: TOKEN,
            block_number: block,
            block_hash: block_hash(block),
            block_timestamp: 1_700_000_000 + block,
            transaction_hash: B256::left_padding_from(&[0xAB, block as u8]),
            transaction_index: 0,
            log_index: 0,
            sender: Address::repeat_byte(0x77),
            recipient,
            amount: U256::from(amount),
        }
    }

    /// The server's side of the RPC contract, in memory: the same
    /// compare-and-set the ledger performs, plus a script of failures.
    #[derive(Default)]
    struct FakeLedger {
        cursor: Mutex<Option<IndexerCursor>>,
        watched: Mutex<Vec<Address>>,
        ranges: Mutex<Vec<RecordFinalizedRange>>,
        heads: Mutex<Vec<FinalizedHeadReport>>,
        faults: Mutex<Vec<ChainFaultReport>>,
        watch_list_calls: Mutex<usize>,
        range_calls: Mutex<usize>,
        /// `record_finalized_range` fails when it is this call (1-based).
        fail_range_call: Mutex<Option<usize>>,
        /// Another replica advances the cursor to this the moment the
        /// worker has read it, so the worker's next report is stale.
        move_cursor_after_read: Mutex<Option<IndexerCursor>>,
    }

    impl FakeLedger {
        fn watch(&self, address: Address) {
            self.watched.lock().unwrap().push(address);
        }

        fn cursor_block(&self) -> Option<u64> {
            self.cursor.lock().unwrap().map(|c| c.block)
        }

        fn ranges(&self) -> Vec<RecordFinalizedRange> {
            self.ranges.lock().unwrap().clone()
        }

        fn observations(&self) -> Vec<PaymentObservation> {
            self.ranges()
                .into_iter()
                .flat_map(|range| range.observations)
                .collect()
        }
    }

    #[async_trait]
    impl IndexerLedger for FakeLedger {
        async fn cursor(&self, _: u64) -> Result<Option<IndexerCursor>, LedgerError> {
            let mut cursor = self.cursor.lock().unwrap();
            let read = *cursor;
            if let Some(moved) = self.move_cursor_after_read.lock().unwrap().take() {
                *cursor = Some(moved);
            }
            Ok(read)
        }

        async fn watch_list(
            &self,
            _: u64,
            query: WatchListQuery,
        ) -> Result<RpcWatchList, LedgerError> {
            *self.watch_list_calls.lock().unwrap() += 1;
            let watched = self.watched.lock().unwrap().clone();
            let fingerprint = format!("fp-{}", watched.len());
            Ok(RpcWatchList {
                addresses: (query.known_fingerprint.as_deref() != Some(&fingerprint))
                    .then_some(watched),
                fingerprint,
            })
        }

        async fn report_finalized_head(
            &self,
            _: u64,
            report: FinalizedHeadReport,
        ) -> Result<(), LedgerError> {
            self.heads.lock().unwrap().push(report);
            Ok(())
        }

        async fn record_finalized_range(
            &self,
            _: u64,
            range: RecordFinalizedRange,
        ) -> Result<RangeApplied, LedgerError> {
            let call = {
                let mut calls = self.range_calls.lock().unwrap();
                *calls += 1;
                *calls
            };
            if *self.fail_range_call.lock().unwrap() == Some(call) {
                return Err(LedgerError::Rpc(RpcError::new(
                    RpcErrorCode::Unavailable,
                    "database unavailable",
                )));
            }
            let mut cursor = self.cursor.lock().unwrap();
            if *cursor != range.expected_cursor {
                return Ok(match *cursor {
                    Some(stored) if stored.block >= range.end_block => {
                        RangeApplied::AlreadyApplied { cursor: stored }
                    }
                    stored => RangeApplied::CursorMismatch { cursor: stored },
                });
            }
            let next = IndexerCursor {
                block: range.end_block,
                block_hash: range.end_block_hash,
                block_timestamp: Some(range.end_block_timestamp),
            };
            *cursor = Some(next);
            self.ranges.lock().unwrap().push(range);
            Ok(RangeApplied::Applied {
                cursor: next,
                funded: vec![],
                expired: vec![],
            })
        }

        async fn report_fault(&self, _: u64, report: ChainFaultReport) -> Result<(), LedgerError> {
            self.faults.lock().unwrap().push(report);
            Ok(())
        }
    }

    fn indexer(
        ledger: Arc<FakeLedger>,
        chain: Arc<MockChain>,
        log_range_size: u64,
        max_ranges: u64,
    ) -> Indexer {
        let (watch_tx, _) = watch::channel(Arc::new(Vec::new()));
        Indexer::new(
            ledger,
            chain,
            config(log_range_size),
            Duration::from_secs(86_400),
            max_ranges,
            Cadence {
                poll: Duration::from_millis(1),
                reconcile: Duration::from_millis(1),
                idle: Duration::from_millis(1),
                failure_backoff: BackoffPolicy::new(
                    Duration::from_millis(1),
                    Duration::from_millis(2),
                ),
            },
            SignalState::new(),
            watch_tx,
        )
    }

    /// Scanning is windowed by `log_range_size` and bounded by `max_ranges`
    /// per pass; every window is one finalized range whose observations are
    /// the transfers to watched addresses in it, and the next pass picks up
    /// exactly where the previous one stopped.
    #[tokio::test]
    async fn ranges_are_windowed_bounded_and_contiguous() {
        let ledger = Arc::new(FakeLedger::default());
        ledger.watch(PAID);
        let chain = Arc::new(MockChain::new(350).with(|state| {
            state.transfers = vec![
                transfer(120, PAID, 5),
                transfer(250, PAID, 7),
                transfer(260, Address::repeat_byte(0x99), 9), // not watched
            ];
        }));
        let worker = indexer(ledger.clone(), chain.clone(), 100, 2);
        let mut cache = WatchCache::default();

        let first = worker.pass(&mut cache).await.unwrap();
        assert_eq!(
            first,
            Pass::Scanned {
                ranges: 2,
                observations: 2
            }
        );
        let windows: Vec<(u64, u64)> = chain
            .state
            .lock()
            .unwrap()
            .log_requests
            .iter()
            .map(|(from, to, _)| (*from, *to))
            .collect();
        assert_eq!(windows, vec![(START, 199), (200, 299)]);
        assert_eq!(ledger.cursor_block(), Some(299));

        let second = worker.pass(&mut cache).await.unwrap();
        assert_eq!(
            second,
            Pass::Scanned {
                ranges: 1,
                observations: 0
            }
        );
        assert_eq!(ledger.cursor_block(), Some(350));
        let ranges = ledger.ranges();
        assert_eq!(
            ranges
                .iter()
                .map(|r| (r.expected_cursor.map(|c| c.block), r.end_block))
                .collect::<Vec<_>>(),
            vec![(None, 199), (Some(199), 299), (Some(299), 350)]
        );
        let observed: Vec<(u64, U256)> = ledger
            .observations()
            .iter()
            .map(|o| (o.block_number, o.amount))
            .collect();
        assert_eq!(observed, vec![(120, U256::from(5)), (250, U256::from(7))]);

        // Up to date: a third pass reports the head and scans nothing.
        let third = worker.pass(&mut cache).await.unwrap();
        assert_eq!(
            third,
            Pass::Scanned {
                ranges: 0,
                observations: 0
            }
        );
        assert_eq!(ledger.heads.lock().unwrap().len(), 3);
        // The watch list travelled once; the fingerprint spared the rest.
        assert_eq!(*ledger.watch_list_calls.lock().unwrap(), 3);
    }

    #[tokio::test]
    async fn provider_range_limits_are_adapted_without_losing_progress() {
        let ledger = Arc::new(FakeLedger::default());
        ledger.watch(PAID);
        let chain = Arc::new(MockChain::new(250).with(|state| {
            state.max_log_range = Some(25);
            state.transfers = vec![transfer(120, PAID, 5), transfer(240, PAID, 7)];
        }));
        let worker = indexer(ledger.clone(), chain.clone(), 100, 20);

        worker.pass(&mut WatchCache::default()).await.unwrap();

        assert_eq!(ledger.cursor_block(), Some(250));
        assert_eq!(
            ledger
                .observations()
                .iter()
                .map(|observation| observation.block_number)
                .collect::<Vec<_>>(),
            [120, 240]
        );
        assert!(
            chain
                .state
                .lock()
                .unwrap()
                .log_requests
                .iter()
                .all(|(from, to, _)| to - from < 25)
        );
    }

    /// A restarted worker (fresh process memory) continues from the
    /// ledger's cursor: nothing is scanned twice, nothing is skipped.
    #[tokio::test]
    async fn a_restart_resumes_from_the_ledger_cursor() {
        let ledger = Arc::new(FakeLedger::default());
        ledger.watch(PAID);
        let chain = Arc::new(MockChain::new(180).with(|state| {
            state.transfers = vec![transfer(150, PAID, 1), transfer(175, PAID, 2)];
        }));
        indexer(ledger.clone(), chain.clone(), 50, 1)
            .pass(&mut WatchCache::default())
            .await
            .unwrap();
        assert_eq!(ledger.cursor_block(), Some(149));

        // Crash. New process, new cache; the chain moved on.
        chain.set(|state| {
            state.latest = 200;
            state.finalized = 200;
        });
        let restarted = indexer(ledger.clone(), chain.clone(), 50, 10);
        restarted.pass(&mut WatchCache::default()).await.unwrap();
        assert_eq!(ledger.cursor_block(), Some(200));
        let windows: Vec<(u64, u64)> = chain
            .state
            .lock()
            .unwrap()
            .log_requests
            .iter()
            .map(|(from, to, _)| (*from, *to))
            .collect();
        assert_eq!(windows, vec![(100, 149), (150, 199), (200, 200)]);
        let observed: Vec<u64> = ledger
            .observations()
            .iter()
            .map(|o| o.block_number)
            .collect();
        assert_eq!(
            observed,
            vec![150, 175],
            "each payment reported exactly once"
        );
    }

    /// Another replica advances the cursor between this worker's cursor
    /// read and its report. The report is refused, the pass restarts, and
    /// the next one continues from the cursor the other replica left: the
    /// refused window is neither re-applied nor skipped.
    #[tokio::test]
    async fn a_cursor_mismatch_restarts_from_the_stored_cursor() {
        let ledger = Arc::new(FakeLedger::default());
        ledger.watch(PAID);
        let chain = Arc::new(MockChain::new(300).with(|state| {
            state.transfers = vec![transfer(150, PAID, 1), transfer(250, PAID, 2)];
        }));
        let worker = indexer(ledger.clone(), chain.clone(), 100, 10);
        let mut cache = WatchCache::default();

        // The other replica applied [100, 199] (and reported block 150's
        // payment) right after we read an empty cursor.
        *ledger.move_cursor_after_read.lock().unwrap() = Some(IndexerCursor {
            block: 199,
            block_hash: block_hash(199),
            block_timestamp: Some(0),
        });
        assert_eq!(worker.pass(&mut cache).await.unwrap(), Pass::Restart);
        assert_eq!(ledger.cursor_block(), Some(199));
        assert!(ledger.ranges().is_empty(), "our stale window was refused");

        let outcome = worker.pass(&mut cache).await.unwrap();
        assert_eq!(
            outcome,
            Pass::Scanned {
                ranges: 2,
                observations: 1
            }
        );
        assert_eq!(
            ledger
                .ranges()
                .iter()
                .map(|r| (r.expected_cursor.map(|c| c.block), r.end_block))
                .collect::<Vec<_>>(),
            vec![(Some(199), 299), (Some(299), 300)]
        );
        let observed: Vec<u64> = ledger
            .observations()
            .iter()
            .map(|o| o.block_number)
            .collect();
        assert_eq!(
            observed,
            vec![250],
            "block 150 belonged to the other replica's window"
        );
    }

    /// A retry of a report whose first attempt committed (the response was
    /// lost) is acknowledged without re-applying, and a stale report from
    /// a worker whose cursor is behind is refused. Both are the contract
    /// the real ledger implements; the fake mirrors it so the tests above
    /// mean something.
    #[tokio::test]
    async fn the_fake_ledger_mirrors_the_compare_and_set_contract() {
        let ledger = FakeLedger::default();
        let range = |expected: Option<u64>, end: u64| RecordFinalizedRange {
            expected_cursor: expected.map(|block| IndexerCursor {
                block,
                block_hash: block_hash(block),
                block_timestamp: Some(0),
            }),
            end_block: end,
            end_block_hash: block_hash(end),
            end_block_timestamp: 0,
            observations: vec![],
        };
        assert!(matches!(
            ledger.record_finalized_range(CHAIN, range(None, 199)).await.unwrap(),
            RangeApplied::Applied { cursor, .. } if cursor.block == 199
        ));
        assert!(matches!(
            ledger.record_finalized_range(CHAIN, range(None, 199)).await.unwrap(),
            RangeApplied::AlreadyApplied { cursor } if cursor.block == 199
        ));
        assert!(matches!(
            ledger.record_finalized_range(CHAIN, range(Some(150), 400)).await.unwrap(),
            RangeApplied::CursorMismatch { cursor: Some(cursor) } if cursor.block == 199
        ));
    }

    /// A finalized block the ledger accounted for changing hash is a
    /// finality violation: the chain is reported halted and nothing is
    /// scanned, on this pass or the next, until the cursor is repaired.
    #[tokio::test]
    async fn a_finality_violation_halts_the_chain_and_scans_nothing() {
        let ledger = Arc::new(FakeLedger::default());
        ledger.watch(PAID);
        let chain = Arc::new(MockChain::new(250).with(|state| {
            state.transfers = vec![transfer(220, PAID, 1)];
        }));
        let worker = indexer(ledger.clone(), chain.clone(), 100, 10);
        let mut cache = WatchCache::default();
        worker.pass(&mut cache).await.unwrap();
        assert_eq!(ledger.cursor_block(), Some(250));

        // The node now serves a different block 250.
        chain.set(|state| {
            state.hashes.insert(250, B256::repeat_byte(0xBA));
            state.latest = 300;
            state.finalized = 300;
        });
        assert_eq!(worker.pass(&mut cache).await.unwrap(), Pass::Halted);
        assert_eq!(worker.pass(&mut cache).await.unwrap(), Pass::Halted);
        let faults = ledger.faults.lock().unwrap().clone();
        assert_eq!(
            faults.len(),
            2,
            "reported every pass; the server deduplicates"
        );
        assert_eq!(faults[0].block, Some(250));
        assert!(faults[0].reason.contains("250"));
        assert_eq!(ledger.cursor_block(), Some(250), "the cursor did not move");
        assert_eq!(
            ledger.heads.lock().unwrap().len(),
            1,
            "no head reported past the violation"
        );

        // The operator repaired the cursor (rewound it to a block both
        // agree on): scanning continues without a restart.
        *ledger.cursor.lock().unwrap() = Some(IndexerCursor {
            block: 200,
            block_hash: block_hash(200),
            block_timestamp: Some(0),
        });
        let outcome = worker.pass(&mut cache).await.unwrap();
        assert_eq!(
            outcome,
            Pass::Scanned {
                ranges: 1,
                observations: 1
            }
        );
        assert_eq!(ledger.cursor_block(), Some(300));
    }

    /// Continuity is checked after the range's first end-header read. If the
    /// branch changes in between, the stored cursor catches it before logs
    /// from the replacement branch can be committed.
    #[tokio::test]
    async fn a_reorg_between_cursor_validation_and_range_read_halts_the_chain() {
        let ledger = Arc::new(FakeLedger::default());
        ledger.watch(PAID);
        *ledger.cursor.lock().unwrap() = Some(IndexerCursor {
            block: 200,
            block_hash: block_hash(200),
            block_timestamp: Some(0),
        });
        let chain = Arc::new(MockChain::new(250).with(|state| {
            state.transfers = vec![transfer(220, PAID, 1)];
            // Cursor header, finalized header, and initial range-end header
            // see the old branch. The following cursor read sees the reorg.
            state.reorg_on_header_request = Some(4);
        }));
        let worker = indexer(ledger.clone(), chain, 100, 10);
        let mut cache = WatchCache::default();

        assert_eq!(worker.pass(&mut cache).await.unwrap(), Pass::Halted);
        assert_eq!(ledger.cursor_block(), Some(200));
        assert!(ledger.ranges().is_empty());
        assert_eq!(ledger.faults.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_empty_watch_list_cannot_fast_forward_across_a_reorg() {
        let ledger = Arc::new(FakeLedger::default());
        *ledger.cursor.lock().unwrap() = Some(IndexerCursor {
            block: 200,
            block_hash: block_hash(200),
            block_timestamp: Some(0),
        });
        let chain = Arc::new(MockChain::new(250).with(|state| {
            // Boundary sees the old branch; continuity sees the replacement.
            state.reorg_on_header_request = Some(2);
        }));
        let worker = indexer(ledger.clone(), chain, 100, 10);

        assert_eq!(
            worker.pass(&mut WatchCache::default()).await.unwrap(),
            Pass::Halted
        );
        assert_eq!(ledger.cursor_block(), Some(200));
        assert!(ledger.ranges().is_empty());
        assert_eq!(ledger.faults.lock().unwrap().len(), 1);
    }

    /// A pass that fails halfway leaves the ledger consistent: the windows
    /// it committed stand, the one it did not is re-scanned, and the
    /// payment in it is reported once.
    #[tokio::test]
    async fn a_failed_pass_resumes_without_losing_or_duplicating_payments() {
        let ledger = Arc::new(FakeLedger::default());
        ledger.watch(PAID);
        let chain = Arc::new(MockChain::new(299).with(|state| {
            state.transfers = vec![transfer(150, PAID, 1), transfer(250, PAID, 2)];
        }));
        let worker = indexer(ledger.clone(), chain.clone(), 100, 10);
        let mut cache = WatchCache::default();

        // The first window's report commits; the second's fails (server down).
        *ledger.fail_range_call.lock().unwrap() = Some(2);
        let failed = worker.pass(&mut cache).await;
        assert!(matches!(failed, Err(PassError::Ledger(_))), "{failed:?}");
        assert!(failed.unwrap_err().retryable());
        assert_eq!(
            ledger.cursor_block(),
            Some(199),
            "the failed window did not commit"
        );

        // Retry: only the failed window is scanned again, and applied once.
        let outcome = worker.pass(&mut cache).await.unwrap();
        assert_eq!(
            outcome,
            Pass::Scanned {
                ranges: 1,
                observations: 1
            }
        );
        assert_eq!(ledger.cursor_block(), Some(299));
        let observed: Vec<u64> = ledger
            .observations()
            .iter()
            .map(|o| o.block_number)
            .collect();
        assert_eq!(observed, vec![150, 250]);
        let windows: Vec<(u64, u64)> = chain
            .state
            .lock()
            .unwrap()
            .log_requests
            .iter()
            .map(|(from, to, _)| (*from, *to))
            .collect();
        assert_eq!(
            windows,
            vec![(100, 199), (200, 299), (200, 299)],
            "the failed window was re-scanned"
        );
    }

    /// With nothing to watch the cursor still follows finality in one
    /// empty range per pass, so the first address bound later does not
    /// trigger a scan from the chain's start block.
    #[tokio::test]
    async fn an_empty_watch_list_advances_the_cursor_without_log_queries() {
        let ledger = Arc::new(FakeLedger::default());
        let chain = Arc::new(MockChain::new(5_000));
        let worker = indexer(ledger.clone(), chain.clone(), 100, 3);
        let mut cache = WatchCache::default();
        assert_eq!(
            worker.pass(&mut cache).await.unwrap(),
            Pass::Scanned {
                ranges: 1,
                observations: 0
            }
        );
        assert_eq!(ledger.cursor_block(), Some(5_000));
        assert!(chain.state.lock().unwrap().log_requests.is_empty());

        ledger.watch(PAID);
        chain.set(|state| {
            state.latest = 5_010;
            state.finalized = 5_010;
        });
        worker.pass(&mut cache).await.unwrap();
        let windows: Vec<(u64, u64)> = chain
            .state
            .lock()
            .unwrap()
            .log_requests
            .iter()
            .map(|(from, to, _)| (*from, *to))
            .collect();
        assert_eq!(windows, vec![(5_001, 5_010)]);
    }
}
