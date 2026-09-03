//! Payday command-line surface.

use std::path::PathBuf;

use alloy_primitives::Address;
use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum Profile {
    Production,
    Local,
}

#[derive(Parser)]
#[command(
    name = "payday",
    about = "Issue and track stablecoin invoices",
    version,
    after_help = "Examples:\n  payday create --amount 25 --to 0x7099…79c8 --issuer 'Acme Corp' --bill-to 'Globex' --reference 'INV-1234'\n  payday create --from-file invoice.json --attachment invoice.pdf\n  payday get pay_0191c8e0 --watch\n  payday list --status partially_paid\n  payday proof download pay_0191c8e0 --output proof.json"
)]
pub struct Cli {
    /// Select endpoint defaults; local uses services started by `just dev`.
    #[arg(
        long,
        global = true,
        value_enum,
        conflicts_with = "sandbox",
        help_heading = "Connection"
    )]
    pub profile: Option<Profile>,
    /// Use the Payday sandbox (unless --api-url or PAYDAY_API_URL is set).
    #[arg(
        long,
        global = true,
        conflicts_with = "profile",
        help_heading = "Connection"
    )]
    pub sandbox: bool,
    /// Base URL of the Payday API.
    #[arg(
        long = "api-url",
        global = true,
        env = "PAYDAY_API_URL",
        hide_env_values = true,
        help_heading = "Connection"
    )]
    pub(crate) api_url_override: Option<String>,
    #[arg(skip = String::new())]
    pub api_url: String,
    #[arg(long, global = true, env = "PAYDAY_AUTH0_ISSUER", hide = true)]
    pub auth0_issuer: Option<String>,
    #[arg(long, global = true, env = "PAYDAY_AUTH0_CLIENT_ID", hide = true)]
    pub auth0_client_id: Option<String>,
    #[arg(long, global = true, env = "PAYDAY_AUTH0_AUDIENCE", hide = true)]
    pub auth0_audience: Option<String>,
    #[arg(long, global = true, env = "PAYDAY_ADMIN_SECRET", hide = true)]
    pub admin_secret: Option<String>,
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

impl Cli {
    pub fn resolve_connection(&mut self) {
        self.api_url = self.api_url_override.take().unwrap_or_else(|| {
            if self.profile == Some(Profile::Local) {
                "http://127.0.0.1:3000"
            } else if self.sandbox {
                "https://api.sandbox.payday.sh"
            } else {
                "https://api.payday.sh"
            }
            .into()
        });
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Issue an invoice with a one-time USDC payment address.
    Create(Box<CreateArgs>),
    /// Check an invoice by payment ID or payment address.
    Get(GetArgs),
    /// List invoices, newest first.
    List(ListArgs),
    /// Mark an invoice cancelled (advisory; on-chain terms remain active).
    Cancel(CancelArgs),
    /// Manage reusable customer records for invoices.
    #[command(subcommand)]
    Customers(CustomersCommand),
    /// Download and verify a settled invoice's Proof of Payment.
    #[command(subcommand)]
    Proof(ProofCommand),
    /// Sign in with an email one-time code and save your API key.
    Login(LoginArgs),
    /// Remove saved credentials for the current API.
    Logout,
    /// Show the account associated with the current API key.
    Whoami,
    /// Rotate or revoke API keys.
    #[command(subcommand)]
    Keys(KeysCommand),
    /// Manage webhook endpoints and inspect deliveries.
    #[command(subcommand)]
    Webhooks(WebhooksCommand),
    /// Operator-only payment recovery actions.
    #[command(subcommand, hide = true)]
    Ops(OpsCommand),
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

#[derive(Clone, Debug, Subcommand)]
pub enum OpsCommand {
    /// Resume a payout after its attention condition has been resolved.
    Release { payment: String },
}

/// Everything the quick path sets is already inside a `--from-file` body, so
/// the two ways of describing an invoice never mix.
const QUICK_PATH_FLAGS: [&str; 10] = [
    "amount",
    "to",
    "issuer",
    "bill_to",
    "heading",
    "reference",
    "expires_in",
    "expires_at",
    "chain_id",
    "token",
];

#[derive(Clone, Debug, Args)]
#[command(
    after_help = "Examples:\n  payday create --amount 25 --to 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 --issuer 'Acme Corp' --bill-to 'Globex' --reference 'INV-1234'\n  payday create --from-file invoice.json --attachment invoice.pdf"
)]
pub struct CreateArgs {
    /// Issue from the API's create body in a JSON file; unknown fields are rejected.
    #[arg(long, value_name = "FILE", conflicts_with_all = QUICK_PATH_FLAGS)]
    pub from_file: Option<PathBuf>,
    /// Amount in USDC, such as "25" or "100.50".
    #[arg(
        long,
        allow_hyphen_values = true,
        required_unless_present = "from_file"
    )]
    pub amount: Option<String>,
    /// Wallet that receives a successful payment.
    #[arg(long, alias = "payout", required_unless_present = "from_file")]
    pub to: Option<String>,
    /// Name of the party issuing the invoice.
    #[arg(long, value_parser = parse_party_name, required_unless_present = "from_file")]
    pub issuer: Option<String>,
    /// Name of the party the invoice is billed to.
    #[arg(long, value_parser = parse_party_name, required_unless_present = "from_file")]
    pub bill_to: Option<String>,
    /// Short description shown to the payer.
    #[arg(long)]
    pub heading: Option<String>,
    /// Time until expiry, such as 30m, 24h, or 7d; defaults to 24h.
    #[arg(long, conflicts_with = "expires_at")]
    pub expires_in: Option<String>,
    /// Exact expiry in RFC 3339 format.
    #[arg(long, conflicts_with = "expires_in")]
    pub expires_at: Option<String>,
    /// Merchant invoice number or external reference.
    #[arg(long, alias = "memo")]
    pub reference: Option<String>,
    /// PDF to attach (at most 5 MiB); uploaded and scanned before the invoice is issued.
    #[arg(long, value_name = "FILE")]
    pub attachment: Option<PathBuf>,
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
#[command(after_help = "Example:\n  payday get pay_0198f80c-8d2f-7dc1-a369-90556a64f700 --watch")]
pub struct GetArgs {
    /// Complete payment ID (`pay_…`) or payment address (`0x…`).
    pub reference: String,
    /// Refresh until the payment reaches a terminal state.
    #[arg(long)]
    pub watch: bool,
    /// Long-poll refresh window in seconds.
    #[arg(long, default_value_t = 2, requires = "watch", value_parser = clap::value_parser!(u64).range(1..=30))]
    pub interval: u64,
    /// Save the invoice PDF rendered by Payday to FILE.
    #[arg(long, value_name = "FILE", conflicts_with = "watch")]
    pub pdf: Option<PathBuf>,
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
    /// Complete payment ID (`pay_…`) or payment address (`0x…`).
    pub reference: String,
}

#[derive(Clone, Debug, Subcommand)]
pub enum CustomersCommand {
    /// Create a reusable counterparty record.
    Create(CreateCustomerArgs),
    /// Show one customer.
    Get { id: uuid::Uuid },
    /// List customers, newest first.
    List(ListCustomersArgs),
}

#[derive(Clone, Debug, Args)]
#[command(
    after_help = "Example:\n  payday customers create --name 'Globex' --email billing@globex.example"
)]
pub struct CreateCustomerArgs {
    /// Customer name; becomes the invoice's bill-to name when referenced.
    #[arg(long, value_parser = parse_party_name)]
    pub name: String,
    /// Billing contact email.
    #[arg(long)]
    pub email: Option<String>,
    /// Free-form details such as a postal address.
    #[arg(long)]
    pub details: Option<String>,
}

#[derive(Clone, Debug, Args)]
pub struct ListCustomersArgs {
    /// Number of customers to show (1–100).
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=100))]
    pub limit: u32,
    /// Continue after this customer ID from the previous page.
    #[arg(long)]
    pub starting_after: Option<uuid::Uuid>,
}

#[derive(Clone, Debug, Subcommand)]
pub enum ProofCommand {
    /// Download a settled invoice's Proof of Payment as JSON.
    Download(DownloadProofArgs),
    /// Verify a Proof of Payment offline, optionally against the chain.
    Verify(VerifyProofArgs),
}

#[derive(Clone, Debug, Args)]
#[command(
    after_help = "Example:\n  payday proof download pay_0198f80c-8d2f-7dc1-a369-90556a64f700 --output proof.json"
)]
pub struct DownloadProofArgs {
    /// Complete payment ID (`pay_…`) or payment address (`0x…`).
    pub reference: String,
    /// Where to write the proof JSON.
    #[arg(long, value_name = "FILE")]
    pub output: PathBuf,
}

#[derive(Clone, Debug, Args)]
#[command(
    after_help = "Example:\n  payday proof verify proof.json --attachment invoice.pdf --trusted-attestor 0x976EA74026E726554dB657fA54763abd0C3a0aa9"
)]
pub struct VerifyProofArgs {
    /// Proof JSON written by `payday proof download`.
    pub proof: PathBuf,
    /// The invoice's PDF attachment, to check against the committed hash.
    #[arg(long, value_name = "FILE")]
    pub attachment: Option<PathBuf>,
    /// Attestation signer to accept; repeat for several. Any signer is accepted when omitted.
    #[arg(long, value_name = "ADDRESS", value_parser = parse_address)]
    pub trusted_attestor: Vec<Address>,
    /// JSON-RPC endpoint for confirming the settlement receipts on chain.
    #[arg(long, value_name = "URL")]
    pub rpc_url: Option<String>,
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

#[derive(Clone, Debug, Subcommand)]
pub enum WebhooksCommand {
    /// Register an HTTPS endpoint. Its signing secret is shown only once.
    Add {
        #[arg(value_parser = parse_https_url)]
        url: String,
    },
    /// List registered webhook endpoints.
    List,
    /// Disable a webhook endpoint.
    Remove { id: uuid::Uuid },
    /// Queue a test event for a webhook endpoint.
    Test { id: uuid::Uuid },
    /// List recent webhook deliveries and attempts.
    Deliveries,
}

/// A party name is committed into the invoice verbatim; the API bounds it to
/// 1–255 bytes, so a blank or oversized name is caught before a round trip.
fn parse_party_name(value: &str) -> Result<String, String> {
    if value.trim().is_empty() || value.len() > 255 {
        return Err("must be 1 to 255 bytes and not blank".into());
    }
    Ok(value.to_string())
}

fn parse_address(value: &str) -> Result<Address, String> {
    value
        .parse()
        .map_err(|_| "must be a 0x-prefixed 20-byte hex address".to_string())
}

fn parse_https_url(value: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(value).map_err(|_| "must be a valid HTTPS URL".to_string())?;
    if url.scheme() != "https" || url.host_str().is_none() {
        return Err("must be a valid HTTPS URL".into());
    }
    Ok(url.to_string())
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
    use std::path::Path;

    use super::*;
    use clap::CommandFactory;

    #[test]
    fn public_surface_is_branded_grouped_and_ergonomic() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("Issue and track stablecoin invoices"));
        assert!(help.contains("Connection"));
        assert!(help.contains("Output"));
        assert!(help.contains("Examples:"));
        assert!(!help.contains("gateway-cli"));
        // Invoice is the product term for the document (product plan §7.4).
        assert!(help.contains("invoice"));

        let cli = Cli::try_parse_from([
            "payday",
            "create",
            "--amount",
            "25",
            "--to",
            "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
            "--issuer",
            "Acme Corp",
            "--bill-to",
            "Globex",
        ])
        .unwrap();
        let Command::Create(args) = cli.command else {
            panic!()
        };
        assert!(args.expires_in.is_none());
        assert_eq!(args.issuer.as_deref(), Some("Acme Corp"));
        assert_eq!(args.bill_to.as_deref(), Some("Globex"));
        assert!(args.reference.is_none());
        assert!(args.from_file.is_none());
        assert!(args.attachment.is_none());
    }

    #[test]
    fn from_file_replaces_the_quick_path_and_allows_an_attachment() {
        let cli = Cli::try_parse_from([
            "payday",
            "create",
            "--from-file",
            "invoice.json",
            "--attachment",
            "invoice.pdf",
            "--idempotency-key",
            "order-1042",
        ])
        .unwrap();
        let Command::Create(args) = cli.command else {
            panic!()
        };
        assert_eq!(args.from_file.as_deref(), Some(Path::new("invoice.json")));
        assert_eq!(args.attachment.as_deref(), Some(Path::new("invoice.pdf")));
        assert_eq!(args.idempotency_key.as_deref(), Some("order-1042"));
        assert!(args.amount.is_none() && args.issuer.is_none());

        // The file is the whole body: no quick-path flag may accompany it.
        for (flag, value) in [
            ("--amount", "25"),
            ("--to", "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"),
            ("--issuer", "Acme"),
            ("--bill-to", "Globex"),
            ("--heading", "March"),
            ("--reference", "INV-1"),
            ("--expires-in", "1h"),
            ("--expires-at", "2030-01-01T00:00:00Z"),
        ] {
            assert!(
                Cli::try_parse_from([
                    "payday",
                    "create",
                    "--from-file",
                    "invoice.json",
                    flag,
                    value
                ])
                .is_err(),
                "{flag} must conflict with --from-file"
            );
        }
        // And the quick path needs every one of its four flags.
        assert!(Cli::try_parse_from(["payday", "create", "--attachment", "invoice.pdf"]).is_err());
    }

    #[test]
    fn customers_and_proof_commands_parse_their_documented_flags() {
        let cli = Cli::try_parse_from([
            "payday",
            "customers",
            "create",
            "--name",
            "Globex",
            "--email",
            "billing@globex.example",
            "--details",
            "1 Globex Way",
        ])
        .unwrap();
        let Command::Customers(CustomersCommand::Create(args)) = cli.command else {
            panic!()
        };
        assert_eq!(args.name, "Globex");
        assert_eq!(args.email.as_deref(), Some("billing@globex.example"));
        assert!(Cli::try_parse_from(["payday", "customers", "create", "--name", " "]).is_err());
        assert!(Cli::try_parse_from(["payday", "customers", "get", "not-a-uuid"]).is_err());
        assert!(
            Cli::try_parse_from([
                "payday",
                "customers",
                "list",
                "--limit",
                "5",
                "--starting-after",
                "0198ec14-39df-7dd0-9994-92b510a89e85"
            ])
            .is_ok()
        );
        assert!(Cli::try_parse_from(["payday", "customers", "list", "--limit", "0"]).is_err());

        let cli = Cli::try_parse_from([
            "payday",
            "proof",
            "verify",
            "proof.json",
            "--attachment",
            "invoice.pdf",
            "--trusted-attestor",
            "0x976EA74026E726554dB657fA54763abd0C3a0aa9",
            "--trusted-attestor",
            "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
            "--rpc-url",
            "http://127.0.0.1:8545",
        ])
        .unwrap();
        let Command::Proof(ProofCommand::Verify(args)) = cli.command else {
            panic!()
        };
        assert_eq!(args.proof, Path::new("proof.json"));
        assert_eq!(args.trusted_attestor.len(), 2);
        assert_eq!(args.rpc_url.as_deref(), Some("http://127.0.0.1:8545"));
        assert!(
            Cli::try_parse_from([
                "payday",
                "proof",
                "verify",
                "proof.json",
                "--trusted-attestor",
                "0x1234"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from(["payday", "proof", "download", "pay_x", "--output", "p.json"])
                .is_ok()
        );
        assert!(
            Cli::try_parse_from(["payday", "proof", "download", "pay_x"]).is_err(),
            "--output is required"
        );
        assert!(
            Cli::try_parse_from(["payday", "get", "pay_x", "--pdf", "a.pdf", "--watch"]).is_err(),
            "--pdf and --watch conflict"
        );
    }

    #[test]
    fn create_requires_both_parties_and_keeps_memo_as_a_reference_alias() {
        let base = [
            "payday",
            "create",
            "--amount",
            "25",
            "--to",
            "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
        ];
        assert!(
            Cli::try_parse_from(base).is_err(),
            "issuer and bill-to are required"
        );
        for (flag, value) in [("--issuer", " "), ("--bill-to", &"x".repeat(256))] {
            let mut args = base.to_vec();
            args.extend(["--issuer", "Acme", "--bill-to", "Globex"]);
            args.extend([flag, value]);
            assert!(Cli::try_parse_from(args).is_err(), "{flag} {value:?}");
        }
        let mut args = base.to_vec();
        args.extend([
            "--issuer",
            "Acme",
            "--bill-to",
            "Globex",
            "--memo",
            "INV-1234",
            "--heading",
            "March retainer",
        ]);
        let cli = Cli::try_parse_from(args).unwrap();
        let Command::Create(args) = cli.command else {
            panic!()
        };
        assert_eq!(args.reference.as_deref(), Some("INV-1234"));
        assert_eq!(args.heading.as_deref(), Some("March retainer"));
        let help = Cli::command()
            .find_subcommand_mut("create")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(!help.contains("--memo"), "memo is a hidden alias");
    }

    #[test]
    fn merchants_can_no_longer_choose_the_recovery_wallet() {
        for flag in ["--refund-to", "--refund"] {
            assert!(
                Cli::try_parse_from([
                    "payday",
                    "create",
                    "--amount",
                    "1",
                    "--to",
                    "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
                    "--issuer",
                    "Acme",
                    "--bill-to",
                    "Globex",
                    flag,
                    "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC",
                ])
                .is_err(),
                "{flag} must be rejected"
            );
        }
    }

    #[test]
    fn sandbox_is_a_default_that_explicit_api_url_overrides() {
        let mut cli = Cli::try_parse_from(["payday", "--sandbox", "logout"]).unwrap();
        cli.resolve_connection();
        assert_eq!(cli.api_url, "https://api.sandbox.payday.sh");
        let mut cli = Cli::try_parse_from([
            "payday",
            "--sandbox",
            "--api-url",
            "https://example.test",
            "logout",
        ])
        .unwrap();
        cli.resolve_connection();
        assert_eq!(cli.api_url, "https://example.test");
    }

    #[test]
    fn local_profile_selects_the_local_api() {
        let mut cli =
            Cli::try_parse_from(["payday", "--profile", "local", "login", "--yes"]).unwrap();
        cli.resolve_connection();
        assert_eq!(cli.profile, Some(Profile::Local));
        assert_eq!(cli.api_url, "http://127.0.0.1:3000");
    }

    #[test]
    fn webhook_arguments_are_validated() {
        assert!(Cli::try_parse_from(["payday", "webhooks", "add", "http://example.test"]).is_err());
        assert!(
            Cli::try_parse_from(["payday", "webhooks", "add", "https://example.test/hook"]).is_ok()
        );
        assert!(Cli::try_parse_from(["payday", "webhooks", "test", "not-a-uuid"]).is_err());
    }

    #[test]
    fn watch_interval_matches_the_api_long_poll_window() {
        assert!(
            Cli::try_parse_from(["payday", "get", "pay_test", "--watch", "--interval", "1"])
                .is_ok()
        );
        assert!(
            Cli::try_parse_from(["payday", "get", "pay_test", "--watch", "--interval", "30"])
                .is_ok()
        );
        assert!(
            Cli::try_parse_from(["payday", "get", "pay_test", "--watch", "--interval", "0"])
                .is_err()
        );
        assert!(
            Cli::try_parse_from(["payday", "get", "pay_test", "--watch", "--interval", "31"])
                .is_err()
        );
    }
}
