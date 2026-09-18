use gum_core::{ChainConfig, ChainRegistry};
use std::{collections::HashMap, net::SocketAddr, time::Duration};

pub struct Config {
    chains: ChainRegistry,
    rpc_urls: HashMap<u64, String>,
    ws_urls: HashMap<u64, String>,
    server_url: String,
    token: String,
    listen: SocketAddr,
    poll: Duration,
    reconcile: Duration,
    idle: Duration,
    late_watch: Duration,
    max_ranges: u64,
    rpc_max_rps: u64,
}

impl Config {
    pub fn from_env() -> Self {
        let chains = ChainRegistry::from_env();
        let required = |n: &str| std::env::var(n).unwrap_or_else(|_| panic!("{n} must be set"));
        let rpc_urls = chains
            .chains()
            .iter()
            .map(|c| (c.chain_id, required(&c.rpc_url_var())))
            .collect::<HashMap<_, _>>();
        let ws_urls = chains
            .chains()
            .iter()
            .filter_map(|c| {
                let explicit = std::env::var(c.rpc_ws_url_var()).ok();
                let value = match explicit {
                    Some(v) if v.is_empty() || v.eq_ignore_ascii_case("off") => None,
                    Some(v) => Some(v),
                    None => derive_ws_url(&rpc_urls[&c.chain_id]),
                };
                value.map(|v| (c.chain_id, v))
            })
            .collect();
        let max_ranges = number("GUM_INDEXER_MAX_RANGES_PER_TICK", 20);
        assert!(
            max_ranges > 0,
            "GUM_INDEXER_MAX_RANGES_PER_TICK must be positive"
        );
        Self {
            chains,
            rpc_urls,
            ws_urls,
            server_url: required("GUM_SERVER_INTERNAL_URL")
                .trim_end_matches('/')
                .to_owned(),
            token: required("GUM_INTERNAL_TOKEN"),
            listen: std::env::var("GUM_INDEXER_LISTEN_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:8080".into())
                .parse()
                .expect("invalid GUM_INDEXER_LISTEN_ADDR"),
            poll: Duration::from_millis(number("GUM_INDEXER_POLL_INTERVAL_MS", 2_000)),
            reconcile: Duration::from_millis(number("GUM_INDEXER_RECONCILE_INTERVAL_MS", 60_000)),
            idle: Duration::from_millis(number("GUM_INDEXER_IDLE_INTERVAL_MS", 300_000)),
            late_watch: Duration::from_secs(
                number("GUM_INDEXER_LATE_WATCH_DAYS", 365).saturating_mul(86_400),
            ),
            max_ranges,
            rpc_max_rps: number("GUM_INDEXER_RPC_MAX_RPS", 40),
        }
    }
    pub fn chains(&self) -> &[ChainConfig] {
        self.chains.chains()
    }
    pub fn rpc_url(&self, id: u64) -> &str {
        &self.rpc_urls[&id]
    }
    pub fn ws_url(&self, id: u64) -> Option<&str> {
        self.ws_urls.get(&id).map(String::as_str)
    }
    pub fn server_url(&self) -> &str {
        &self.server_url
    }
    pub fn token(&self) -> &str {
        &self.token
    }
    pub fn listen(&self) -> SocketAddr {
        self.listen
    }
    pub fn poll(&self) -> Duration {
        self.poll
    }
    pub fn reconcile(&self) -> Duration {
        self.reconcile
    }
    pub fn idle(&self) -> Duration {
        self.idle
    }
    pub fn late_watch(&self) -> Duration {
        self.late_watch
    }
    pub fn max_ranges(&self) -> u64 {
        self.max_ranges
    }
    pub fn rpc_max_rps(&self) -> u64 {
        self.rpc_max_rps
    }
}
fn number(name: &str, default: u64) -> u64 {
    std::env::var(name).map_or(default, |v| {
        v.parse().unwrap_or_else(|e| panic!("invalid {name}: {e}"))
    })
}
fn derive_ws_url(url: &str) -> Option<String> {
    url.strip_prefix("https://")
        .map(|x| format!("wss://{x}"))
        .or_else(|| url.strip_prefix("http://").map(|x| format!("ws://{x}")))
}
