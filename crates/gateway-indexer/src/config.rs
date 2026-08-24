use alloy_primitives::Address;
use gateway_core::ChainId;

/// Default indexer poll interval when `GATEWAY_INDEXER_POLL_INTERVAL_MS` is unset.
const DEFAULT_INDEXER_POLL_INTERVAL_MS: u64 = 2000;

pub struct Config {
    database_url: String,
    chain_id: ChainId,
    rpc_url: String,
    indexer_poll_interval_ms: u64,
    /// Address of the deployed `PaymentFactory`, whose `execute` the sweep pass
    /// calls. Matches `GATEWAY_FACTORY_ADDRESS` used by `gatewayd`.
    factory_address: Address,
    /// Hex private key of the backend account that signs sweep transactions.
    signer_key: String,
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

        let factory_address = std::env::var("GATEWAY_FACTORY_ADDRESS")
            .expect("GATEWAY_FACTORY_ADDRESS must be set")
            .parse()
            .unwrap_or_else(|e| panic!("invalid GATEWAY_FACTORY_ADDRESS: {e}"));

        Config {
            database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            chain_id: ChainId(chain_id),
            rpc_url: std::env::var("GATEWAY_RPC_URL").expect("GATEWAY_RPC_URL must be set"),
            indexer_poll_interval_ms,
            factory_address,
            signer_key: std::env::var("GATEWAY_SIGNER_KEY").expect("GATEWAY_SIGNER_KEY must be set"),
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

    pub fn factory_address(&self) -> Address {
        self.factory_address
    }

    pub fn signer_key(&self) -> &str {
        &self.signer_key
    }
}
