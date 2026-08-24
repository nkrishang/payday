//! Chain adapter: the gateway's view of the blockchain.
//!
//! The [`ChainClient`] trait is the seam the indexer depends on, so the
//! reconcile and sweep logic can be unit-tested against a mock without a live
//! RPC. [`AlloyChainClient`] is the production implementation backed by an Alloy
//! HTTP provider with a signing wallet (the backend key that calls
//! `PaymentFactory.execute`).

use alloy_network::{EthereumWallet, TransactionBuilder};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use alloy_provider::{DynProvider, Provider, ProviderBuilder};
use alloy_rpc_types_eth::{BlockNumberOrTag, Filter, TransactionRequest};
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::{SolCall, sol};
use alloy_transport::TransportError;
use async_trait::async_trait;
use thiserror::Error;

sol! {
    /// `PaymentFactory.execute`: CREATE3-deploys the `Payment` contract at the
    /// counterfactual payment address, whose constructor sweeps the funds to the
    /// receiver. Reverts with `DeploymentFailed` if the address already has code
    /// (already executed) or the token transfer fails.
    function execute(address token, uint256 amount, address receiver, bytes32 salt);
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
    /// contract revert, which is classified into a [`SweepOutcome`].
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

/// The result of attempting to sweep one invoice's payment address.
///
/// These are the *non-transient* outcomes: each maps to a terminal-ish invoice
/// transition. A transient failure (transport error, unmined tx) is reported as
/// `Err(ChainError)` instead, so the caller retries rather than transitioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepOutcome {
    /// Our `execute` transaction was accepted. The caller persists its hash
    /// immediately and follows the receipt on later passes.
    Submitted { tx_hash: B256 },
    /// The `Payment` contract already exists at the payment address, so the
    /// payment was already executed — by our own earlier (recovered) transaction
    /// or by a permissionless third-party call. Treated as fulfilled.
    AlreadyDeployed,
    /// The ERC-20 transfer deterministically reverted and no Payment code was
    /// deployed. This may indicate pause, blacklist, or underfunding and needs
    /// operator intervention.
    TokenTransferFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SweepReceipt {
    pub succeeded: bool,
    pub block: u64,
    pub block_hash: B256,
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

    /// Mined receipt for a submitted sweep, or `None` while still pending.
    async fn get_sweep_receipt(&self, tx_hash: B256) -> Result<Option<SweepReceipt>, ChainError>;

    /// Successful USDC `Transfer` logs in an inclusive block range.
    async fn get_usdc_transfers(
        &self,
        token: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<UsdcTransfer>, ChainError>;

    /// Sweep one invoice: call `PaymentFactory.execute(token, amount, receiver,
    /// salt)` on `factory` to move the funds at `payment_address` to `receiver`.
    ///
    /// Returns a [`SweepOutcome`] for the deterministic cases (executed, already
    /// executed, or permanently blocked), all decided from on-chain reads rather
    /// than by parsing revert strings. A transient failure returns
    /// `Err(ChainError)`, signalling the caller to retry later.
    async fn sweep(
        &self,
        factory: Address,
        token: Address,
        amount: U256,
        receiver: Address,
        salt: B256,
        payment_address: Address,
    ) -> Result<SweepOutcome, ChainError>;
}

/// Production [`ChainClient`] backed by an Alloy HTTP provider with a signing
/// wallet for the sweep transactions.
pub struct AlloyChainClient {
    provider: DynProvider,
}

impl AlloyChainClient {
    /// Connect to the RPC endpoint at `rpc_url` (e.g. `http://127.0.0.1:8545`),
    /// signing sweep transactions with `signer` (the backend key). Reads work
    /// the same as an unsigned provider; the wallet only adds send capability.
    pub async fn connect(rpc_url: &str, signer: PrivateKeySigner) -> Result<Self, ChainError> {
        let provider = ProviderBuilder::new()
            .wallet(EthereumWallet::from(signer))
            .connect(rpc_url)
            .await
            .map_err(|error| ChainError::rpc("connect", error))?
            .erased();

        Ok(Self { provider })
    }

    /// Fetch the chain ID reported by the node. Used for a startup assertion
    /// that the configured chain matches what the RPC endpoint actually serves.
    pub async fn get_chain_id(&self) -> Result<u64, ChainError> {
        self.provider
            .get_chain_id()
            .await
            .map_err(|error| ChainError::rpc("eth_chainId", error))
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

    fn execute_calldata(token: Address, amount: U256, receiver: Address, salt: B256) -> Bytes {
        executeCall {
            token,
            amount,
            receiver,
            salt,
        }
        .abi_encode()
        .into()
    }

    /// Submit `execute`. Returns its hash as soon as the provider accepts it,
    /// before receipt polling, or `None` for a verified estimation revert.
    async fn try_send_execute(
        &self,
        factory: Address,
        calldata: Bytes,
    ) -> Result<Option<B256>, ChainError> {
        let tx = TransactionRequest::default()
            .with_to(factory)
            .with_input(calldata);

        let pending = match self.provider.send_transaction(tx).await {
            Ok(pending) => pending,
            // Only explicit execution-revert responses are contract failures.
            // Rate limits, internal errors, auth failures, and malformed requests
            // retain their RPC classification and are never treated as reverts.
            Err(e) if is_execution_revert(&e) => return Ok(None),
            Err(e) => return Err(ChainError::rpc("eth_sendTransaction", e)),
        };

        Ok(Some(*pending.tx_hash()))
    }

    /// Simulate `execute` with `eth_call` to tell a deterministic revert from a
    /// transient miss. `Ok(true)` = it reverts (deterministic), `Ok(false)` = it
    /// would succeed, `Err` = a transport failure.
    async fn would_execute_revert(
        &self,
        factory: Address,
        calldata: Bytes,
    ) -> Result<bool, ChainError> {
        let tx = TransactionRequest::default()
            .with_to(factory)
            .with_input(calldata);

        match self.provider.call(tx).await {
            Ok(_) => Ok(false),
            Err(e) if is_execution_revert(&e) => Ok(true),
            Err(e) => Err(ChainError::rpc("eth_call", e)),
        }
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

    async fn get_sweep_receipt(&self, tx_hash: B256) -> Result<Option<SweepReceipt>, ChainError> {
        let Some(receipt) = self
            .provider
            .get_transaction_receipt(tx_hash)
            .await
            .map_err(|error| ChainError::rpc("eth_getTransactionReceipt", error))?
        else {
            return Ok(None);
        };
        let block = receipt.block_number.ok_or_else(|| {
            ChainError::Transient("execute receipt has no block number".to_string())
        })?;
        let block_hash = receipt.block_hash.ok_or_else(|| {
            ChainError::Transient("execute receipt has no block hash".to_string())
        })?;
        Ok(Some(SweepReceipt {
            succeeded: receipt.status(),
            block,
            block_hash,
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

    async fn sweep(
        &self,
        factory: Address,
        token: Address,
        amount: U256,
        receiver: Address,
        salt: B256,
        payment_address: Address,
    ) -> Result<SweepOutcome, ChainError> {
        // Idempotency / crash recovery: if the payment address already has code,
        // the sweep already happened (our earlier tx that landed before we
        // recorded it, or a third-party call). No transaction needed.
        if self.has_code(payment_address).await? {
            return Ok(SweepOutcome::AlreadyDeployed);
        }

        let calldata = Self::execute_calldata(token, amount, receiver, salt);

        match self.try_send_execute(factory, calldata.clone()).await {
            Ok(Some(tx_hash)) => {
                return Ok(SweepOutcome::Submitted { tx_hash });
            }
            Ok(None) => {}
            Err(error) => {
                if self.has_code(payment_address).await? {
                    return Ok(SweepOutcome::AlreadyDeployed);
                }
                return Err(error);
            }
        }

        // The transaction did not confirm successfully. Classify from fresh
        // on-chain reads rather than the failure itself, so a race (someone
        // executed between our checks) or a transient miss is handled correctly.
        if self.has_code(payment_address).await? {
            return Ok(SweepOutcome::AlreadyDeployed);
        }

        if self.would_execute_revert(factory, calldata).await? {
            Ok(SweepOutcome::TokenTransferFailed)
        } else {
            // A fresh simulation says it would succeed, so the earlier failure
            // was transient. Retry on the next pass.
            Err(ChainError::Transient(
                "execute did not confirm but would succeed on retry".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use alloy_json_rpc::{ErrorPayload, RpcError};
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
}
