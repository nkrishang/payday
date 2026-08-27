use std::str::FromStr;

use alloy_primitives::Address;
use gateway_core::ChainId;

pub struct Config {
    bind_addr: String,
    database_url: String,
    auth0: Option<Auth0Config>,
    chain_id: ChainId,
    factory_address: Address,
    usdc_address: Address,
}

impl Config {
    pub fn from_env() -> Self {
        let auth0_issuer = std::env::var("GATEWAY_AUTH0_ISSUER").ok();
        let auth0_audience = std::env::var("GATEWAY_AUTH0_AUDIENCE").ok();
        let auth0_client_id = std::env::var("GATEWAY_AUTH0_CLIENT_ID").ok();
        let auth0 = match (auth0_issuer, auth0_audience, auth0_client_id) {
            (Some(issuer), Some(audience), Some(client_id)) => Some(Auth0Config {
                issuer,
                audience,
                client_id,
            }),
            (None, None, None) => None,
            _ => panic!(
                "GATEWAY_AUTH0_ISSUER, GATEWAY_AUTH0_AUDIENCE, and GATEWAY_AUTH0_CLIENT_ID must be set together"
            ),
        };

        let factory_address =
            std::env::var("GATEWAY_FACTORY_ADDRESS").expect("GATEWAY_FACTORY_ADDRESS must be set");
        let factory_address = Address::from_str(&factory_address)
            .unwrap_or_else(|e| panic!("invalid GATEWAY_FACTORY_ADDRESS '{factory_address}': {e}"));

        let chain_id: u64 = std::env::var("GATEWAY_CHAIN_ID")
            .expect("GATEWAY_CHAIN_ID must be set")
            .parse()
            .unwrap_or_else(|e| panic!("invalid GATEWAY_CHAIN_ID: {e}"));

        let usdc_address =
            std::env::var("GATEWAY_USDC_ADDRESS").expect("GATEWAY_USDC_ADDRESS must be set");
        let usdc_address = Address::from_str(&usdc_address)
            .unwrap_or_else(|e| panic!("invalid GATEWAY_USDC_ADDRESS '{usdc_address}': {e}"));

        Config {
            bind_addr: std::env::var("GATEWAY_BIND_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:3000".into()),
            database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            auth0,
            chain_id: ChainId(chain_id),
            factory_address,
            usdc_address,
        }
    }

    pub fn bind_addr(&self) -> &str {
        &self.bind_addr
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    pub fn auth0(&self) -> Option<&Auth0Config> {
        self.auth0.as_ref()
    }

    pub fn chain_id(&self) -> ChainId {
        self.chain_id
    }

    pub fn factory_address(&self) -> Address {
        self.factory_address
    }

    pub fn usdc_address(&self) -> Address {
        self.usdc_address
    }
}

pub struct Auth0Config {
    pub issuer: String,
    pub audience: String,
    pub client_id: String,
}
