//! Read-only chain access for the API: a wallet's USDC balance on each
//! chain, and each chain's USDC as an EIP-712 domain.
//!
//! `POST /v1/withdrawals` snapshots balances into legs and hands the
//! merchant typed data to sign under the token's own domain. The domain is
//! read from the token at startup (`name()`, `version()`) and checked
//! against its `DOMAIN_SEPARATOR()`, because Circle's deployments differ in
//! the name ("USDC" on Monad, "USD Coin" on Base and Arbitrum) and a wrong
//! domain would produce signatures the token rejects only once relayed.

use std::collections::HashMap;

use alloy_primitives::{Address, U256};
use alloy_provider::{DynProvider, Provider, ProviderBuilder};
use alloy_rpc_types_eth::{TransactionInput, TransactionRequest};
use alloy_sol_types::{SolCall, sol};
use async_trait::async_trait;
use gateway_core::{ChainRegistry, UsdcDomain};
use thiserror::Error;

use crate::deployment::redact_urls;

sol! {
    function name() view returns (string);
    function version() view returns (string);
    function DOMAIN_SEPARATOR() view returns (bytes32);
    function balanceOf(address account) view returns (uint256);
}

#[derive(Debug, Error)]
pub enum ChainReadError {
    #[error("chain {0} is not served by this deployment")]
    Unsupported(u64),
    #[error("{operation} failed on chain {chain_id}: {message}")]
    Rpc {
        chain_id: u64,
        operation: &'static str,
        message: String,
    },
}

/// The seam the withdrawal routes depend on, so they are testable without an RPC.
#[async_trait]
pub trait ChainReads: Send + Sync {
    /// `wallet`'s USDC balance on `chain_id` at the latest block.
    async fn usdc_balance(&self, chain_id: u64, wallet: Address) -> Result<U256, ChainReadError>;

    /// The chain's USDC as an EIP-712 domain, as read at startup.
    fn usdc_domain(&self, chain_id: u64) -> Option<&UsdcDomain>;
}

struct ReadableChain {
    provider: DynProvider,
    domain: UsdcDomain,
}

/// Production [`ChainReads`] over one unsigned Alloy provider per chain.
pub struct AlloyChainReader {
    chains: HashMap<u64, ReadableChain>,
}

impl AlloyChainReader {
    /// Connect to every registered chain and read its USDC domain.
    pub async fn connect(
        registry: &ChainRegistry,
        rpc_url: impl Fn(u64) -> String,
    ) -> Result<Self, ChainReadError> {
        let mut chains = HashMap::new();
        for chain in registry.chains() {
            let chain_id = chain.chain_id;
            let rpc =
                |operation: &'static str, error: &dyn std::fmt::Display| ChainReadError::Rpc {
                    chain_id,
                    operation,
                    message: redact_urls(&error.to_string()),
                };
            let provider = ProviderBuilder::new()
                .connect(&rpc_url(chain_id))
                .await
                .map_err(|error| rpc("connect", &error))?
                .erased();
            let call = |input: Vec<u8>| {
                TransactionRequest::default()
                    .to(chain.usdc)
                    .input(TransactionInput::new(input.into()))
            };
            let name = provider
                .call(call(nameCall {}.abi_encode()))
                .await
                .map_err(|error| rpc("eth_call USDC.name", &error))?;
            let name = nameCall::abi_decode_returns(&name)
                .map_err(|error| rpc("eth_call USDC.name", &error))?;
            let version = provider
                .call(call(versionCall {}.abi_encode()))
                .await
                .map_err(|error| rpc("eth_call USDC.version", &error))?;
            let version = versionCall::abi_decode_returns(&version)
                .map_err(|error| rpc("eth_call USDC.version", &error))?;
            let separator = provider
                .call(call(DOMAIN_SEPARATORCall {}.abi_encode()))
                .await
                .map_err(|error| rpc("eth_call USDC.DOMAIN_SEPARATOR", &error))?;
            let separator = DOMAIN_SEPARATORCall::abi_decode_returns(&separator)
                .map_err(|error| rpc("eth_call USDC.DOMAIN_SEPARATOR", &error))?;
            let domain = UsdcDomain {
                name,
                version,
                chain_id,
                token: chain.usdc,
            };
            if domain.separator() != separator {
                return Err(ChainReadError::Rpc {
                    chain_id,
                    operation: "USDC domain check",
                    message: format!(
                        "EIP712Domain(name {:?}, version {:?}, chainId {chain_id}, verifyingContract {}) hashes to {}, but the token's DOMAIN_SEPARATOR is {separator}; the token is not a FiatToken this build understands",
                        domain.name,
                        domain.version,
                        chain.usdc,
                        domain.separator()
                    ),
                });
            }
            tracing::info!(chain_id, usdc = %chain.usdc, name = %domain.name, version = %domain.version, "read USDC EIP-712 domain");
            chains.insert(chain_id, ReadableChain { provider, domain });
        }
        Ok(Self { chains })
    }
}

#[async_trait]
impl ChainReads for AlloyChainReader {
    async fn usdc_balance(&self, chain_id: u64, wallet: Address) -> Result<U256, ChainReadError> {
        let chain = self
            .chains
            .get(&chain_id)
            .ok_or(ChainReadError::Unsupported(chain_id))?;
        let call =
            TransactionRequest::default()
                .to(chain.domain.token)
                .input(TransactionInput::new(
                    balanceOfCall { account: wallet }.abi_encode().into(),
                ));
        let output = chain
            .provider
            .call(call)
            .await
            .map_err(|error| ChainReadError::Rpc {
                chain_id,
                operation: "eth_call USDC.balanceOf",
                message: redact_urls(&error.to_string()),
            })?;
        balanceOfCall::abi_decode_returns(&output).map_err(|error| ChainReadError::Rpc {
            chain_id,
            operation: "eth_call USDC.balanceOf",
            message: error.to_string(),
        })
    }

    fn usdc_domain(&self, chain_id: u64) -> Option<&UsdcDomain> {
        self.chains.get(&chain_id).map(|chain| &chain.domain)
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Mutex;

    use super::*;

    /// Balances set by a test, and a domain per registered chain.
    pub(crate) struct FakeChainReader {
        pub balances: Mutex<HashMap<(u64, Address), U256>>,
        pub domains: HashMap<u64, UsdcDomain>,
        /// Chains whose balance read fails, as an unreachable RPC would.
        pub failing: Mutex<Vec<u64>>,
    }

    impl FakeChainReader {
        pub(crate) fn new(registry: &ChainRegistry) -> Self {
            Self {
                balances: Mutex::new(HashMap::new()),
                domains: registry
                    .chains()
                    .iter()
                    .map(|chain| {
                        (
                            chain.chain_id,
                            UsdcDomain {
                                name: "USD Coin".into(),
                                version: "2".into(),
                                chain_id: chain.chain_id,
                                token: chain.usdc,
                            },
                        )
                    })
                    .collect(),
                failing: Mutex::new(Vec::new()),
            }
        }

        pub(crate) fn set_balance(&self, chain_id: u64, wallet: Address, amount: u64) {
            self.balances
                .lock()
                .unwrap()
                .insert((chain_id, wallet), U256::from(amount));
        }
    }

    #[async_trait]
    impl ChainReads for FakeChainReader {
        async fn usdc_balance(
            &self,
            chain_id: u64,
            wallet: Address,
        ) -> Result<U256, ChainReadError> {
            if self.failing.lock().unwrap().contains(&chain_id) {
                return Err(ChainReadError::Rpc {
                    chain_id,
                    operation: "eth_call USDC.balanceOf",
                    message: "connection refused".into(),
                });
            }
            if !self.domains.contains_key(&chain_id) {
                return Err(ChainReadError::Unsupported(chain_id));
            }
            Ok(self
                .balances
                .lock()
                .unwrap()
                .get(&(chain_id, wallet))
                .copied()
                .unwrap_or(U256::ZERO))
        }

        fn usdc_domain(&self, chain_id: u64) -> Option<&UsdcDomain> {
            self.domains.get(&chain_id)
        }
    }
}
