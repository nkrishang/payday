//! The supported networks: their identity, the stablecoin contracts each
//! carries, and the contract generation deployed on each. Both services read
//! the same `PAYDAY_CHAINS` JSON, so a chain is either supported everywhere
//! or nowhere, and a currency is served on a chain exactly when that chain's
//! entry lists its contract.

use std::fmt;

use alloy_primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Currency, FactoryAddress, TokenAddress};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChainId(pub u64);

impl fmt::Display for ChainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What a deposit request commits to about one network it may be paid on:
/// the chain, the exact contract of the request's currency there, and the
/// factory the address is derived through. A payer's attestation selects one
/// of these; the address is derived from that one. A request has one
/// currency, so no two of its networks share a chain.
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

/// One stablecoin contract on one chain: the issuer's canonical deployment
/// there (Circle's native USDC proxy; Tether's USDT0 on Monad and Arbitrum),
/// never a bridged look-alike.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenConfig {
    pub currency: Currency,
    pub address: Address,
}

/// One supported chain as `PAYDAY_CHAINS` describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainConfig {
    pub chain_id: u64,
    /// The currencies served on this chain, each by its exact contract. A
    /// currency absent here is simply not offered on this chain.
    pub tokens: Vec<TokenConfig>,
    pub factory: Address,
    pub batch_sweeper: Address,
    /// keccak256 of the runtime bytecode at `factory`.
    pub factory_code_hash: B256,
    /// keccak256 of the runtime bytecode at `batch_sweeper`.
    pub batch_sweeper_code_hash: B256,
    /// Where a fresh database starts indexing this chain; never genesis. A
    /// currency added to a live chain needs no earlier block: no request
    /// could have offered it before the deployment that serves it.
    pub start_block: u64,
    pub finality_source: FinalitySource,
    pub finality_confirmations: u64,
    /// Typical block interval; paces the wake catch-up after a signal.
    pub block_time_ms: u64,
    /// `eth_getLogs` range ceiling the provider accepts on this chain.
    pub log_range_size: u64,
    /// The sweep signer's low-balance alarm level on this chain, in wei of
    /// its native token: one chain's hundred batches is another's dust, and
    /// the number only means anything against the chain's own gas price.
    /// Absent, the indexer's shared default applies.
    #[serde(default)]
    pub signer_low_balance_wei: Option<u64>,
    #[serde(default)]
    pub explorer_base_url: Option<String>,
    /// Circle's CCTP V2 on this chain and the withdrawal forwarder deployed
    /// against it. Absent on a chain without CCTP (local Anvil): USDC there
    /// can only leave by a same-chain transfer. CCTP moves USDC alone; no
    /// other currency ever has a bridge leg.
    #[serde(default)]
    pub cctp: Option<CctpConfig>,
}

/// The largest amount a single CCTP V2 standard-transfer burn may move:
/// Circle documents a 10,000,000 USDC per-transaction limit and its
/// `TokenMessengerV2` reverts above it. A whole-balance withdrawal whose
/// bridge leg exceeds it could never execute, so `POST /v1/withdrawals`
/// refuses to create one; the web panel applies the same bound to the
/// balances it shows.
pub const MAX_CCTP_BURN_PER_MESSAGE: U256 = U256::from_limbs([10_000_000_000_000, 0, 0, 0]);

/// CCTP V2 as deployed on one chain, plus Payday's `WithdrawalForwarder`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CctpConfig {
    /// Circle's domain id for the chain (Arbitrum 3, Base 6, Monad 15).
    pub domain: u32,
    pub token_messenger: Address,
    pub message_transmitter: Address,
    /// The `WithdrawalForwarder` a merchant authorizes as the payee of a
    /// bridge leg.
    pub forwarder: Address,
    /// keccak256 of the runtime bytecode at `forwarder`.
    pub forwarder_code_hash: B256,
}

impl ChainConfig {
    /// The contract serving `currency` on this chain, if it is offered here.
    pub fn token(&self, currency: Currency) -> Option<&TokenConfig> {
        self.tokens.iter().find(|token| token.currency == currency)
    }

    /// The currency `address` serves on this chain, if it is one of ours.
    pub fn currency_of(&self, address: Address) -> Option<Currency> {
        self.tokens
            .iter()
            .find(|token| token.address == address)
            .map(|token| token.currency)
    }

    /// Every contract address served on this chain, in registry order: the
    /// indexer's log filter.
    pub fn token_addresses(&self) -> Vec<Address> {
        self.tokens.iter().map(|token| token.address).collect()
    }

    /// The terms a request in `currency` commits to on this chain, if the
    /// chain serves it.
    pub fn network(&self, currency: Currency) -> Option<NetworkTerms> {
        self.token(currency).map(|token| NetworkTerms {
            chain_id: ChainId(self.chain_id),
            token: TokenAddress(token.address),
            factory: FactoryAddress(self.factory),
        })
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
    #[error("PAYDAY_CHAINS chain {0}: tokens must list at least one currency")]
    NoTokens(u64),
    #[error("PAYDAY_CHAINS chain {chain_id}: {currency} is listed more than once")]
    DuplicateCurrency { chain_id: u64, currency: Currency },
    #[error("PAYDAY_CHAINS chain {chain_id}: {address} is listed under more than one currency")]
    DuplicateToken { chain_id: u64, address: Address },
    #[error(
        "PAYDAY_CHAINS chain {0}: a cctp block needs USDC on the chain; CCTP burns and mints USDC alone"
    )]
    CctpWithoutUsdc(u64),
    #[error("PAYDAY_CHAINS must list USDC on at least one chain")]
    NoUsdc,
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
/// offers them; the first is the default.
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
            if chain.tokens.is_empty() {
                return Err(ChainRegistryError::NoTokens(chain.chain_id));
            }
            for (index, token) in chain.tokens.iter().enumerate() {
                let earlier = &chain.tokens[..index];
                if earlier.iter().any(|t| t.currency == token.currency) {
                    return Err(ChainRegistryError::DuplicateCurrency {
                        chain_id: chain.chain_id,
                        currency: token.currency,
                    });
                }
                if earlier.iter().any(|t| t.address == token.address) {
                    return Err(ChainRegistryError::DuplicateToken {
                        chain_id: chain.chain_id,
                        address: token.address,
                    });
                }
            }
            if chain.cctp.is_some() && chain.token(Currency::Usdc).is_none() {
                return Err(ChainRegistryError::CctpWithoutUsdc(chain.chain_id));
            }
        }
        if !chains
            .iter()
            .any(|chain| chain.token(Currency::Usdc).is_some())
        {
            return Err(ChainRegistryError::NoUsdc);
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

    /// The first chain serving USDC, in registry order. The registry refuses
    /// to exist without one.
    pub fn first_usdc(&self) -> (&ChainConfig, &TokenConfig) {
        self.chains
            .iter()
            .find_map(|chain| chain.token(Currency::Usdc).map(|token| (chain, token)))
            .expect("ChainRegistry::new requires USDC on some chain")
    }

    /// The terms a new deposit request in `currency` commits to, in
    /// canonical order: one entry per chain serving it. Empty when no chain
    /// does.
    pub fn networks(&self, currency: Currency) -> Vec<NetworkTerms> {
        let mut networks: Vec<NetworkTerms> = self
            .chains
            .iter()
            .filter_map(|chain| chain.network(currency))
            .collect();
        networks.sort_by_key(|network| network.chain_id);
        networks
    }

    /// The currencies at least one chain serves, in code order.
    pub fn currencies(&self) -> Vec<Currency> {
        Currency::ALL
            .into_iter()
            .filter(|currency| {
                self.chains
                    .iter()
                    .any(|chain| chain.token(*currency).is_some())
            })
            .collect()
    }

    /// The contract serving `currency` on `chain_id`, if both are served.
    pub fn token(&self, chain_id: u64, currency: Currency) -> Option<&TokenConfig> {
        self.get(chain_id).and_then(|chain| chain.token(currency))
    }

    /// The currency `address` serves on `chain_id`, if it is one of ours there.
    pub fn currency_of(&self, chain_id: u64, address: Address) -> Option<Currency> {
        self.get(chain_id)
            .and_then(|chain| chain.currency_of(address))
    }

    pub fn explorer_base_url(&self, chain_id: u64) -> Option<&str> {
        self.get(chain_id)
            .and_then(|chain| chain.explorer_base_url.as_deref())
    }

    /// CCTP on `chain_id`, if the chain has it.
    pub fn cctp(&self, chain_id: u64) -> Option<&CctpConfig> {
        self.get(chain_id).and_then(|chain| chain.cctp.as_ref())
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

    const USDC: &str = "0x754704Bc059F8C67012fEd69BC8A327a5aafb603";
    const USDT: &str = "0xe7cd86e13AC4309349F30B3435a9d337750fC82D";

    fn chain(id: u64) -> serde_json::Value {
        serde_json::json!({
            "chain_id": id,
            "tokens": [{"currency": "USDC", "address": USDC}],
            "factory": "0x5FbDB2315678afecb367f032d93F642f64180aa3",
            "batch_sweeper": "0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0",
            "factory_code_hash": format!("0x{}", "ab".repeat(32)),
            "batch_sweeper_code_hash": format!("0x{}", "cd".repeat(32)),
            "start_block": 100,
            "finality_source": "finalized",
            "finality_confirmations": 0,
            "block_time_ms": 300,
            "log_range_size": 100,
            "explorer_base_url": "https://monadvision.com"
        })
    }

    fn with_usdt(mut chain: serde_json::Value) -> serde_json::Value {
        chain["tokens"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"currency": "USDT", "address": USDT}));
        chain
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
        let networks = registry.networks(Currency::Usdc);
        assert_eq!(networks[0].chain_id, ChainId(143));
        assert_eq!(networks[1].chain_id, ChainId(8453));
        assert_eq!(
            networks[0].token.0,
            registry.first().token(Currency::Usdc).unwrap().address
        );
        assert_eq!(registry.networks(Currency::Usdt), vec![]);
        assert_eq!(registry.currencies(), vec![Currency::Usdc]);
    }

    #[test]
    fn a_currency_is_offered_exactly_on_the_chains_listing_its_contract() {
        // USDT on Monad only: a USDT request commits to that one network,
        // while USDC still commits to both.
        let json = serde_json::json!([with_usdt(chain(143)), chain(8453)]).to_string();
        let registry = ChainRegistry::parse(&json).unwrap();
        assert_eq!(registry.currencies(), vec![Currency::Usdc, Currency::Usdt]);
        let usdt = registry.networks(Currency::Usdt);
        assert_eq!(usdt.len(), 1);
        assert_eq!(usdt[0].chain_id, ChainId(143));
        assert_eq!(usdt[0].token.0, USDT.parse::<Address>().unwrap());
        assert_eq!(registry.networks(Currency::Usdc).len(), 2);
        assert_eq!(
            registry.token(143, Currency::Usdt).map(|t| t.address),
            Some(USDT.parse().unwrap())
        );
        assert_eq!(registry.token(8453, Currency::Usdt), None);
        assert_eq!(
            registry.currency_of(143, USDT.parse().unwrap()),
            Some(Currency::Usdt)
        );
        assert_eq!(registry.currency_of(8453, USDT.parse().unwrap()), None);
        assert_eq!(
            registry.get(143).unwrap().token_addresses(),
            vec![USDC.parse::<Address>().unwrap(), USDT.parse().unwrap()]
        );
        let (first, token) = registry.first_usdc();
        assert_eq!((first.chain_id, token.currency), (143, Currency::Usdc));
    }

    #[test]
    fn registry_rejects_token_lists_that_could_not_serve() {
        let mut none = chain(143);
        none["tokens"] = serde_json::json!([]);
        assert_eq!(
            ChainRegistry::parse(&serde_json::json!([none]).to_string()).unwrap_err(),
            ChainRegistryError::NoTokens(143)
        );
        let twice = with_usdt(with_usdt(chain(143)));
        assert_eq!(
            ChainRegistry::parse(&serde_json::json!([twice]).to_string()).unwrap_err(),
            ChainRegistryError::DuplicateCurrency {
                chain_id: 143,
                currency: Currency::Usdt
            }
        );
        let mut same_address = chain(143);
        same_address["tokens"] = serde_json::json!([
            {"currency": "USDC", "address": USDC},
            {"currency": "USDT", "address": USDC},
        ]);
        assert_eq!(
            ChainRegistry::parse(&serde_json::json!([same_address]).to_string()).unwrap_err(),
            ChainRegistryError::DuplicateToken {
                chain_id: 143,
                address: USDC.parse().unwrap()
            }
        );
        // USDT alone: fine on one chain, but the registry needs USDC somewhere.
        let mut usdt_only = chain(143);
        usdt_only["tokens"] = serde_json::json!([{"currency": "USDT", "address": USDT}]);
        assert_eq!(
            ChainRegistry::parse(&serde_json::json!([usdt_only.clone()]).to_string()).unwrap_err(),
            ChainRegistryError::NoUsdc
        );
        ChainRegistry::parse(&serde_json::json!([usdt_only.clone(), chain(8453)]).to_string())
            .unwrap();
        // CCTP is USDC's bridge: a chain without USDC cannot carry it.
        usdt_only["cctp"] = serde_json::json!({
            "domain": 15,
            "token_messenger": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
            "message_transmitter": "0x81D40F21F12A8F0E3252Bccb954D722d4c464B64",
            "forwarder": "0x1111111111111111111111111111111111111111",
            "forwarder_code_hash": format!("0x{}", "ef".repeat(32)),
        });
        assert_eq!(
            ChainRegistry::parse(&serde_json::json!([usdt_only, chain(8453)]).to_string())
                .unwrap_err(),
            ChainRegistryError::CctpWithoutUsdc(143)
        );
        let mut unknown_currency = chain(143);
        unknown_currency["tokens"] = serde_json::json!([{"currency": "AUSD", "address": USDT}]);
        assert!(matches!(
            ChainRegistry::parse(&serde_json::json!([unknown_currency]).to_string()).unwrap_err(),
            ChainRegistryError::Json(_)
        ));
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
    fn cctp_is_optional_per_chain() {
        let mut with_cctp = chain(143);
        with_cctp["cctp"] = serde_json::json!({
            "domain": 15,
            "token_messenger": "0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d",
            "message_transmitter": "0x81D40F21F12A8F0E3252Bccb954D722d4c464B64",
            "forwarder": "0x1111111111111111111111111111111111111111",
            "forwarder_code_hash": format!("0x{}", "ef".repeat(32)),
        });
        let json = serde_json::json!([with_cctp, chain(31337)]).to_string();
        let registry = ChainRegistry::parse(&json).unwrap();
        assert_eq!(registry.cctp(143).unwrap().domain, 15);
        assert!(registry.cctp(31337).is_none());
        assert!(registry.cctp(1).is_none());

        let mut partial = chain(143);
        partial["cctp"] = serde_json::json!({ "domain": 15 });
        assert!(matches!(
            ChainRegistry::parse(&serde_json::json!([partial]).to_string()).unwrap_err(),
            ChainRegistryError::Json(_)
        ));
    }

    #[test]
    fn signer_low_balance_is_optional_per_chain() {
        let registry = ChainRegistry::parse(&serde_json::json!([chain(143)]).to_string()).unwrap();
        assert_eq!(registry.first().signer_low_balance_wei, None);

        let mut bound = chain(8453);
        bound["signer_low_balance_wei"] = serde_json::json!(5_000_000_000_000_000u64);
        let registry = ChainRegistry::parse(&serde_json::json!([bound]).to_string()).unwrap();
        assert_eq!(
            registry.first().signer_low_balance_wei,
            Some(5_000_000_000_000_000)
        );
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
