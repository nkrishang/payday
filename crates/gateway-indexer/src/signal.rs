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
use alloy_pubsub::{ConnectionHandle, PubSubConnect};
use alloy_rpc_client::{ClientBuilder, RpcClient};
use alloy_rpc_types_eth::Filter;
use alloy_transport::{TransportErrorKind, TransportResult};
use alloy_transport_ws::WsConnect;
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
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
/// Bound on one `eth_subscribe` round trip. Subscription establishment must
/// never wedge the session: a server that answers pings but never answers
/// the subscription request would otherwise hang the session loop without a
/// failure, leaving `connected` true while the socket serves nothing. Every
/// subscribe is timed, and a timeout fails the session into the reconnect
/// path.
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(15);
/// Notifications are forwarded from one task per subscription chunk to the
/// session through this channel. Depth only smooths bursts; a wake is a
/// block number, so a deep backlog is worthless and the reconciler's timer
/// covers anything dropped.
const SIGNAL_CHANNEL_DEPTH: usize = 256;
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
    /// Wakes the reconciler when a healthy signal drops, so it can fall back
    /// to its polling cadence now instead of at the next scheduled pass.
    health: Notify,
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
        let previous = self.connected.swap(connected, Ordering::AcqRel);
        if previous && !connected {
            // A drop is urgent: the reconciler must not wait out a reconcile
            // interval that was scheduled while the signal was still healthy.
            self.health.notify_one();
        }
    }

    /// Resolves when the signal's health changes in a way the reconciler must
    /// react to (currently: a connected signal disconnected).
    pub async fn health_changed(&self) {
        self.health.notified().await;
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
    /// The watch list emptied: nothing to subscribe to, so the socket is
    /// closed rather than kept alive for nothing. Not a failure: no backoff.
    Idle,
}

/// One chunk's forwarded notifications. `Ended` is sent when the
/// subscription stream ends while the session is still running: the server
/// dropped the subscription or the socket died underneath it. It carries the
/// chunk's local id and generation, because a chunk replaced moments before
/// its abort lands can still have one queued. Identity is the pair, not the
/// id alone: Alloy derives the local id from the request's parameters, so
/// re-subscribing an identical chunk yields the same id, and the replaced
/// chunk's queued dying gasp must not be mistaken for the new chunk's. An
/// aborted task usually sends nothing.
enum ChunkMessage {
    Log(SignalLog),
    Ended(B256, u64),
}

struct ActiveChunk {
    /// The addresses this chunk subscribes to. Subscription identity on the
    /// wire is the full request, and everything but this recipient list is
    /// constant, so address-list equality is subscription equality.
    addresses: Vec<Address>,
    id: B256,
    /// Session-unique incarnation of this subscription. Kept with the chunk
    /// across refreshes: a retained chunk stays the same generation.
    generation: u64,
    /// Forwards notifications into the session channel until the stream ends
    /// or the chunk is replaced.
    task: JoinHandle<()>,
}

/// The session's live chunks. The channel is owned by the session and
/// outlives every refresh: a chunk retained across a watch-list change keeps
/// its forwarding task, and that task's sender must keep feeding the
/// receiver the session polls, so a refresh may only add tasks on the same
/// channel, never replace it.
struct ActiveSubscriptions {
    chunks: Vec<ActiveChunk>,
    tx: mpsc::Sender<ChunkMessage>,
}

/// Does this termination belong to a chunk the session still holds? Identity
/// is the (subscription id, generation) pair: Alloy derives the id from the
/// request's parameters, so re-subscribing an identical chunk repeats the id,
/// and a replaced chunk's queued dying gasp must not fail the session that
/// just re-added it.
fn ending_is_current(chunks: &[ActiveChunk], id: B256, generation: u64) -> bool {
    chunks
        .iter()
        .any(|chunk| chunk.id == id && chunk.generation == generation)
}

/// The production signal: one WebSocket to the RPC endpoint, re-established
/// with backoff for as long as the process runs.
pub struct TransferSignal {
    ws_url: String,
    chain_id: u64,
    token: Address,
    fallback_logged: AtomicBool,
    /// Source of session-unique chunk generations. Process-unique is more
    /// than the session needs, but it costs nothing and keeps every chunk
    /// distinct across reconnects.
    next_generation: AtomicU64,
}

/// A [`WsConnect`] whose pubsub service is forbidden from reconnecting on its
/// own. Alloy's pubsub service attempts one internal reconnect before it even
/// checks its retry limit, and a successful one is invisible to the
/// application: it re-subscribes server-side without returning any
/// acknowledgement, so a failed resubscription would leave a socket that
/// answers `eth_chainId` while every subscription stream is dead. Refusing
/// `try_reconnect` makes every drop end the service, which surfaces here as
/// a failed keepalive and lets `TransferSignal::run` own reconnection with
/// its backoff and health reporting.
struct FailFastReconnect {
    inner: WsConnect,
}

impl PubSubConnect for FailFastReconnect {
    fn is_local(&self) -> bool {
        self.inner.is_local()
    }

    async fn connect(&self) -> TransportResult<ConnectionHandle> {
        self.inner.connect().await
    }

    async fn try_reconnect(&self) -> TransportResult<ConnectionHandle> {
        Err(TransportErrorKind::custom_str(
            "websocket closed; the transfer signal owns reconnection",
        ))
    }
}

/// Whether an `eth_subscribe` rejection means the node does not support the
/// subscription kind, as opposed to a transient failure or a filter problem.
/// Only a rejection of the kind itself justifies falling back to the standard
/// `logs` subscription.
fn is_unsupported_subscription(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("-32601")
        || error.contains("method not found")
        || error.contains("invalid method")
        || error.contains("unknown method")
        // Anvil-style nodes that do not know the kind reject the whole
        // request as invalid params: either naming the variant ("unknown
        // variant `monadLogs`") or, when the filter carries a recipient
        // list, failing to match the request against any call it knows
        // ("did not match any variant of untagged enum EthRpcCall"). A
        // genuine filter problem is reported by the handler, never as a
        // request that parses into nothing, so this stays narrow.
        || (error.contains("-32602")
            && (error.contains("unknown variant") || error.contains("did not match any variant")))
}

impl TransferSignal {
    pub fn new(ws_url: String, chain_id: u64, token: Address) -> Self {
        Self {
            ws_url,
            chain_id,
            token,
            fallback_logged: AtomicBool::new(false),
            next_generation: AtomicU64::new(0),
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
            // No socket while there is nothing to watch: a chain with no open
            // bound request costs no keepalives and no notifications. The
            // reconciler keeps its own (idle) cadence meanwhile.
            if watch_list.borrow_and_update().is_empty() {
                tokio::select! {
                    changed = watch_list.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        continue;
                    }
                    _ = shutdown.changed() => return,
                }
            }
            let connected_at = tokio::time::Instant::now();
            match self.session(&mut watch_list, &state, &mut shutdown).await {
                SessionEnd::Shutdown => return,
                SessionEnd::Idle => {
                    state.set_connected(false);
                    backoff = RECONNECT_BACKOFF_BASE;
                    info!("transfer signal closed: nothing to watch");
                }
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

    async fn connect(&self) -> Result<RpcClient, String> {
        // Reconnection is owned by `run`. `FailFastReconnect` refuses the
        // pubsub service's internal reconnect, so a dropped socket tears the
        // service down and surfaces as a failed session, instead of Alloy
        // silently re-establishing the socket and re-subscribing on its own
        // (which would bypass the connection-time chain-id check, the health
        // flag, and this loop's backoff, and whose resubscription failures
        // are invisible to the application).
        let client = tokio::time::timeout(
            CONNECT_TIMEOUT,
            ClientBuilder::default().pubsub(FailFastReconnect {
                // Zero retries and the refusing `try_reconnect` each close
                // half the gap: the override makes even a successful internal
                // reconnect impossible, and zero retries makes the refusing
                // reconnect fail the service immediately instead of retrying
                // it on Alloy's schedule (ten attempts, three seconds apart)
                // while the health flag still says connected.
                inner: WsConnect::new(self.ws_url.clone()).with_max_retries(0),
            }),
        )
        .await
        .map_err(|_| "websocket connect timed out".to_string())?
        .map_err(|error| redact_urls(&error.to_string()))?;
        let chain_id: U64 =
            tokio::time::timeout(KEEPALIVE_TIMEOUT, client.request("eth_chainId", ()))
                .await
                .map_err(|_| "eth_chainId over websocket timed out".to_string())?
                .map_err(|error| redact_urls(&error.to_string()))?;
        if chain_id.to::<u64>() != self.chain_id {
            return Err(format!(
                "websocket endpoint serves chain {}, expected {}",
                chain_id, self.chain_id
            ));
        }
        Ok(client)
    }

    async fn subscribe_chunk(
        &self,
        client: &RpcClient,
        kind: SubscriptionKind,
        chunk: &[Address],
        tx: mpsc::Sender<ChunkMessage>,
    ) -> Result<ActiveChunk, String> {
        // Identity of this incarnation, distinct even from a chunk with the
        // same subscription id (Alloy derives the id from the request, so an
        // identical re-subscription would repeat it).
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        // Subscribed through the raw client rather than the provider's typed
        // `subscribe`: the raw subscription surfaces malformed payloads and
        // broadcast lag, which the typed stream would log at debug level and
        // silently drop.
        let mut call = client.request::<(&'static str, Filter), B256>(
            "eth_subscribe",
            (kind.method(), transfer_filter(self.token, chunk)),
        );
        call.set_is_subscription();
        let id = tokio::time::timeout(SUBSCRIBE_TIMEOUT, call)
            .await
            .map_err(|_| "eth_subscribe timed out".to_string())?
            .map_err(|error| redact_urls(&error.to_string()))?;
        let frontend = client
            .pubsub_frontend()
            .ok_or("rpc client has no pubsub frontend".to_string())?;
        let mut subscription =
            tokio::time::timeout(SUBSCRIBE_TIMEOUT, frontend.get_subscription(id))
                .await
                .map_err(|_| "subscription receiver timed out".to_string())?
                .map_err(|error| redact_urls(&error.to_string()))?;
        let task = tokio::spawn(async move {
            loop {
                match subscription.recv().await {
                    Ok(value) => match serde_json::from_str::<SignalLog>(value.get()) {
                        Ok(log) => {
                            if tx.send(ChunkMessage::Log(log)).await.is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            // A malformed notification says nothing about the
                            // socket; skip it. The signal has no ledger
                            // authority, so a decode failure costs only
                            // latency.
                            warn!(error = %redact_urls(&error.to_string()), "transfer signal dropped an undecodable notification");
                        }
                    },
                    Err(RecvError::Lagged(missed)) => {
                        warn!(
                            missed,
                            "transfer signal fell behind the provider; the reconciler's timer covers any missed wake"
                        );
                    }
                    // Natural end: the provider is gone or the server dropped
                    // the subscription. Tell the session, which fails into its
                    // reconnect path if the chunk is still current. (An
                    // aborted task usually never gets here, and the session
                    // ignores `Ended` for chunks it already replaced — matched
                    // by generation, since an identical re-subscription would
                    // otherwise share the id.)
                    Err(RecvError::Closed) => break,
                }
            }
            let _ = tx.send(ChunkMessage::Ended(id, generation)).await;
        });
        Ok(ActiveChunk {
            addresses: chunk.to_vec(),
            id,
            generation,
            task,
        })
    }

    /// Subscribe one chunk, discovering on the session's first chunk whether
    /// the node speaks `monadLogs`. Used by both session establishment and
    /// the watch-list refresh, because a session that started with an empty
    /// watch list meets its first subscription only on a refresh — and that
    /// first rejection must fall back, not fail the session forever.
    async fn subscribe_chunk_discovering(
        &self,
        client: &RpcClient,
        kind: &mut SubscriptionKind,
        chunk: &[Address],
        tx: &mpsc::Sender<ChunkMessage>,
        first_of_session: bool,
    ) -> Result<ActiveChunk, String> {
        match self.subscribe_chunk(client, *kind, chunk, tx.clone()).await {
            Ok(subscribed) => Ok(subscribed),
            Err(error)
                if *kind == SubscriptionKind::MonadLogs
                    && first_of_session
                    && is_unsupported_subscription(&error) =>
            {
                // Fall back only when the node rejects the subscription
                // kind itself: falling back on a transient failure would
                // mask the real problem and quietly change the latency
                // characteristics of every payment.
                if !self.fallback_logged.swap(true, Ordering::AcqRel) {
                    info!(
                        error,
                        "node rejected monadLogs; using the standard logs subscription"
                    );
                }
                *kind = SubscriptionKind::Logs;
                self.subscribe_chunk(client, *kind, chunk, tx.clone()).await
            }
            Err(error) => Err(error),
        }
    }

    /// Subscribe every chunk of `list`, discovering on the first chunk whether
    /// the node speaks `monadLogs`.
    async fn subscribe_all(
        &self,
        client: &RpcClient,
        kind: &mut SubscriptionKind,
        list: &[Address],
        tx: &mpsc::Sender<ChunkMessage>,
    ) -> Result<ActiveSubscriptions, String> {
        let mut active = ActiveSubscriptions {
            chunks: Vec::new(),
            tx: tx.clone(),
        };
        for chunk in list.chunks(SUBSCRIPTION_CHUNK) {
            let subscribed = self
                .subscribe_chunk_discovering(
                    client,
                    kind,
                    chunk,
                    &active.tx,
                    active.chunks.is_empty(),
                )
                .await?;
            active.chunks.push(subscribed);
        }
        Ok(active)
    }

    async fn unsubscribe_all(&self, client: &RpcClient, ids: Vec<B256>) {
        let Ok(frontend) = client.pubsub_frontend().ok_or(()) else {
            return;
        };
        for id in ids {
            if let Err(error) = frontend.unsubscribe(id) {
                debug!(error = %redact_urls(&error.to_string()), "eth_unsubscribe failed; the session will drop it");
            }
        }
    }

    /// Wake the reconciler right after this session's subscriptions became
    /// live. A subscription only covers transfers from the moment the node
    /// accepted it: a payment that raced the session's establishment (an
    /// indexer restart with requests already bound), a socket drop, or a
    /// watch-list refresh that just added a freshly bound address was never
    /// notified, and without help it would sit unnoticed until the
    /// reconciler's timer. Read the provider's tip and wake with it: the
    /// reconciler's catch-up keeps passing until the cursor covers that
    /// block, and every transfer the subscription could have missed is at or
    /// below it. A failed read only costs latency — the timer covers the
    /// gap — so it warns rather than fails the session.
    async fn wake_unnotified_history(&self, client: &RpcClient, state: &SignalState) {
        let read = tokio::time::timeout(
            KEEPALIVE_TIMEOUT,
            client.request::<(), U64>("eth_blockNumber", ()),
        )
        .await;
        match read {
            Ok(Ok(tip)) => {
                let block = tip.to::<u64>();
                info!(
                    block,
                    "transfer signal subscriptions are live; waking the reconciler to cover any transfer they could have missed"
                );
                state.wake(block);
            }
            Ok(Err(error)) => warn!(
                error = %redact_urls(&error.to_string()),
                "could not read the tip after subscribing; the reconciler's timer covers any earlier transfer"
            ),
            Err(_) => warn!(
                "reading the tip after subscribing timed out; the reconciler's timer covers any earlier transfer"
            ),
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
        let client = match self.connect().await {
            Ok(client) => client,
            Err(error) => return SessionEnd::Failed(error),
        };
        let mut kind = SubscriptionKind::MonadLogs;
        let mut list = watch_list.borrow_and_update().clone();
        // One channel per session: chunks retained across a watch-list change
        // keep forwarding into it, so the receiver never changes hands.
        let (tx, mut rx) = mpsc::channel(SIGNAL_CHANNEL_DEPTH);
        let mut active = match self.subscribe_all(&client, &mut kind, &list, &tx).await {
            Ok(active) => active,
            Err(error) => return SessionEnd::Failed(error),
        };
        state.set_connected(true);
        info!(
            subscription = kind.method(),
            watched_addresses = list.len(),
            subscriptions = active.chunks.len(),
            "transfer signal connected"
        );
        if !list.is_empty() {
            self.wake_unnotified_history(&client, state).await;
        }

        let mut keepalive = tokio::time::interval(KEEPALIVE_INTERVAL);
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        keepalive.tick().await;
        loop {
            tokio::select! {
                // An empty watch list has no chunks; receiving from the
                // channel would report the session as ended.
                message = rx.recv(), if !active.chunks.is_empty() => {
                    match message {
                        Some(ChunkMessage::Log(log)) => {
                            if let Some(block) = log.wake_block() {
                                info!(block, "transfer signal wake");
                                state.wake(block);
                            }
                        }
                        Some(ChunkMessage::Ended(id, generation)) => {
                            if ending_is_current(&active.chunks, id, generation) {
                                // A current chunk's stream ending means the
                                // server dropped the subscription or the
                                // socket died; fail the session into its
                                // reconnect path. The generation matters:
                                // Alloy derives the id from the request, so a
                                // chunk replaced and re-added identically
                                // repeats the id, and the replaced chunk's
                                // queued dying gasp must not be mistaken for
                                // the new chunk's.
                                return SessionEnd::Failed("subscription stream ended".to_string());
                            }
                            // An already-replaced chunk's dying gasp: its
                            // abort raced the message queue. Ignore it.
                        }
                        None => {
                            return SessionEnd::Failed("subscription channel closed".to_string())
                        }
                    }
                }
                _ = keepalive.tick() => {
                    // The keepalive is also the periodic chain-id check: an
                    // endpoint that silently moved chains is not the one the
                    // filter and the reconciler's assumptions were built on.
                    let read = tokio::time::timeout(
                        KEEPALIVE_TIMEOUT,
                        client.request::<(), U64>("eth_chainId", ()),
                    )
                    .await;
                    match read {
                        Ok(Ok(id)) if id.to::<u64>() == self.chain_id => {}
                        Ok(Ok(id)) => {
                            return SessionEnd::Failed(format!(
                                "websocket endpoint now serves chain {id}, expected {}",
                                self.chain_id
                            ))
                        }
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
                    if next.is_empty() {
                        let ids: Vec<B256> = active.chunks.iter().map(|chunk| chunk.id).collect();
                        for chunk in &active.chunks {
                            chunk.task.abort();
                        }
                        self.unsubscribe_all(&client, ids).await;
                        return SessionEnd::Idle;
                    }
                    // Diff by chunk, and never subscribe-then-unsubscribe an
                    // unchanged chunk: Alloy keys a subscription by its full
                    // request, so a replacement chunk identical to the old
                    // one shares the old subscription's local id, and
                    // unsubscribing that id would kill the freshly
                    // subscribed stream for good. Retained chunks keep
                    // streaming into the same session channel, so no
                    // transfer lands in a gap.
                    let mut replacement = ActiveSubscriptions {
                        chunks: Vec::new(),
                        tx: tx.clone(),
                    };
                    // A session that started with an empty watch list has
                    // never exercised its subscription kind; its first
                    // refresh must discover, exactly like establishment.
                    let mut discover = active.chunks.is_empty();
                    for chunk in next.chunks(SUBSCRIPTION_CHUNK).map(<[Address]>::to_vec) {
                        if *shutdown.borrow() {
                            return SessionEnd::Shutdown;
                        }
                        if let Some(index) = active
                            .chunks
                            .iter()
                            .position(|active| active.addresses == chunk)
                        {
                            let retained = active.chunks.remove(index);
                            replacement.chunks.push(retained);
                            continue;
                        }
                        let subscribed = match self
                            .subscribe_chunk_discovering(
                                &client,
                                &mut kind,
                                &chunk,
                                &replacement.tx,
                                discover,
                            )
                            .await
                        {
                            Ok(subscribed) => subscribed,
                            // The new set is subscribed before the old one is
                            // dropped, but an error still fails the whole
                            // session: a half-updated subscription set is
                            // not a state worth trying to salvage.
                            Err(error) => return SessionEnd::Failed(error),
                        };
                        discover = false;
                        replacement.chunks.push(subscribed);
                    }
                    let previous = std::mem::replace(&mut active, replacement);
                    let removed = previous.chunks.len();
                    let removed_ids: Vec<B256> =
                        previous.chunks.iter().map(|chunk| chunk.id).collect();
                    for chunk in &previous.chunks {
                        // Aborting the task drops its stream receiver with
                        // it; the server-side subscription goes with the
                        // unsubscribe below.
                        chunk.task.abort();
                    }
                    self.unsubscribe_all(&client, removed_ids).await;
                    info!(watched_addresses = next.len(), subscriptions = active.chunks.len(), removed_subscriptions = removed, "transfer signal watch list updated");
                    // The addresses this refresh just subscribed were not
                    // covered by any subscription while the request was being
                    // bound; a payment in that window was never notified.
                    self.wake_unnotified_history(&client, state).await;
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

    #[tokio::test]
    async fn a_replaced_chunks_queued_ending_does_not_fail_the_reeadded_chunk() {
        // The ABA race: a chunk is removed, its abort races its `Ended` into
        // the queue, and the watch list later re-adds the identical chunk —
        // which Alloy ids identically, because the id is the request's
        // params hash. The stale `Ended` must be ignored; only the new
        // chunk's own ending may fail the session.
        let id = B256::from([7u8; 32]);
        let old_chunk = ActiveChunk {
            addresses: vec![address!("0x0000000000000000000000000000000000000001")],
            id,
            generation: 1,
            task: tokio::spawn(async {}),
        };
        let readded_chunk = ActiveChunk {
            addresses: vec![address!("0x0000000000000000000000000000000000000001")],
            id,
            generation: 2,
            task: tokio::spawn(async {}),
        };
        // The replaced chunk's generation, arriving after re-add: stale.
        assert!(!ending_is_current(&[readded_chunk], id, 1));
        // A chunk equal on the wire but with its own generation: current.
        assert!(ending_is_current(
            &[ActiveChunk {
                addresses: vec![address!("0x0000000000000000000000000000000000000001")],
                id,
                generation: 2,
                task: tokio::spawn(async {}),
            }],
            id,
            2
        ));
        // A retained chunk keeps its generation across refreshes.
        assert!(ending_is_current(&[old_chunk], id, 1));
    }

    #[tokio::test]
    async fn a_healthy_signal_disconnecting_wakes_the_reconciler() {
        let state = SignalState::new();
        let reconciler = {
            let state = state.clone();
            tokio::spawn(async move { state.health_changed().await })
        };
        // Connecting again is not a health event the reconciler must react
        // to; only the loss of a healthy signal is.
        state.set_connected(true);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !reconciler.is_finished(),
            "connection must not wake the health listener"
        );
        state.set_connected(false);
        tokio::time::timeout(Duration::from_secs(1), reconciler)
            .await
            .expect("disconnect must wake the health listener")
            .unwrap();
    }

    #[test]
    fn only_an_unsupported_subscription_kind_falls_back() {
        for rejection in [
            "Method not found: monadLogs",
            "method 'monadLogs' does not exist (code -32601)",
            "unknown method: monadLogs",
            "invalid method \"monadLogs\"",
            // Anvil's rejection, verbatim from its eth_subscribe: the node
            // does not know the kind and says so in a -32602 message.
            "server returned an error response: error code -32602: unknown variant `monadLogs`, expected one of `newHeads`, `logs`, `newPendingTransactions`, `syncing`, `transactionReceipts`",
            // The same node's rejection when the filter carries a recipient
            // list: the request parses into no call it knows at all.
            "server returned an error response: error code -32602: data did not match any variant of untagged enum EthRpcCall",
        ] {
            assert!(
                is_unsupported_subscription(rejection),
                "{rejection} should fall back"
            );
        }
        for transient in [
            "the request was throttled; try again (code -32005)",
            "connection closed before a response was received",
            "wss://x.quiknode.pro/SECRET/: read timed out",
            "unsupported filter parameter in topics[2]",
            // A filter problem wrapped in the same invalid-params code: it
            // never names the subscription kind, so it must not fall back.
            "server returned an error response: error code -32602: invalid topics filter",
        ] {
            assert!(
                !is_unsupported_subscription(transient),
                "{transient} must not silently fall back"
            );
        }
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
