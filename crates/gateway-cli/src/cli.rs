//! Payday command-line surface.

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "payday", about = "Get paid in USDC with Payday", version)]
pub struct Cli {
    /// Base URL of the Payday API.
    #[arg(
        long,
        global = true,
        env = "PAYDAY_API_URL",
        default_value = "https://api.payday.sh"
    )]
    pub api_url: String,
    /// Bearer API key. Prefer PAYDAY_API_KEY so it stays out of shell history.
    #[arg(long, global = true, env = "PAYDAY_API_KEY", hide_env_values = true)]
    pub api_key: Option<String>,
    /// Auth0 tenant issuer used for account signup and key management.
    #[arg(long, global = true, env = "PAYDAY_AUTH0_ISSUER")]
    pub auth0_issuer: Option<String>,
    /// Public Auth0 Native application client ID.
    #[arg(long, global = true, env = "PAYDAY_AUTH0_CLIENT_ID")]
    pub auth0_client_id: Option<String>,
    /// Auth0 API audience for api.payday.sh.
    #[arg(long, global = true, env = "PAYDAY_AUTH0_AUDIENCE")]
    pub auth0_audience: Option<String>,
    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Create a one-time USDC payment address.
    Create(CreateArgs),
    /// Check a payment by ID or unambiguous prefix.
    Get(GetArgs),
    /// List payments, newest first.
    List(ListArgs),
    /// Sign in and manage your account API key.
    #[command(subcommand)]
    Account(AccountCommand),
}

#[derive(Clone, Debug, Subcommand)]
pub enum AccountCommand {
    /// Sign in and create or replace the account's API key.
    Create(CreateAccountArgs),
    /// Show non-secret metadata for the current API key.
    Get,
}

#[derive(Clone, Debug, Args)]
pub struct CreateAccountArgs {
    /// Replace an existing API key without asking for confirmation.
    #[arg(short, long)]
    pub yes: bool,
}

#[derive(Clone, Debug, Args)]
pub struct CreateArgs {
    /// Chain ID for this payment.
    #[arg(long)]
    pub chain_id: u64,
    /// Circle-issued USDC contract address.
    #[arg(long)]
    pub token: String,
    /// Address that receives a successful payment.
    #[arg(long)]
    pub payout: String,
    /// Amount in USDC, such as "100.50".
    #[arg(long)]
    pub amount: String,
    /// Seconds before the payment expires.
    #[arg(long, default_value_t = 86_400)]
    pub expires_in: u64,
    /// Address that receives funds sent too late.
    #[arg(long)]
    pub refund: String,
    /// Idempotency key. A UUIDv7 is generated when omitted.
    #[arg(long)]
    pub idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Args)]
pub struct GetArgs {
    /// Payment ID (`pay_…`) or any unambiguous prefix.
    pub id: String,
}

#[derive(Clone, Debug, Args)]
pub struct ListArgs {
    /// Number of payments to show (1–100).
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=100))]
    pub limit: u32,
    /// Continue after this payment ID from the previous page.
    #[arg(long)]
    pub starting_after: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn product_name_and_top_level_payment_commands_are_the_public_surface() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("Get paid in USDC with Payday"));
        assert!(help.contains("create"));
        assert!(help.contains("get"));
        assert!(help.contains("list"));
        assert!(!help.contains("gateway-cli"));
        assert!(!help.contains("invoice"));
        assert_eq!(Cli::command().get_name(), "payday");
    }

    #[test]
    fn nested_invoice_command_is_rejected() {
        assert!(Cli::try_parse_from(["payday", "invoice", "get", "pay_0"]).is_err());
    }
}
