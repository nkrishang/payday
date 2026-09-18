//! Gum's database schema, embedded at compile time.
//!
//! One migration set covers the three schemas the services use:
//!
//! * `public` — the application ledger, written only by `gum-server`;
//! * `bus` — durable messaging between services (`gum-bus`);
//! * `execution` — transaction execution state, written only by `gum-signers`.
//!
//! Migrations are applied explicitly (`gum-server migrate`, the local
//! runner, the test harness), never implicitly by a service at startup:
//! a service that starts against an older schema fails its readiness
//! check instead of racing another replica to alter tables.
//! `#[sqlx::test(migrator = "gum_schema::MIGRATOR")]` runs the same set.

use sqlx::migrate::Migrator;

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Open a pool. Migrations are *not* applied here (see above).
pub async fn connect(
    database_url: &str,
    max_connections: u32,
) -> Result<sqlx::PgPool, sqlx::Error> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(database_url)
        .await
}
