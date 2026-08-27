mod accounts;
mod cursor;
mod invoices;
mod sweeps;

pub use accounts::{AccountId, AccountRepository, ApiKeyMetadata, IssueApiKeyError, IssuedApiKey};
pub use cursor::{CursorRepository, IndexerCursor};
pub use invoices::{
    CreateInvoiceInput, DbInvoice, DbInvoiceError, InvoiceRepository, PaymentObservation,
    RangeOutcome,
};
use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
pub use sweeps::{
    BatchResolution, InvoiceOutcome, MinedBatch, SWEEPABLE_STATUSES, SweepBatch, SweepQueueStats,
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
