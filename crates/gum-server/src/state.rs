use alloy_primitives::Address;
use gum_core::{ChainRegistry, ProofOfPayment};
use gum_ledger::{
    AccountRepository, AttachmentRepository, CustomerRepository, InvoiceRepository,
    PayerSessionRepository, ProofRepository, RelayIntentRepository, WebhookRepository,
    WithdrawalRepository,
};
use gum_relay::{RelayApi, RelayChain, RelayError};
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
use crate::chain_reader::ChainReads;
use crate::payer_identity::PayerVerification;
use crate::pregenerated_wallet::WalletPregenerator;

/// `(account, deposit request, payment address)`.
type ProofCacheKey = (Uuid, Uuid, Address);

/// Shared application state passed to all Axum handlers via `.with_state()`.
#[derive(Clone)]
pub struct AppState {
    pub repo: InvoiceRepository,
    pub accounts: AccountRepository,
    pub attachments: AttachmentRepository,
    pub customers: CustomerRepository,
    pub proofs: ProofRepository,
    pub payer_sessions: PayerSessionRepository,
    pub withdrawals: WithdrawalRepository,
    /// Balances and USDC domains per chain, for withdrawals; `None` leaves
    /// the withdrawal routes answering `withdrawals_unavailable`.
    pub(crate) chain_reader: Option<Arc<dyn ChainReads>>,
    /// Verifies dashboard sessions; `None` means only API keys authenticate.
    pub merchant_verifier: Option<PrivyVerifier>,
    /// The payer audience; `None` leaves gated invoices unverifiable and the
    /// email routes answering `verification_unavailable`.
    pub payer_verification: Option<PayerVerification>,
    /// The networks a deposit request may be paid on.
    pub networks: Arc<ChainRegistry>,
    /// Gum's own recovery wallet: the recovery term every payment contract
    /// commits to, fixed at issuance and never a payer's.
    pub recovery_address: Address,
    /// Cross-chain payments through Relay; `None` without
    /// `GUM_RELAY_API_KEY`, and the relay routes answer `relay_unavailable`.
    pub relay: Option<Arc<RelayService>>,
    pub relay_intents: RelayIntentRepository,
    pub payer: PayerAccess,
    pub webhooks: WebhookRepository,
    /// 256-bit AEAD key. Webhook APIs remain unavailable when not configured.
    pub webhook_encryption_key: Option<[u8; 32]>,
    pub api_key_prefix: String,
    pub rate_limits: Arc<Mutex<HashMap<Uuid, (f64, Instant)>>>,
    /// Bucket capacity, and tokens refilled per second, of the per-account
    /// rate limiter: `GUM_RATE_LIMIT_PER_MINUTE`, 60 by default.
    pub rate_limit_per_minute: f64,
    proof_cache: Arc<StdMutex<HashMap<ProofCacheKey, ProofOfPayment>>>,
    /// The attachment bucket and the attestation key; the service configures
    /// both at startup, and a route that needs one answers 500 without it.
    attachment_store: Option<AttachmentStore>,
    attestor: Option<VerificationAttestor>,
    /// `None` unless `GUM_PRIVY_APP_SECRET` is configured; wallet
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
        recovery_address: Address,
        payer: PayerAccess,
        api_key_prefix: String,
        webhook_encryption_key: Option<[u8; 32]>,
        attachment_store: Option<AttachmentStore>,
        attestor: Option<VerificationAttestor>,
        payer_verification: Option<PayerVerification>,
        pregenerated_wallets: Option<Arc<dyn WalletPregenerator>>,
        chain_reader: Option<Arc<dyn ChainReads>>,
        relay: Option<Arc<dyn RelayApi>>,
    ) -> Self {
        let pool = repo.pool().clone();
        Self {
            webhooks: WebhookRepository::new(pool.clone()),
            withdrawals: WithdrawalRepository::new(pool.clone()),
            relay: relay.map(|api| Arc::new(RelayService::new(api))),
            relay_intents: RelayIntentRepository::new(pool.clone()),
            chain_reader,
            attachments: AttachmentRepository::new(pool.clone()),
            customers: CustomerRepository::new(pool.clone()),
            proofs: ProofRepository::new(pool.clone()),
            payer_sessions: PayerSessionRepository::new(pool.clone()),
            repo,
            accounts,
            merchant_verifier,
            payer_verification,
            networks,
            recovery_address,
            payer,
            webhook_encryption_key,
            api_key_prefix,
            rate_limits: Arc::new(Mutex::new(HashMap::new())),
            rate_limit_per_minute: std::env::var("GUM_RATE_LIMIT_PER_MINUTE")
                .ok()
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|limit| *limit > 0.0)
                .unwrap_or(60.0),
            proof_cache: Arc::new(StdMutex::new(HashMap::new())),
            attachment_store,
            attestor,
            pregenerated_wallets,
            pregenerate_wallet_asked: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    pub fn relay(&self) -> Result<&RelayService, ApiError> {
        self.relay
            .as_deref()
            .ok_or_else(ApiError::relay_unavailable)
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

/// Relay's chain list changes rarely and is asked for on every checkout
/// that opens the cross-chain option, so it is read once an hour.
const RELAY_CHAINS_TTL: Duration = Duration::from_secs(3600);

/// Relay, with its chain list cached.
pub struct RelayService {
    api: Arc<dyn RelayApi>,
    chains: Mutex<Option<(Instant, Arc<Vec<RelayChain>>)>>,
}

impl RelayService {
    pub fn new(api: Arc<dyn RelayApi>) -> Self {
        Self {
            api,
            chains: Mutex::new(None),
        }
    }

    pub fn api(&self) -> &dyn RelayApi {
        self.api.as_ref()
    }

    /// A handle for the background pollers that outlive any one request.
    pub fn shared_api(&self) -> Arc<dyn RelayApi> {
        self.api.clone()
    }

    /// The chains Relay serves, refreshed hourly. A failed refresh keeps the
    /// stale list rather than taking the option away.
    pub async fn chains(&self) -> Result<Arc<Vec<RelayChain>>, RelayError> {
        let mut cached = self.chains.lock().await;
        if let Some((read_at, chains)) = cached.as_ref()
            && read_at.elapsed() < RELAY_CHAINS_TTL
        {
            return Ok(Arc::clone(chains));
        }
        match self.api.chains().await {
            Ok(chains) => {
                let chains = Arc::new(chains);
                *cached = Some((Instant::now(), Arc::clone(&chains)));
                Ok(chains)
            }
            Err(error) => match cached.as_ref() {
                Some((_, chains)) => {
                    tracing::warn!(%error, "relay chain list refresh failed; serving the last one");
                    Ok(Arc::clone(chains))
                }
                None => Err(error),
            },
        }
    }
}
