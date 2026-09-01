mod account;
mod cli;
mod client;
mod credentials;
mod error;
mod presentation;

use std::io::{self, IsTerminal, Write};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{CommandFactory, Parser, error::ErrorKind};
use gateway_core::{
    Amount, BeneficiaryAddress, CreatePaymentRequest, PaymentAddress, PaymentResponse,
    PaymentStatus, RecoveryAddress, TokenAddress, USDC_DECIMALS, payment_id, resolve_expiration,
};
use uuid::Uuid;

use account::{AccountClient, ApiKeyMetadata};
use cli::{
    Cli, Command, CreateArgs, DocsTopic, GetArgs, KeyActionArgs, KeysCommand, LoginArgs,
    OpsCommand, Profile, RevokeArgs, WebhooksCommand,
};
use client::GatewayClient;
use error::CliError;
use presentation::{Presentation, terminal};

const PRODUCTION_AUTH0_ISSUER: &str = "https://dev-5ojfw164vnkjnk6m.us.auth0.com/";
const PRODUCTION_AUTH0_CLIENT_ID: &str = "vL8df7gnvLdQtNllctp8FWkZZGuYjzsq";
const PRODUCTION_AUTH0_AUDIENCE: &str = "https://api.payday.sh";

struct Output {
    body: String,
    signed_in: Option<String>,
    next: Option<String>,
}

#[tokio::main]
async fn main() {
    let mut cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            error.exit()
        }
        Err(error) if std::env::args().any(|arg| arg == "--json") => {
            eprintln!(
                "{}",
                serde_json::json!({"error":{"code":"invalid_input","message":error.to_string()}})
            );
            std::process::exit(2);
        }
        Err(error) => error.exit(),
    };
    cli.resolve_connection();
    let result = match cli.command.clone() {
        command
        @ (Command::Create(_) | Command::Get(_) | Command::List(_) | Command::Cancel(_)) => {
            run_payment(&cli, command).await
        }
        Command::Login(args) => run_login(&cli, args).await.map(account_output),
        Command::Logout => run_logout(&cli).map(account_output),
        Command::Whoami => run_whoami(&cli).await.map(account_output),
        Command::Keys(command) => run_keys(&cli, command).await.map(account_output),
        Command::Webhooks(command) => run_webhooks(&cli, command).await,
        Command::Ops(command) => run_ops(&cli, command).await,
        Command::Completions { shell } => completions(shell).map(account_output),
        Command::Docs { topic } => Ok(account_output(docs(topic).into())),
        Command::Upgrade => upgrade().await.map(account_output),
    };

    match result {
        Ok(output) => print_output(output, cli.json, io::stdout().is_terminal()),
        Err(err) => {
            if cli.json {
                eprintln!("{}", err.json());
            } else {
                eprintln!(
                    "{}",
                    Presentation::for_stderr(cli.color, cli.plain, cli.verbose)
                        .error(&err.to_string())
                );
            }
            if cli.verbose
                && let Some(detail) = err.diagnostic()
            {
                eprintln!("detail: {detail}");
            }
            std::process::exit(err.exit_code());
        }
    }
}

async fn run_ops(cli: &Cli, command: OpsCommand) -> Result<Output, CliError> {
    let secret = cli.admin_secret.as_deref().ok_or_else(|| {
        CliError::InvalidInput("operator credential is required; set PAYDAY_ADMIN_SECRET".into())
    })?;
    let client = GatewayClient::new(&cli.api_url, secret)?;
    let payment = match command {
        OpsCommand::Release { payment } => client.release_payment(&payment).await?,
    };
    let presentation = Presentation::new(cli.color, cli.plain, cli.verbose);
    Ok(Output {
        body: if cli.json {
            json(&payment)?
        } else {
            presentation.payment(&payment, false)
        },
        signed_in: None,
        next: None,
    })
}

async fn run_webhooks(cli: &Cli, command: WebhooksCommand) -> Result<Output, CliError> {
    let client = GatewayClient::new(&cli.api_url, &api_key(cli)?)?;
    let value = match command {
        WebhooksCommand::Add { url } => serde_json::to_value(client.add_webhook(&url).await?),
        WebhooksCommand::List => serde_json::to_value(client.list_webhooks().await?),
        WebhooksCommand::Remove { id } => Ok(client.remove_webhook(id).await?),
        WebhooksCommand::Test { id } => Ok(client.test_webhook(id).await?),
        WebhooksCommand::Deliveries => serde_json::to_value(client.webhook_deliveries().await?),
    }
    .map_err(|error| CliError::InvalidInput(format!("could not encode response: {error}")))?;
    let body = if cli.json {
        json(&value)?
    } else {
        webhook_text(&value)
    };
    Ok(Output {
        body,
        signed_in: None,
        next: None,
    })
}

fn webhook_text(value: &serde_json::Value) -> String {
    if let Some(items) = value.as_array() {
        if items.is_empty() {
            return "  No webhooks or deliveries found.".into();
        }
        return items
            .iter()
            .map(webhook_line)
            .collect::<Vec<_>>()
            .join("\n");
    }
    if let Some(secret) = value.get("secret").and_then(|v| v.as_str()) {
        return format!(
            "  ✓ Webhook added\n\n  URL     {}\n  ID      {}\n  Secret  {}\n\n  → Save this secret now; it will not be shown again.",
            value["url"].as_str().unwrap_or(""),
            value["id"].as_str().unwrap_or(""),
            secret,
        );
    }
    if let Some(id) = value.get("delivery_id").and_then(|v| v.as_str()) {
        return format!("  ✓ Test delivery {id} queued.");
    }
    if value.get("disabled").and_then(|v| v.as_bool()) == Some(true) {
        return format!(
            "  ✓ Webhook {} removed.",
            value["id"].as_str().unwrap_or("")
        );
    }
    webhook_line(value)
}

fn webhook_line(value: &serde_json::Value) -> String {
    match (
        value.get("url").and_then(|v| v.as_str()),
        value.get("state").and_then(|v| v.as_str()),
    ) {
        (Some(url), _) => format!("  {:<38}  {}", value["id"].as_str().unwrap_or("—"), url),
        (_, Some(state)) => format!(
            "  {:<38}  {:<12}  {} attempts",
            value["id"].as_str().unwrap_or("—"),
            state,
            value["attempt_count"].as_u64().unwrap_or(0),
        ),
        _ => value.to_string(),
    }
}

fn print_output(output: Output, json: bool, interactive: bool) {
    if !json && interactive {
        let brand = if std::env::var_os("NO_COLOR").is_none() {
            let cyan = anstyle::Style::new()
                .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Cyan)))
                .bold();
            let dim = anstyle::Style::new().fg_color(Some(anstyle::Color::Rgb(anstyle::RgbColor(
                0x44, 0x44, 0x44,
            ))));
            format!(
                "{}payday{}{} · stablecoin payments{}",
                cyan.render(),
                cyan.render_reset(),
                dim.render(),
                dim.render_reset(),
            )
        } else {
            "payday · stablecoin payments".to_string()
        };
        println!("{brand}\n");
        if let Some(email) = output.signed_in {
            println!("Signed in as {email}.\n");
        }
    }
    if !output.body.is_empty() {
        println!("{}", output.body);
    }
    if !json
        && interactive
        && let Some(next) = output.next
    {
        println!("\n  → {next}");
    }
}

fn account_output(body: String) -> Output {
    Output {
        body,
        signed_in: None,
        next: None,
    }
}

fn completions(shell: clap_complete::Shell) -> Result<String, CliError> {
    let mut output = Vec::new();
    clap_complete::generate(shell, &mut Cli::command(), "payday", &mut output);
    String::from_utf8(output)
        .map_err(|error| CliError::InvalidInput(format!("failed to generate completions: {error}")))
}

fn docs(topic: Option<DocsTopic>) -> &'static str {
    match topic {
        None => {
            "  Payday guides\n\n    getting-started  Create and inspect your first payment\n    authentication   Sign in and manage API keys\n    environment      Configure Payday\n\n  → Run `payday docs <topic>` to read a guide."
        }
        Some(DocsTopic::GettingStarted) => {
            "  Getting started\n\n    1. Run `payday login`\n    2. Create a payment: `payday create --amount 25 --to 0x…`\n    3. Follow it:    `payday get <PAYMENT_ID> --watch`\n\n  → Add --json for machine-readable output."
        }
        Some(DocsTopic::Authentication) => {
            "  Authentication\n\n    payday login        Sign in by email and save a profile\n    payday whoami       Show the current account\n    payday keys rotate  Replace your API key\n    payday keys revoke  Immediately invalidate keys\n    payday logout       Remove saved credentials\n\n  → PAYDAY_API_KEY overrides saved credentials for scripts."
        }
        Some(DocsTopic::Environment) => {
            "  Environment\n\n    PAYDAY_API_URL   Payday API endpoint\n    PAYDAY_API_KEY   Bearer API key override\n    NO_COLOR         Disable colored output\n\n  → Command-line flags override environment values."
        }
    }
}

async fn upgrade() -> Result<String, CliError> {
    #[cfg(windows)]
    return Err(CliError::InvalidInput(
        "Automatic upgrades are not supported on Windows; install the latest release archive"
            .into(),
    ));

    #[cfg(not(windows))]
    {
        use std::fs::{self, OpenOptions};
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let asset = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => "payday-linux-x86_64.tar.gz",
            ("macos", "x86_64") => "payday-macos-x86_64.tar.gz",
            ("macos", "aarch64") => "payday-macos-aarch64.tar.gz",
            (os, arch) => {
                return Err(CliError::InvalidInput(format!(
                    "Automatic upgrades are not available for {os}-{arch}"
                )));
            }
        };
        let base = "https://github.com/nkrishang/payday/releases/latest/download";
        let sums = download(&format!("{base}/SHA256SUMS"), 1024 * 1024).await?;
        let archive = download(&format!("{base}/{asset}"), 100 * 1024 * 1024).await?;
        let binary = verified_binary(asset, &sums, &archive)?;

        let executable = std::env::current_exe()
            .map_err(|error| CliError::InvalidInput(format!("Could not locate Payday: {error}")))?;
        let staged = executable.with_file_name(format!(".payday.new.{}", std::process::id()));
        let install = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)?;
            file.write_all(&binary)?;
            file.sync_all()?;
            fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
            fs::rename(&staged, &executable)
        })();
        if let Err(error) = install {
            let _ = fs::remove_file(&staged);
            return Err(CliError::InvalidInput(format!(
                "Could not install upgrade: {error}"
            )));
        }
        Ok("Payday is up to date.".into())
    }
}

#[cfg(not(windows))]
fn verified_binary(asset: &str, sums: &[u8], archive: &[u8]) -> Result<Vec<u8>, CliError> {
    use std::io::{Cursor, Read};
    use std::path::Path;

    use flate2::read::GzDecoder;
    use sha2::{Digest, Sha256};

    const MAX_BINARY_BYTES: u64 = 100 * 1024 * 1024;
    let expected = std::str::from_utf8(sums)
        .ok()
        .and_then(|sums| {
            sums.lines().find_map(|line| {
                let mut fields = line.split_whitespace();
                let hash = fields.next()?;
                (fields.next()? == asset).then_some(hash)
            })
        })
        .ok_or_else(|| CliError::InvalidInput(format!("Release checksum missing for {asset}")))?;
    let actual = format!("{:x}", Sha256::digest(archive));
    if actual != expected {
        return Err(CliError::InvalidInput(
            "Release checksum did not match".into(),
        ));
    }

    for entry in tar::Archive::new(GzDecoder::new(Cursor::new(archive)))
        .entries()
        .map_err(|error| CliError::InvalidInput(format!("Invalid release archive: {error}")))?
    {
        let entry = entry
            .map_err(|error| CliError::InvalidInput(format!("Invalid release archive: {error}")))?;
        if entry.path().is_ok_and(|path| path == Path::new("payday")) {
            let mut binary = Vec::new();
            entry
                .take(MAX_BINARY_BYTES + 1)
                .read_to_end(&mut binary)
                .map_err(|error| {
                    CliError::InvalidInput(format!("Invalid release binary: {error}"))
                })?;
            if binary.is_empty() || binary.len() as u64 > MAX_BINARY_BYTES {
                return Err(CliError::InvalidInput(
                    "Release binary has an invalid size".into(),
                ));
            }
            return Ok(binary);
        }
    }
    Err(CliError::InvalidInput(
        "Release archive did not contain payday".into(),
    ))
}

#[cfg(not(windows))]
async fn download(url: &str, max_bytes: u64) -> Result<Vec<u8>, CliError> {
    let response = reqwest::get(url)
        .await
        .map_err(|source| CliError::Transport {
            url: url.into(),
            source,
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(CliError::UnexpectedResponse {
            status: status.as_u16(),
            body: format!("could not download {url}"),
        });
    }
    if response
        .content_length()
        .is_some_and(|size| size > max_bytes)
    {
        return Err(CliError::InvalidInput(
            "Release download is unexpectedly large".into(),
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|source| CliError::Transport {
            url: url.into(),
            source,
        })?;
    if bytes.len() as u64 > max_bytes {
        return Err(CliError::InvalidInput(
            "Release download is unexpectedly large".into(),
        ));
    }
    Ok(bytes.to_vec())
}

async fn run_payment(cli: &Cli, command: Command) -> Result<Output, CliError> {
    let api_key = api_key(cli)?;
    let client = GatewayClient::new(&cli.api_url, &api_key)?;
    let presentation = Presentation::new(cli.color, cli.plain, cli.verbose);
    match command {
        Command::Create(args) => {
            let payment = create(&client, args).await?;
            Ok(Output {
                body: if cli.json {
                    json(&payment)?
                } else {
                    presentation.payment(&payment, true)
                },
                signed_in: None,
                next: None,
            })
        }
        Command::Get(args) if args.watch => Ok(Output {
            body: watch(&client, &presentation, args, !cli.plain).await?,
            signed_in: None,
            next: None,
        }),
        Command::Get(args) => {
            let payment = get(&client, &args.reference).await?;
            Ok(Output {
                body: if cli.json {
                    json(&payment)?
                } else {
                    presentation.payment(&payment, false)
                },
                signed_in: None,
                next: None,
            })
        }
        Command::List(args) => {
            if let Some(status) = &args.status {
                PaymentStatus::from_str(status).map_err(|_| {
                    CliError::InvalidInput(format!("Unknown payment status '{status}'"))
                })?;
            }
            let page = client
                .list_payments(
                    args.status.as_deref(),
                    args.limit,
                    args.starting_after.as_deref(),
                )
                .await?;
            let body = if cli.json {
                json(&page)?
            } else {
                presentation.list(&page.payments)
            };
            let next = page
                .next_cursor
                .map(|cursor| {
                    format!(
                        "Run `payday list --limit {} --starting-after {cursor}` for more payments.",
                        args.limit
                    )
                })
                .or_else(|| Some("Run `payday create` to accept a payment.".into()));
            Ok(Output {
                body,
                signed_in: None,
                next,
            })
        }
        Command::Cancel(args) => {
            let cancelled = client
                .cancel_payment(payment_reference(&args.reference)?)
                .await?;
            Ok(Output {
                body: if cli.json {
                    json(&cancelled)?
                } else {
                    format!(
                        "  ✓ Payment {} cancelled (advisory)\n\n  {}\n  → The address keeps its on-chain settlement terms.",
                        cancelled.payment.id, cancelled.advisory
                    )
                },
                signed_in: None,
                next: None,
            })
        }
        Command::Login(_)
        | Command::Logout
        | Command::Whoami
        | Command::Keys(_)
        | Command::Webhooks(_)
        | Command::Ops(_)
        | Command::Completions { .. }
        | Command::Docs { .. }
        | Command::Upgrade => unreachable!(),
    }
}

async fn create(client: &GatewayClient, args: CreateArgs) -> Result<PaymentResponse, CliError> {
    require_nonblank("amount", &args.amount)?;
    let amount = Amount::from_decimal_str(&args.amount, USDC_DECIMALS)
        .map_err(|error| CliError::InvalidInput(format!("Amount {error}")))?;
    if amount.0.is_zero() {
        return Err(CliError::InvalidInput(
            "Amount must be greater than zero".into(),
        ));
    }
    BeneficiaryAddress::from_str(&args.to)
        .map_err(|error| CliError::InvalidInput(format!("Payout address {error}")))?;
    if let Some(refund) = &args.refund_to {
        RecoveryAddress::from_str(refund)
            .map_err(|error| CliError::InvalidInput(format!("Refund address {error}")))?;
    }
    if let Some(token) = &args.token {
        TokenAddress::from_str(token)
            .map_err(|error| CliError::InvalidInput(format!("Token address {error}")))?;
    }
    let relative = args
        .expires_in
        .as_deref()
        .or_else(|| args.expires_at.is_none().then_some("24h"));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expiration = resolve_expiration(relative, args.expires_at.as_deref(), None, now)
        .map_err(|error| CliError::InvalidInput(format!("Expiry {error}")))?;
    let expires_in = expiration
        .intent
        .strip_prefix("in:")
        .map(str::parse)
        .transpose()
        .map_err(|_| CliError::InvalidInput("Expiry is too large".into()))?;
    let idempotency_key = args
        .idempotency_key
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let req = CreatePaymentRequest {
        chain_id: args.chain_id.map(|value| value.to_string()),
        token_address: args.token,
        payout_address: args.to.clone(),
        amount: args.amount,
        expires_in,
        expires_at: args.expires_at,
        refund_address: Some(args.refund_to.unwrap_or(args.to)),
        memo: args.memo,
        reference: None,
        metadata: serde_json::json!({}),
    };
    client.create_payment(&req, &idempotency_key).await
}

async fn get(client: &GatewayClient, reference: &str) -> Result<PaymentResponse, CliError> {
    client.get_payment(payment_reference(reference)?).await
}

/// A payment resolves only by its complete ID or its payment address, so reject
/// anything else here rather than spending a round trip to learn the same.
fn payment_reference(reference: &str) -> Result<&str, CliError> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Err(CliError::InvalidInput(
            "A payment ID or payment address is required\n→ Run `payday list` to see your payments."
                .into(),
        ));
    }
    if payment_id(reference).is_some() || PaymentAddress::from_str(reference).is_ok() {
        return Ok(reference);
    }
    Err(CliError::InvalidInput(format!(
        "'{}' is not a complete payment ID or payment address\n\
         → Payment ID:      pay_0198f80c-8d2f-7dc1-a369-90556a64f700\n\
         → Payment address: 0x70997970C51812dc3A010C7d01b50e0d17dc79C8\n\
         → Run `payday list` to see your payments.",
        elide(reference)
    )))
}

/// Keep a pasted-in reference from overrunning the error it appears in.
fn elide(value: &str) -> String {
    let mut short: String = value.chars().take(46).collect();
    if short.chars().count() < value.chars().count() {
        short.push('…');
    }
    short
}

async fn watch(
    client: &GatewayClient,
    output: &Presentation,
    args: GetArgs,
    allow_redraw: bool,
) -> Result<String, CliError> {
    if args.interval == 0 {
        return Err(CliError::InvalidInput(
            "Watch interval must be at least one second".into(),
        ));
    }
    let reference = payment_reference(&args.reference)?.to_owned();
    let interactive = allow_redraw && io::stdout().is_terminal();
    let mut previous = String::new();
    let mut first = true;
    loop {
        let payment = if first {
            first = false;
            get(client, &reference).await?
        } else {
            client
                .wait_for_payment_change(&reference, args.interval)
                .await?
        };
        let frame = output.payment(&payment, false);
        if interactive {
            print!("\x1b[2J\x1b[H{frame}");
            io::stdout()
                .flush()
                .map_err(|error| CliError::InvalidInput(error.to_string()))?;
        } else if frame != previous {
            if !previous.is_empty() {
                println!();
            }
            println!("{frame}");
            previous = frame;
        }
        if terminal(payment.status) {
            return Ok(String::new());
        }
    }
}

fn json(value: &impl serde::Serialize) -> Result<String, CliError> {
    serde_json::to_string_pretty(value)
        .map_err(|error| CliError::InvalidInput(format!("failed to serialize response: {error}")))
}

fn require_nonblank(field: &str, value: &str) -> Result<(), CliError> {
    if value.trim().is_empty() {
        Err(CliError::InvalidInput(format!(
            "--{field} must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn api_key(cli: &Cli) -> Result<String, CliError> {
    if let Some(key) = std::env::var("PAYDAY_API_KEY")
        .ok()
        .filter(|key| !key.is_empty())
    {
        return Ok(key);
    }
    credentials::load(
        &credentials::path()?,
        credentials::profile(&cli.api_url),
        &cli.api_url,
    )?
    .ok_or_else(|| {
        CliError::InvalidInput("not signed in; run `payday login` or set PAYDAY_API_KEY".into())
    })
}

fn auth_setting<'a>(
    value: Option<&'a str>,
    fallback: &'a str,
    name: &str,
) -> Result<&'a str, CliError> {
    value
        .filter(|v| !v.is_empty())
        .or((!fallback.is_empty()).then_some(fallback))
        .ok_or_else(|| {
            CliError::Config(format!(
                "Auth0 {name} is required for this API; set PAYDAY_AUTH0_{} (see docs/authentication.md)",
                name.to_ascii_uppercase()
            ))
        })
}

async fn run_login(cli: &Cli, args: LoginArgs) -> Result<String, CliError> {
    let client = account_client(cli)?;
    let (email, token) = authenticate(&client, cli.profile == Some(Profile::Local)).await?;
    let existing = client.metadata_optional(&token).await?;
    if let Some(metadata) = &existing {
        let key = metadata.key_hint.as_deref().unwrap_or("revoked key");
        eprintln!(
            "  ! Account already has {key} (generation {}).",
            metadata.generation
        );
        if !args.yes && !confirm("  Rotate it? [y/N] ")? {
            return Ok("  No changes made.".into());
        }
    }
    let path = credentials::path()?;
    credentials::prepare(&path)?;
    let issued = client
        .create(&token, existing.map(|metadata| metadata.generation))
        .await?;
    credentials::save(
        &path,
        credentials::profile(&cli.api_url),
        &cli.api_url,
        &issued.api_key,
    )?;
    if cli.json {
        return json(&issued);
    }
    let display_key = if args.show {
        issued.api_key.clone()
    } else {
        key_hint(&issued.api_key)
    };
    Ok(format!(
        "  ✓ Signed in as {email}\n  ✓ API key saved ({display_key})\n\n  → payday create --help"
    ))
}

fn account_client(cli: &Cli) -> Result<AccountClient, CliError> {
    let local = cli.profile == Some(Profile::Local);
    let production = credentials::normalized_api_url(&cli.api_url)?
        == credentials::normalized_api_url("https://api.payday.sh")?;
    let issuer = auth_setting(
        cli.auth0_issuer.as_deref(),
        if local {
            "http://127.0.0.1:3001"
        } else if production {
            PRODUCTION_AUTH0_ISSUER
        } else {
            ""
        },
        "issuer",
    )?;
    let client_id = auth_setting(
        cli.auth0_client_id.as_deref(),
        if local {
            "payday-cli-local"
        } else if production {
            PRODUCTION_AUTH0_CLIENT_ID
        } else {
            ""
        },
        "client_id",
    )?;
    let audience = auth_setting(
        cli.auth0_audience.as_deref(),
        if local {
            "payday-api-local"
        } else if production {
            PRODUCTION_AUTH0_AUDIENCE
        } else {
            ""
        },
        "audience",
    )?;
    AccountClient::new(&cli.api_url, issuer, client_id, audience)
}

async fn authenticate(client: &AccountClient, local: bool) -> Result<(String, String), CliError> {
    let email = prompt_line("  Email  › ")?;
    validate_email(&email)?;
    client.send_otp(&email).await?;
    if local {
        eprintln!(
            "  ✓ Code issued for {} · check the [identity] dev log · r to resend",
            mask_email(&email)
        );
    } else {
        eprintln!(
            "  ✓ Code sent to {} · expires in 3 min · r to resend",
            mask_email(&email)
        );
    }
    let token = 'otp: loop {
        let mut wrong = 0;
        loop {
            let code = prompt_secret("  Code   › ")?;
            if code.eq_ignore_ascii_case("r") {
                client.send_otp(&email).await?;
                eprintln!("  ✓ Fresh code sent · expires in 3 min");
                break;
            }
            if !valid_code(&code) {
                eprintln!("  ! Enter the 6-digit code, or r to resend.");
                continue;
            }
            match client.authenticate(&email, &code).await {
                Ok(token) => break 'otp token,
                Err(CliError::Auth { message, .. }) if message == "That code didn't match." => {
                    wrong += 1;
                    if wrong == 3 {
                        return Err(CliError::Auth {
                            message:
                                "That code didn't match. Run `payday login` to request a new code."
                                    .into(),
                            detail: None,
                        });
                    }
                    eprintln!("  ✗ That code didn't match. {} attempts left.", 3 - wrong);
                }
                Err(error) => return Err(error),
            }
        }
    };
    Ok((email, token))
}

fn run_logout(cli: &Cli) -> Result<String, CliError> {
    let removed = credentials::remove(&credentials::path()?, credentials::profile(&cli.api_url))?;
    Ok(if removed {
        "Signed out. Saved credentials removed."
    } else {
        "Already signed out."
    }
    .into())
}

async fn run_whoami(cli: &Cli) -> Result<String, CliError> {
    let key = api_key(cli)?;
    let metadata = GatewayClient::new(&cli.api_url, &key)?.account().await?;
    format_metadata(&metadata, cli.json)
}

async fn run_keys(cli: &Cli, command: KeysCommand) -> Result<String, CliError> {
    let client = account_client(cli)?;
    let (_, token) = authenticate(&client, cli.profile == Some(Profile::Local)).await?;
    let metadata = client.metadata(&token).await?;
    match command {
        KeysCommand::Rotate(KeyActionArgs { show, yes }) => {
            if !yes && !confirm("  Rotate the current key? [y/N] ")? {
                return Ok("  No changes made.".into());
            }
            let path = credentials::path()?;
            credentials::prepare(&path)?;
            let issued = client.create(&token, Some(metadata.generation)).await?;
            credentials::save(
                &path,
                credentials::profile(&cli.api_url),
                &cli.api_url,
                &issued.api_key,
            )?;
            let key = if show {
                issued.api_key.clone()
            } else {
                key_hint(&issued.api_key)
            };
            Ok(format!(
                "  ✓ API key rotated (generation {})\n  ✓ Saved ({key})\n  ! Previous key remains valid for 24 hours.",
                issued.generation,
            ))
        }
        KeysCommand::Revoke(RevokeArgs { yes }) => {
            if !yes && !confirm("  Revoke the current and previous key immediately? [y/N] ")? {
                return Ok("  No changes made.".into());
            }
            client.revoke(&token, metadata.generation).await?;
            credentials::remove(&credentials::path()?, credentials::profile(&cli.api_url))?;
            Ok("  ✓ API keys revoked. Saved credentials removed.".into())
        }
    }
}

fn format_metadata(metadata: &ApiKeyMetadata, json: bool) -> Result<String, CliError> {
    if json {
        return serde_json::to_string_pretty(metadata).map_err(|error| {
            CliError::InvalidInput(format!("failed to serialize response: {error}"))
        });
    }
    Ok(format!(
        "  Account    {}\n  Key        {}\n  Generation {}\n  Created    {}\n  Rotated    {}\n  Previous   {}",
        metadata.account_id,
        metadata.key_hint.as_deref().unwrap_or("revoked"),
        metadata.generation,
        metadata.created_at,
        metadata.rotated_at.as_deref().unwrap_or("never"),
        metadata
            .previous_key_expires_at
            .as_deref()
            .map(|expiry| format!("valid until {expiry}"))
            .unwrap_or_else(|| "none".into()),
    ))
}

fn key_hint(key: &str) -> String {
    let suffix = key.chars().rev().take(4).collect::<String>();
    format!(
        "{}…{}",
        &key[..key.len().min(12)],
        suffix.chars().rev().collect::<String>()
    )
}

fn prompt_line(prompt: &str) -> Result<String, CliError> {
    eprint!("{prompt}");
    io::stderr()
        .flush()
        .map_err(|e| CliError::InvalidInput(format!("failed to write prompt: {e}")))?;
    let mut value = String::new();
    let read = io::stdin()
        .read_line(&mut value)
        .map_err(|e| CliError::InvalidInput(format!("failed to read input: {e}")))?;
    let value = value.trim();
    if read == 0 || value.is_empty() {
        return Err(CliError::InvalidInput("input must not be empty".into()));
    }
    Ok(value.into())
}

fn prompt_secret(prompt: &str) -> Result<String, CliError> {
    if io::stdin().is_terminal() {
        rpassword::prompt_password(prompt)
            .map_err(|e| CliError::InvalidInput(format!("failed to read code: {e}")))
    } else {
        eprintln!("Input is not a terminal; reading the code from standard input.");
        prompt_line(prompt)
    }
}

fn validate_email(value: &str) -> Result<(), CliError> {
    let (local, domain) = value.split_once('@').unwrap_or(("", ""));
    if local.is_empty()
        || domain.contains('@')
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
        || value.chars().any(char::is_whitespace)
    {
        Err(CliError::InvalidInput("enter a valid email address".into()))
    } else {
        Ok(())
    }
}
fn mask_email(value: &str) -> String {
    let (local, domain) = value.split_once('@').unwrap_or((value, ""));
    let first = local.chars().next().unwrap_or('*');
    format!(
        "{first}{}@{domain}",
        "*".repeat(local.chars().count().saturating_sub(1))
    )
}
fn valid_code(value: &str) -> bool {
    value.len() == 6 && value.bytes().all(|b| b.is_ascii_digit())
}

fn confirm(prompt: &str) -> Result<bool, CliError> {
    eprint!("{prompt}");
    io::stderr()
        .flush()
        .map_err(|e| CliError::InvalidInput(format!("failed to write prompt: {e}")))?;
    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .map_err(|e| CliError::InvalidInput(format!("failed to read input: {e}")))?;
    Ok(matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[cfg(test)]
mod tests {
    use super::{account_client, mask_email, payment_reference, valid_code, validate_email};
    use crate::cli::{Cli, ColorChoice, Command, Profile};

    #[test]
    fn references_resolve_only_as_complete_ids_or_addresses() {
        let id = "pay_0198f80c-8d2f-7dc1-a369-90556a64f700";
        assert_eq!(payment_reference(id).unwrap(), id);
        assert_eq!(payment_reference(&format!(" {id}\n")).unwrap(), id);
        let address = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
        assert_eq!(payment_reference(address).unwrap(), address);

        for rejected in [
            "",
            "   ",
            "pay_0198f80c",
            "pay_",
            "0198f80c-8d2f-7dc1-a369-90556a64f700",
        ] {
            let error = payment_reference(rejected).unwrap_err();
            assert_eq!(error.exit_code(), 2, "{rejected}");
            assert!(
                error.to_string().contains("payday list"),
                "expected a next step for {rejected}"
            );
        }
    }

    #[test]
    fn login_input_helpers_are_strict() {
        assert!(validate_email("person@example.com").is_ok());
        assert!(validate_email("no-at-sign").is_err());
        assert!(validate_email("person@example.com@evil.test").is_err());
        assert_eq!(mask_email("person@example.com"), "p*****@example.com");
        assert!(valid_code("123456"));
        assert!(!valid_code("12345x"));
    }

    #[test]
    fn non_production_api_requires_explicit_identity_configuration() {
        let mut cli = Cli {
            profile: None,
            sandbox: false,
            api_url_override: None,
            api_url: "http://localhost:3000".into(),
            auth0_issuer: None,
            auth0_client_id: None,
            auth0_audience: None,
            admin_secret: None,
            json: false,
            verbose: false,
            plain: false,
            color: ColorChoice::Auto,
            command: Command::Logout,
        };
        assert!(account_client(&cli).is_err());

        cli.profile = Some(Profile::Local);
        assert!(account_client(&cli).is_ok());
    }

    #[cfg(not(windows))]
    #[test]
    fn upgrade_extracts_only_a_checksum_verified_binary() {
        use std::io::Write;

        use flate2::{Compression, write::GzEncoder};
        use sha2::{Digest, Sha256};

        let mut archive = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut tar = tar::Builder::new(&mut archive);
            let bytes = b"payday binary";
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, "payday", &bytes[..]).unwrap();
            tar.finish().unwrap();
        }
        archive.flush().unwrap();
        let archive = archive.finish().unwrap();
        let asset = "payday-linux-x86_64.tar.gz";
        let sums = format!("{:x}  {asset}\n", Sha256::digest(&archive));
        assert_eq!(
            super::verified_binary(asset, sums.as_bytes(), &archive).unwrap(),
            b"payday binary"
        );
        assert!(
            super::verified_binary(
                asset,
                format!("{}  {asset}\n", "0".repeat(64)).as_bytes(),
                &archive
            )
            .is_err()
        );
    }
}
