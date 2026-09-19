//! Read-only chain access for the API: a wallet's balance in each stablecoin
//! contract on each chain, and each contract as an EIP-712 domain.
//!
//! `POST /v1/withdrawals` snapshots balances into legs and hands the
//! merchant typed data to sign under the token's own domain. The domain is
//! read from the token at startup (`name()`, `version()`) and checked
//! against its `DOMAIN_SEPARATOR()`, because deployments differ in the name
//! ("USDC" on Monad, "USD Coin" on Base and Arbitrum; "USDT0" on Monad) and
//! Tether's USDT0 exposes no `version()` at all (its separator hashes under
//! "1"); a wrong domain would produce signatures the token rejects only once
//! relayed. The same pass reads `decimals()` and refuses to start when a
//! contract disagrees with the currency it is configured as: every amount
//! the API renders assumes the currency's decimals.
//!
//! What these checks cannot tell is *which* stablecoin an address is: both
//! issuers' contracts are self-consistent under their own name, so swapping
//! the USDC and USDT0 addresses in `GUM_CHAINS` would start cleanly and
//! mislabel every request. Issuer identity — the canonical deployment of
//! each currency on each chain, never a bridged look-alike — is an
//! operator-trusted input; verify each `tokens` address against the issuer's
//! own documentation when editing the registry (see
//! `docs/production-runbook.md`, "Changing the chains").

use std::collections::HashMap;

use alloy_primitives::{Address, U256};
use alloy_provider::{DynProvider, Provider, ProviderBuilder};
use alloy_rpc_types_eth::{TransactionInput, TransactionRequest};
use alloy_sol_types::{SolCall, sol};
use async_trait::async_trait;
use gum_core::{ChainRegistry, Currency, TokenDomain};
use thiserror::Error;

use gum_chain::redact_urls;

sol! {
    function name() view returns (string);
    function version() view returns (string);
    function decimals() view returns (uint8);
    function DOMAIN_SEPARATOR() view returns (bytes32);
    function balanceOf(address account) view returns (uint256);
}

/// The versions tried, in order, for a token without a `version()` getter.
const VERSION_CANDIDATES: [&str; 2] = ["1", "2"];

#[derive(Debug, Error)]
pub enum ChainReadError {
    #[error("chain {0} is not served by this deployment")]
    Unsupported(u64),
    #[error("token {token} is not served on chain {chain_id}")]
    UnsupportedToken { chain_id: u64, token: Address },
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
    /// `wallet`'s balance in `token` on `chain_id` at the latest block.
    async fn balance(
        &self,
        chain_id: u64,
        token: Address,
        wallet: Address,
    ) -> Result<U256, ChainReadError>;

    /// `token` on `chain_id` as an EIP-712 domain, as read at startup.
    fn domain(&self, chain_id: u64, token: Address) -> Option<&TokenDomain>;
}

struct ReadableChain {
    provider: DynProvider,
    domains: HashMap<Address, TokenDomain>,
}

/// Production [`ChainReads`] over one unsigned Alloy provider per chain.
pub struct AlloyChainReader {
    chains: HashMap<u64, ReadableChain>,
}

impl AlloyChainReader {
    /// Connect to every registered chain and read each of its tokens'
    /// domain and decimals.
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
            let mut domains = HashMap::new();
            for token in &chain.tokens {
                let domain =
                    read_domain(&provider, chain_id, token.currency, token.address, &rpc).await?;
                tracing::info!(
                    chain_id,
                    currency = %token.currency,
                    token = %token.address,
                    name = %domain.name,
                    version = %domain.version,
                    "read token EIP-712 domain"
                );
                domains.insert(token.address, domain);
            }
            chains.insert(chain_id, ReadableChain { provider, domains });
        }
        Ok(Self { chains })
    }
}

/// One token's domain, verified against the contract's own separator, plus
/// the decimals check.
async fn read_domain(
    provider: &DynProvider,
    chain_id: u64,
    currency: Currency,
    token: Address,
    rpc: &impl Fn(&'static str, &dyn std::fmt::Display) -> ChainReadError,
) -> Result<TokenDomain, ChainReadError> {
    let call = |input: Vec<u8>| {
        TransactionRequest::default()
            .to(token)
            .input(TransactionInput::new(input.into()))
    };
    let decimals = provider
        .call(call(decimalsCall {}.abi_encode()))
        .await
        .map_err(|error| rpc("eth_call token.decimals", &error))?;
    let decimals = decimalsCall::abi_decode_returns(&decimals)
        .map_err(|error| rpc("eth_call token.decimals", &error))?;
    if decimals != currency.decimals() {
        return Err(ChainReadError::Rpc {
            chain_id,
            operation: "token decimals check",
            message: format!(
                "{token} reports {decimals} decimals but is configured as {currency}, which has {}; the registry names the wrong contract",
                currency.decimals()
            ),
        });
    }
    let name = provider
        .call(call(nameCall {}.abi_encode()))
        .await
        .map_err(|error| rpc("eth_call token.name", &error))?;
    let name =
        nameCall::abi_decode_returns(&name).map_err(|error| rpc("eth_call token.name", &error))?;
    let separator = provider
        .call(call(DOMAIN_SEPARATORCall {}.abi_encode()))
        .await
        .map_err(|error| rpc("eth_call token.DOMAIN_SEPARATOR", &error))?;
    let separator = DOMAIN_SEPARATORCall::abi_decode_returns(&separator)
        .map_err(|error| rpc("eth_call token.DOMAIN_SEPARATOR", &error))?;
    // Circle's FiatToken exposes `version()`; USDT0 does not, and reverts.
    // A revert is answered by trying the versions a stablecoin plausibly
    // signs under, each checked against the separator below.
    let versions: Vec<String> = match provider.call(call(versionCall {}.abi_encode())).await {
        Ok(output) => vec![
            versionCall::abi_decode_returns(&output)
                .map_err(|error| rpc("eth_call token.version", &error))?,
        ],
        Err(_) => VERSION_CANDIDATES.iter().map(|v| v.to_string()).collect(),
    };
    let mut tried = Vec::new();
    for version in versions {
        let domain = TokenDomain {
            name: name.clone(),
            version,
            chain_id,
            token,
        };
        if domain.separator() == separator {
            return Ok(domain);
        }
        tried.push(domain);
    }
    Err(ChainReadError::Rpc {
        chain_id,
        operation: "token domain check",
        message: format!(
            "no EIP712Domain(name {:?}, version {:?}, chainId {chain_id}, verifyingContract {token}) hashes to the token's DOMAIN_SEPARATOR {separator}; the token is not one this build understands",
            name,
            tried
                .iter()
                .map(|domain| domain.version.as_str())
                .collect::<Vec<_>>()
                .join("|")
        ),
    })
}

#[async_trait]
impl ChainReads for AlloyChainReader {
    async fn balance(
        &self,
        chain_id: u64,
        token: Address,
        wallet: Address,
    ) -> Result<U256, ChainReadError> {
        let chain = self
            .chains
            .get(&chain_id)
            .ok_or(ChainReadError::Unsupported(chain_id))?;
        if !chain.domains.contains_key(&token) {
            return Err(ChainReadError::UnsupportedToken { chain_id, token });
        }
        let call = TransactionRequest::default()
            .to(token)
            .input(TransactionInput::new(
                balanceOfCall { account: wallet }.abi_encode().into(),
            ));
        let output = chain
            .provider
            .call(call)
            .await
            .map_err(|error| ChainReadError::Rpc {
                chain_id,
                operation: "eth_call token.balanceOf",
                message: redact_urls(&error.to_string()),
            })?;
        balanceOfCall::abi_decode_returns(&output).map_err(|error| ChainReadError::Rpc {
            chain_id,
            operation: "eth_call token.balanceOf",
            message: error.to_string(),
        })
    }

    fn domain(&self, chain_id: u64, token: Address) -> Option<&TokenDomain> {
        self.chains
            .get(&chain_id)
            .and_then(|chain| chain.domains.get(&token))
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Mutex;

    use super::*;

    /// Balances set by a test, and a domain per registered token.
    pub(crate) struct FakeChainReader {
        pub balances: Mutex<HashMap<(u64, Address, Address), U256>>,
        pub domains: HashMap<(u64, Address), TokenDomain>,
        /// The USDC contract per chain, so tests set balances by chain alone.
        usdc: HashMap<u64, Address>,
        /// Chains whose balance read fails, as an unreachable RPC would.
        pub failing: Mutex<Vec<u64>>,
    }

    impl FakeChainReader {
        pub(crate) fn new(registry: &ChainRegistry) -> Self {
            let mut domains = HashMap::new();
            let mut usdc = HashMap::new();
            for chain in registry.chains() {
                for token in &chain.tokens {
                    let (name, version) = match token.currency {
                        Currency::Usdc => ("USD Coin", "2"),
                        Currency::Usdt => ("USDT0", "1"),
                    };
                    domains.insert(
                        (chain.chain_id, token.address),
                        TokenDomain {
                            name: name.into(),
                            version: version.into(),
                            chain_id: chain.chain_id,
                            token: token.address,
                        },
                    );
                    if token.currency == Currency::Usdc {
                        usdc.insert(chain.chain_id, token.address);
                    }
                }
            }
            Self {
                balances: Mutex::new(HashMap::new()),
                domains,
                usdc,
                failing: Mutex::new(Vec::new()),
            }
        }

        /// The wallet's USDC balance on `chain_id`.
        pub(crate) fn set_balance(&self, chain_id: u64, wallet: Address, amount: u64) {
            let token = self.usdc[&chain_id];
            self.set_token_balance(chain_id, token, wallet, amount);
        }

        pub(crate) fn set_token_balance(
            &self,
            chain_id: u64,
            token: Address,
            wallet: Address,
            amount: u64,
        ) {
            self.balances
                .lock()
                .unwrap()
                .insert((chain_id, token, wallet), U256::from(amount));
        }
    }

    #[async_trait]
    impl ChainReads for FakeChainReader {
        async fn balance(
            &self,
            chain_id: u64,
            token: Address,
            wallet: Address,
        ) -> Result<U256, ChainReadError> {
            if self.failing.lock().unwrap().contains(&chain_id) {
                return Err(ChainReadError::Rpc {
                    chain_id,
                    operation: "eth_call token.balanceOf",
                    message: "connection refused".into(),
                });
            }
            if !self.domains.contains_key(&(chain_id, token)) {
                return Err(ChainReadError::Unsupported(chain_id));
            }
            Ok(self
                .balances
                .lock()
                .unwrap()
                .get(&(chain_id, token, wallet))
                .copied()
                .unwrap_or(U256::ZERO))
        }

        fn domain(&self, chain_id: u64, token: Address) -> Option<&TokenDomain> {
            self.domains.get(&(chain_id, token))
        }
    }
}
