//! gum-signers: the execution service and the only process that holds keys.
//!
//! It consumes `ExecutionCommand`s from the bus into the `execution`
//! schema, runs one [`executor::ChainWorker`] per chain that turns queued
//! jobs into signed, tracked, finalized transactions, and publishes
//! `ExecutionEvent`s describing what happened. It never reads or writes
//! the application ledger.

pub mod config;
pub mod executor;
pub mod handlers;
pub mod step;
pub mod store;
pub mod sweep;

pub use config::{Config, SignerConfig};
pub use executor::{ChainWorker, PassReport, Policy};
pub use handlers::{ChainControlHandler, CommandHandler};
