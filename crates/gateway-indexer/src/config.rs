use std::time::Duration;

use alloy_primitives::{Address, B256, U256};
use gateway_core::ChainId;

/// Default indexer poll interval when `PAYDAY_INDEXER_POLL_INTERVAL_MS` is unset.
/// This is the reconcile cadence only while the transfer signal is down.
const DEFAULT_INDEXER_POLL_INTERVAL_MS: u64 = 2000;
/// Reconcile cadence while the transfer signal is connected. The signal
/// wakes a pass the moment a payment finalizes; this timer is the backstop
/// that keeps the finalized cursor moving and catches anything the socket
/// missed, so it can be slow.
const DEFAULT_INDEXER_RECONCILE_INTERVAL_MS: u64 = 60_000;
/// How long a settled address stays in the signal's watch list.
const DEFAULT_LATE_WATCH_DAYS: u64 = 30;
const DEFAULT_FINALITY_CONFIRMATIONS: u64 = 2;
const DEFAULT_LOG_RANGE_SIZE: u64 = 100;
const DEFAULT_MAX_RANGES_PER_TICK: u64 = 20;
/// Provider request budgets are per second (QuickNode's is 50); pacing below
/// that keeps catch-up bursts from tripping them. 0 disables pacing.
const DEFAULT_RPC_MAX_RPS: u64 = 40;
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
    /// WebSocket endpoint for the transfer signal; `None` disables it.
    rpc_ws_url: Option<String>,
    indexer_poll_interval: Duration,
    indexer_reconcile_interval: Duration,
    late_watch_window: Duration,
    finality_source: FinalitySource,
    finality_confirmations: u64,
    log_range_size: u64,
    max_ranges_per_tick: u64,
    rpc_max_rps: u64,
    usdc_start_block: u64,
    factory_address: Address,
    factory_code_hash: B256,
    batch_sweeper_address: Address,
    batch_sweeper_code_hash: B256,
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
        let chain_id: u64 = std::env::var("PAYDAY_CHAIN_ID")
            .expect("PAYDAY_CHAIN_ID must be set")
            .parse()
            .unwrap_or_else(|e| panic!("invalid PAYDAY_CHAIN_ID: {e}"));

        let usdc_address = parse_address_env("PAYDAY_USDC_ADDRESS");
        let factory_address = parse_address_env("PAYDAY_FACTORY_ADDRESS");
        let batch_sweeper_address = parse_address_env("PAYDAY_BATCH_SWEEPER_ADDRESS");
        // keccak256 of the runtime bytecode `eth_getCode` returns for each
        // contract: the deployed generation this build was reviewed against.
        let factory_code_hash = parse_b256_env("PAYDAY_FACTORY_CODE_HASH");
        let batch_sweeper_code_hash = parse_b256_env("PAYDAY_BATCH_SWEEPER_CODE_HASH");

        let finality_source = match std::env::var("PAYDAY_FINALITY_SOURCE").as_deref() {
            Ok("finalized") | Err(_) => FinalitySource::FinalizedTag,
            Ok("latest") => FinalitySource::Latest,
            Ok(other) => {
                panic!("invalid PAYDAY_FINALITY_SOURCE '{other}': expected finalized or latest")
            }
        };
        let finality_confirmations = parse_u64_env(
            "PAYDAY_FINALITY_CONFIRMATIONS",
            DEFAULT_FINALITY_CONFIRMATIONS,
        );
        let log_range_size = parse_u64_env("PAYDAY_LOG_RANGE_SIZE", DEFAULT_LOG_RANGE_SIZE);
        assert!(log_range_size > 0, "PAYDAY_LOG_RANGE_SIZE must be positive");
        let max_ranges_per_tick = parse_u64_env(
            "PAYDAY_INDEXER_MAX_RANGES_PER_TICK",
            DEFAULT_MAX_RANGES_PER_TICK,
        );
        assert!(
            max_ranges_per_tick > 0,
            "PAYDAY_INDEXER_MAX_RANGES_PER_TICK must be positive"
        );
        let rpc_max_rps = parse_u64_env("PAYDAY_INDEXER_RPC_MAX_RPS", DEFAULT_RPC_MAX_RPS);
        // No default: a fresh database with the variable missing must not
        // start a backfill from genesis.
        let usdc_start_block = std::env::var("PAYDAY_USDC_START_BLOCK")
            .expect("PAYDAY_USDC_START_BLOCK must be set")
            .parse()
            .unwrap_or_else(|e| panic!("invalid PAYDAY_USDC_START_BLOCK: {e}"));

        let signer = match (
            std::env::var("PAYDAY_SIGNER_KEY").ok(),
            std::env::var("PAYDAY_KMS_KEY_ID").ok(),
        ) {
            (Some(key), None) => SignerConfig::Local(key),
            (None, Some(key_id)) => SignerConfig::AwsKms(key_id),
            (None, None) => {
                panic!("exactly one of PAYDAY_SIGNER_KEY or PAYDAY_KMS_KEY_ID must be set")
            }
            (Some(_), Some(_)) => {
                panic!("PAYDAY_SIGNER_KEY and PAYDAY_KMS_KEY_ID cannot both be set")
            }
        };

        let rpc_url = std::env::var("PAYDAY_RPC_URL").expect("PAYDAY_RPC_URL must be set");
        // QuickNode and Anvil serve WebSocket on the HTTP URL's host and
        // path, so the signal needs no second secret; `off` disables it and
        // an explicit URL overrides the derivation.
        let rpc_ws_url = match std::env::var("PAYDAY_RPC_WS_URL") {
            Ok(value) if value.is_empty() || value.eq_ignore_ascii_case("off") => None,
            Ok(value) => Some(value),
            Err(_) => derive_ws_url(&rpc_url),
        };

        Config {
            database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            chain_id: ChainId(chain_id),
            rpc_url,
            rpc_ws_url,
            indexer_poll_interval: Duration::from_millis(parse_u64_env(
                "PAYDAY_INDEXER_POLL_INTERVAL_MS",
                DEFAULT_INDEXER_POLL_INTERVAL_MS,
            )),
            indexer_reconcile_interval: Duration::from_millis(parse_u64_env(
                "PAYDAY_INDEXER_RECONCILE_INTERVAL_MS",
                DEFAULT_INDEXER_RECONCILE_INTERVAL_MS,
            )),
            late_watch_window: Duration::from_secs(
                parse_u64_env("PAYDAY_INDEXER_LATE_WATCH_DAYS", DEFAULT_LATE_WATCH_DAYS)
                    .saturating_mul(24 * 3600),
            ),
            finality_source,
            finality_confirmations,
            log_range_size,
            max_ranges_per_tick,
            rpc_max_rps,
            usdc_start_block,
            factory_address,
            factory_code_hash,
            batch_sweeper_address,
            batch_sweeper_code_hash,
            usdc_address,
            sweep_pending_timeout: Duration::from_secs(parse_u64_env(
                "PAYDAY_SWEEP_PENDING_TIMEOUT_SECS",
                DEFAULT_SWEEP_PENDING_TIMEOUT_SECS,
            )),
            sweep_max_submissions: parse_u64_env(
                "PAYDAY_SWEEP_MAX_SUBMISSIONS",
                DEFAULT_SWEEP_MAX_SUBMISSIONS as u64,
            )
            .try_into()
            .expect("PAYDAY_SWEEP_MAX_SUBMISSIONS is too large"),
            sweep_max_attempts: parse_u64_env(
                "PAYDAY_SWEEP_MAX_ATTEMPTS",
                DEFAULT_SWEEP_MAX_ATTEMPTS as u64,
            )
            .try_into()
            .expect("PAYDAY_SWEEP_MAX_ATTEMPTS is too large"),
            signer_low_balance_wei: U256::from(
                std::env::var("PAYDAY_SIGNER_LOW_BALANCE_WEI")
                    .ok()
                    .map(|value| {
                        value.parse::<u128>().unwrap_or_else(|e| {
                            panic!("invalid PAYDAY_SIGNER_LOW_BALANCE_WEI: {e}")
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

    pub fn rpc_ws_url(&self) -> Option<&str> {
        self.rpc_ws_url.as_deref()
    }

    pub fn indexer_poll_interval(&self) -> Duration {
        self.indexer_poll_interval
    }

    pub fn indexer_reconcile_interval(&self) -> Duration {
        self.indexer_reconcile_interval
    }

    pub fn late_watch_window(&self) -> Duration {
        self.late_watch_window
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

    pub fn rpc_max_rps(&self) -> u64 {
        self.rpc_max_rps
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

    pub fn factory_code_hash(&self) -> B256 {
        self.factory_code_hash
    }

    pub fn batch_sweeper_address(&self) -> Address {
        self.batch_sweeper_address
    }

    pub fn batch_sweeper_code_hash(&self) -> B256 {
        self.batch_sweeper_code_hash
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

/// The WebSocket URL an HTTP RPC URL implies: the same host, path and token,
/// on the matching WebSocket scheme. Anything else has no derivation.
fn derive_ws_url(rpc_url: &str) -> Option<String> {
    rpc_url
        .strip_prefix("https://")
        .map(|rest| format!("wss://{rest}"))
        .or_else(|| {
            rpc_url
                .strip_prefix("http://")
                .map(|rest| format!("ws://{rest}"))
        })
}

fn parse_address_env(name: &str) -> Address {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be set"))
        .parse()
        .unwrap_or_else(|error| panic!("invalid {name}: {error}"))
}

fn parse_b256_env(name: &str) -> B256 {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be set"))
        .parse()
        .unwrap_or_else(|error| panic!("invalid {name}: expected 0x-prefixed 32-byte hex: {error}"))
}

fn parse_u64_env(name: &str, default: u64) -> u64 {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .unwrap_or_else(|e| panic!("invalid {name}: {e}")),
        Err(_) => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_url_mirrors_the_http_endpoint() {
        assert_eq!(
            derive_ws_url("https://x.quiknode.pro/TOKEN/").as_deref(),
            Some("wss://x.quiknode.pro/TOKEN/")
        );
        assert_eq!(
            derive_ws_url("http://127.0.0.1:8545").as_deref(),
            Some("ws://127.0.0.1:8545")
        );
        assert_eq!(derive_ws_url("ipc:///tmp/anvil.ipc"), None);
    }
}
