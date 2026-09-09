//! Transfer signal: a WebSocket subscription that wakes the block indexer the
//! moment a finalized USDC transfer to one of our payment addresses exists.
//!
//! The signal carries **no ledger authority**. It never writes an
//! observation; it only records the highest block it saw a matching transfer
//! in and nudges the reconciler, which then runs the same finalized range
//! scan it runs on its timer. A missed, duplicated, or reordered notification
//! therefore costs latency, never correctness, which is what lets the
//! reconciler run at a slow cadence (one `eth_getLogs` per hundred blocks)
//! while payments are still detected within a block of finality.
//!
//! On Monad the subscription is `monadLogs`, which re-delivers each log as
//! the block advances `Proposed -> Voted -> Finalized -> Verified`; only the
//! finalized states wake the reconciler. Nodes without it (Anvil) fall back
//! to the standard `logs` subscription, which fires at proposal; the
//! reconciler's wake handling then waits for the block to finalize.
//!
//! Notifications are billed per message by the provider, so the filter is as
//! narrow as the node allows: the USDC contract, the `Transfer` topic, and
//! the recipient set in `topics[2]`. Every other USDC transfer on the chain
//! stays on the node.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use alloy_primitives::{Address, B256, U64, keccak256};
use alloy_provider::{DynProvider, Provider, ProviderBuilder};
use alloy_rpc_types_eth::Filter;
use alloy_transport_ws::WsConnect;
use futures::StreamExt;
use futures::stream::{BoxStream, SelectAll};
use serde::Deserialize;
use tokio::sync::{Notify, watch};
use tracing::{debug, info, warn};

use crate::chain::redact_urls;

/// Recipients per subscription. Each subscription is one `eth_subscribe`
/// request carrying the whole OR-list in `topics[2]`; splitting keeps every
/// request comfortably inside provider payload limits, and a change to one
/// chunk of the watch list only re-subscribes that chunk's worth of
/// addresses. Measured against Monad's public endpoint: 1,000- and 5,000-address
/// lists subscribe fine, so 500 leaves a wide margin.
pub const SUBSCRIPTION_CHUNK: usize = 500;
/// A read over the socket at this cadence proves the connection is live and
/// keeps provider idle timers from closing it. Thirty seconds is one call per
/// keepalive; the reconciler's own cadence is a minute.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const RECONNECT_BACKOFF_BASE: Duration = Duration::from_secs(1);
const RECONNECT_BACKOFF_CAP: Duration = Duration::from_secs(30);

/// The shared, lock-free seam between the signal task and the reconciler.
///
/// Wakes coalesce: a burst of notifications for one block collapses into a
/// single "highest target block" and one pending permit, so the reconciler
/// runs one pass per burst, not one per message.
#[derive(Default)]
pub struct SignalState {
    /// Highest block a matching transfer was reported in; 0 means none
    /// pending. Blocks are never 0 in practice (the USDC start block is far
    /// past genesis), so the sentinel is safe.
    target: AtomicU64,
    wake: Notify,
    connected: AtomicBool,
}

impl SignalState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Record a matching transfer at `block` and wake the reconciler.
    pub fn wake(&self, block: u64) {
        self.target.fetch_max(block, Ordering::AcqRel);
        self.wake.notify_one();
    }

    /// Consume the pending target block, if any.
    pub fn take_target(&self) -> Option<u64> {
        let block = self.target.swap(0, Ordering::AcqRel);
        (block > 0).then_some(block)
    }

    /// Resolves once a wake is pending. `Notify` stores one permit, so a
    /// wake that arrives while the reconciler is busy is not lost.
    pub async fn notified(&self) {
        self.wake.notified().await;
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::Release);
    }
}

/// The recipient list the signal subscribes to, replaced wholesale on change.
pub type WatchList = Arc<Vec<Address>>;

/// Node-side commit state of a `monadLogs` notification. Absent on the
/// standard `logs` subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
enum CommitState {
    Proposed,
    Voted,
    Finalized,
    Verified,
    #[serde(other)]
    Unknown,
}

/// The three fields a notification needs to become a wake-up; everything
/// else in the payload is skipped by serde without being materialised.
#[derive(Debug, Deserialize)]
struct SignalLog {
    #[serde(rename = "blockNumber")]
    block_number: Option<U64>,
    #[serde(rename = "commitState", default)]
    commit_state: Option<CommitState>,
    #[serde(default)]
    removed: bool,
}

impl SignalLog {
    /// The block to wake the reconciler for, if this notification is one
    /// worth acting on: a finalized `monadLogs` state, or any standard `logs`
    /// delivery (the reconciler waits for finality itself).
    fn wake_block(&self) -> Option<u64> {
        if self.removed {
            return None;
        }
        let finalized = match self.commit_state {
            None | Some(CommitState::Finalized) | Some(CommitState::Verified) => true,
            Some(CommitState::Proposed) | Some(CommitState::Voted) | Some(CommitState::Unknown) => {
                false
            }
        };
        finalized
            .then_some(self.block_number?)
            .map(|number| number.to::<u64>())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubscriptionKind {
    MonadLogs,
    Logs,
}

impl SubscriptionKind {
    fn method(self) -> &'static str {
        match self {
            Self::MonadLogs => "monadLogs",
            Self::Logs => "logs",
        }
    }
}

/// Build the subscription filter for one chunk of the watch list.
fn transfer_filter(token: Address, recipients: &[Address]) -> Filter {
    Filter::new()
        .address(token)
        .event_signature(keccak256("Transfer(address,address,uint256)"))
        .topic2(
            recipients
                .iter()
                .map(|address| address.into_word())
                .collect::<Vec<B256>>(),
        )
}

enum SessionEnd {
    Shutdown,
    Failed(String),
}

struct ActiveSubscriptions {
    ids: Vec<B256>,
    streams: SelectAll<BoxStream<'static, SignalLog>>,
}

/// The production signal: one WebSocket to the RPC endpoint, re-established
/// with backoff for as long as the process runs.
pub struct TransferSignal {
    ws_url: String,
    chain_id: u64,
    token: Address,
    fallback_logged: AtomicBool,
}

impl TransferSignal {
    pub fn new(ws_url: String, chain_id: u64, token: Address) -> Self {
        Self {
            ws_url,
            chain_id,
            token,
            fallback_logged: AtomicBool::new(false),
        }
    }

    /// Run until `shutdown` flips. Never returns an error: a signal that
    /// cannot connect leaves `state` disconnected, and the reconciler falls
    /// back to its polling cadence.
    pub async fn run(
        self,
        mut watch_list: watch::Receiver<WatchList>,
        state: Arc<SignalState>,
        mut shutdown: watch::Receiver<bool>,
    ) {
        let mut backoff = RECONNECT_BACKOFF_BASE;
        loop {
            if *shutdown.borrow() {
                return;
            }
            let connected_at = tokio::time::Instant::now();
            match self.session(&mut watch_list, &state, &mut shutdown).await {
                SessionEnd::Shutdown => return,
                SessionEnd::Failed(reason) => {
                    let was_connected = state.is_connected();
                    state.set_connected(false);
                    // A session that held for a while earned a fresh backoff;
                    // a session that died at once keeps growing it.
                    if connected_at.elapsed() > RECONNECT_BACKOFF_CAP {
                        backoff = RECONNECT_BACKOFF_BASE;
                    }
                    if was_connected {
                        warn!(
                            reason,
                            backoff_ms = backoff.as_millis() as u64,
                            "transfer signal disconnected"
                        );
                    } else {
                        warn!(
                            reason,
                            backoff_ms = backoff.as_millis() as u64,
                            "transfer signal connection failed"
                        );
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = shutdown.changed() => return,
                    }
                    backoff = (backoff * 2).min(RECONNECT_BACKOFF_CAP);
                }
            }
        }
    }

    async fn connect(&self) -> Result<DynProvider, String> {
        // The pubsub service can reconnect on its own, but then a dropped
        // socket is invisible to the health flag and its resubscription races
        // ours. Zero retries makes every drop end the session, and `run`
        // owns reconnection with backoff.
        let connect = WsConnect::new(self.ws_url.clone()).with_max_retries(0);
        let provider =
            tokio::time::timeout(CONNECT_TIMEOUT, ProviderBuilder::new().connect_ws(connect))
                .await
                .map_err(|_| "websocket connect timed out".to_string())?
                .map_err(|error| redact_urls(&error.to_string()))?
                .erased();
        let chain_id = tokio::time::timeout(KEEPALIVE_TIMEOUT, provider.get_chain_id())
            .await
            .map_err(|_| "eth_chainId over websocket timed out".to_string())?
            .map_err(|error| redact_urls(&error.to_string()))?;
        if chain_id != self.chain_id {
            return Err(format!(
                "websocket endpoint serves chain {chain_id}, expected {}",
                self.chain_id
            ));
        }
        Ok(provider)
    }

    async fn subscribe_chunk(
        &self,
        provider: &DynProvider,
        kind: SubscriptionKind,
        chunk: &[Address],
    ) -> Result<(B256, BoxStream<'static, SignalLog>), String> {
        let subscription = provider
            .subscribe::<_, SignalLog>((kind.method(), transfer_filter(self.token, chunk)))
            .await
            .map_err(|error| redact_urls(&error.to_string()))?;
        let id = subscription.local_id().to_owned();
        Ok((id, subscription.into_stream().boxed()))
    }

    /// Subscribe every chunk of `list`, discovering on the first chunk whether
    /// the node speaks `monadLogs`.
    async fn subscribe_all(
        &self,
        provider: &DynProvider,
        kind: &mut SubscriptionKind,
        list: &[Address],
    ) -> Result<ActiveSubscriptions, String> {
        let mut active = ActiveSubscriptions {
            ids: Vec::new(),
            streams: SelectAll::new(),
        };
        for chunk in list.chunks(SUBSCRIPTION_CHUNK) {
            let subscribed = match self.subscribe_chunk(provider, *kind, chunk).await {
                Ok(subscribed) => subscribed,
                Err(error) if *kind == SubscriptionKind::MonadLogs && active.ids.is_empty() => {
                    if !self.fallback_logged.swap(true, Ordering::AcqRel) {
                        info!(
                            error,
                            "node rejected monadLogs; using the standard logs subscription"
                        );
                    }
                    *kind = SubscriptionKind::Logs;
                    self.subscribe_chunk(provider, *kind, chunk).await?
                }
                Err(error) => return Err(error),
            };
            active.ids.push(subscribed.0);
            active.streams.push(subscribed.1);
        }
        Ok(active)
    }

    async fn unsubscribe_all(&self, provider: &DynProvider, ids: Vec<B256>) {
        for id in ids {
            if let Err(error) = provider.unsubscribe(id).await {
                debug!(error = %redact_urls(&error.to_string()), "eth_unsubscribe failed; the session will drop it");
            }
        }
    }

    /// One connection's lifetime: subscribe, forward wakes, keep the socket
    /// alive, follow watch-list changes. Returns when the socket dies or the
    /// process shuts down.
    async fn session(
        &self,
        watch_list: &mut watch::Receiver<WatchList>,
        state: &SignalState,
        shutdown: &mut watch::Receiver<bool>,
    ) -> SessionEnd {
        let provider = match self.connect().await {
            Ok(provider) => provider,
            Err(error) => return SessionEnd::Failed(error),
        };
        let mut kind = SubscriptionKind::MonadLogs;
        let mut list = watch_list.borrow_and_update().clone();
        let mut active = match self.subscribe_all(&provider, &mut kind, &list).await {
            Ok(active) => active,
            Err(error) => return SessionEnd::Failed(error),
        };
        state.set_connected(true);
        info!(
            subscription = kind.method(),
            watched_addresses = list.len(),
            subscriptions = active.ids.len(),
            "transfer signal connected"
        );

        let mut keepalive = tokio::time::interval(KEEPALIVE_INTERVAL);
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        keepalive.tick().await;
        loop {
            tokio::select! {
                // An empty watch list has no streams; polling an empty
                // `SelectAll` would report the session as ended.
                item = active.streams.next(), if !active.ids.is_empty() => {
                    match item {
                        Some(log) => {
                            if let Some(block) = log.wake_block() {
                                debug!(block, "transfer signal wake");
                                state.wake(block);
                            }
                        }
                        None => return SessionEnd::Failed("subscription stream ended".to_string()),
                    }
                }
                _ = keepalive.tick() => {
                    match tokio::time::timeout(KEEPALIVE_TIMEOUT, provider.get_chain_id()).await {
                        Ok(Ok(_)) => {}
                        Ok(Err(error)) => return SessionEnd::Failed(redact_urls(&error.to_string())),
                        Err(_) => return SessionEnd::Failed("keepalive timed out".to_string()),
                    }
                }
                changed = watch_list.changed() => {
                    if changed.is_err() {
                        return SessionEnd::Shutdown;
                    }
                    let next = watch_list.borrow_and_update().clone();
                    if next == list {
                        continue;
                    }
                    // Subscribe the new set before dropping the old one so
                    // no transfer lands in a gap between the two.
                    let replacement = match self.subscribe_all(&provider, &mut kind, &next).await {
                        Ok(replacement) => replacement,
                        Err(error) => return SessionEnd::Failed(error),
                    };
                    let previous = std::mem::replace(&mut active, replacement);
                    self.unsubscribe_all(&provider, previous.ids).await;
                    info!(watched_addresses = next.len(), subscriptions = active.ids.len(), "transfer signal watch list updated");
                    list = next;
                }
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return SessionEnd::Shutdown;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    /// A `monadLogs` notification exactly as Monad's node delivered it.
    const MONAD_LOG: &str = r#"{"blockId":"0x0e8297f58b5a8ed8723abf21a3b98530b93baf2e23d9702e3bff0f4a289396e3","commitState":"Verified","address":"0x754704bc059f8c67012fed69bc8a327a5aafb603","topics":["0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef","0x0000000000000000000000005afd3ec861f6104af26e8755abcc1f876de77620","0x000000000000000000000000fb78fcae443eb423b59b8c186518c5df94416344"],"data":"0x0000000000000000000000000000000000000000000000000000000006b84654","blockHash":"0x108626b210fd60110e11fa17da483cb0cb77db0b956699dc92ef48967b7fcfca","blockNumber":"0x6277b15","blockTimestamp":"0x6aa0f907","transactionHash":"0xe53927aa36da32acb1e06d0a910b8119595eb59fd504f3cbbd84d8ac4b639e26","transactionIndex":"0x3","logIndex":"0x2f","removed":false}"#;

    fn with_state(state: &str) -> SignalLog {
        serde_json::from_str(&MONAD_LOG.replace("\"Verified\"", &format!("\"{state}\""))).unwrap()
    }

    #[test]
    fn only_finalized_commit_states_wake_the_reconciler() {
        assert_eq!(with_state("Verified").wake_block(), Some(0x6277b15));
        assert_eq!(with_state("Finalized").wake_block(), Some(0x6277b15));
        assert_eq!(with_state("Proposed").wake_block(), None);
        assert_eq!(with_state("Voted").wake_block(), None);
        assert_eq!(with_state("SomethingNew").wake_block(), None);
    }

    #[test]
    fn standard_logs_without_commit_state_wake_unless_removed() {
        let standard: SignalLog = serde_json::from_str(
            r#"{"address":"0x754704bc059f8c67012fed69bc8a327a5aafb603","blockNumber":"0x10","removed":false,"topics":[]}"#,
        )
        .unwrap();
        assert_eq!(standard.wake_block(), Some(16));
        let removed: SignalLog =
            serde_json::from_str(r#"{"blockNumber":"0x10","removed":true}"#).unwrap();
        assert_eq!(removed.wake_block(), None);
        let pending: SignalLog = serde_json::from_str(r#"{"blockNumber":null}"#).unwrap();
        assert_eq!(pending.wake_block(), None);
    }

    #[test]
    fn wakes_coalesce_to_the_highest_block() {
        let state = SignalState::new();
        assert_eq!(state.take_target(), None);
        state.wake(10);
        state.wake(12);
        state.wake(11);
        assert_eq!(state.take_target(), Some(12));
        assert_eq!(state.take_target(), None);
    }

    #[test]
    fn filter_narrows_to_token_transfer_and_recipient_set() {
        let token = address!("0x754704Bc059F8C67012fEd69BC8A327a5aafb603");
        let a = address!("0x0000000000000000000000000000000000000001");
        let b = address!("0x0000000000000000000000000000000000000002");
        let json = serde_json::to_value(transfer_filter(token, &[a, b])).unwrap();
        assert_eq!(
            json["address"],
            serde_json::json!("0x754704bc059f8c67012fed69bc8a327a5aafb603")
        );
        let topics = json["topics"].as_array().unwrap();
        assert_eq!(
            topics[0],
            serde_json::json!("0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef")
        );
        assert!(topics[1].is_null(), "sender is unconstrained");
        let recipients = topics[2].as_array().unwrap();
        assert_eq!(recipients.len(), 2);
        assert!(recipients.contains(&serde_json::json!(a.into_word())));
        assert!(recipients.contains(&serde_json::json!(b.into_word())));
        assert!(json.get("fromBlock").is_none());
    }
}
