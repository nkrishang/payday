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

use alloy_eips::eip2718::Encodable2718;
use alloy_network::{Ethereum, EthereumWallet, NetworkWallet, TransactionBuilder};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use alloy_provider::{DynProvider, Provider, ProviderBuilder};
use alloy_rpc_types_eth::{BlockId, BlockNumberOrTag, Filter, TransactionRequest};
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
    event Recovered(address indexed recovery, uint256 amount);
}

/// A validated USDC `Transfer` log with the metadata needed for durable ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsdcTransfer {
    pub block_number: u64,
    pub block_hash: B256,
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

impl ChainError {
    fn rpc(operation: &'static str, err: TransportError) -> Self {
        if operation == "eth_getLogs" && is_log_range_too_large(&err) {
            return Self::LogRangeTooLarge(err.to_string());
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
            (None, transport.to_string(), retryable)
        } else {
            (None, err.to_string(), false)
        };

        Self::Rpc {
            operation,
            code: RpcCode(code),
            message,
            retryable,
        }
    }

    #[cfg(test)]
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
}

/// What one helper-transaction item did to its payment address, decoded from
/// the batch receipt. Exactly one applies per item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SweepOutcome {
    /// `Payment` was deployed and paid the receiver.
    Settled { amount: U256 },
    /// `Payment` was deployed after expiry and paid the recovery address.
    Recovered { amount: U256 },
    /// `Payment` already existed; `recover` forwarded `amount` (possibly zero).
    Collected { amount: U256 },
    /// `execute` or `recover` reverted. The bytes are `DeploymentFailed()` for
    /// any constructor failure, so they cannot classify the cause.
    Failed { revert_data: Bytes },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementEvent {
    pub transaction_hash: B256,
    pub block_number: u64,
    pub block_hash: B256,
}

/// Chain access needed by the indexer. Standard Ethereum JSON-RPC methods keep
/// the implementation compatible with QuickNode and local Anvil.
#[async_trait]
pub trait ChainClient: Send + Sync {
    /// Newest block number the node reports.
    async fn latest_block_number(&self) -> Result<u64, ChainError>;

    /// Block number behind the node's `finalized` tag.
    async fn finalized_block_number(&self) -> Result<u64, ChainError>;

    /// Canonical header at an exact height; `Transient` if the node lacks it.
    async fn block_header(&self, number: u64) -> Result<BlockHeader, ChainError>;

    /// Successful USDC `Transfer` logs in an inclusive block range.
    async fn usdc_transfers(
        &self,
        token: Address,
        from_block: u64,
        to_block: u64,
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

    /// Transaction that created and drained `Payment`, searched only through
    /// the finalized block whose hash the caller verified.
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
}

/// Production [`ChainClient`] backed by an Alloy HTTP provider with a signing
/// wallet for the sweep transactions.
pub struct AlloyChainClient {
    provider: DynProvider,
    wallet: EthereumWallet,
    signer: Address,
    chain_id: u64,
}

impl AlloyChainClient {
    /// Connect to the RPC endpoint at `rpc_url` (e.g. `http://127.0.0.1:8545`),
    /// signing sweep transactions with `signer` (the backend key). Reads work
    /// the same as an unsigned provider; the wallet only adds send capability.
    pub async fn connect(rpc_url: &str, wallet: EthereumWallet) -> Result<Self, ChainError> {
        let signer = <EthereumWallet as NetworkWallet<Ethereum>>::default_signer_address(&wallet);
        let provider = ProviderBuilder::new()
            .wallet(wallet.clone())
            .connect(rpc_url)
            .await
            .map_err(|error| ChainError::rpc("connect", error))?
            .erased();
        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|error| ChainError::rpc("eth_chainId", error))?;

        Ok(Self {
            provider,
            wallet,
            signer,
            chain_id,
        })
    }

    /// Fetch the chain ID reported by the node. Used for a startup assertion
    /// that the configured chain matches what the RPC endpoint actually serves.
    pub async fn get_chain_id(&self) -> Result<u64, ChainError> {
        self.provider
            .get_chain_id()
            .await
            .map_err(|error| ChainError::rpc("eth_chainId", error))
    }

    /// Factory bound into the configured BatchSweeper's immutable constructor.
    pub async fn get_batch_sweeper_factory(
        &self,
        batch_sweeper: Address,
    ) -> Result<Address, ChainError> {
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

    async fn call_at(
        &self,
        to: Address,
        input: Bytes,
        block: Option<u64>,
    ) -> Result<Bytes, TransportError> {
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
                })
                .collect(),
        }
        .abi_encode()
        .into()
    }

    async fn header(&self, tag: BlockNumberOrTag) -> Result<BlockHeader, ChainError> {
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
    async fn latest_block_number(&self) -> Result<u64, ChainError> {
        self.provider
            .get_block_number()
            .await
            .map_err(|error| ChainError::rpc("eth_blockNumber", error))
    }

    async fn finalized_block_number(&self) -> Result<u64, ChainError> {
        self.header(BlockNumberOrTag::Finalized)
            .await
            .map(|header| header.number)
    }

    async fn block_header(&self, number: u64) -> Result<BlockHeader, ChainError> {
        self.header(BlockNumberOrTag::Number(number)).await
    }

    async fn usdc_transfers(
        &self,
        token: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<UsdcTransfer>, ChainError> {
        let signature = keccak256("Transfer(address,address,uint256)");
        let filter = Filter::new()
            .address(token)
            .event_signature(signature)
            .from_block(from_block)
            .to_block(to_block);
        let logs = self
            .provider
            .get_logs(&filter)
            .await
            .map_err(|error| ChainError::rpc("eth_getLogs", error))?;

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

                Ok(UsdcTransfer {
                    block_number,
                    block_hash: log.block_hash.ok_or_else(|| {
                        ChainError::Transient("USDC log missing block hash".to_string())
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
        logs.first()
            .and_then(|log| Some((log.transaction_hash?, log.block_number?, log.block_hash?)))
            .map(|(transaction_hash, block_number, block_hash)| SettlementEvent {
                transaction_hash,
                block_number,
                block_hash,
            })
            .ok_or_else(|| {
                ChainError::Transient(format!(
                    "payment {payment} is deployed but has no finalized settlement event in blocks {from_block}..={to_block}"
                ))
            })
    }

    async fn sweep_receipt(
        &self,
        tx_hash: B256,
        batch_sweeper: Address,
    ) -> Result<Option<SweepReceipt>, ChainError> {
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

        let malformed = |event: &str, error: alloy_sol_types::Error| {
            ChainError::Transient(format!("malformed {event} event in {tx_hash}: {error}"))
        };
        let mut outcomes = HashMap::new();
        // Payment events describe fresh deployments; sweeper events describe
        // items that hit an existing deployment or failed. A `Recovered`
        // emitted by `recover` is superseded by the sweeper's `SweepRecovered`.
        for log in receipt.logs() {
            let Some(topic) = log.topics().first() else {
                continue;
            };
            if log.address() == batch_sweeper {
                if *topic == SweepFailed::SIGNATURE_HASH {
                    let event = SweepFailed::decode_log(&log.inner)
                        .map_err(|error| malformed("SweepFailed", error))?;
                    outcomes.insert(
                        event.data.paymentAddress,
                        SweepOutcome::Failed {
                            revert_data: event.data.revertData.clone(),
                        },
                    );
                } else if *topic == SweepRecovered::SIGNATURE_HASH {
                    let event = SweepRecovered::decode_log(&log.inner)
                        .map_err(|error| malformed("SweepRecovered", error))?;
                    outcomes.insert(
                        event.data.paymentAddress,
                        SweepOutcome::Collected {
                            amount: event.data.amount,
                        },
                    );
                }
            } else if *topic == Settled::SIGNATURE_HASH {
                let event =
                    Settled::decode_log(&log.inner).map_err(|error| malformed("Settled", error))?;
                outcomes.insert(
                    log.address(),
                    SweepOutcome::Settled {
                        amount: event.data.amount,
                    },
                );
            } else if *topic == Recovered::SIGNATURE_HASH {
                let event = Recovered::decode_log(&log.inner)
                    .map_err(|error| malformed("Recovered", error))?;
                outcomes
                    .entry(log.address())
                    .or_insert(SweepOutcome::Recovered {
                        amount: event.data.amount,
                    });
            }
        }
        Ok(Some(SweepReceipt {
            succeeded: receipt.status(),
            block,
            block_hash,
            transaction_index,
            outcomes,
        }))
    }

    async fn signer_nonce(&self, pending: bool) -> Result<u64, ChainError> {
        let count = self.provider.get_transaction_count(self.signer);
        if pending {
            count.pending().await
        } else {
            count.await
        }
        .map_err(|error| ChainError::rpc("eth_getTransactionCount", error))
    }

    async fn signer_balance(&self) -> Result<U256, ChainError> {
        self.provider
            .get_balance(self.signer)
            .await
            .map_err(|error| ChainError::rpc("eth_getBalance", error))
    }

    async fn estimate_fees(&self) -> Result<FeeEstimate, ChainError> {
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
        }]);

        let decoded = executeBatchCall::abi_decode(&calldata).unwrap();
        assert_eq!(decoded.sweeps.len(), 1);
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
