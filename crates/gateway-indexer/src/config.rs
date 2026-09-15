use std::collections::HashMap;
use std::time::Duration;

use alloy_primitives::U256;
use gateway_core::{ChainConfig, ChainRegistry};

/// Default indexer poll interval when `PAYDAY_INDEXER_POLL_INTERVAL_MS` is unset.
/// This is the reconcile cadence only while a chain is watched and its
/// transfer signal is down.
const DEFAULT_INDEXER_POLL_INTERVAL_MS: u64 = 2000;
/// Reconcile cadence while a chain is watched and its transfer signal is
/// connected. The signal wakes a pass the moment a payment lands; this timer
/// is the backstop that keeps the cursor moving and catches anything the
/// socket missed, so it can be slow.
const DEFAULT_INDEXER_RECONCILE_INTERVAL_MS: u64 = 60_000;
/// Reconcile cadence while a chain has nothing to watch: no open bound
/// request, nothing uncollected, nothing recently settled. A pass then costs
/// two calls and fast-forwards the cursor, so it only needs to keep the
/// chain clock (expiry) moving.
const DEFAULT_INDEXER_IDLE_INTERVAL_MS: u64 = 300_000;
/// How long a settled address stays in the watch list, which every range
/// scan is filtered by. A year: the cost is one `eth_getLogs` per 500
/// addresses per range, and a late transfer outside the window is invisible.
const DEFAULT_LATE_WATCH_DAYS: u64 = 365;
const DEFAULT_MAX_RANGES_PER_TICK: u64 = 20;
/// Provider request budgets are per second (QuickNode's is 50); pacing below
/// that keeps catch-up bursts from tripping them. 0 disables pacing.
const DEFAULT_RPC_MAX_RPS: u64 = 40;
const DEFAULT_SWEEP_PENDING_TIMEOUT_SECS: u64 = 60;
const DEFAULT_SWEEP_MAX_SUBMISSIONS: u32 = 5;
const DEFAULT_SWEEP_MAX_ATTEMPTS: u32 = 8;
/// 0.05 native tokens: roughly a hundred batches at Monad's fee levels, and
/// far more at an L2's. The fallback for chains whose registry entry carries
/// no `signer_low_balance_wei` of its own.
const DEFAULT_SIGNER_LOW_BALANCE_WEI: u128 = 50_000_000_000_000_000;

/// One process indexes and sweeps every chain in the registry. What differs
/// per chain (USDC, contracts, finality, range cap, the low-balance alarm
/// level) is in the registry; what is operational policy (cadences, sweep
/// limits, the signer) is shared and lives here.
pub struct Config {
    database_url: String,
    networks: ChainRegistry,
    /// `PAYDAY_RPC_URL_<chain_id>`, one per registered chain.
    rpc_urls: HashMap<u64, String>,
    /// WebSocket endpoint for each chain's transfer signal; absent disables it.
    rpc_ws_urls: HashMap<u64, String>,
    indexer_poll_interval: Duration,
    indexer_reconcile_interval: Duration,
    indexer_idle_interval: Duration,
    late_watch_window: Duration,
    max_ranges_per_tick: u64,
    rpc_max_rps: u64,
    sweep_pending_timeout: Duration,
    sweep_max_submissions: u32,
    sweep_max_attempts: u32,
    signer_low_balance_wei: U256,
    signer: SignerConfig,
    /// Circle's attestation service (`PAYDAY_CCTP_IRIS_URL`), mainnet by default.
    iris_url: String,
    /// Relay (`PAYDAY_RELAY_URL`), for following cross-chain payments.
    relay_url: String,
    /// `PAYDAY_RELAY_API_KEY`; absent, no cross-chain payment is offered
    /// and there is nothing to follow.
    relay_api_key: Option<String>,
}

/// The sweep signer pool selected at startup: one or more keys, each with
/// one helper transaction in flight at a time. Local keys keep Anvil fully
/// self-contained; production uses non-exportable AWS KMS keys through the
/// ECS task role. The same keys sign on every chain: one address each,
/// everywhere, with a nonce stream per chain.
pub enum SignerConfig {
    Local(Vec<String>),
    AwsKms(Vec<String>),
}

impl Config {
    pub fn from_env() -> Self {
        let networks = ChainRegistry::from_env();
        let required =
            |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set"));
        let rpc_urls: HashMap<u64, String> = networks
            .chains()
            .iter()
            .map(|chain| (chain.chain_id, required(&chain.rpc_url_var())))
            .collect();
        // QuickNode and Anvil serve WebSocket on the HTTP URL's host and
        // path, so the signal needs no second secret; `off` disables it and
        // an explicit URL overrides the derivation.
        let rpc_ws_urls = networks
            .chains()
            .iter()
            .filter_map(|chain| {
                let derived = derive_ws_url(&rpc_urls[&chain.chain_id]);
                let url = match std::env::var(chain.rpc_ws_url_var()) {
                    Ok(value) if value.is_empty() || value.eq_ignore_ascii_case("off") => None,
                    Ok(value) => Some(value),
                    Err(_) => derived,
                };
                url.map(|url| (chain.chain_id, url))
            })
            .collect();

        let max_ranges_per_tick = parse_u64_env(
            "PAYDAY_INDEXER_MAX_RANGES_PER_TICK",
            DEFAULT_MAX_RANGES_PER_TICK,
        );
        assert!(
            max_ranges_per_tick > 0,
            "PAYDAY_INDEXER_MAX_RANGES_PER_TICK must be positive"
        );
        let rpc_max_rps = parse_u64_env("PAYDAY_INDEXER_RPC_MAX_RPS", DEFAULT_RPC_MAX_RPS);

        let signer = match (
            parse_list_env("PAYDAY_SIGNER_KEYS"),
            parse_list_env("PAYDAY_KMS_KEY_IDS"),
        ) {
            (Some(keys), None) => SignerConfig::Local(keys),
            (None, Some(key_ids)) => SignerConfig::AwsKms(key_ids),
            (None, None) => {
                panic!("exactly one of PAYDAY_SIGNER_KEYS or PAYDAY_KMS_KEY_IDS must be set")
            }
            (Some(_), Some(_)) => {
                panic!("PAYDAY_SIGNER_KEYS and PAYDAY_KMS_KEY_IDS cannot both be set")
            }
        };

        Config {
            database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            networks,
            rpc_urls,
            rpc_ws_urls,
            indexer_poll_interval: Duration::from_millis(parse_u64_env(
                "PAYDAY_INDEXER_POLL_INTERVAL_MS",
                DEFAULT_INDEXER_POLL_INTERVAL_MS,
            )),
            indexer_reconcile_interval: Duration::from_millis(parse_u64_env(
                "PAYDAY_INDEXER_RECONCILE_INTERVAL_MS",
                DEFAULT_INDEXER_RECONCILE_INTERVAL_MS,
            )),
            indexer_idle_interval: Duration::from_millis(parse_u64_env(
                "PAYDAY_INDEXER_IDLE_INTERVAL_MS",
                DEFAULT_INDEXER_IDLE_INTERVAL_MS,
            )),
            late_watch_window: Duration::from_secs(
                parse_u64_env("PAYDAY_INDEXER_LATE_WATCH_DAYS", DEFAULT_LATE_WATCH_DAYS)
                    .saturating_mul(24 * 3600),
            ),
            max_ranges_per_tick,
            rpc_max_rps,
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
            iris_url: std::env::var("PAYDAY_CCTP_IRIS_URL")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| crate::iris::DEFAULT_IRIS_URL.to_owned()),
            relay_url: std::env::var("PAYDAY_RELAY_URL")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| gateway_relay::DEFAULT_RELAY_URL.to_owned()),
            relay_api_key: std::env::var("PAYDAY_RELAY_API_KEY")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
        }
    }

    pub fn relay_url(&self) -> &str {
        &self.relay_url
    }

    pub fn relay_api_key(&self) -> Option<&str> {
        self.relay_api_key.as_deref()
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    pub fn networks(&self) -> &ChainRegistry {
        &self.networks
    }

    pub fn iris_url(&self) -> &str {
        &self.iris_url
    }

    pub fn chains(&self) -> &[ChainConfig] {
        self.networks.chains()
    }

    pub fn rpc_url(&self, chain_id: u64) -> &str {
        &self.rpc_urls[&chain_id]
    }

    pub fn rpc_ws_url(&self, chain_id: u64) -> Option<&str> {
        self.rpc_ws_urls.get(&chain_id).map(String::as_str)
    }

    pub fn indexer_poll_interval(&self) -> Duration {
        self.indexer_poll_interval
    }

    pub fn indexer_reconcile_interval(&self) -> Duration {
        self.indexer_reconcile_interval
    }

    pub fn indexer_idle_interval(&self) -> Duration {
        self.indexer_idle_interval
    }

    pub fn late_watch_window(&self) -> Duration {
        self.late_watch_window
    }

    pub fn max_ranges_per_tick(&self) -> u64 {
        self.max_ranges_per_tick
    }

    pub fn rpc_max_rps(&self) -> u64 {
        self.rpc_max_rps
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

/// A comma-separated list; `None` when the variable is unset. An empty list
/// is a configuration error, not "unset": a pool needs at least one key.
fn parse_list_env(name: &str) -> Option<Vec<String>> {
    let value = std::env::var(name).ok()?;
    let items: Vec<String> = value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect();
    assert!(!items.is_empty(), "{name} must list at least one key");
    Some(items)
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
