//! Chain adapter: the gateway's view of the blockchain.
//!
//! The [`ChainClient`] trait is the seam the indexer depends on, so the
//! reconcile and sweep logic can be unit-tested against a mock without a live
//! RPC. [`AlloyChainClient`] is the production implementation backed by an Alloy
//! HTTP provider with a signing wallet (the backend key that calls
//! `BatchSweeper.executeBatch`).
//!
//! Only standard JSON-RPC is used, and state reads pin a block *number* whose
//! canonical hash the caller has already verified, so nothing depends on
//! EIP-1898 block-hash parameters being supported by the provider.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use alloy_eips::eip2718::Encodable2718;
use alloy_network::{Ethereum, EthereumWallet, NetworkWallet, TransactionBuilder};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use alloy_provider::{DynProvider, Provider, ProviderBuilder};
use alloy_rpc_types_eth::{BlockId, BlockNumberOrTag, Filter, Log, TransactionRequest};
use alloy_sol_types::{SolCall, SolEvent, sol};
use alloy_transport::TransportError;
use async_trait::async_trait;
use thiserror::Error;

/// Gas provisioned for each isolated item. The helper transaction is not
/// simulated: an out-of-gas item is caught by BatchSweeper and would look like
/// a successful outer transaction, so the budget is fixed and verified by
/// `BatchSweeper.t.sol` against a full batch. Monad bills the limit, so this is
/// also the price of a sweep.
pub const SWEEP_BATCH_BASE_GAS: u64 = 100_000;
pub const SWEEP_GAS_PER_ITEM: u64 = 400_000;

sol! {
    struct Sweep {
        address token;
        uint256 amount;
        address receiver;
        uint64 expirationTimestamp;
        address recovery;
        bytes32 salt;
        uint256 chainId;
    }

    function executeBatch(Sweep[] sweeps);
    function factory() view returns (address);
    function settled() view returns (bool);
    function paused() view returns (bool);
    function isBlacklisted(address account) view returns (bool);
    function balanceOf(address account) view returns (uint256);

    event SweepFailed(address indexed paymentAddress, address indexed token, bytes revertData);
    event SweepRecovered(address indexed paymentAddress, address indexed token, uint256 amount);
    event Settled(address indexed receiver, uint256 amount);
    event Recovered(address indexed recovery, address indexed token, uint256 amount);
}

/// Recipients per `eth_getLogs` call: one OR-array in `topics[2]`. Every
/// range scan is filtered by the watch list, so the cost of a range grows
/// with the addresses Payday watches and never with the chain's USDC volume.
/// Providers accept thousands; 500 keeps every request small and matches the
/// signal's subscription chunk.
pub const LOG_FILTER_CHUNK: usize = 500;

/// A validated USDC `Transfer` log with the metadata needed for durable ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsdcTransfer {
    pub block_number: u64,
    pub block_hash: B256,
    /// The containing block's timestamp, carried by the log itself
    /// (`blockTimestamp`, served by Monad and Anvil), so classifying a
    /// transfer against a deadline costs no header read.
    pub block_timestamp: u64,
    pub transaction_hash: B256,
    pub transaction_index: u64,
    pub log_index: u64,
    pub sender: Address,
    pub recipient: Address,
    pub amount: U256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHeader {
    pub number: u64,
    pub hash: B256,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeeEstimate {
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
}

impl FeeEstimate {
    /// The smallest bump most nodes accept for a same-nonce replacement is
    /// 10%; use 12.5% so rounding can never fall short.
    pub fn bumped(self) -> Self {
        let bump = |fee: u128| fee + fee / 8 + 1;
        Self {
            max_fee_per_gas: bump(self.max_fee_per_gas),
            max_priority_fee_per_gas: bump(self.max_priority_fee_per_gas),
        }
    }

    pub fn max(self, other: Self) -> Self {
        Self {
            max_fee_per_gas: self.max_fee_per_gas.max(other.max_fee_per_gas),
            max_priority_fee_per_gas: self
                .max_priority_fee_per_gas
                .max(other.max_priority_fee_per_gas),
        }
    }
}

/// Errors from talking to the chain over RPC.
#[derive(Debug, Error)]
pub enum ChainError {
    /// A provider or transport failure. Retryability is retained so callers do
    /// not turn infrastructure failures into permanent payment failures.
    #[error("{operation} RPC error{code}: {message}")]
    Rpc {
        operation: &'static str,
        code: RpcCode,
        message: String,
        retryable: bool,
    },

    /// The provider rejected an `eth_getLogs` range/result as too large. The
    /// acquisition loop should split the range rather than retry it unchanged.
    #[error("eth_getLogs range is too large: {0}")]
    LogRangeTooLarge(String),

    /// Previously committed canonical history no longer agrees with the RPC.
    /// All irreversible activity for this chain must halt.
    #[error("finality violation: {0}")]
    FinalityViolation(String),

    /// A read failed for a non-deterministic reason and should be retried.
    #[error("transient chain failure: {0}")]
    Transient(String),
}

#[derive(Debug, Clone, Copy)]
pub struct RpcCode(Option<i64>);

impl std::fmt::Display for RpcCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(code) => write!(f, " ({code})"),
            None => Ok(()),
        }
    }
}

/// Replace every `http://`, `https://`, `ws://`, or `wss://` URL in `message`
/// with `<rpc-url>`.
/// Transport errors from reqwest and alloy print the full request URL, and
/// the RPC URL carries the provider token, so any message derived from one
/// must pass through here before it reaches a log line or a panic. The
/// WebSocket URL carries the same token as the HTTP one, so the `ws` schemes
/// are redacted too.
pub(crate) fn redact_urls(message: &str) -> String {
    let mut redacted = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(start) = url_start(rest) {
        redacted.push_str(&rest[..start]);
        let url = &rest[start..];
        let end = url
            .find(|c: char| c.is_whitespace() || c == ')' || c == '"')
            .unwrap_or(url.len());
        redacted.push_str("<rpc-url>");
        rest = &url[end..];
    }
    redacted.push_str(rest);
    redacted
}

fn url_start(message: &str) -> Option<usize> {
    ["http://", "https://", "ws://", "wss://"]
        .iter()
        .filter_map(|scheme| message.find(scheme))
        .min()
}

impl ChainError {
    fn rpc(operation: &'static str, err: TransportError) -> Self {
        if operation == "eth_getLogs" && is_log_range_too_large(&err) {
            return Self::LogRangeTooLarge(redact_urls(&err.to_string()));
        }

        let (code, message, retryable) = if let Some(payload) = err.as_error_resp() {
            (
                Some(payload.code),
                payload.message.to_string(),
                payload.is_retry_err()
                    || matches!(payload.code, -32000 | -32001 | -32002 | -32005 | -32011)
                    || payload.code == -32603,
            )
        } else if let Some(transport) = err.as_transport_err() {
            let retryable = transport
                .as_http_error()
                .map(|http| !matches!(http.status, 400 | 401 | 403 | 404 | 413))
                .unwrap_or(true);
            (None, redact_urls(&transport.to_string()), retryable)
        } else {
            (None, redact_urls(&err.to_string()), false)
        };

        Self::Rpc {
            operation,
            code: RpcCode(code),
            message,
            retryable,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Transient(_) | Self::LogRangeTooLarge(_))
            || matches!(
                self,
                Self::Rpc {
                    retryable: true,
                    ..
                }
            )
    }

    pub fn is_permanent_rpc(&self) -> bool {
        matches!(
            self,
            Self::Rpc {
                retryable: false,
                ..
            }
        )
    }
}

fn is_nonce_consumed(err: &TransportError) -> bool {
    err.as_error_resp().is_some_and(|payload| {
        let message = payload.message.to_ascii_lowercase();
        message.contains("nonce too low") || message.contains("nonce is too low")
    })
}

fn is_known_transaction(err: &TransportError) -> bool {
    err.as_error_resp().is_some_and(|payload| {
        let message = payload.message.to_ascii_lowercase();
        message.contains("already known") || message.contains("known transaction")
    })
}

fn is_log_range_too_large(err: &TransportError) -> bool {
    if err
        .as_transport_err()
        .and_then(|transport| transport.as_http_error())
        .is_some_and(|http| http.status == 413)
    {
        return true;
    }
    let Some(payload) = err.as_error_resp() else {
        return false;
    };
    let message = payload.message.to_ascii_lowercase();
    message.contains("block range")
        || message.contains("range is too large")
        || message.contains("too many results")
        || message.contains("more than 10000 results")
        || message.contains("response size")
        || message.contains("result set too large")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SweepRequest {
    pub token: Address,
    pub amount: U256,
    pub receiver: Address,
    pub expiration_timestamp: u64,
    pub recovery: Address,
    pub salt: B256,
    /// The chain the payer chose; part of the address like every other field.
    pub chain_id: u64,
}

/// What one helper-transaction item did to its payment address, decoded from
/// the batch receipt. Exactly one applies per item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SweepOutcome {
    /// `Payment` was deployed and paid the receiver exactly `amount`. Any
    /// overpayment went to the platform recovery wallet as `recovered_amount`
    /// (zero when the address held exactly the invoice amount).
    Settled {
        amount: U256,
        recovered_amount: U256,
    },
    /// `Payment` was deployed after expiry and paid the recovery address.
    Recovered { amount: U256 },
    /// `Payment` already existed; `recover` forwarded `amount` (possibly zero).
    Collected { amount: U256 },
    /// `execute` or `recover` reverted. The bytes are `DeploymentFailed()` for
    /// any constructor failure, so they cannot classify the cause.
    Failed { revert_data: Bytes },
}

/// Events seen so far for one payment address while decoding a receipt.
/// Kept separate from [`SweepOutcome`] so a duplicate event is detected no
/// matter which order the node returned the logs in.
#[derive(Default)]
struct PendingOutcome {
    settled: Option<U256>,
    recovered: Option<U256>,
    sweeper: Option<SweepOutcome>,
}

/// Decode the per-address outcomes of a `BatchSweeper.executeBatch` receipt.
///
/// Payment events (`Settled`, `Recovered`) describe fresh deployments and are
/// keyed by the emitting contract; sweeper events (`SweepRecovered`,
/// `SweepFailed`) describe items that hit an existing deployment or failed and
/// name the payment address explicitly. A `Recovered` emitted by `recover()`
/// precedes the sweeper's `SweepRecovered`, so the sweeper event wins for that
/// address. Every other combination of two events for one address is a
/// malformed receipt, which is reported as transient so the batch is retried
/// against the node rather than finalized on a misreading.
pub fn decode_sweep_outcomes(
    logs: &[Log],
    batch_sweeper: Address,
    tx_hash: B256,
) -> Result<HashMap<Address, SweepOutcome>, ChainError> {
    let malformed = |event: &str, error: alloy_sol_types::Error| {
        ChainError::Transient(format!("malformed {event} event in {tx_hash}: {error}"))
    };
    let duplicate = |event: &str, address: Address| {
        ChainError::Transient(format!(
            "malformed sweep receipt {tx_hash}: duplicate {event} event for payment address {address}"
        ))
    };
    let conflict = |address: Address, detail: &str| {
        ChainError::Transient(format!(
            "malformed sweep receipt {tx_hash}: payment address {address} {detail}"
        ))
    };

    let mut pending: HashMap<Address, PendingOutcome> = HashMap::new();
    for log in logs {
        let Some(topic) = log.topics().first() else {
            continue;
        };
        if log.address() == batch_sweeper {
            let (payment, outcome) = if *topic == SweepFailed::SIGNATURE_HASH {
                let event = SweepFailed::decode_log(&log.inner)
                    .map_err(|error| malformed("SweepFailed", error))?;
                (
                    event.data.paymentAddress,
                    SweepOutcome::Failed {
                        revert_data: event.data.revertData.clone(),
                    },
                )
            } else if *topic == SweepRecovered::SIGNATURE_HASH {
                let event = SweepRecovered::decode_log(&log.inner)
                    .map_err(|error| malformed("SweepRecovered", error))?;
                (
                    event.data.paymentAddress,
                    SweepOutcome::Collected {
                        amount: event.data.amount,
                    },
                )
            } else {
                continue;
            };
            let entry = pending.entry(payment).or_default();
            if entry.sweeper.is_some() {
                return Err(duplicate("BatchSweeper", payment));
            }
            entry.sweeper = Some(outcome);
        } else if *topic == Settled::SIGNATURE_HASH {
            let event =
                Settled::decode_log(&log.inner).map_err(|error| malformed("Settled", error))?;
            let entry = pending.entry(log.address()).or_default();
            if entry.settled.is_some() {
                return Err(duplicate("Settled", log.address()));
            }
            entry.settled = Some(event.data.amount);
        } else if *topic == Recovered::SIGNATURE_HASH {
            let event =
                Recovered::decode_log(&log.inner).map_err(|error| malformed("Recovered", error))?;
            let entry = pending.entry(log.address()).or_default();
            if entry.recovered.is_some() {
                return Err(duplicate("Recovered", log.address()));
            }
            entry.recovered = Some(event.data.amount);
        }
    }

    pending
        .into_iter()
        .map(|(address, entry)| {
            let outcome = match (entry.sweeper, entry.settled, entry.recovered) {
                (Some(_), Some(_), _) => {
                    return Err(conflict(
                        address,
                        "both settled and reported by BatchSweeper",
                    ));
                }
                // A reverted `recover()` emits nothing, so a `Recovered` next
                // to a `SweepFailed` cannot have come from the failed item.
                (Some(SweepOutcome::Failed { .. }), None, Some(_)) => {
                    return Err(conflict(
                        address,
                        "both recovered and reported failed by BatchSweeper",
                    ));
                }
                (Some(sweeper), None, _) => sweeper,
                (None, Some(amount), recovered) => SweepOutcome::Settled {
                    amount,
                    recovered_amount: recovered.unwrap_or(U256::ZERO),
                },
                (None, None, Some(amount)) => SweepOutcome::Recovered { amount },
                (None, None, None) => unreachable!("an entry is only created by an event"),
            };
            Ok((address, outcome))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepReceipt {
    pub succeeded: bool,
    pub block: u64,
    pub block_hash: B256,
    pub transaction_index: u64,
    pub outcomes: HashMap<Address, SweepOutcome>,
}

/// An exact signed helper transaction. Persist this before broadcasting it so
/// a restart can safely resend the same nonce, calldata, fees, and signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedSweepTransaction {
    pub hash: B256,
    pub raw: Bytes,
}

/// Token-level facts read at a pinned block to classify a failed item.
/// `None` means the token does not expose that view (or the call failed).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FailureProbe {
    /// A `Payment` exists, so the failed call was `recover` (destination: recovery).
    pub code_present: bool,
    pub paused: Option<bool>,
    pub payment_blacklisted: Option<bool>,
    pub receiver_blacklisted: Option<bool>,
    pub recovery_blacklisted: Option<bool>,
    pub balance: Option<U256>,
}

/// The transaction that deployed a `Payment` somebody else executed, with
/// the amounts its constructor routed. `settled` is the `Settled` amount when
/// the deployment paid the receiver; `recovered` is what went to the recovery
/// wallet in that same transaction (the overpayment remainder of a live
/// deployment or the whole balance of an expired one), zero when nothing did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementEvent {
    pub transaction_hash: B256,
    pub block_number: u64,
    pub block_hash: B256,
    pub settled: Option<U256>,
    pub recovered: U256,
}

/// Chain access needed by the indexer. Standard Ethereum JSON-RPC methods keep
/// the implementation compatible with QuickNode and local Anvil.
#[async_trait]
pub trait ChainClient: Send + Sync {
    /// Chain ID reported by the RPC endpoint.
    async fn get_chain_id(&self) -> Result<u64, ChainError>;

    /// Newest block number the node reports.
    async fn latest_block_number(&self) -> Result<u64, ChainError>;

    /// Header behind the node's `finalized` tag.
    async fn finalized_header(&self) -> Result<BlockHeader, ChainError>;

    /// Canonical header at an exact height; `Transient` if the node lacks it.
    async fn block_header(&self, number: u64) -> Result<BlockHeader, ChainError>;

    /// Successful USDC `Transfer` logs in an inclusive block range addressed
    /// to `recipients` (chunked into as many calls as the list needs). An
    /// empty list fetches nothing.
    async fn usdc_transfers(
        &self,
        token: Address,
        from_block: u64,
        to_block: u64,
        recipients: &[Address],
    ) -> Result<Vec<UsdcTransfer>, ChainError>;

    /// Mined receipt for a helper transaction, or `None` while unmined.
    async fn sweep_receipt(
        &self,
        tx_hash: B256,
        batch_sweeper: Address,
    ) -> Result<Option<SweepReceipt>, ChainError>;

    /// The signer's mined (`latest`) or queued (`pending`) transaction count.
    async fn signer_nonce(&self, pending: bool) -> Result<u64, ChainError>;

    async fn signer_balance(&self) -> Result<U256, ChainError>;

    async fn estimate_fees(&self) -> Result<FeeEstimate, ChainError>;

    /// Sign one helper transaction without broadcasting it.
    async fn prepare_sweep_batch(
        &self,
        batch_sweeper: Address,
        sweeps: &[SweepRequest],
        nonce: u64,
        gas_limit: u64,
        fees: FeeEstimate,
    ) -> Result<PreparedSweepTransaction, ChainError>;

    /// Idempotently broadcast a transaction that is already durable.
    async fn broadcast_sweep_transaction(
        &self,
        transaction: &PreparedSweepTransaction,
    ) -> Result<(), ChainError>;

    /// `Payment.settled()` at a block whose hash the caller verified.
    async fn payment_settled(&self, payment: Address, block: u64) -> Result<bool, ChainError>;

    /// Transaction that created and drained `Payment`, with the amounts its
    /// events carried, searched only through the finalized block whose hash
    /// the caller verified.
    async fn payment_settlement_tx(
        &self,
        payment: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<SettlementEvent, ChainError>;

    /// Read the token facts that distinguish a transient failure from a
    /// permanent one, at a block whose hash the caller verified.
    async fn probe_failure(
        &self,
        token: Address,
        payment: Address,
        receiver: Address,
        recovery: Address,
        block: u64,
    ) -> Result<FailureProbe, ChainError>;

    /// `keccak256` of the runtime bytecode at `address` (latest block). An
    /// address without code hashes as empty bytes; the caller treats any
    /// mismatch with the expected generation as fatal.
    async fn code_hash(&self, address: Address) -> Result<B256, ChainError>;

    /// Factory bound into the given BatchSweeper's immutable constructor.
    async fn batch_sweeper_factory(&self, batch_sweeper: Address) -> Result<Address, ChainError>;
}

/// Production [`ChainClient`] backed by an Alloy HTTP provider with a signing
/// wallet for the sweep transactions.
pub struct AlloyChainClient {
    provider: DynProvider,
    wallet: EthereumWallet,
    signer: Address,
    chain_id: u64,
    pacer: RpcPacer,
    /// Whether this node has been seen omitting `blockTimestamp` from logs,
    /// so the header fallback is announced once rather than per range.
    timestamp_fallback_logged: AtomicBool,
}

/// Spaces outgoing RPC calls at least `min_interval` apart so bursty callers
/// (the catch-up loop's parallel header fetches over a backlog of blocks)
/// stay under the provider's requests-per-second budget. Tripping that budget
/// fails whole indexing ticks with 429s, and during a long catch-up the
/// re-fetches then cost more requests than the work they complete.
/// `max_per_second` 0 disables pacing (local Anvil needs none).
#[derive(Clone)]
struct RpcPacer {
    min_interval: Duration,
    next_slot: Arc<tokio::sync::Mutex<Instant>>,
}

impl RpcPacer {
    fn new(max_per_second: u64) -> Self {
        let min_interval = if max_per_second == 0 {
            Duration::ZERO
        } else {
            Duration::from_nanos(1_000_000_000u64.checked_div(max_per_second).unwrap_or(0))
                .max(Duration::from_millis(1))
        };
        Self {
            min_interval,
            next_slot: Arc::new(tokio::sync::Mutex::new(Instant::now())),
        }
    }

    async fn acquire(&self) {
        let wait = {
            let mut next_slot = self.next_slot.lock().await;
            let now = Instant::now();
            let slot = (*next_slot).max(now);
            *next_slot = slot + self.min_interval;
            slot - now
        };
        tokio::time::sleep(wait).await;
    }
}

impl AlloyChainClient {
    /// Connect to the RPC endpoint at `rpc_url` (e.g. `http://127.0.0.1:8545`),
    /// signing sweep transactions with `signer` (the backend key). Reads work
    /// the same as an unsigned provider; the wallet only adds send capability.
    /// `rpc_max_rps` paces outgoing calls (0 disables pacing).
    pub async fn connect(
        rpc_url: &str,
        wallet: EthereumWallet,
        rpc_max_rps: u64,
    ) -> Result<Self, ChainError> {
        let pacer = RpcPacer::new(rpc_max_rps);
        let signer = <EthereumWallet as NetworkWallet<Ethereum>>::default_signer_address(&wallet);
        let provider = ProviderBuilder::new()
            .wallet(wallet.clone())
            .connect(rpc_url)
            .await
            .map_err(|error| ChainError::rpc("connect", error))?
            .erased();
        pacer.acquire().await;
        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|error| ChainError::rpc("eth_chainId", error))?;

        Ok(Self {
            provider,
            wallet,
            signer,
            chain_id,
            pacer,
            timestamp_fallback_logged: AtomicBool::new(false),
        })
    }

    /// Fetch the chain ID reported by the node. Used for a startup assertion
    /// that the configured chain matches what the RPC endpoint actually serves.
    pub async fn get_chain_id(&self) -> Result<u64, ChainError> {
        self.pacer.acquire().await;
        self.provider
            .get_chain_id()
            .await
            .map_err(|error| ChainError::rpc("eth_chainId", error))
    }

    async fn call_at(
        &self,
        to: Address,
        input: Bytes,
        block: Option<u64>,
    ) -> Result<Bytes, TransportError> {
        self.pacer.acquire().await;
        let tx = TransactionRequest::default().with_to(to).with_input(input);
        let call = self.provider.call(tx);
        match block {
            Some(number) => call.block(BlockId::number(number)).await,
            None => call.await,
        }
    }

    /// A view call whose absence on the token is a fact, not a failure.
    async fn optional_bool(&self, to: Address, input: Bytes, block: u64) -> Option<bool> {
        let output = self.call_at(to, input, Some(block)).await.ok()?;
        (output.len() == 32).then(|| output[31] == 1)
    }

    fn execute_batch_calldata(sweeps: &[SweepRequest]) -> Bytes {
        executeBatchCall {
            sweeps: sweeps
                .iter()
                .map(|sweep| Sweep {
                    token: sweep.token,
                    amount: sweep.amount,
                    receiver: sweep.receiver,
                    expirationTimestamp: sweep.expiration_timestamp,
                    recovery: sweep.recovery,
                    salt: sweep.salt,
                    chainId: U256::from(sweep.chain_id),
                })
                .collect(),
        }
        .abi_encode()
        .into()
    }

    /// One `eth_getLogs` for USDC transfers in the range to `recipients`,
    /// validated but not yet timestamped.
    async fn transfer_logs(
        &self,
        token: Address,
        from_block: u64,
        to_block: u64,
        recipients: &[Address],
    ) -> Result<Vec<Log>, ChainError> {
        let signature = keccak256("Transfer(address,address,uint256)");
        let filter = Filter::new()
            .address(token)
            .event_signature(signature)
            .from_block(from_block)
            .to_block(to_block)
            .topic2(
                recipients
                    .iter()
                    .map(|address| address.into_word())
                    .collect::<Vec<_>>(),
            );
        self.pacer.acquire().await;
        self.provider
            .get_logs(&filter)
            .await
            .map_err(|error| ChainError::rpc("eth_getLogs", error))
    }

    async fn header(&self, tag: BlockNumberOrTag) -> Result<BlockHeader, ChainError> {
        self.pacer.acquire().await;
        self.provider
            .get_block_by_number(tag)
            .await
            .map_err(|error| ChainError::rpc("eth_getBlockByNumber", error))?
            .map(|block| BlockHeader {
                number: block.header.number,
                hash: block.header.hash,
                timestamp: block.header.timestamp,
            })
            .ok_or_else(|| ChainError::Transient(format!("block {tag} is unavailable")))
    }
}

/// Gas limit for a helper transaction carrying `sweep_count` items.
pub fn sweep_batch_gas_limit(sweep_count: usize) -> u64 {
    SWEEP_BATCH_BASE_GAS.saturating_add(
        SWEEP_GAS_PER_ITEM.saturating_mul(u64::try_from(sweep_count).unwrap_or(u64::MAX)),
    )
}

#[async_trait]
impl ChainClient for AlloyChainClient {
    async fn get_chain_id(&self) -> Result<u64, ChainError> {
        AlloyChainClient::get_chain_id(self).await
    }

    async fn latest_block_number(&self) -> Result<u64, ChainError> {
        self.pacer.acquire().await;
        self.provider
            .get_block_number()
            .await
            .map_err(|error| ChainError::rpc("eth_blockNumber", error))
    }

    async fn finalized_header(&self) -> Result<BlockHeader, ChainError> {
        self.header(BlockNumberOrTag::Finalized).await
    }

    async fn block_header(&self, number: u64) -> Result<BlockHeader, ChainError> {
        self.header(BlockNumberOrTag::Number(number)).await
    }

    async fn usdc_transfers(
        &self,
        token: Address,
        from_block: u64,
        to_block: u64,
        recipients: &[Address],
    ) -> Result<Vec<UsdcTransfer>, ChainError> {
        let signature = keccak256("Transfer(address,address,uint256)");
        // An empty list is a scan for nothing; the caller fast-forwards
        // instead, but answer honestly if asked.
        let mut logs = Vec::new();
        for chunk in recipients.chunks(LOG_FILTER_CHUNK) {
            logs.extend(
                self.transfer_logs(token, from_block, to_block, chunk)
                    .await?,
            );
        }

        // Monad and Anvil put the block timestamp on the log itself; nodes
        // that do not (some L2 clients) cost one header read per block that
        // carries a transfer to us, which the recipient filter keeps rare.
        let mut timestamps: HashMap<u64, u64> = HashMap::new();
        for log in &logs {
            if log.block_timestamp.is_some() {
                continue;
            }
            let Some(number) = log.block_number else {
                continue;
            };
            if timestamps.contains_key(&number) {
                continue;
            }
            if !self.timestamp_fallback_logged.swap(true, Ordering::AcqRel) {
                tracing::warn!(
                    chain_id = self.chain_id,
                    "node omits blockTimestamp from logs; reading a header per transfer-bearing block"
                );
            }
            let header = self.header(BlockNumberOrTag::Number(number)).await?;
            timestamps.insert(number, header.timestamp);
        }

        logs.into_iter()
            .map(|log| {
                let topics = log.topics();
                if log.address() != token
                    || log.removed
                    || topics.len() != 3
                    || topics[0] != signature
                {
                    return Err(ChainError::Transient(
                        "RPC returned a malformed USDC Transfer log".to_string(),
                    ));
                }
                let data = &log.inner.data.data;
                if data.len() != 32 {
                    return Err(ChainError::Transient(
                        "RPC returned a USDC Transfer with invalid amount data".to_string(),
                    ));
                }

                let block_number = log.block_number.ok_or_else(|| {
                    ChainError::Transient("USDC log missing block number".to_string())
                })?;
                if !(from_block..=to_block).contains(&block_number) {
                    return Err(ChainError::Transient(format!(
                        "RPC returned USDC log from block {block_number} outside requested range {from_block}..={to_block}"
                    )));
                }
                if !recipients.contains(&Address::from_word(topics[2])) {
                    return Err(ChainError::Transient(
                        "RPC returned a USDC Transfer outside the requested recipient filter"
                            .to_string(),
                    ));
                }

                Ok(UsdcTransfer {
                    block_number,
                    block_hash: log.block_hash.ok_or_else(|| {
                        ChainError::Transient("USDC log missing block hash".to_string())
                    })?,
                    block_timestamp: log
                        .block_timestamp
                        .or_else(|| timestamps.get(&block_number).copied())
                        .ok_or_else(|| {
                            ChainError::Transient("USDC log missing block timestamp".to_string())
                        })?,
                    transaction_hash: log.transaction_hash.ok_or_else(|| {
                        ChainError::Transient("USDC log missing transaction hash".to_string())
                    })?,
                    transaction_index: log.transaction_index.ok_or_else(|| {
                        ChainError::Transient("USDC log missing transaction index".to_string())
                    })?,
                    log_index: log.log_index.ok_or_else(|| {
                        ChainError::Transient("USDC log missing log index".to_string())
                    })?,
                    sender: Address::from_word(topics[1]),
                    recipient: Address::from_word(topics[2]),
                    amount: U256::from_be_slice(data),
                })
            })
            .collect()
    }

    async fn payment_settlement_tx(
        &self,
        payment: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<SettlementEvent, ChainError> {
        let settled = Settled::SIGNATURE_HASH;
        let recovered = Recovered::SIGNATURE_HASH;
        let filter = Filter::new()
            .address(payment)
            .from_block(from_block)
            .to_block(to_block);
        let mut logs = self
            .provider
            .get_logs(&filter)
            .await
            .map_err(|error| ChainError::rpc("eth_getLogs", error))?
            .into_iter()
            .filter(|log| {
                !log.removed
                    && log.address() == payment
                    && log
                        .topics()
                        .first()
                        .is_some_and(|topic| *topic == settled || *topic == recovered)
            })
            .collect::<Vec<_>>();
        logs.sort_by_key(|log| (log.block_number, log.transaction_index, log.log_index));
        // The constructor always emits at least one of the two events, so the
        // earliest is the deployment; an overpaid live deployment emits both
        // in that one transaction and the ledger needs both amounts.
        let (transaction_hash, block_number, block_hash) = logs
            .first()
            .and_then(|log| Some((log.transaction_hash?, log.block_number?, log.block_hash?)))
            .ok_or_else(|| {
                ChainError::Transient(format!(
                    "payment {payment} is deployed but has no finalized settlement event in blocks {from_block}..={to_block}"
                ))
            })?;
        let malformed = |event: &str| {
            ChainError::Transient(format!(
                "malformed settlement transaction {transaction_hash} for payment {payment}: duplicate {event} event"
            ))
        };
        let mut settled_amount = None;
        let mut recovered_amount = None;
        for log in logs
            .iter()
            .filter(|log| log.transaction_hash == Some(transaction_hash))
        {
            if log.topics()[0] == settled {
                let event = Settled::decode_log(&log.inner).map_err(|error| {
                    ChainError::Transient(format!(
                        "malformed Settled event in {transaction_hash}: {error}"
                    ))
                })?;
                if settled_amount.replace(event.data.amount).is_some() {
                    return Err(malformed("Settled"));
                }
            } else {
                let event = Recovered::decode_log(&log.inner).map_err(|error| {
                    ChainError::Transient(format!(
                        "malformed Recovered event in {transaction_hash}: {error}"
                    ))
                })?;
                // `recover()` is intentionally callable more than once. A
                // deployment and a later recovery can therefore emit several
                // Recovered logs in one transaction; all are ledgered.
                recovered_amount = Some(
                    recovered_amount
                        .unwrap_or(U256::ZERO)
                        .checked_add(event.data.amount)
                        .ok_or_else(|| malformed("Recovered amount overflow"))?,
                );
            }
        }
        Ok(SettlementEvent {
            transaction_hash,
            block_number,
            block_hash,
            settled: settled_amount,
            recovered: recovered_amount.unwrap_or(U256::ZERO),
        })
    }

    async fn sweep_receipt(
        &self,
        tx_hash: B256,
        batch_sweeper: Address,
    ) -> Result<Option<SweepReceipt>, ChainError> {
        self.pacer.acquire().await;
        let Some(receipt) = self
            .provider
            .get_transaction_receipt(tx_hash)
            .await
            .map_err(|error| ChainError::rpc("eth_getTransactionReceipt", error))?
        else {
            return Ok(None);
        };
        if receipt.to != Some(batch_sweeper) {
            return Err(ChainError::FinalityViolation(format!(
                "sweep transaction {tx_hash} targeted {:?}, not configured BatchSweeper {batch_sweeper}",
                receipt.to
            )));
        }
        let block = receipt.block_number.ok_or_else(|| {
            ChainError::Transient("execute receipt has no block number".to_string())
        })?;
        let block_hash = receipt.block_hash.ok_or_else(|| {
            ChainError::Transient("execute receipt has no block hash".to_string())
        })?;
        let transaction_index = receipt.transaction_index.ok_or_else(|| {
            ChainError::Transient("execute receipt has no transaction index".to_string())
        })?;
        let outcomes = decode_sweep_outcomes(receipt.logs(), batch_sweeper, tx_hash)?;
        Ok(Some(SweepReceipt {
            succeeded: receipt.status(),
            block,
            block_hash,
            transaction_index,
            outcomes,
        }))
    }

    async fn signer_nonce(&self, pending: bool) -> Result<u64, ChainError> {
        self.pacer.acquire().await;
        let count = self.provider.get_transaction_count(self.signer);
        if pending {
            count.pending().await
        } else {
            count.await
        }
        .map_err(|error| ChainError::rpc("eth_getTransactionCount", error))
    }

    async fn signer_balance(&self) -> Result<U256, ChainError> {
        self.pacer.acquire().await;
        self.provider
            .get_balance(self.signer)
            .await
            .map_err(|error| ChainError::rpc("eth_getBalance", error))
    }

    async fn estimate_fees(&self) -> Result<FeeEstimate, ChainError> {
        self.pacer.acquire().await;
        self.provider
            .estimate_eip1559_fees()
            .await
            .map(|estimate| FeeEstimate {
                max_fee_per_gas: estimate.max_fee_per_gas,
                max_priority_fee_per_gas: estimate.max_priority_fee_per_gas,
            })
            .map_err(|error| ChainError::rpc("eth_feeHistory", error))
    }

    async fn prepare_sweep_batch(
        &self,
        batch_sweeper: Address,
        sweeps: &[SweepRequest],
        nonce: u64,
        gas_limit: u64,
        fees: FeeEstimate,
    ) -> Result<PreparedSweepTransaction, ChainError> {
        let tx = TransactionRequest::default()
            .with_to(batch_sweeper)
            .with_chain_id(self.chain_id)
            .with_nonce(nonce)
            .with_gas_limit(gas_limit)
            .with_max_fee_per_gas(fees.max_fee_per_gas)
            .with_max_priority_fee_per_gas(fees.max_priority_fee_per_gas)
            .with_input(Self::execute_batch_calldata(sweeps));
        let envelope = tx.build(&self.wallet).await.map_err(|error| {
            ChainError::Transient(format!("could not sign sweep transaction: {error}"))
        })?;
        let raw: Bytes = envelope.encoded_2718().into();
        Ok(PreparedSweepTransaction {
            hash: keccak256(&raw),
            raw,
        })
    }

    async fn broadcast_sweep_transaction(
        &self,
        transaction: &PreparedSweepTransaction,
    ) -> Result<(), ChainError> {
        self.pacer.acquire().await;
        match self.provider.send_raw_transaction(&transaction.raw).await {
            Ok(pending) if *pending.tx_hash() == transaction.hash => Ok(()),
            Ok(pending) => Err(ChainError::FinalityViolation(format!(
                "RPC returned transaction hash {} for signed transaction {}",
                pending.tx_hash(),
                transaction.hash
            ))),
            // Resending the exact signed bytes after a crash is successful if
            // the node already knows them or has mined their nonce.
            Err(error) if is_known_transaction(&error) || is_nonce_consumed(&error) => Ok(()),
            Err(error) => Err(ChainError::rpc("eth_sendRawTransaction", error)),
        }
    }

    async fn payment_settled(&self, payment: Address, block: u64) -> Result<bool, ChainError> {
        let output = self
            .call_at(payment, settledCall {}.abi_encode().into(), Some(block))
            .await
            .map_err(|error| ChainError::rpc("eth_call Payment.settled", error))?;
        settledCall::abi_decode_returns(&output).map_err(|error| {
            ChainError::Transient(format!(
                "could not decode Payment.settled response: {error}"
            ))
        })
    }

    async fn probe_failure(
        &self,
        token: Address,
        payment: Address,
        receiver: Address,
        recovery: Address,
        block: u64,
    ) -> Result<FailureProbe, ChainError> {
        self.pacer.acquire().await;
        let code = self
            .provider
            .get_code_at(payment)
            .block_id(BlockId::number(block))
            .await
            .map_err(|error| ChainError::rpc("eth_getCode", error))?;
        let balance = self
            .call_at(
                token,
                balanceOfCall { account: payment }.abi_encode().into(),
                Some(block),
            )
            .await
            .ok()
            .and_then(|output| balanceOfCall::abi_decode_returns(&output).ok());
        let blacklisted = |account: Address| {
            self.optional_bool(
                token,
                isBlacklistedCall { account }.abi_encode().into(),
                block,
            )
        };
        Ok(FailureProbe {
            code_present: !code.is_empty(),
            paused: self
                .optional_bool(token, pausedCall {}.abi_encode().into(), block)
                .await,
            payment_blacklisted: blacklisted(payment).await,
            receiver_blacklisted: blacklisted(receiver).await,
            recovery_blacklisted: blacklisted(recovery).await,
            balance,
        })
    }

    async fn code_hash(&self, address: Address) -> Result<B256, ChainError> {
        self.pacer.acquire().await;
        self.provider
            .get_code_at(address)
            .await
            .map(|code| keccak256(&code))
            .map_err(|error| ChainError::rpc("eth_getCode", error))
    }

    async fn batch_sweeper_factory(&self, batch_sweeper: Address) -> Result<Address, ChainError> {
        let output = self
            .call_at(batch_sweeper, factoryCall {}.abi_encode().into(), None)
            .await
            .map_err(|error| ChainError::rpc("eth_call BatchSweeper.factory", error))?;
        factoryCall::abi_decode_returns(&output).map_err(|error| {
            ChainError::Transient(format!(
                "could not decode BatchSweeper.factory response: {error}"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use alloy_json_rpc::{ErrorPayload, RpcError};
    use alloy_primitives::{address, b256};
    use alloy_transport::TransportErrorKind;

    use super::*;

    fn rpc_error(code: i64, message: &'static str, data: Option<&'static str>) -> TransportError {
        RpcError::ErrorResp(ErrorPayload {
            code,
            message: Cow::Borrowed(message),
            data: data
                .map(serde_json::value::to_raw_value)
                .transpose()
                .unwrap(),
        })
    }

    #[test]
    fn consumed_nonces_are_recognised_from_the_node_message() {
        assert!(is_nonce_consumed(&rpc_error(-32000, "nonce too low", None)));
        assert!(is_nonce_consumed(&rpc_error(
            -32000,
            "Nonce is too low: next nonce 7, tx nonce 6",
            None
        )));
        assert!(!is_nonce_consumed(&rpc_error(
            -32000,
            "replacement transaction underpriced",
            None
        )));
    }

    #[test]
    fn quicknode_range_errors_are_split_but_rate_limits_are_not() {
        assert!(is_log_range_too_large(&TransportErrorKind::http_error(
            413,
            "Request Entity Too Large".to_string()
        )));
        assert!(is_log_range_too_large(&rpc_error(
            -32602,
            "Block range limit exceeded",
            None
        )));
        assert!(!is_log_range_too_large(&rpc_error(
            429,
            "requests per second exceeded",
            None
        )));

        let rate_limit = ChainError::rpc(
            "eth_getLogs",
            rpc_error(429, "requests per second exceeded", None),
        );
        assert!(rate_limit.is_retryable());
        assert!(!rate_limit.is_permanent_rpc());

        let unauthorized = ChainError::rpc(
            "eth_getLogs",
            TransportErrorKind::http_error(401, "Unauthorized".to_string()),
        );
        assert!(!unauthorized.is_retryable());
        assert!(unauthorized.is_permanent_rpc());
    }

    #[test]
    fn transport_error_messages_never_carry_the_rpc_url() {
        let leaked = "error sending request for url (https://x.quiknode.pro/SECRET/)";
        assert_eq!(
            redact_urls(leaked),
            "error sending request for url (<rpc-url>)"
        );
        assert_eq!(
            redact_urls("tried \"https://a.example/T1\" then http://b.example/T2 next"),
            "tried \"<rpc-url>\" then <rpc-url> next"
        );
        // The WebSocket endpoint carries the same provider token as the HTTP
        // one, so both ws schemes are redacted too.
        assert_eq!(
            redact_urls("wss://x.quiknode.pro/SECRET/ closed: ws://127.0.0.1:8545 refused"),
            "<rpc-url> closed: <rpc-url> refused"
        );
        assert_eq!(
            redact_urls("connection reset by peer"),
            "connection reset by peer"
        );

        let transport = ChainError::rpc("eth_blockNumber", TransportErrorKind::custom_str(leaked));
        let message = transport.to_string();
        assert!(!message.contains("quiknode"), "{message}");
        assert!(!message.contains("SECRET"), "{message}");
        assert!(message.contains("eth_blockNumber RPC error: "), "{message}");
        assert!(message.contains("<rpc-url>"), "{message}");

        let payload = ChainError::rpc(
            "eth_call",
            rpc_error(-32000, "execution reverted: InsufficientTokenBalance", None),
        );
        assert!(
            payload
                .to_string()
                .contains("execution reverted: InsufficientTokenBalance")
        );
    }

    #[test]
    fn batch_calldata_includes_expiration_and_recovery() {
        let token = address!("0x0000000000000000000000000000000000000001");
        let receiver = address!("0x0000000000000000000000000000000000000002");
        let recovery = address!("0x0000000000000000000000000000000000000003");
        let salt = b256!("0x0000000000000000000000000000000000000000000000000000000000000004");
        let calldata = AlloyChainClient::execute_batch_calldata(&[SweepRequest {
            token,
            amount: U256::from(5),
            receiver,
            expiration_timestamp: 1_900_000_000,
            recovery,
            salt,
            chain_id: 8453,
        }]);

        let decoded = executeBatchCall::abi_decode(&calldata).unwrap();
        assert_eq!(decoded.sweeps.len(), 1);
        assert_eq!(decoded.sweeps[0].chainId, U256::from(8453));
        assert_eq!(decoded.sweeps[0].token, token);
        assert_eq!(decoded.sweeps[0].amount, U256::from(5));
        assert_eq!(decoded.sweeps[0].receiver, receiver);
        assert_eq!(decoded.sweeps[0].expirationTimestamp, 1_900_000_000);
        assert_eq!(decoded.sweeps[0].recovery, recovery);
        assert_eq!(decoded.sweeps[0].salt, salt);
    }

    #[test]
    fn batch_gas_limit_scales_per_isolated_sweep() {
        assert_eq!(sweep_batch_gas_limit(1), 500_000);
        assert_eq!(sweep_batch_gas_limit(20), 8_100_000);
    }

    const SWEEPER: Address = address!("0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0");
    const PAYMENT: Address = address!("0x00000000000000000000000000000000000000AA");
    const RECEIVER: Address = address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8");
    const RECOVERY: Address = address!("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc");
    const TX: B256 = b256!("0x00000000000000000000000000000000000000000000000000000000000000ff");

    /// A receipt log as the node would return it, built from a typed event.
    fn log(emitter: Address, data: alloy_primitives::LogData) -> Log {
        Log {
            inner: alloy_primitives::Log {
                address: emitter,
                data,
            },
            ..Log::default()
        }
    }

    fn settled(amount: u64) -> Log {
        log(
            PAYMENT,
            Settled {
                receiver: RECEIVER,
                amount: U256::from(amount),
            }
            .encode_log_data(),
        )
    }

    fn recovered(amount: u64) -> Log {
        log(
            PAYMENT,
            Recovered {
                recovery: RECOVERY,
                token: address!("0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512"),
                amount: U256::from(amount),
            }
            .encode_log_data(),
        )
    }

    fn sweep_recovered(amount: u64) -> Log {
        log(
            SWEEPER,
            SweepRecovered {
                paymentAddress: PAYMENT,
                token: address!("0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512"),
                amount: U256::from(amount),
            }
            .encode_log_data(),
        )
    }

    fn sweep_failed() -> Log {
        log(
            SWEEPER,
            SweepFailed {
                paymentAddress: PAYMENT,
                token: address!("0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512"),
                revertData: Bytes::from_static(&[0x30, 0x11, 0x64, 0x25]),
            }
            .encode_log_data(),
        )
    }

    #[test]
    fn receipt_combines_settled_and_recovered_events() {
        let expected = HashMap::from([(
            PAYMENT,
            SweepOutcome::Settled {
                amount: U256::from(100),
                recovered_amount: U256::from(50),
            },
        )]);
        assert_eq!(
            decode_sweep_outcomes(&[settled(100), recovered(50)], SWEEPER, TX).unwrap(),
            expected
        );
        assert_eq!(
            decode_sweep_outcomes(&[recovered(50), settled(100)], SWEEPER, TX).unwrap(),
            expected,
            "log order must not matter"
        );

        assert_eq!(
            decode_sweep_outcomes(&[settled(100)], SWEEPER, TX).unwrap(),
            HashMap::from([(
                PAYMENT,
                SweepOutcome::Settled {
                    amount: U256::from(100),
                    recovered_amount: U256::ZERO,
                },
            )]),
            "an exact payment records no recovery"
        );
        assert_eq!(
            decode_sweep_outcomes(&[recovered(40)], SWEEPER, TX).unwrap(),
            HashMap::from([(
                PAYMENT,
                SweepOutcome::Recovered {
                    amount: U256::from(40),
                },
            )]),
            "a standalone Recovered is an expired deployment"
        );
        // `recover()` emits Recovered before the sweeper's SweepRecovered.
        let collected = HashMap::from([(
            PAYMENT,
            SweepOutcome::Collected {
                amount: U256::from(30),
            },
        )]);
        assert_eq!(
            decode_sweep_outcomes(&[recovered(30), sweep_recovered(30)], SWEEPER, TX).unwrap(),
            collected
        );
        assert_eq!(
            decode_sweep_outcomes(&[sweep_recovered(30), recovered(30)], SWEEPER, TX).unwrap(),
            collected
        );
        assert!(matches!(
            decode_sweep_outcomes(&[sweep_failed()], SWEEPER, TX).unwrap(),
            outcomes if matches!(outcomes[&PAYMENT], SweepOutcome::Failed { .. })
        ));
    }

    #[test]
    fn receipt_rejects_conflicting_duplicate_events() {
        let malformed: [(&str, Vec<Log>); 8] = [
            ("two Settled", vec![settled(100), settled(100)]),
            ("two Recovered", vec![recovered(50), recovered(50)]),
            (
                "two sweeper events",
                vec![sweep_recovered(30), sweep_failed()],
            ),
            (
                "Settled then sweeper",
                vec![settled(100), sweep_recovered(100)],
            ),
            ("sweeper then Settled", vec![sweep_failed(), settled(100)]),
            // Only `recover()` pairs a Recovered with a sweeper event, and a
            // failed item emitted nothing.
            ("failed then Recovered", vec![sweep_failed(), recovered(30)]),
            ("Recovered then failed", vec![recovered(30), sweep_failed()]),
            (
                "Settled, Recovered and sweeper",
                vec![settled(100), recovered(50), sweep_recovered(150)],
            ),
        ];
        for (name, logs) in malformed {
            let error = decode_sweep_outcomes(&logs, SWEEPER, TX).unwrap_err();
            assert!(matches!(error, ChainError::Transient(_)), "{name}");
            let message = error.to_string();
            assert!(message.contains(&TX.to_string()), "{name}: {message}");
            assert!(message.contains(&PAYMENT.to_string()), "{name}: {message}");
        }
    }

    #[test]
    fn replacement_fees_bump_by_at_least_an_eighth() {
        let fees = FeeEstimate {
            max_fee_per_gas: 800,
            max_priority_fee_per_gas: 8,
        };
        assert_eq!(
            fees.bumped(),
            FeeEstimate {
                max_fee_per_gas: 901,
                max_priority_fee_per_gas: 10
            }
        );
        let fresh = FeeEstimate {
            max_fee_per_gas: 1_000,
            max_priority_fee_per_gas: 1,
        };
        assert_eq!(
            fees.bumped().max(fresh),
            FeeEstimate {
                max_fee_per_gas: 1_000,
                max_priority_fee_per_gas: 10
            }
        );
    }
}
