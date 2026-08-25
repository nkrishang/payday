use alloy_primitives::Address;
use gateway_core::ChainId;

/// Default indexer poll interval when `GATEWAY_INDEXER_POLL_INTERVAL_MS` is unset.
const DEFAULT_INDEXER_POLL_INTERVAL_MS: u64 = 2000;
const DEFAULT_FINALITY_CONFIRMATIONS: u64 = 12;
const DEFAULT_LOG_RANGE_SIZE: u64 = 100;

pub struct Config {
    database_url: String,
    chain_id: ChainId,
    rpc_url: String,
    indexer_poll_interval_ms: u64,
    finality_confirmations: u64,
    log_range_size: u64,
    usdc_start_block: u64,
    factory_address: Address,
    batch_sweeper_address: Address,
    usdc_address: Address,
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

        let indexer_poll_interval_ms = match std::env::var("GATEWAY_INDEXER_POLL_INTERVAL_MS") {
            Ok(v) => v
                .parse()
                .unwrap_or_else(|e| panic!("invalid GATEWAY_INDEXER_POLL_INTERVAL_MS: {e}")),
            Err(_) => DEFAULT_INDEXER_POLL_INTERVAL_MS,
        };

        let usdc_address = std::env::var("GATEWAY_USDC_ADDRESS")
            .expect("GATEWAY_USDC_ADDRESS must be set")
            .parse()
            .unwrap_or_else(|e| panic!("invalid GATEWAY_USDC_ADDRESS: {e}"));
        let factory_address = parse_address_env("GATEWAY_FACTORY_ADDRESS");
        let batch_sweeper_address = parse_address_env("GATEWAY_BATCH_SWEEPER_ADDRESS");

        let finality_confirmations = parse_u64_env(
            "GATEWAY_FINALITY_CONFIRMATIONS",
            DEFAULT_FINALITY_CONFIRMATIONS,
        );
        let log_range_size = parse_u64_env("GATEWAY_LOG_RANGE_SIZE", DEFAULT_LOG_RANGE_SIZE);
        assert!(
            log_range_size > 0,
            "GATEWAY_LOG_RANGE_SIZE must be positive"
        );
        let usdc_start_block = parse_u64_env("GATEWAY_USDC_START_BLOCK", 0);

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
            indexer_poll_interval_ms,
            finality_confirmations,
            log_range_size,
            usdc_start_block,
            factory_address,
            batch_sweeper_address,
            usdc_address,
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

    pub fn indexer_poll_interval_ms(&self) -> u64 {
        self.indexer_poll_interval_ms
    }

    pub fn finality_confirmations(&self) -> u64 {
        self.finality_confirmations
    }

    pub fn log_range_size(&self) -> u64 {
        self.log_range_size
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
