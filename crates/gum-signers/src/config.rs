//! Process configuration from the environment. Every `PAYDAY_*` variable
//! that this service reads is named here and nowhere else.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use alloy_primitives::U256;
use gum_bus::BackoffPolicy;
use gum_core::{ChainConfig, ChainRegistry};

use crate::executor::Policy;

/// Where the pool's keys live. Exactly one of the two is configured.
#[derive(Debug, Clone)]
pub enum SignerConfig {
    /// Hex private keys; local development and tests only.
    Local(Vec<String>),
    /// AWS KMS key ids; the process never sees the private key.
    AwsKms(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct Config {
    chains: ChainRegistry,
    rpc_urls: HashMap<u64, String>,
    database_url: String,
    listen: SocketAddr,
    poll_interval: Duration,
    pending_timeout: Duration,
    max_submissions: u32,
    max_attempts: u32,
    rpc_max_rps: u64,
    default_low_balance_wei: U256,
    signer: SignerConfig,
}

impl Config {
    pub fn from_env() -> Self {
        let chains = ChainRegistry::from_env();
        let required =
            |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set"));
        let rpc_urls = chains
            .chains()
            .iter()
            .map(|chain| (chain.chain_id, required(&chain.rpc_url_var())))
            .collect();
        let local_keys = list("PAYDAY_SIGNER_KEYS");
        let kms_key_ids = list("PAYDAY_KMS_KEY_IDS");
        let signer = match (local_keys.is_empty(), kms_key_ids.is_empty()) {
            (false, true) => SignerConfig::Local(local_keys),
            (true, false) => SignerConfig::AwsKms(kms_key_ids),
            _ => panic!("exactly one of PAYDAY_SIGNER_KEYS and PAYDAY_KMS_KEY_IDS must be set"),
        };
        let max_submissions = number("PAYDAY_SWEEP_MAX_SUBMISSIONS", 5);
        assert!(
            max_submissions > 0,
            "PAYDAY_SWEEP_MAX_SUBMISSIONS must be positive"
        );
        let max_attempts = number("PAYDAY_SWEEP_MAX_ATTEMPTS", 8);
        assert!(
            max_attempts > 0,
            "PAYDAY_SWEEP_MAX_ATTEMPTS must be positive"
        );
        Self {
            chains,
            rpc_urls,
            database_url: required("DATABASE_URL"),
            listen: std::env::var("PAYDAY_SIGNERS_LISTEN_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:8080".into())
                .parse()
                .expect("invalid PAYDAY_SIGNERS_LISTEN_ADDR"),
            poll_interval: Duration::from_millis(number("PAYDAY_SIGNERS_POLL_INTERVAL_MS", 2_000)),
            pending_timeout: Duration::from_secs(number("PAYDAY_SWEEP_PENDING_TIMEOUT_SECS", 60)),
            max_submissions: max_submissions as u32,
            max_attempts: max_attempts as u32,
            rpc_max_rps: number("PAYDAY_SIGNERS_RPC_MAX_RPS", 20),
            // 0.05 native token: enough for many sweeps, low enough to alert
            // well before a signer is unusable.
            default_low_balance_wei: std::env::var("PAYDAY_SIGNER_LOW_BALANCE_WEI")
                .ok()
                .map(|value| {
                    value
                        .parse()
                        .expect("PAYDAY_SIGNER_LOW_BALANCE_WEI must be an integer")
                })
                .unwrap_or(U256::from(50_000_000_000_000_000u64)),
            signer,
        }
    }

    pub fn chains(&self) -> &[ChainConfig] {
        self.chains.chains()
    }

    pub fn rpc_url(&self, chain_id: u64) -> &str {
        &self.rpc_urls[&chain_id]
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    pub fn listen(&self) -> SocketAddr {
        self.listen
    }

    pub fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    pub fn rpc_max_rps(&self) -> u64 {
        self.rpc_max_rps
    }

    pub fn signer(&self) -> &SignerConfig {
        &self.signer
    }

    /// The executor policy for one chain; the low-balance threshold may be
    /// overridden per chain in the registry.
    pub fn policy(&self, chain: &ChainConfig) -> Policy {
        Policy {
            pending_timeout: self.pending_timeout,
            max_submissions: self.max_submissions,
            max_attempts: self.max_attempts,
            retry_backoff: BackoffPolicy::new(Duration::from_secs(5), Duration::from_secs(300)),
            low_balance_wei: chain
                .signer_low_balance_wei
                .map(U256::from)
                .unwrap_or(self.default_low_balance_wei),
        }
    }
}

fn number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .map(|value| {
            value
                .parse()
                .unwrap_or_else(|_| panic!("{name} must be an integer"))
        })
        .unwrap_or(default)
}

fn list(name: &str) -> Vec<String> {
    std::env::var(name)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}
