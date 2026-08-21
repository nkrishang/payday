use gateway_core::ChainId;

/// Default indexer poll interval when `GATEWAY_INDEXER_POLL_INTERVAL_MS` is unset.
const DEFAULT_INDEXER_POLL_INTERVAL_MS: u64 = 2000;

pub struct Config {
    database_url: String,
    chain_id: ChainId,
    rpc_url: String,
    indexer_poll_interval_ms: u64,
}

impl Config {
    pub fn from_env() -> Self {
        let chain_id: u64 = std::env::var("GATEWAY_CHAIN_ID")
            .expect("GATEWAY_CHAIN_ID must be set")
            .parse()
            .unwrap_or_else(|e| panic!("invalid GATEWAY_CHAIN_ID: {e}"));

        let indexer_poll_interval_ms = match std::env::var("GATEWAY_INDEXER_POLL_INTERVAL_MS") {
            Ok(v) => v
                .parse()
                .unwrap_or_else(|e| panic!("invalid GATEWAY_INDEXER_POLL_INTERVAL_MS: {e}")),
            Err(_) => DEFAULT_INDEXER_POLL_INTERVAL_MS,
        };

        Config {
            database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            chain_id: ChainId(chain_id),
            rpc_url: std::env::var("GATEWAY_RPC_URL").expect("GATEWAY_RPC_URL must be set"),
            indexer_poll_interval_ms,
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

    pub fn indexer_poll_interval_ms(&self) -> u64 {
        self.indexer_poll_interval_ms
    }
}
