mod auth;
mod error;
mod health;
mod invoices;
mod routes;

pub(crate) use auth::ApiKey;
pub use routes::router;
