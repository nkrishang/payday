use std::time::Duration;

use alloy_primitives::{Address, U256};
use gateway_core::ChainId;

/// Default indexer poll interval when `GATEWAY_INDEXER_POLL_INTERVAL_MS` is unset.
const DEFAULT_INDEXER_POLL_INTERVAL_MS: u64 = 2000;
const DEFAULT_FINALITY_CONFIRMATIONS: u64 = 2;
const DEFAULT_LOG_RANGE_SIZE: u64 = 100;
const DEFAULT_MAX_RANGES_PER_TICK: u64 = 20;
const DEFAULT_SWEEP_PENDING_TIMEOUT_SECS: u64 = 60;
const DEFAULT_SWEEP_MAX_SUBMISSIONS: u32 = 5;
const DEFAULT_SWEEP_MAX_ATTEMPTS: u32 = 8;
/// 0.05 native tokens: roughly a hundred batches at Monad's fee levels.
const DEFAULT_SIGNER_LOW_BALANCE_WEI: u128 = 50_000_000_000_000_000;

/// Which chain reading anchors the finality boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalitySource {
    /// The node's `finalized` block tag. Monad maps it to "irreversible
    /// without a hard fork"; this is the production setting.
    FinalizedTag,
    /// The latest block. Only for chains or local nodes without a meaningful
    /// `finalized` tag, combined with a reviewed confirmation depth.
    Latest,
}

pub struct Config {
    database_url: String,
    chain_id: ChainId,
    rpc_url: String,
    indexer_poll_interval: Duration,
    finality_source: FinalitySource,
    finality_confirmations: u64,
    log_range_size: u64,
    max_ranges_per_tick: u64,
    usdc_start_block: u64,
    factory_address: Address,
    batch_sweeper_address: Address,
    usdc_address: Address,
    sweep_pending_timeout: Duration,
    sweep_max_submissions: u32,
    sweep_max_attempts: u32,
    signer_low_balance_wei: U256,
    signer: SignerConfig,
}

/// Sweep signer selected at startup. Local keys keep Anvil fully self-contained;
/// production uses a non-exportable AWS KMS key through the ECS task role.
pub enum SignerConfig {
    Local(String),
    AwsKms(String),
}

impl Config {
    pub fn from_env() -> Self {
        let chain_id: u64 = std::env::var("GATEWAY_CHAIN_ID")
            .expect("GATEWAY_CHAIN_ID must be set")
            .parse()
            .unwrap_or_else(|e| panic!("invalid GATEWAY_CHAIN_ID: {e}"));

        let usdc_address = parse_address_env("GATEWAY_USDC_ADDRESS");
        let factory_address = parse_address_env("GATEWAY_FACTORY_ADDRESS");
        let batch_sweeper_address = parse_address_env("GATEWAY_BATCH_SWEEPER_ADDRESS");

        let finality_source = match std::env::var("GATEWAY_FINALITY_SOURCE").as_deref() {
            Ok("finalized") | Err(_) => FinalitySource::FinalizedTag,
            Ok("latest") => FinalitySource::Latest,
            Ok(other) => {
                panic!("invalid GATEWAY_FINALITY_SOURCE '{other}': expected finalized or latest")
            }
        };
        let finality_confirmations = parse_u64_env(
            "GATEWAY_FINALITY_CONFIRMATIONS",
            DEFAULT_FINALITY_CONFIRMATIONS,
        );
        let log_range_size = parse_u64_env("GATEWAY_LOG_RANGE_SIZE", DEFAULT_LOG_RANGE_SIZE);
        assert!(
            log_range_size > 0,
            "GATEWAY_LOG_RANGE_SIZE must be positive"
        );
        let max_ranges_per_tick = parse_u64_env(
            "GATEWAY_INDEXER_MAX_RANGES_PER_TICK",
            DEFAULT_MAX_RANGES_PER_TICK,
        );
        assert!(
            max_ranges_per_tick > 0,
            "GATEWAY_INDEXER_MAX_RANGES_PER_TICK must be positive"
        );
        // No default: a fresh database with the variable missing must not
        // start a backfill from genesis.
        let usdc_start_block = std::env::var("GATEWAY_USDC_START_BLOCK")
            .expect("GATEWAY_USDC_START_BLOCK must be set")
            .parse()
            .unwrap_or_else(|e| panic!("invalid GATEWAY_USDC_START_BLOCK: {e}"));

        let signer = match (
            std::env::var("GATEWAY_SIGNER_KEY").ok(),
            std::env::var("GATEWAY_KMS_KEY_ID").ok(),
        ) {
            (Some(key), None) => SignerConfig::Local(key),
            (None, Some(key_id)) => SignerConfig::AwsKms(key_id),
            (None, None) => {
                panic!("exactly one of GATEWAY_SIGNER_KEY or GATEWAY_KMS_KEY_ID must be set")
            }
            (Some(_), Some(_)) => {
                panic!("GATEWAY_SIGNER_KEY and GATEWAY_KMS_KEY_ID cannot both be set")
            }
        };

        Config {
            database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            chain_id: ChainId(chain_id),
            rpc_url: std::env::var("GATEWAY_RPC_URL").expect("GATEWAY_RPC_URL must be set"),
            indexer_poll_interval: Duration::from_millis(parse_u64_env(
                "GATEWAY_INDEXER_POLL_INTERVAL_MS",
                DEFAULT_INDEXER_POLL_INTERVAL_MS,
            )),
            finality_source,
            finality_confirmations,
            log_range_size,
            max_ranges_per_tick,
            usdc_start_block,
            factory_address,
            batch_sweeper_address,
            usdc_address,
            sweep_pending_timeout: Duration::from_secs(parse_u64_env(
                "GATEWAY_SWEEP_PENDING_TIMEOUT_SECS",
                DEFAULT_SWEEP_PENDING_TIMEOUT_SECS,
            )),
            sweep_max_submissions: parse_u64_env(
                "GATEWAY_SWEEP_MAX_SUBMISSIONS",
                DEFAULT_SWEEP_MAX_SUBMISSIONS as u64,
            )
            .try_into()
            .expect("GATEWAY_SWEEP_MAX_SUBMISSIONS is too large"),
            sweep_max_attempts: parse_u64_env(
                "GATEWAY_SWEEP_MAX_ATTEMPTS",
                DEFAULT_SWEEP_MAX_ATTEMPTS as u64,
            )
            .try_into()
            .expect("GATEWAY_SWEEP_MAX_ATTEMPTS is too large"),
            signer_low_balance_wei: U256::from(
                std::env::var("GATEWAY_SIGNER_LOW_BALANCE_WEI")
                    .ok()
                    .map(|value| {
                        value.parse::<u128>().unwrap_or_else(|e| {
                            panic!("invalid GATEWAY_SIGNER_LOW_BALANCE_WEI: {e}")
                        })
                    })
                    .unwrap_or(DEFAULT_SIGNER_LOW_BALANCE_WEI),
            ),
            signer,
        }
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    pub fn chain_id(&self) -> ChainId {
        self.chain_id
    }

    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    pub fn indexer_poll_interval(&self) -> Duration {
        self.indexer_poll_interval
    }

    pub fn finality_source(&self) -> FinalitySource {
        self.finality_source
    }

    pub fn finality_confirmations(&self) -> u64 {
        self.finality_confirmations
    }

    pub fn log_range_size(&self) -> u64 {
        self.log_range_size
    }

    pub fn max_ranges_per_tick(&self) -> u64 {
        self.max_ranges_per_tick
    }

    pub fn usdc_start_block(&self) -> u64 {
        self.usdc_start_block
    }

    pub fn usdc_address(&self) -> Address {
        self.usdc_address
    }

    pub fn factory_address(&self) -> Address {
        self.factory_address
    }

    pub fn batch_sweeper_address(&self) -> Address {
        self.batch_sweeper_address
    }

    pub fn sweep_pending_timeout(&self) -> Duration {
        self.sweep_pending_timeout
    }

    pub fn sweep_max_submissions(&self) -> u32 {
        self.sweep_max_submissions
    }

    pub fn sweep_max_attempts(&self) -> u32 {
        self.sweep_max_attempts
    }

    pub fn signer_low_balance_wei(&self) -> U256 {
        self.signer_low_balance_wei
    }

    pub fn signer(&self) -> &SignerConfig {
        &self.signer
    }
}

fn parse_address_env(name: &str) -> Address {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be set"))
        .parse()
        .unwrap_or_else(|error| panic!("invalid {name}: {error}"))
}

fn parse_u64_env(name: &str, default: u64) -> u64 {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .unwrap_or_else(|e| panic!("invalid {name}: {e}")),
        Err(_) => default,
    }
}
