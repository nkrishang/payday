//! Command-line surface (clap definitions).

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "gateway-cli",
    about = "Client for the stablecoin payment gateway HTTP API",
    version
)]
pub struct Cli {
    /// Base URL of the gateway API.
    #[arg(
        long,
        global = true,
        env = "GATEWAY_API_URL",
        default_value = "http://127.0.0.1:3000"
    )]
    pub api_url: String,

    /// Bearer API key. Prefer the GATEWAY_API_KEY environment variable to
    /// avoid exposing the key in shell history and process listings.
    #[arg(long, global = true, env = "GATEWAY_API_KEY", hide_env_values = true)]
    pub api_key: Option<String>,

    /// Emit machine-readable JSON instead of a human-readable summary.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create and query invoices.
    #[command(subcommand)]
    Invoice(InvoiceCommand),
}

#[derive(Debug, Subcommand)]
pub enum InvoiceCommand {
    /// Create a new invoice and print its payment instructions.
    Create(CreateArgs),
    /// Fetch an existing invoice by ID.
    Get(GetArgs),
}

#[derive(Debug, Args)]
pub struct CreateArgs {
    /// Chain ID the invoice is scoped to.
    #[arg(long)]
    pub chain_id: u64,

    /// Circle-issued USDC contract address for the selected chain.
    #[arg(long)]
    pub token: String,

    /// Beneficiary address that will receive the settled funds.
    #[arg(long)]
    pub beneficiary: String,

    /// Amount in full human-readable units (e.g. "100.5").
    #[arg(long)]
    pub amount: String,

    /// Unix timestamp in seconds after which funds are sent to recovery.
    #[arg(long)]
    pub expiration_timestamp: u64,

    /// Address that receives all funds when execution occurs after expiration.
    #[arg(long)]
    pub recovery: String,

    /// Idempotency key. If omitted, a fresh UUIDv7 is generated per invocation.
    #[arg(long)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Args)]
pub struct GetArgs {
    /// Invoice ID (UUID).
    pub id: String,
}
