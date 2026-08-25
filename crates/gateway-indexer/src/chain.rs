//! Chain adapter: the gateway's view of the blockchain.
//!
//! The [`ChainClient`] trait is the seam the indexer depends on, so the
//! reconcile and sweep logic can be unit-tested against a mock without a live
//! RPC. [`AlloyChainClient`] is the production implementation backed by an Alloy
//! HTTP provider with a signing wallet (the backend key that calls
//! `BatchSweeper.executeBatch`).

use alloy_network::{Ethereum, EthereumWallet, NetworkWallet, TransactionBuilder};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use alloy_provider::{DynProvider, Provider, ProviderBuilder};
use alloy_rpc_types_eth::{BlockId, BlockNumberOrTag, Filter, TransactionRequest};
use alloy_sol_types::{SolCall, SolEvent, sol};
use alloy_transport::TransportError;
use async_trait::async_trait;
use thiserror::Error;

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

    event SweepFailed(address indexed paymentAddress, address indexed token, bytes revertData);
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

    /// A sweep did not confirm for a non-deterministic reason (the transaction
    /// was not mined, or a read failed) and should be retried. Distinct from a
    /// per-item contract revert, which is reported in the batch receipt.
    #[error("transient sweep failure: {0}")]
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

fn is_execution_revert(err: &TransportError) -> bool {
    let Some(payload) = err.as_error_resp() else {
        return false;
    };
    let message = payload.message.to_ascii_lowercase();
    payload.code == 3
        || payload.code == -32015
        || (payload.code == -32000
            && (message.contains("execution reverted") || message.contains("vm execution error")))
        || payload.as_revert_data().is_some()
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepReceipt {
    pub succeeded: bool,
    pub block: u64,
    pub block_hash: B256,
    /// Per-item failures emitted by the configured BatchSweeper. Revert bytes
    /// are deliberately not used for classification.
    pub failures: Vec<(Address, Address)>,
}

/// Chain access needed by the indexer. Standard Ethereum JSON-RPC methods keep
/// the implementation compatible with QuickNode and local Anvil.
#[async_trait]
pub trait ChainClient: Send + Sync {
    /// Current canonical head block number.
    async fn get_block_number(&self) -> Result<u64, ChainError>;

    /// Canonical block hash at an exact height.
    async fn get_block_hash(&self, block: u64) -> Result<B256, ChainError>;

    /// Whether the deterministic Payment contract currently exists.
    async fn payment_deployed(&self, address: Address) -> Result<bool, ChainError>;

    /// Whether Payment code existed at an exact canonical block.
    async fn payment_deployed_at(
        &self,
        address: Address,
        block_hash: B256,
    ) -> Result<bool, ChainError>;

    /// Whether a submitted transaction is still known to the node/mempool.
    async fn transaction_known(&self, tx_hash: B256) -> Result<bool, ChainError>;

    /// Whether the signer has a transaction occupying the next mined nonce.
    async fn signer_has_pending_transaction(&self) -> Result<bool, ChainError>;

    /// Mined receipt for a submitted sweep, or `None` while still pending.
    async fn get_sweep_receipt(
        &self,
        tx_hash: B256,
        batch_sweeper: Address,
    ) -> Result<Option<SweepReceipt>, ChainError>;

    /// Successful USDC `Transfer` logs in an inclusive block range.
    async fn get_usdc_transfers(
        &self,
        token: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<UsdcTransfer>, ChainError>;

    /// Submit one helper transaction that independently executes every sweep.
    async fn submit_sweep_batch(
        &self,
        batch_sweeper: Address,
        sweeps: &[SweepRequest],
    ) -> Result<B256, ChainError>;
}

/// Production [`ChainClient`] backed by an Alloy HTTP provider with a signing
/// wallet for the sweep transactions.
pub struct AlloyChainClient {
    provider: DynProvider,
    signer: Address,
}

impl AlloyChainClient {
    /// Connect to the RPC endpoint at `rpc_url` (e.g. `http://127.0.0.1:8545`),
    /// signing sweep transactions with `signer` (the backend key). Reads work
    /// the same as an unsigned provider; the wallet only adds send capability.
    pub async fn connect(rpc_url: &str, wallet: EthereumWallet) -> Result<Self, ChainError> {
        let signer = <EthereumWallet as NetworkWallet<Ethereum>>::default_signer_address(&wallet);
        let provider = ProviderBuilder::new()
            .with_simple_nonce_management()
            .wallet(wallet)
            .connect(rpc_url)
            .await
            .map_err(|error| ChainError::rpc("connect", error))?
            .erased();

        Ok(Self { provider, signer })
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
        let tx = TransactionRequest::default()
            .with_to(batch_sweeper)
            .with_input(factoryCall {}.abi_encode());
        let output = self
            .provider
            .call(tx)
            .await
            .map_err(|error| ChainError::rpc("eth_call BatchSweeper.factory", error))?;
        factoryCall::abi_decode_returns(&output).map_err(|error| {
            ChainError::Transient(format!(
                "could not decode BatchSweeper.factory response: {error}"
            ))
        })
    }

    /// Whether an address has contract code deployed. `execute` deploying the
    /// `Payment` contract is the only way code lands at a payment address, so
    /// code present means the sweep already happened.
    async fn has_code(&self, addr: Address) -> Result<bool, ChainError> {
        let code = self
            .provider
            .get_code_at(addr)
            .await
            .map_err(|error| ChainError::rpc("eth_getCode", error))?;
        Ok(!code.is_empty())
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
}

#[async_trait]
impl ChainClient for AlloyChainClient {
    async fn get_block_number(&self) -> Result<u64, ChainError> {
        self.provider
            .get_block_number()
            .await
            .map_err(|error| ChainError::rpc("eth_blockNumber", error))
    }

    async fn get_block_hash(&self, block: u64) -> Result<B256, ChainError> {
        self.provider
            .get_block_by_number(BlockNumberOrTag::Number(block))
            .await
            .map_err(|error| ChainError::rpc("eth_getBlockByNumber", error))?
            .map(|block| block.header.hash)
            .ok_or_else(|| ChainError::Transient(format!("block {block} is unavailable")))
    }

    async fn payment_deployed(&self, address: Address) -> Result<bool, ChainError> {
        self.has_code(address).await
    }

    async fn payment_deployed_at(
        &self,
        address: Address,
        block_hash: B256,
    ) -> Result<bool, ChainError> {
        let code = self
            .provider
            .get_code_at(address)
            .block_id(BlockId::hash_canonical(block_hash))
            .await
            .map_err(|error| ChainError::rpc("eth_getCode", error))?;
        Ok(!code.is_empty())
    }

    async fn transaction_known(&self, tx_hash: B256) -> Result<bool, ChainError> {
        self.provider
            .get_transaction_by_hash(tx_hash)
            .await
            .map(|transaction| transaction.is_some())
            .map_err(|error| ChainError::rpc("eth_getTransactionByHash", error))
    }

    async fn signer_has_pending_transaction(&self) -> Result<bool, ChainError> {
        let latest = self
            .provider
            .get_transaction_count(self.signer)
            .await
            .map_err(|error| ChainError::rpc("eth_getTransactionCount latest", error))?;
        let pending = self
            .provider
            .get_transaction_count(self.signer)
            .pending()
            .await
            .map_err(|error| ChainError::rpc("eth_getTransactionCount pending", error))?;
        Ok(pending > latest)
    }

    async fn get_sweep_receipt(
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
        let failures = receipt
            .logs()
            .iter()
            .filter(|log| log.address() == batch_sweeper)
            .filter(|log| log.topics().first() == Some(&SweepFailed::SIGNATURE_HASH))
            .map(|log| {
                SweepFailed::decode_log(&log.inner)
                    .map(|log| (log.data.paymentAddress, log.data.token))
                    .map_err(|error| {
                        ChainError::Transient(format!(
                            "malformed SweepFailed event in {tx_hash}: {error}"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(SweepReceipt {
            succeeded: receipt.status(),
            block,
            block_hash,
            failures,
        }))
    }

    async fn get_usdc_transfers(
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

    async fn submit_sweep_batch(
        &self,
        batch_sweeper: Address,
        sweeps: &[SweepRequest],
    ) -> Result<B256, ChainError> {
        let tx = TransactionRequest::default()
            .with_to(batch_sweeper)
            .with_input(Self::execute_batch_calldata(sweeps));
        let pending = self.provider.send_transaction(tx).await.map_err(|error| {
            if is_execution_revert(&error) {
                ChainError::Transient(format!("batch sweep reverted during submission: {error}"))
            } else {
                ChainError::rpc("eth_sendTransaction", error)
            }
        })?;
        Ok(*pending.tx_hash())
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
    fn only_execution_errors_are_contract_reverts() {
        assert!(is_execution_revert(&rpc_error(
            3,
            "execution reverted",
            Some("0x12345678")
        )));
        assert!(is_execution_revert(&rpc_error(
            -32015,
            "VM execution error",
            None
        )));
        assert!(!is_execution_revert(&rpc_error(
            429,
            "Too Many Requests",
            None
        )));
        assert!(!is_execution_revert(&rpc_error(
            -32603,
            "Internal error",
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
}
