use crate::{
    ledger::IndexerLedger,
    signal::{SignalState, WatchList},
};
use chrono::Utc;
use gum_chain::{ChainError, ChainReader};
use gum_contracts::rpc::{
    ChainFaultReport, FinalizedHeadReport, IndexerCursor, PaymentObservation, RangeApplied,
    RecordFinalizedRange, WatchListQuery,
};
use gum_core::ChainConfig;
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

pub struct Indexer {
    ledger: Arc<dyn IndexerLedger>,
    chain: Arc<dyn ChainReader>,
    config: ChainConfig,
    late_watch: Duration,
    max_ranges: u64,
    poll: Duration,
    reconcile: Duration,
    idle: Duration,
    signal: Arc<SignalState>,
    watch_tx: watch::Sender<WatchList>,
}
impl Indexer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ledger: Arc<dyn IndexerLedger>,
        chain: Arc<dyn ChainReader>,
        config: ChainConfig,
        late_watch: Duration,
        max_ranges: u64,
        poll: Duration,
        reconcile: Duration,
        idle: Duration,
        signal: Arc<SignalState>,
        watch_tx: watch::Sender<WatchList>,
    ) -> Self {
        Self {
            ledger,
            chain,
            config,
            late_watch,
            max_ranges,
            poll,
            reconcile,
            idle,
            signal,
            watch_tx,
        }
    }
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        tracing::info!(chain_id = self.config.chain_id, "chain worker started");
        let mut fingerprint = None;
        let mut addresses = Arc::new(Vec::new());
        loop {
            if *shutdown.borrow() {
                return;
            }
            match self.pass(&mut fingerprint, &mut addresses).await {
                Ok(Pass::Halted) => return,
                Ok(Pass::Restart) => continue,
                Ok(Pass::Done) => {}
                Err(error) => {
                    tracing::warn!(error=%error, retryable=error.retryable(), "reconcile pass failed")
                }
            }
            let cadence = if addresses.is_empty() {
                self.idle
            } else if self.signal.is_connected() {
                self.reconcile
            } else {
                self.poll
            };
            tokio::select! { _=tokio::time::sleep(cadence)=>{}, _=self.signal.notified()=>{let _=self.signal.take_target();}, _=self.signal.health_changed()=>{}, _=shutdown.changed()=>{} }
        }
    }
    async fn pass(
        &self,
        fingerprint: &mut Option<String>,
        addresses: &mut Arc<Vec<alloy_primitives::Address>>,
    ) -> Result<Pass, PassError> {
        let mut cursor = self.ledger.cursor(self.config.chain_id).await?;
        if let Some(c) = cursor {
            let header = self.chain.block_header(c.block).await?;
            if header.hash != c.block_hash {
                let reason = format!("finalized cursor block {} changed hash", c.block);
                self.ledger
                    .report_fault(
                        self.config.chain_id,
                        ChainFaultReport {
                            reason: reason.clone(),
                            block: Some(c.block),
                        },
                    )
                    .await?;
                tracing::error!(
                    block = c.block,
                    error = reason,
                    "chain halted after finality violation"
                );
                return Ok(Pass::Halted);
            }
        }
        let boundary = gum_chain::finality_boundary(
            self.chain.as_ref(),
            self.config.finality_source,
            self.config.finality_confirmations,
        )
        .await?;
        self.ledger
            .report_finalized_head(
                self.config.chain_id,
                FinalizedHeadReport {
                    block: boundary.number,
                    block_hash: boundary.hash,
                    block_timestamp: boundary.timestamp,
                },
            )
            .await?;
        let recent_since =
            Utc::now() - chrono::Duration::from_std(self.late_watch).unwrap_or_default();
        let list = self
            .ledger
            .watch_list(
                self.config.chain_id,
                WatchListQuery {
                    recent_since,
                    known_fingerprint: fingerprint.clone(),
                },
            )
            .await?;
        *fingerprint = Some(list.fingerprint);
        if let Some(value) = list.addresses {
            *addresses = Arc::new(value);
            let _ = self.watch_tx.send(addresses.clone());
        }
        let start = cursor.map_or(self.config.start_block, |c| c.block.saturating_add(1));
        if start > boundary.number {
            return Ok(Pass::Done);
        }
        if addresses.is_empty() {
            return self
                .apply(
                    cursor,
                    boundary.number,
                    boundary.hash,
                    boundary.timestamp,
                    Vec::new(),
                )
                .await;
        }
        let tokens = self.config.token_addresses();
        let mut from = start;
        let mut ranges = 0;
        while from <= boundary.number && ranges < self.max_ranges {
            let to = boundary
                .number
                .min(from.saturating_add(self.config.log_range_size - 1));
            tracing::debug!(
                chain_id = self.config.chain_id,
                from_block = from,
                to_block = to,
                "scanning finalized range"
            );
            let transfers = self
                .chain
                .token_transfers(&tokens, from, to, addresses)
                .await?;
            let header = self.chain.block_header(to).await?;
            let observations=transfers.into_iter().map(|t|{tracing::info!(payment_address=%t.recipient,tx_hash=%t.transaction_hash,amount=%t.amount,"payment observed");PaymentObservation{token:t.token,block_number:t.block_number,block_hash:t.block_hash,block_timestamp:t.block_timestamp,transaction_hash:t.transaction_hash,transaction_index:t.transaction_index,log_index:t.log_index,sender:t.sender,recipient:t.recipient,amount:t.amount}}).collect();
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
        Ok(Pass::Done)
    }
    async fn apply(
        &self,
        expected: Option<IndexerCursor>,
        end: u64,
        hash: alloy_primitives::B256,
        timestamp: u64,
        observations: Vec<PaymentObservation>,
    ) -> Result<Pass, PassError> {
        match self
            .ledger
            .record_finalized_range(
                self.config.chain_id,
                RecordFinalizedRange {
                    expected_cursor: expected,
                    end_block: end,
                    end_block_hash: hash,
                    end_block_timestamp: timestamp,
                    observations,
                },
            )
            .await?
        {
            RangeApplied::CursorMismatch { .. } => Ok(Pass::Restart),
            _ => Ok(Pass::Done),
        }
    }
}
enum Pass {
    Done,
    Restart,
    Halted,
}
#[derive(Debug, thiserror::Error)]
enum PassError {
    #[error(transparent)]
    Chain(#[from] ChainError),
    #[error(transparent)]
    Ledger(#[from] crate::ledger::LedgerError),
}
impl PassError {
    fn retryable(&self) -> bool {
        match self {
            Self::Chain(e) => e.is_retryable(),
            Self::Ledger(e) => e.is_retryable(),
        }
    }
}
