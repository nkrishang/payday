use std::str::FromStr;

use alloy_primitives::Address;
use gateway_core::ChainId;

pub struct Config {
    bind_addr: String,
    database_url: String,
    chain_id: ChainId,
    factory_address: Address,
}

impl Config {
    pub fn from_env() -> Self {
        let factory_address = std::env::var("GATEWAY_FACTORY_ADDRESS")
            .expect("GATEWAY_FACTORY_ADDRESS must be set");
        let factory_address = Address::from_str(&factory_address)
            .unwrap_or_else(|e| panic!("invalid GATEWAY_FACTORY_ADDRESS '{factory_address}': {e}"));

        let chain_id: u64 = std::env::var("GATEWAY_CHAIN_ID")
            .expect("GATEWAY_CHAIN_ID must be set")
            .parse()
            .unwrap_or_else(|e| panic!("invalid GATEWAY_CHAIN_ID: {e}"));

        Config {
            bind_addr: std::env::var("GATEWAY_BIND_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:3000".into()),
            database_url: std::env::var("DATABASE_URL")
                .expect("DATABASE_URL must be set"),
            chain_id: ChainId(chain_id),
            factory_address,
        }
    }

    pub fn bind_addr(&self) -> &str {
        &self.bind_addr
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    pub fn chain_id(&self) -> ChainId {
        self.chain_id
    }

    pub fn factory_address(&self) -> Address {
        self.factory_address
    }
}
