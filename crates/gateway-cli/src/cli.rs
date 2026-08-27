//! Payday command-line surface.

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "payday",
    about = "Accept and track stablecoin payments",
    version,
    after_help = "Examples:\n  payday create --amount 25 --to 0x7099…79c8 --memo 'Order 1234'\n  payday get pay_0191c8e0 --watch\n  payday list --status partially_paid"
)]
pub struct Cli {
    /// Base URL of the Payday API.
    #[arg(
        long,
        global = true,
        env = "PAYDAY_API_URL",
        hide_env_values = true,
        default_value = "https://api.payday.sh",
        help_heading = "Connection"
    )]
    pub api_url: String,
    #[arg(long, global = true, env = "PAYDAY_AUTH0_ISSUER", hide = true)]
    pub auth0_issuer: Option<String>,
    #[arg(long, global = true, env = "PAYDAY_AUTH0_CLIENT_ID", hide = true)]
    pub auth0_client_id: Option<String>,
    #[arg(long, global = true, env = "PAYDAY_AUTH0_AUDIENCE", hide = true)]
    pub auth0_audience: Option<String>,
    /// Emit machine-readable JSON.
    #[arg(long, global = true, help_heading = "Output")]
    pub json: bool,
    /// Include operational details.
    #[arg(long, global = true, help_heading = "Output")]
    pub verbose: bool,
    /// Disable colors, redraws, and terminal styling.
    #[arg(long, global = true, help_heading = "Output")]
    pub plain: bool,
    /// When to use color.
    #[arg(long, global = true, value_enum, default_value_t = ColorChoice::Auto, help_heading = "Output")]
    pub color: ColorChoice,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Create a one-time USDC payment address.
    Create(CreateArgs),
    /// Check a payment by ID, prefix, or payment address.
    Get(GetArgs),
    /// List payments, newest first.
    List(ListArgs),
    /// Mark a payment cancelled (advisory; on-chain terms remain active).
    Cancel(CancelArgs),
    /// Sign in with an email one-time code and save your API key.
    Login(LoginArgs),
    /// Remove saved credentials for the current API.
    Logout,
    /// Show the account associated with the current API key.
    Whoami,
    /// Rotate or revoke API keys.
    #[command(subcommand)]
    Keys(KeysCommand),
    /// Generate shell completion code.
    Completions { shell: clap_complete::Shell },
    /// Read a built-in Payday guide.
    Docs {
        #[arg(value_enum)]
        topic: Option<DocsTopic>,
    },
    /// Install the latest verified Payday release.
    Upgrade,
}

#[derive(Clone, Debug, Args)]
#[command(
    after_help = "Example:\n  payday create --amount 25 --to 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 --memo 'Order 1234'"
)]
pub struct CreateArgs {
    /// Amount in USDC, such as "25" or "100.50".
    #[arg(long, allow_hyphen_values = true)]
    pub amount: String,
    /// Wallet that receives a successful payment.
    #[arg(long, alias = "payout")]
    pub to: String,
    /// Time until expiry, such as 30m, 24h, or 7d; defaults to 24h.
    #[arg(long, conflicts_with = "expires_at")]
    pub expires_in: Option<String>,
    /// Exact expiry in RFC 3339 format.
    #[arg(long, conflicts_with = "expires_in")]
    pub expires_at: Option<String>,
    /// Wallet for late or leftover funds; defaults to --to.
    #[arg(long, alias = "refund")]
    pub refund_to: Option<String>,
    /// Merchant-facing order reference.
    #[arg(long)]
    pub memo: Option<String>,
    /// Idempotency key for safe scripted retries.
    #[arg(long)]
    pub idempotency_key: Option<String>,
    /// Override the deployment's chain (advanced).
    #[arg(long, hide = true)]
    pub chain_id: Option<u64>,
    /// Override the deployment's USDC contract (advanced).
    #[arg(long, hide = true)]
    pub token: Option<String>,
}

#[derive(Clone, Debug, Args)]
#[command(after_help = "Example:\n  payday get pay_0191c8e0 --watch")]
pub struct GetArgs {
    /// Payment ID (`pay_…`), unambiguous prefix, or payment address.
    pub reference: String,
    /// Refresh until the payment reaches a terminal state.
    #[arg(long)]
    pub watch: bool,
    /// Refresh interval in seconds.
    #[arg(long, default_value_t = 2, requires = "watch")]
    pub interval: u64,
}

#[derive(Clone, Debug, Args)]
#[command(after_help = "Example:\n  payday list --status partially_paid --limit 20")]
pub struct ListArgs {
    /// Filter by public payment status.
    #[arg(long)]
    pub status: Option<String>,
    /// Number of payments to show (1–100).
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=100))]
    pub limit: u32,
    /// Continue after this payment ID from the previous page.
    #[arg(long)]
    pub starting_after: Option<String>,
}

#[derive(Clone, Debug, Args)]
pub struct CancelArgs {
    /// Payment ID, prefix, or payment address.
    pub reference: String,
}

#[derive(Clone, Debug, Args)]
pub struct LoginArgs {
    #[arg(long)]
    pub show: bool,
    #[arg(short, long)]
    pub yes: bool,
}

#[derive(Clone, Debug, Subcommand)]
pub enum KeysCommand {
    /// Replace the API key after email verification.
    Rotate(KeyActionArgs),
    /// Immediately revoke the current and grace-period API keys.
    Revoke(RevokeArgs),
}

#[derive(Clone, Debug, Args)]
pub struct KeyActionArgs {
    #[arg(long)]
    pub show: bool,
    #[arg(short, long)]
    pub yes: bool,
}

#[derive(Clone, Debug, Args)]
pub struct RevokeArgs {
    #[arg(short, long)]
    pub yes: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum DocsTopic {
    GettingStarted,
    Authentication,
    Environment,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn public_surface_is_branded_grouped_and_ergonomic() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("Accept and track stablecoin payments"));
        assert!(help.contains("Connection"));
        assert!(help.contains("Output"));
        assert!(help.contains("Examples:"));
        assert!(!help.contains("gateway-cli"));
        assert!(!help.contains("invoice"));

        let cli = Cli::try_parse_from([
            "payday",
            "create",
            "--amount",
            "25",
            "--to",
            "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
        ])
        .unwrap();
        let Command::Create(args) = cli.command else {
            panic!()
        };
        assert!(args.expires_in.is_none());
        assert!(args.refund_to.is_none());
    }
}
