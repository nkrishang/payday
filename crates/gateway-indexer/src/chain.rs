//! Chain adapter: the gateway's read-only view of the blockchain.
//!
//! The [`ChainClient`] trait is the seam the indexer depends on, so the
//! reconcile logic can be unit-tested against a mock without a live RPC.
//! [`AlloyChainClient`] is the production implementation backed by an Alloy
//! HTTP provider.

use alloy_primitives::{Address, U256};
use alloy_provider::bindings::IMulticall3::getEthBalanceCall;
use alloy_provider::{CallItem, DynProvider, MULTICALL3_ADDRESS, Provider, ProviderBuilder};
use alloy_sol_types::SolCall;
use async_trait::async_trait;
use thiserror::Error;

/// Errors from talking to the chain over RPC.
#[derive(Debug, Error)]
pub enum ChainError {
    /// The RPC transport or node returned an error.
    #[error("chain RPC error: {0}")]
    Rpc(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl ChainError {
    fn rpc<E: std::error::Error + Send + Sync + 'static>(err: E) -> Self {
        ChainError::Rpc(Box::new(err))
    }
}

/// Read-only chain access needed by the indexer.
///
/// Kept intentionally small: the indexer only needs the current head height and
/// the canonical balances of a set of payment addresses. This is the seam
/// mocked in tests.
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
}

/// Production [`ChainClient`] backed by an Alloy HTTP provider.
pub struct AlloyChainClient {
    provider: DynProvider,
}

impl AlloyChainClient {
    /// Connect to the RPC endpoint at `rpc_url` (e.g. `http://127.0.0.1:8545`).
    pub async fn connect(rpc_url: &str) -> Result<Self, ChainError> {
        let provider = ProviderBuilder::new()
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
}
