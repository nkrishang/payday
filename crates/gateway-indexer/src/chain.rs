//! Chain adapter: the gateway's view of the blockchain.
//!
//! The [`ChainClient`] trait is the seam the indexer depends on, so the
//! reconcile and sweep logic can be unit-tested against a mock without a live
//! RPC. [`AlloyChainClient`] is the production implementation backed by an Alloy
//! HTTP provider with a signing wallet (the backend key that calls
//! `PaymentFactory.execute`).

use alloy_network::{EthereumWallet, TransactionBuilder};
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_provider::bindings::IMulticall3::getEthBalanceCall;
use alloy_provider::{CallItem, DynProvider, MULTICALL3_ADDRESS, Provider, ProviderBuilder};
use alloy_rpc_types_eth::TransactionRequest;
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::{SolCall, sol};
use async_trait::async_trait;
use thiserror::Error;

sol! {
    /// `PaymentFactory.execute`: CREATE3-deploys the `Payment` contract at the
    /// counterfactual payment address, whose constructor sweeps the funds to the
    /// receiver. Reverts with `DeploymentFailed` if the address already has code
    /// (already executed) or the receiver rejects the transfer.
    function execute(address token, uint256 amount, address receiver, bytes32 salt);
}

/// Errors from talking to the chain over RPC.
#[derive(Debug, Error)]
pub enum ChainError {
    /// The RPC transport or node returned an error.
    #[error("chain RPC error: {0}")]
    Rpc(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// A sweep did not confirm for a non-deterministic reason (the transaction
    /// was not mined, or a read failed) and should be retried. Distinct from a
    /// contract revert, which is classified into a [`SweepOutcome`].
    #[error("transient sweep failure: {0}")]
    Transient(String),
}

impl ChainError {
    fn rpc<E: std::error::Error + Send + Sync + 'static>(err: E) -> Self {
        ChainError::Rpc(Box::new(err))
    }
}

/// The result of attempting to sweep one invoice's payment address.
///
/// These are the *non-transient* outcomes: each maps to a terminal-ish invoice
/// transition. A transient failure (transport error, unmined tx) is reported as
/// `Err(ChainError)` instead, so the caller retries rather than transitioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepOutcome {
    /// Our `execute` transaction mined successfully and swept the funds.
    Executed { tx_hash: B256, block: u64 },
    /// The `Payment` contract already exists at the payment address, so the
    /// payment was already executed — by our own earlier (recovered) transaction
    /// or by a permissionless third-party call. Treated as fulfilled.
    AlreadyDeployed,
    /// The deploy reverts and the address has no code: the sweep cannot succeed
    /// (the beneficiary rejects the native transfer). Requires manual
    /// intervention; must not be auto-retried.
    Blocked,
}

/// Chain access needed by the indexer: read balances/head for funding, and send
/// the sweep transaction for execution. This is the seam mocked in tests.
#[async_trait]
pub trait ChainClient: Send + Sync {
    /// Current canonical head block number.
    async fn get_block_number(&self) -> Result<u64, ChainError>;

    /// Native-token balances of `addrs` at the latest block, in base units
    /// (wei), returned in the same order as `addrs`.
    ///
    /// One call reads many addresses so the indexer does not fan out one RPC
    /// request per invoice. The batch is bounded by the caller (see the
    /// indexer's Multicall chunk size), and the caller runs multiple batches
    /// concurrently.
    async fn get_balances(&self, addrs: &[Address]) -> Result<Vec<U256>, ChainError>;

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
            .map_err(ChainError::rpc)?
            .erased();

        Ok(Self { provider })
    }

    /// Fetch the chain ID reported by the node. Used for a startup assertion
    /// that the configured chain matches what the RPC endpoint actually serves.
    pub async fn get_chain_id(&self) -> Result<u64, ChainError> {
        self.provider.get_chain_id().await.map_err(ChainError::rpc)
    }

    /// Whether an address has contract code deployed. `execute` deploying the
    /// `Payment` contract is the only way code lands at a payment address, so
    /// code present means the sweep already happened.
    async fn has_code(&self, addr: Address) -> Result<bool, ChainError> {
        let code = self
            .provider
            .get_code_at(addr)
            .await
            .map_err(ChainError::rpc)?;
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

    /// Send the `execute` transaction and wait for its receipt. Returns
    /// `Ok(Some((tx_hash, block)))` only on a mined, successful sweep;
    /// `Ok(None)` when the transaction reverted or the node rejected it as a
    /// guaranteed revert (the caller then classifies via on-chain reads); and
    /// `Err` for a transport failure (retryable).
    async fn try_send_execute(
        &self,
        factory: Address,
        calldata: Bytes,
    ) -> Result<Option<(B256, u64)>, ChainError> {
        let tx = TransactionRequest::default()
            .with_to(factory)
            .with_input(calldata);

        let pending = match self.provider.send_transaction(tx).await {
            Ok(pending) => pending,
            // A JSON-RPC error response means the node executed the call (e.g.
            // during gas estimation) and it reverted — deterministic, so let the
            // caller classify it. Anything else is a transport failure -> retry.
            Err(e) if e.as_error_resp().is_some() => return Ok(None),
            Err(e) => return Err(ChainError::rpc(e)),
        };

        let receipt = pending.get_receipt().await.map_err(ChainError::rpc)?;
        if receipt.status() {
            Ok(Some((receipt.transaction_hash, receipt.block_number.unwrap_or_default())))
        } else {
            Ok(None)
        }
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
            Err(e) if e.as_error_resp().is_some() => Ok(true),
            Err(e) => Err(ChainError::rpc(e)),
        }
    }
}

#[async_trait]
impl ChainClient for AlloyChainClient {
    async fn get_block_number(&self) -> Result<u64, ChainError> {
        self.provider
            .get_block_number()
            .await
            .map_err(ChainError::rpc)
    }

    /// Read every balance in one Multicall3 `aggregate` call. Each sub-call is
    /// `Multicall3.getEthBalance(addr)`, so N native-balance reads collapse into
    /// a single `eth_call` round-trip against the canonical Multicall3 contract
    /// (deployed at the same address on Anvil and virtually every EVM chain).
    async fn get_balances(&self, addrs: &[Address]) -> Result<Vec<U256>, ChainError> {
        if addrs.is_empty() {
            return Ok(Vec::new());
        }

        let mut multicall = self.provider.multicall().dynamic::<getEthBalanceCall>();
        for &addr in addrs {
            let call = CallItem::<getEthBalanceCall>::new(
                MULTICALL3_ADDRESS,
                getEthBalanceCall { addr }.abi_encode().into(),
            );
            multicall = multicall.add_call_dynamic(call);
        }

        // `getEthBalance` cannot revert, so `aggregate` (all-or-nothing) is
        // safe: any error here is a transport/node failure for the whole batch,
        // which the indexer retries on its next tick.
        let balances = multicall.aggregate().await.map_err(ChainError::rpc)?;
        Ok(balances)
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

        if let Some((tx_hash, block)) = self.try_send_execute(factory, calldata.clone()).await? {
            return Ok(SweepOutcome::Executed { tx_hash, block });
        }

        // The transaction did not confirm successfully. Classify from fresh
        // on-chain reads rather than the failure itself, so a race (someone
        // executed between our checks) or a transient miss is handled correctly.
        if self.has_code(payment_address).await? {
            return Ok(SweepOutcome::AlreadyDeployed);
        }

        if self.would_execute_revert(factory, calldata).await? {
            // Reverts and no code was deployed -> the deploy can never succeed:
            // the beneficiary rejects the transfer. Permanently blocked.
            Ok(SweepOutcome::Blocked)
        } else {
            // A fresh simulation says it would succeed, so the earlier failure
            // was transient. Retry on the next pass.
            Err(ChainError::Transient(
                "execute did not confirm but would succeed on retry".to_string(),
            ))
        }
    }
}
