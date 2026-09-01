use std::str::FromStr;

use alloy_primitives::Address;
use gateway_core::ChainId;

pub struct Config {
    bind_addr: String,
    database_url: String,
    status_only: bool,
    auth0: Option<Auth0Config>,
    dev_identity: bool,
    chain_id: ChainId,
    factory_address: Address,
    usdc_address: Address,
    public_base_url: String,
    explorer_base_url: Option<String>,
    api_key_prefix: String,
    webhook_encryption_key: Option<[u8; 32]>,
    notification_from_address: Option<String>,
    status_stale_seconds: u64,
}

impl Config {
    pub fn from_env() -> Self {
        let status_only = std::env::var("PAYDAY_STATUS_ONLY").as_deref() == Ok("true");
        let dev_identity = std::env::var("PAYDAY_DEV_IDENTITY").as_deref() == Ok("1");
        let auth0_issuer = std::env::var("PAYDAY_AUTH0_ISSUER").ok();
        let auth0_audience = std::env::var("PAYDAY_AUTH0_AUDIENCE").ok();
        let auth0_client_id = std::env::var("PAYDAY_AUTH0_CLIENT_ID").ok();
        let auth0 = match (auth0_issuer, auth0_audience, auth0_client_id) {
            (Some(issuer), Some(audience), Some(client_id)) => Some(Auth0Config {
                issuer,
                audience,
                client_id,
            }),
            (None, None, None) => None,
            _ => panic!(
                "PAYDAY_AUTH0_ISSUER, PAYDAY_AUTH0_AUDIENCE, and PAYDAY_AUTH0_CLIENT_ID must be set together"
            ),
        };

        let factory_address =
            std::env::var("PAYDAY_FACTORY_ADDRESS").expect("PAYDAY_FACTORY_ADDRESS must be set");
        let factory_address = Address::from_str(&factory_address)
            .unwrap_or_else(|e| panic!("invalid PAYDAY_FACTORY_ADDRESS '{factory_address}': {e}"));

        let chain_id: u64 = std::env::var("PAYDAY_CHAIN_ID")
            .expect("PAYDAY_CHAIN_ID must be set")
            .parse()
            .unwrap_or_else(|e| panic!("invalid PAYDAY_CHAIN_ID: {e}"));

        let usdc_address =
            std::env::var("PAYDAY_USDC_ADDRESS").expect("PAYDAY_USDC_ADDRESS must be set");
        let usdc_address = Address::from_str(&usdc_address)
            .unwrap_or_else(|e| panic!("invalid PAYDAY_USDC_ADDRESS '{usdc_address}': {e}"));

        let api_key_prefix =
            std::env::var("PAYDAY_API_KEY_PREFIX").unwrap_or_else(|_| "payday_live_".into());
        validate_api_key_prefix(&api_key_prefix).unwrap_or_else(|message| panic!("{message}"));
        let webhook_encryption_key =
            std::env::var("PAYDAY_WEBHOOK_ENCRYPTION_KEY")
                .ok()
                .map(|value| {
                    decode_webhook_encryption_key(&value)
                        .unwrap_or_else(|message| panic!("{message}"))
                });

        Config {
            bind_addr: std::env::var("PAYDAY_BIND_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:3000".into()),
            database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            status_only,
            auth0,
            dev_identity,
            chain_id: ChainId(chain_id),
            factory_address,
            usdc_address,
            public_base_url: std::env::var("PAYDAY_PUBLIC_BASE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:3000".into()),
            explorer_base_url: std::env::var("PAYDAY_EXPLORER_BASE_URL").ok(),
            api_key_prefix,
            webhook_encryption_key,
            notification_from_address: std::env::var("PAYDAY_NOTIFICATION_FROM_ADDRESS").ok(),
            status_stale_seconds: std::env::var("PAYDAY_STATUS_INDEXER_STALE_SECONDS")
                .map_or(Ok(120), |value| value.parse())
                .expect("PAYDAY_STATUS_INDEXER_STALE_SECONDS must be an integer"),
        }
    }

    pub fn bind_addr(&self) -> &str {
        &self.bind_addr
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    pub fn status_only(&self) -> bool {
        self.status_only
    }

    pub fn status_stale_seconds(&self) -> u64 {
        self.status_stale_seconds
    }

    pub fn auth0(&self) -> Option<&Auth0Config> {
        self.auth0.as_ref()
    }

    pub fn dev_identity(&self) -> bool {
        self.dev_identity
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

    pub fn public_base_url(&self) -> &str {
        &self.public_base_url
    }

    pub fn explorer_base_url(&self) -> Option<&str> {
        self.explorer_base_url.as_deref()
    }

    pub fn api_key_prefix(&self) -> &str {
        &self.api_key_prefix
    }

    pub fn webhook_encryption_key(&self) -> Option<[u8; 32]> {
        self.webhook_encryption_key
    }

    pub fn notification_from_address(&self) -> Option<&str> {
        self.notification_from_address.as_deref()
    }
}

fn validate_api_key_prefix(value: &str) -> Result<(), &'static str> {
    match value {
        "payday_live_" | "payday_test_" => Ok(()),
        _ => Err("PAYDAY_API_KEY_PREFIX must be exactly payday_live_ or payday_test_"),
    }
}

fn decode_webhook_encryption_key(value: &str) -> Result<[u8; 32], &'static str> {
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| "PAYDAY_WEBHOOK_ENCRYPTION_KEY must be valid standard base64")?;
    decoded
        .try_into()
        .map_err(|_| "PAYDAY_WEBHOOK_ENCRYPTION_KEY must decode to exactly 32 bytes")
}

pub struct Auth0Config {
    pub issuer: String,
    pub audience: String,
    pub client_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_supported_api_key_prefixes() {
        assert!(validate_api_key_prefix("payday_live_").is_ok());
        assert!(validate_api_key_prefix("payday_test_").is_ok());
        assert!(validate_api_key_prefix("payday_dev_").is_err());
        assert!(validate_api_key_prefix("payday_live").is_err());
    }

    #[test]
    fn encryption_key_requires_standard_base64_of_32_bytes() {
        assert_eq!(
            decode_webhook_encryption_key("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=")
                .unwrap()
                .len(),
            32
        );
        assert!(decode_webhook_encryption_key("not base64").is_err());
        assert!(decode_webhook_encryption_key("c2hvcnQ=").is_err());
    }
}
