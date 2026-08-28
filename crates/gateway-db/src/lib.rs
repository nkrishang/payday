mod accounts;
mod cursor;
mod invoices;
mod notifications;
mod sweeps;
mod webhooks;

pub use accounts::{
    API_KEY_GRACE_HOURS, AccountId, AccountRepository, ApiKeyMetadata, IssueApiKeyError,
    IssuedApiKey,
};
pub use cursor::{CursorRepository, FinalizedHead, IndexerCursor};
pub use invoices::{
    CreateInvoiceInput, DbIndexerFreshness, DbInvoice, DbInvoiceError, DbInvoiceTransfer,
    InvoiceRepository, PaymentObservation, RangeOutcome, ReleasePaymentError,
};
pub use notifications::{NotificationEvent, NotificationRepository};
use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
pub use sweeps::{
    BatchResolution, InvoiceOutcome, MinedBatch, SWEEPABLE_STATUSES, SweepBatch, SweepQueueStats,
    SweeperStatus,
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
