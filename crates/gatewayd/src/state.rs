use crate::db::InvoiceRepository;
use alloy_primitives::Address;
use gateway_core::ChainId;

/// Shared application state passed to all Axum handlers via `.with_state()`.
#[derive(Clone)]
pub struct AppState {
    pub repo: InvoiceRepository,
    pub chain_id: ChainId,
    pub factory_address: Address,
}

impl AppState {
    pub fn new(repo: InvoiceRepository, chain_id: ChainId, factory_address: Address) -> Self {
        Self {
            repo,
            chain_id,
            factory_address,
        }
    }
}
