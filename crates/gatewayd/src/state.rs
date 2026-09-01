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
    /// Payday's custodial recovery wallet, stamped on every new invoice.
    /// Merchants cannot choose it.
    pub recovery_address: Address,
    pub payer: PayerAccess,
    pub webhooks: WebhookRepository,
    /// 256-bit AEAD key. Webhook APIs remain unavailable when not configured.
    pub webhook_encryption_key: Option<[u8; 32]>,
    pub api_key_prefix: String,
    pub status_stale_seconds: u64,
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
        recovery_address: Address,
        payer: PayerAccess,
        api_key_prefix: String,
        webhook_encryption_key: Option<[u8; 32]>,
        status_stale_seconds: u64,
    ) -> Self {
        let webhooks = WebhookRepository::new(repo.pool().clone());
        Self {
            repo,
            accounts,
            identity_verifier,
            chain_id,
            factory_address,
            usdc_address,
            recovery_address,
            payer,
            webhooks,
            webhook_encryption_key,
            api_key_prefix,
            status_stale_seconds,
            rate_limits: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}
