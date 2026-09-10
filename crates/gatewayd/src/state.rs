use alloy_primitives::Address;
use gateway_core::{ChainRegistry, ProofOfPayment};
use gateway_db::{
    AccountRepository, AttachmentRepository, CustomerRepository, InvoiceRepository,
    IssuerRepository, OnboardingDemoPaymentRepository, PayerSessionRepository, ProofRepository,
    WebhookRepository,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use uuid::Uuid;

/// A merchant resending their code, or two tabs open on the same sign-up,
/// should not re-trigger a Privy user-creation call; a legitimate sign-up
/// only ever needs one.
const PREGENERATE_WALLET_COOLDOWN: Duration = Duration::from_secs(30);
/// Bounds `pregenerate_wallet_asked` against an arbitrary stream of emails;
/// the same trade-off `AppState::cache_proof` makes.
const PREGENERATE_WALLET_ASKED_CAP: usize = 4_096;

use crate::api::error::ApiError;
use crate::api::{PrivyVerifier, payer::PayerAccess};
use crate::attachments::AttachmentStore;
use crate::attestation::VerificationAttestor;
use crate::onboarding_payer::OnboardingPayerSigner;
use crate::payer_identity::PayerVerification;
use crate::pregenerated_wallet::WalletPregenerator;

/// Shared application state passed to all Axum handlers via `.with_state()`.
#[derive(Clone)]
pub struct AppState {
    pub repo: InvoiceRepository,
    pub accounts: AccountRepository,
    pub attachments: AttachmentRepository,
    pub customers: CustomerRepository,
    pub issuers: IssuerRepository,
    pub proofs: ProofRepository,
    pub payer_sessions: PayerSessionRepository,
    pub onboarding_demo_payments: OnboardingDemoPaymentRepository,
    /// Verifies dashboard sessions; `None` means only API keys authenticate.
    pub merchant_verifier: Option<PrivyVerifier>,
    /// The payer audience; `None` leaves gated invoices unverifiable and the
    /// email routes answering `verification_unavailable`.
    pub payer_verification: Option<PayerVerification>,
    /// The networks a deposit request may be paid on.
    pub networks: Arc<ChainRegistry>,
    pub payer: PayerAccess,
    pub webhooks: WebhookRepository,
    /// 256-bit AEAD key. Webhook APIs remain unavailable when not configured.
    pub webhook_encryption_key: Option<[u8; 32]>,
    pub api_key_prefix: String,
    pub rate_limits: Arc<Mutex<HashMap<Uuid, (f64, Instant)>>>,
    proof_cache: Arc<StdMutex<HashMap<(Uuid, Uuid, Address), ProofOfPayment>>>,
    /// The attachment bucket and the attestation key; the service configures
    /// both at startup, and a route that needs one answers 500 without it.
    attachment_store: Option<AttachmentStore>,
    attestor: Option<VerificationAttestor>,
    /// `None` unless a deployment has deliberately funded and configured a
    /// wallet for the onboarding walkthrough's one demo transfer.
    onboarding_payer: Option<OnboardingPayerSigner>,
    /// `None` unless `PAYDAY_PRIVY_APP_SECRET` is configured; wallet
    /// pregeneration (pregenerated_wallet.rs) is a latency optimization, not
    /// a dependency, so its absence never blocks sign-in.
    pregenerated_wallets: Option<Arc<dyn WalletPregenerator>>,
    /// One entry per email that has asked for pregeneration recently, so a
    /// merchant resending their code cannot re-trigger it needlessly and an
    /// arbitrary stream of emails cannot grow this without bound. Bounded and
    /// cleared the same way `proof_cache` is, not decayed per-entry: a
    /// legitimate sign-up only ever needs this once.
    pregenerate_wallet_asked: Arc<StdMutex<HashMap<String, Instant>>>,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repo: InvoiceRepository,
        accounts: AccountRepository,
        merchant_verifier: Option<PrivyVerifier>,
        networks: Arc<ChainRegistry>,
        payer: PayerAccess,
        api_key_prefix: String,
        webhook_encryption_key: Option<[u8; 32]>,
        attachment_store: Option<AttachmentStore>,
        attestor: Option<VerificationAttestor>,
        payer_verification: Option<PayerVerification>,
        onboarding_payer: Option<OnboardingPayerSigner>,
        pregenerated_wallets: Option<Arc<dyn WalletPregenerator>>,
    ) -> Self {
        let pool = repo.pool().clone();
        Self {
            webhooks: WebhookRepository::new(pool.clone()),
            attachments: AttachmentRepository::new(pool.clone()),
            customers: CustomerRepository::new(pool.clone()),
            issuers: IssuerRepository::new(pool.clone()),
            proofs: ProofRepository::new(pool.clone()),
            payer_sessions: PayerSessionRepository::new(pool.clone()),
            onboarding_demo_payments: OnboardingDemoPaymentRepository::new(pool),
            onboarding_payer,
            repo,
            accounts,
            merchant_verifier,
            payer_verification,
            networks,
            payer,
            webhook_encryption_key,
            api_key_prefix,
            rate_limits: Arc::new(Mutex::new(HashMap::new())),
            proof_cache: Arc::new(StdMutex::new(HashMap::new())),
            attachment_store,
            attestor,
            pregenerated_wallets,
            pregenerate_wallet_asked: Arc::new(StdMutex::new(HashMap::new())),
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

    pub fn onboarding_payer(&self) -> Result<&OnboardingPayerSigner, ApiError> {
        self.onboarding_payer
            .as_ref()
            .ok_or_else(ApiError::onboarding_deposit_unavailable)
    }

    /// The pregenerator to call for `email`, if the deployment has one
    /// configured and this email has not already asked recently. Checking
    /// and marking are one step so two concurrent requests for the same
    /// email cannot both slip through the cooldown.
    pub fn pregenerate_wallet_for(
        &self,
        email: &str,
    ) -> Result<Arc<dyn WalletPregenerator>, ApiError> {
        let pregenerator = self
            .pregenerated_wallets
            .clone()
            .ok_or_else(ApiError::wallet_pregeneration_unavailable)?;
        let now = Instant::now();
        let mut asked = self
            .pregenerate_wallet_asked
            .lock()
            .expect("pregenerate wallet lock poisoned");
        if asked
            .get(email)
            .is_some_and(|last| now.duration_since(*last) < PREGENERATE_WALLET_COOLDOWN)
        {
            return Err(ApiError::rate_limited());
        }
        if asked.len() >= PREGENERATE_WALLET_ASKED_CAP {
            asked.clear();
        }
        asked.insert(email.to_owned(), now);
        Ok(pregenerator)
    }

    pub fn cached_proof(
        &self,
        account_id: Uuid,
        invoice_id: Uuid,
        attestor: Address,
    ) -> Option<ProofOfPayment> {
        self.proof_cache
            .lock()
            .expect("proof cache lock poisoned")
            .get(&(account_id, invoice_id, attestor))
            .cloned()
    }

    pub fn cache_proof(
        &self,
        account_id: Uuid,
        invoice_id: Uuid,
        attestor: Address,
        proof: ProofOfPayment,
    ) {
        let mut cache = self.proof_cache.lock().expect("proof cache lock poisoned");
        if cache.len() >= 1_024 {
            cache.clear();
        }
        cache.insert((account_id, invoice_id, attestor), proof);
    }
}
