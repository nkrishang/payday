use alloy_primitives::Address;
use gateway_core::ChainId;
use gateway_db::{AccountRepository, InvoiceRepository, WebhookRepository};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::api::{Auth0Verifier, payer::PayerAccess};

/// Shared application state passed to all Axum handlers via `.with_state()`.
#[derive(Clone)]
pub struct AppState {
    pub repo: InvoiceRepository,
    pub accounts: AccountRepository,
    pub identity_verifier: Option<Auth0Verifier>,
    pub chain_id: ChainId,
    pub factory_address: Address,
    pub usdc_address: Address,
    pub payer: PayerAccess,
    pub webhooks: WebhookRepository,
    /// 256-bit AEAD key. Webhook APIs remain unavailable when not configured.
    pub webhook_encryption_key: Option<[u8; 32]>,
    pub api_key_prefix: String,
    pub rate_limits: Arc<Mutex<HashMap<Uuid, (f64, Instant)>>>,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repo: InvoiceRepository,
        accounts: AccountRepository,
        identity_verifier: Option<Auth0Verifier>,
        chain_id: ChainId,
        factory_address: Address,
        usdc_address: Address,
        payer: PayerAccess,
        api_key_prefix: String,
        webhook_encryption_key: Option<[u8; 32]>,
    ) -> Self {
        let webhooks = WebhookRepository::new(repo.pool().clone());
        Self {
            repo,
            accounts,
            identity_verifier,
            chain_id,
            factory_address,
            usdc_address,
            payer,
            webhooks,
            webhook_encryption_key,
            api_key_prefix,
            rate_limits: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}
