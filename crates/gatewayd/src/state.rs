use alloy_primitives::Address;
use gateway_core::ChainId;
use gateway_db::{
    AccountRepository, AttachmentRepository, CustomerRepository, InvoiceRepository,
    PayerSessionRepository, ProofRepository, VerificationRepository, WebhookRepository,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::{Auth0Verifier, payer::PayerAccess};
use crate::attachments::AttachmentStore;
use crate::attestation::VerificationAttestor;
use crate::identity::PayerIdentityProvider;
use crate::payer_identity::PayerVerification;

/// Shared application state passed to all Axum handlers via `.with_state()`.
#[derive(Clone)]
pub struct AppState {
    pub repo: InvoiceRepository,
    pub accounts: AccountRepository,
    pub attachments: AttachmentRepository,
    pub customers: CustomerRepository,
    pub proofs: ProofRepository,
    pub payer_sessions: PayerSessionRepository,
    pub verifications: VerificationRepository,
    pub identity_verifier: Option<Auth0Verifier>,
    /// The document-and-liveness provider; `None` leaves identity start
    /// answering `verification_unavailable`.
    pub identity: Option<Arc<dyn PayerIdentityProvider>>,
    /// The payer audience; `None` leaves gated invoices unverifiable and the
    /// email routes answering `verification_unavailable`.
    pub payer_verification: Option<PayerVerification>,
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
    /// The attachment bucket and the attestation key are `None` only in
    /// status-only mode, whose router never reaches the routes that need them.
    attachment_store: Option<AttachmentStore>,
    attestor: Option<VerificationAttestor>,
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
        attachment_store: Option<AttachmentStore>,
        attestor: Option<VerificationAttestor>,
        payer_verification: Option<PayerVerification>,
        identity: Option<Arc<dyn PayerIdentityProvider>>,
    ) -> Self {
        let pool = repo.pool().clone();
        Self {
            webhooks: WebhookRepository::new(pool.clone()),
            attachments: AttachmentRepository::new(pool.clone()),
            customers: CustomerRepository::new(pool.clone()),
            proofs: ProofRepository::new(pool.clone()),
            payer_sessions: PayerSessionRepository::new(pool.clone()),
            verifications: VerificationRepository::new(pool),
            identity,
            repo,
            accounts,
            identity_verifier,
            payer_verification,
            chain_id,
            factory_address,
            usdc_address,
            recovery_address,
            payer,
            webhook_encryption_key,
            api_key_prefix,
            status_stale_seconds,
            rate_limits: Arc::new(Mutex::new(HashMap::new())),
            attachment_store,
            attestor,
        }
    }

    pub fn attachment_store(&self) -> Result<&AttachmentStore, ApiError> {
        self.attachment_store
            .as_ref()
            .ok_or_else(|| ApiError::internal("attachment storage is not configured"))
    }

    pub fn attestor(&self) -> Result<&VerificationAttestor, ApiError> {
        self.attestor
            .as_ref()
            .ok_or_else(|| ApiError::internal("attestation signing is not configured"))
    }
}
