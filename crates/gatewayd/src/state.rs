use alloy_primitives::Address;
use gateway_core::ChainId;
use gateway_db::{AccountRepository, InvoiceRepository};

use crate::api::Auth0Verifier;

/// Shared application state passed to all Axum handlers via `.with_state()`.
#[derive(Clone)]
pub struct AppState {
    pub repo: InvoiceRepository,
    pub accounts: AccountRepository,
    pub identity_verifier: Option<Auth0Verifier>,
    pub chain_id: ChainId,
    pub factory_address: Address,
    pub usdc_address: Address,
}

impl AppState {
    pub fn new(
        repo: InvoiceRepository,
        accounts: AccountRepository,
        identity_verifier: Option<Auth0Verifier>,
        chain_id: ChainId,
        factory_address: Address,
        usdc_address: Address,
    ) -> Self {
        Self {
            repo,
            accounts,
            identity_verifier,
            chain_id,
            factory_address,
            usdc_address,
        }
    }
}
