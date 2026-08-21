//! Chain adapter: the gateway's read-only view of the blockchain.
//!
//! The [`ChainClient`] trait is the seam the indexer depends on, so the
//! reconcile logic can be unit-tested against a mock without a live RPC.
//! [`AlloyChainClient`] is the production implementation backed by an Alloy
//! HTTP provider.

use alloy_primitives::{Address, U256};
use alloy_provider::{DynProvider, Provider, ProviderBuilder};
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
/// the canonical balance of a payment address. This is the seam mocked in tests.
#[async_trait]
pub trait ChainClient: Send + Sync {
    /// Current canonical head block number.
    async fn get_block_number(&self) -> Result<u64, ChainError>;

    /// Native-token balance of `addr` at the latest block, in base units (wei).
    async fn get_balance(&self, addr: Address) -> Result<U256, ChainError>;
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

    async fn get_balance(&self, addr: Address) -> Result<U256, ChainError> {
        self.provider
            .get_balance(addr)
            .await
            .map_err(ChainError::rpc)
    }
}
