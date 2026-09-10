//! The supported networks: their identity, their native USDC, and the
//! contract generation deployed on each. Both services read the same
//! `PAYDAY_CHAINS` JSON, so a chain is either supported everywhere or nowhere.

use std::fmt;

use alloy_primitives::{Address, B256};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{FactoryAddress, TokenAddress};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChainId(pub u64);

impl fmt::Display for ChainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What a deposit request commits to about one network it may be paid on:
/// the chain, the exact USDC contract, and the factory the address is
/// derived through. A payer's attestation selects one of these; the address
/// is derived from that one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NetworkTerms {
    pub chain_id: ChainId,
    pub token: TokenAddress,
    pub factory: FactoryAddress,
}

/// Which chain reading anchors the finality boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FinalitySource {
    /// The node's `finalized` tag. Monad maps it to "irreversible without a
    /// hard fork"; on an L2 it means L1 finality, tens of minutes behind.
    Finalized,
    /// The latest block minus `finality_confirmations`. Seconds on an L2,
    /// trusting the sequencer not to reorder what it has already served.
    Latest,
}

/// One supported chain as `PAYDAY_CHAINS` describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainConfig {
    pub chain_id: u64,
    /// Circle's native USDC proxy on this chain.
    pub usdc: Address,
    pub factory: Address,
    pub batch_sweeper: Address,
    /// keccak256 of the runtime bytecode at `factory`.
    pub factory_code_hash: B256,
    /// keccak256 of the runtime bytecode at `batch_sweeper`.
    pub batch_sweeper_code_hash: B256,
    /// Where a fresh database starts indexing; never genesis.
    pub usdc_start_block: u64,
    pub finality_source: FinalitySource,
    pub finality_confirmations: u64,
    /// Typical block interval; paces the wake catch-up after a signal.
    pub block_time_ms: u64,
    /// `eth_getLogs` range ceiling the provider accepts on this chain.
    pub log_range_size: u64,
    #[serde(default)]
    pub explorer_base_url: Option<String>,
}

impl ChainConfig {
    pub fn network(&self) -> NetworkTerms {
        NetworkTerms {
            chain_id: ChainId(self.chain_id),
            token: TokenAddress(self.usdc),
            factory: FactoryAddress(self.factory),
        }
    }

    /// The environment variable carrying this chain's RPC URL. The URL is a
    /// secret (it carries the provider token), so it never sits in the JSON.
    pub fn rpc_url_var(&self) -> String {
        format!("PAYDAY_RPC_URL_{}", self.chain_id)
    }

    /// Optional WebSocket override; `off` disables the transfer signal.
    pub fn rpc_ws_url_var(&self) -> String {
        format!("PAYDAY_RPC_WS_URL_{}", self.chain_id)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChainRegistryError {
    #[error("PAYDAY_CHAINS is not valid JSON: {0}")]
    Json(String),
    #[error("PAYDAY_CHAINS must list at least one chain")]
    Empty,
    #[error("PAYDAY_CHAINS lists chain {0} more than once")]
    Duplicate(u64),
    #[error("PAYDAY_CHAINS chain {chain_id}: {field} must be positive")]
    NotPositive { chain_id: u64, field: &'static str },
    #[error(
        "PAYDAY_CHAINS chain {chain_id}: factory {actual} differs from chain {first_chain_id}'s {expected}; \
         wrong-chain rescue only works when every chain deploys the factory at the same address"
    )]
    FactoryMismatch {
        chain_id: u64,
        first_chain_id: u64,
        expected: Address,
        actual: Address,
    },
}

/// The ordered set of supported chains. Order is the order the checkout
/// offers them and the default (first) is where the onboarding demo pays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainRegistry {
    chains: Vec<ChainConfig>,
}

impl ChainRegistry {
    pub fn new(chains: Vec<ChainConfig>) -> Result<Self, ChainRegistryError> {
        if chains.is_empty() {
            return Err(ChainRegistryError::Empty);
        }
        for (index, chain) in chains.iter().enumerate() {
            if chains[..index].iter().any(|c| c.chain_id == chain.chain_id) {
                return Err(ChainRegistryError::Duplicate(chain.chain_id));
            }
            for (field, value) in [
                ("chain_id", chain.chain_id),
                ("block_time_ms", chain.block_time_ms),
                ("log_range_size", chain.log_range_size),
            ] {
                if value == 0 {
                    return Err(ChainRegistryError::NotPositive {
                        chain_id: chain.chain_id,
                        field,
                    });
                }
            }
        }
        // A payment address derives from the factory, and wrong-chain rescue
        // returns funds only because the factory sits at the *same* address
        // everywhere. Deployment procedure (one fresh deployer key, nonce 0)
        // is what makes that true; a registry that would accept different
        // factory addresses per chain lets a procedural slip break recovery
        // silently, so refuse to serve such a configuration at all.
        let first = &chains[0];
        for chain in &chains[1..] {
            if chain.factory != first.factory {
                return Err(ChainRegistryError::FactoryMismatch {
                    chain_id: chain.chain_id,
                    first_chain_id: first.chain_id,
                    expected: first.factory,
                    actual: chain.factory,
                });
            }
        }
        Ok(Self { chains })
    }

    pub fn parse(json: &str) -> Result<Self, ChainRegistryError> {
        let chains: Vec<ChainConfig> = serde_json::from_str(json)
            .map_err(|error| ChainRegistryError::Json(error.to_string()))?;
        Self::new(chains)
    }

    /// `PAYDAY_CHAINS`, or a panic naming what is wrong: a service without a
    /// valid registry has nothing to serve.
    pub fn from_env() -> Self {
        let json = std::env::var("PAYDAY_CHAINS").expect("PAYDAY_CHAINS must be set");
        Self::parse(&json).unwrap_or_else(|error| panic!("{error}"))
    }

    pub fn chains(&self) -> &[ChainConfig] {
        &self.chains
    }

    pub fn get(&self, chain_id: u64) -> Option<&ChainConfig> {
        self.chains.iter().find(|chain| chain.chain_id == chain_id)
    }

    pub fn first(&self) -> &ChainConfig {
        &self.chains[0]
    }

    /// The terms every new deposit request commits to, in canonical order.
    pub fn networks(&self) -> Vec<NetworkTerms> {
        let mut networks: Vec<NetworkTerms> =
            self.chains.iter().map(ChainConfig::network).collect();
        networks.sort_by_key(|network| network.chain_id);
        networks
    }

    pub fn explorer_base_url(&self, chain_id: u64) -> Option<&str> {
        self.get(chain_id)
            .and_then(|chain| chain.explorer_base_url.as_deref())
    }
}

/// Display names for the chains the product knows about.
pub fn chain_name(id: u64) -> &'static str {
    match id {
        1 => "Ethereum",
        143 => "Monad",
        10_143 => "Monad Testnet",
        8453 => "Base",
        84_532 => "Base Sepolia",
        42_161 => "Arbitrum One",
        421_614 => "Arbitrum Sepolia",
        31_337 => "Local",
        31_338 => "Local 2",
        _ => "Unknown",
    }
}

/// The gas token's symbol: Monad and its testnet pay in MON, everything
/// else the product runs is ETH-denominated.
pub fn native_symbol(id: u64) -> &'static str {
    match id {
        143 | 10_143 => "MON",
        _ => "ETH",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(id: u64) -> serde_json::Value {
        serde_json::json!({
            "chain_id": id,
            "usdc": "0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
            "factory": "0x5FbDB2315678afecb367f032d93F642f64180aa3",
            "batch_sweeper": "0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0",
            "factory_code_hash": format!("0x{}", "ab".repeat(32)),
            "batch_sweeper_code_hash": format!("0x{}", "cd".repeat(32)),
            "usdc_start_block": 100,
            "finality_source": "finalized",
            "finality_confirmations": 0,
            "block_time_ms": 300,
            "log_range_size": 100,
            "explorer_base_url": "https://monadvision.com"
        })
    }

    #[test]
    fn registry_parses_orders_networks_and_names_rpc_variables() {
        let json = serde_json::json!([chain(143), chain(8453)]).to_string();
        let registry = ChainRegistry::parse(&json).unwrap();
        assert_eq!(registry.chains().len(), 2);
        assert_eq!(registry.first().chain_id, 143);
        assert_eq!(
            registry.get(8453).unwrap().rpc_url_var(),
            "PAYDAY_RPC_URL_8453"
        );
        assert_eq!(
            registry.get(8453).unwrap().rpc_ws_url_var(),
            "PAYDAY_RPC_WS_URL_8453"
        );
        assert_eq!(registry.get(1), None);
        assert_eq!(
            registry.explorer_base_url(143),
            Some("https://monadvision.com")
        );
        let networks = registry.networks();
        assert_eq!(networks[0].chain_id, ChainId(143));
        assert_eq!(networks[1].chain_id, ChainId(8453));
        assert_eq!(networks[0].token.0, registry.first().usdc);
    }

    #[test]
    fn registry_rejects_factories_that_differ_across_chains() {
        // Wrong-chain rescue depends on every chain deploying the factory at
        // the same address; a registry that would mix them must not serve.
        let mut other = chain(8453);
        other["factory"] = serde_json::json!("0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512");
        assert_eq!(
            ChainRegistry::parse(&serde_json::json!([chain(143), other]).to_string()).unwrap_err(),
            ChainRegistryError::FactoryMismatch {
                chain_id: 8453,
                first_chain_id: 143,
                expected: "0x5FbDB2315678afecb367f032d93F642f64180aa3"
                    .parse()
                    .unwrap(),
                actual: "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512"
                    .parse()
                    .unwrap(),
            }
        );
        // The same factory on every chain is still accepted.
        ChainRegistry::parse(&serde_json::json!([chain(143), chain(8453)]).to_string()).unwrap();
    }

    #[test]
    fn registry_rejects_empty_duplicate_and_zero_values() {
        assert_eq!(
            ChainRegistry::parse("[]").unwrap_err(),
            ChainRegistryError::Empty
        );
        let dup = serde_json::json!([chain(143), chain(143)]).to_string();
        assert_eq!(
            ChainRegistry::parse(&dup).unwrap_err(),
            ChainRegistryError::Duplicate(143)
        );
        let mut zero = chain(143);
        zero["block_time_ms"] = serde_json::json!(0);
        assert_eq!(
            ChainRegistry::parse(&serde_json::json!([zero]).to_string()).unwrap_err(),
            ChainRegistryError::NotPositive {
                chain_id: 143,
                field: "block_time_ms"
            }
        );
        let mut unknown = chain(143);
        unknown["name"] = serde_json::json!("Monad");
        assert!(matches!(
            ChainRegistry::parse(&serde_json::json!([unknown]).to_string()).unwrap_err(),
            ChainRegistryError::Json(_)
        ));
    }

    #[test]
    fn names_and_gas_symbols_cover_the_supported_chains() {
        assert_eq!(chain_name(143), "Monad");
        assert_eq!(chain_name(8453), "Base");
        assert_eq!(chain_name(42_161), "Arbitrum One");
        assert_eq!(chain_name(7), "Unknown");
        assert_eq!(native_symbol(143), "MON");
        assert_eq!(native_symbol(8453), "ETH");
    }
}
