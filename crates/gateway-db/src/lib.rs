mod accounts;
mod attachments;
mod cursor;
mod customers;
mod invoices;
mod issuers;
mod notifications;
mod onboarding;
mod proofs;
mod sweeps;
mod verifications;
mod webhooks;

pub use accounts::{
    API_KEY_GRACE_HOURS, AccountId, AccountRepository, ApiKeyMetadata, IssueApiKeyError,
    IssuedApiKey, ProvisionAccountError,
};
pub use attachments::{
    AttachInvoiceError, AttachmentRepository, AttachmentStatus, AttachmentStatusParseError,
    CreateAttachmentUpload, DbAttachment,
};
pub use cursor::{CursorRepository, FinalizedHead, IndexerCursor};
pub use customers::{CreateCustomerInput, CustomerRepository, DbCustomer};
pub use invoices::{
    CreateInvoiceInput, CustomerInvoiceStats, DbIndexerFreshness, DbInvoice, DbInvoiceError,
    DbInvoiceTransfer, InsertIssuedInvoice, InsertIssuedInvoiceError, InvoiceRepository,
    IssuanceRequest, PaymentObservation, RangeOutcome, ReleasePaymentError, same_issuance,
};
pub use issuers::{
    CreateIssuerInput, CreatePayoutAddressInput, DbIssuer, DbIssuerPayoutAddress, DbPayoutAddress,
    ISSUER_NAME_UNIQUE, IssuerRepository, StartIssuerEmailError, is_duplicate_issuer_name,
};
pub use notifications::{NotificationEvent, NotificationRepository};
pub use onboarding::{OnboardingClaim, OnboardingDemoPaymentRepository};
pub use proofs::{DbInvoiceSettlement, DbSettlementTransfer, ProofRepository};
use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
pub use sweeps::{
    BatchResolution, InvoiceOutcome, MinedBatch, RecoveredFundsInput, RecoveryReason,
    SWEEPABLE_STATUSES, SweepBatch, SweepQueueStats, SweeperStatus,
};
pub use verifications::{
    CreatedPayerSession, DbPayerSession, DbVerificationAttempt, EmailVerificationAttempt,
    PAYER_SESSION_TTL, PayerSessionRepository, StartEmailVerificationError, VerificationCompletion,
    payer_ref,
};
pub use webhooks::{
    DeliveryClaim, WebhookAttempt, WebhookDelivery, WebhookEndpoint, WebhookEvent,
    WebhookRepository,
};

/// The gateway's schema migrations, embedded at compile time. This crate owns
/// the migrations for the shared database; both services run them on startup via
/// [`connect`], and `#[sqlx::test(migrator = "gateway_db::MIGRATOR")]` reuses
/// this same set so tests stay in lockstep with production schema.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;

    MIGRATOR.run(&pool).await?;

    tracing::info!("database migrations applied");

    Ok(pool)
}

/// Build a short-timeout pool without opening a connection. The independently
/// deployed public status process must start even while RDS is unavailable.
pub fn connect_lazy(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect_lazy(database_url)
}
