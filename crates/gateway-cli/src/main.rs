mod account;
mod cli;
mod client;
mod credentials;
mod error;

use std::io::{self, IsTerminal, Write};

use clap::Parser;
use gateway_core::{CreatePaymentRequest, PaymentResponse, PaymentStatus};
use uuid::Uuid;

use account::{AccountClient, ApiKeyMetadata};
use cli::{Cli, Command, CreateArgs, GetArgs, KeyActionArgs, KeysCommand, LoginArgs, RevokeArgs};
use client::GatewayClient;
use error::CliError;

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
    let cli = Cli::parse();
    let result = match cli.command.clone() {
        command @ (Command::Create(_) | Command::Get(_) | Command::List(_)) => {
            run_payment(&cli, command).await
        }
        Command::Login(args) => run_login(&cli, args).await.map(account_output),
        Command::Logout => run_logout(&cli).map(account_output),
        Command::Whoami => run_whoami(&cli).await.map(account_output),
        Command::Keys(command) => run_keys(&cli, command).await.map(account_output),
    };

    match result {
        Ok(output) => print_output(output, cli.json, io::stdout().is_terminal()),
        Err(err) => {
            eprintln!("error: {err}");
            if cli.verbose
                && let Some(detail) = err.diagnostic()
            {
                eprintln!("detail: {detail}");
            }
            std::process::exit(err.exit_code());
        }
    }
}

fn print_output(output: Output, json: bool, interactive: bool) {
    if !json && interactive {
        println!("Payday — stablecoin payments\n");
        if let Some(email) = output.signed_in {
            println!("Signed in as {email}.\n");
        }
    }
    println!("{}", output.body);
    if !json
        && interactive
        && let Some(next) = output.next
    {
        println!("\nNext: {next}");
    }
}

fn account_output(body: String) -> Output {
    Output {
        body,
        signed_in: None,
        next: None,
    }
}

async fn run_payment(cli: &Cli, command: Command) -> Result<Output, CliError> {
    let api_key = api_key(cli)?;
    let client = GatewayClient::new(&cli.api_url, &api_key)?;
    match command {
        Command::Create(args) => {
            let payment = create(&client, args).await?;
            Ok(Output {
                body: format_payment(&payment, cli.json),
                signed_in: None,
                next: Some(format!(
                    "Send {} {} to {}.",
                    payment.amount, payment.currency, payment.address
                )),
            })
        }
        Command::Get(args) => {
            let payment = get(&client, args).await?;
            let next = Some(payment_next(&payment));
            Ok(Output {
                body: format_payment(&payment, cli.json),
                signed_in: None,
                next,
            })
        }
        Command::List(args) => {
            let page = client
                .list_payments(args.limit, args.starting_after.as_deref())
                .await?;
            let body = if cli.json {
                serde_json::to_string_pretty(&page).expect("payments must serialize")
            } else if page.payments.is_empty() {
                "No payments yet.".into()
            } else {
                page.payments
                    .iter()
                    .map(format_payment_line)
                    .collect::<Vec<_>>()
                    .join("\n")
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
        Command::Login(_) | Command::Logout | Command::Whoami | Command::Keys(_) => unreachable!(),
    }
}

fn payment_next(payment: &PaymentResponse) -> String {
    match payment.status {
        PaymentStatus::AwaitingPayment => format!(
            "Send {} {} to {}.",
            payment.amount, payment.currency, payment.address
        ),
        PaymentStatus::PartiallyPaid => format!(
            "Send the remaining {} {} to {}.",
            payment.remaining, payment.currency, payment.address
        ),
        PaymentStatus::Paid => format!(
            "Run `payday get {}` again to refresh its status.",
            payment.id
        ),
        _ => "Run `payday create` to create another payment.".into(),
    }
}

async fn create(client: &GatewayClient, args: CreateArgs) -> Result<PaymentResponse, CliError> {
    require_nonblank("token", &args.token)?;
    require_nonblank("payout", &args.payout)?;
    require_nonblank("refund", &args.refund)?;
    require_nonblank("amount", &args.amount)?;
    let idempotency_key = args
        .idempotency_key
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let req = CreatePaymentRequest {
        chain_id: args.chain_id.to_string(),
        token_address: args.token,
        payout_address: args.payout,
        amount: args.amount,
        expires_in: args.expires_in,
        refund_address: args.refund,
    };
    eprintln!("using idempotency key: {idempotency_key}");
    client.create_payment(&req, &idempotency_key).await
}

async fn get(client: &GatewayClient, args: GetArgs) -> Result<PaymentResponse, CliError> {
    require_nonblank("id", &args.id)?;
    client.get_payment(&args.id).await
}

fn format_payment(payment: &PaymentResponse, json: bool) -> String {
    if json {
        return serde_json::to_string_pretty(payment).expect("payment must serialize");
    }
    let destination = short(&payment.payout_address);
    match payment.status {
        PaymentStatus::AwaitingPayment => format!(
            "Waiting for {} {} · expires {}\n{}\n{}",
            payment.amount, payment.currency, payment.expires_at, payment.id, payment.address
        ),
        PaymentStatus::PartiallyPaid => format!(
            "{} of {} {} received · {} remaining\n{}",
            payment.received, payment.amount, payment.currency, payment.remaining, payment.id
        ),
        PaymentStatus::Paid => format!(
            "Payment confirmed — settling to {destination}\n{}",
            payment.id
        ),
        PaymentStatus::Settled => format!(
            "{} {} sent to {destination} · tx {}\n{}",
            payment.received,
            payment.currency,
            payment
                .settlement_tx_hash
                .as_deref()
                .unwrap_or("external settlement"),
            payment.id
        ),
        PaymentStatus::Expired => format!(
            "Expired {} · received funds are being returned to {}\n{}",
            payment.expires_at,
            short(&payment.refund_address),
            payment.id
        ),
        PaymentStatus::Returned => format!(
            "{} {} returned to {} · tx {}\n{}",
            payment.received,
            payment.currency,
            short(&payment.refund_address),
            payment
                .settlement_tx_hash
                .as_deref()
                .unwrap_or("external settlement"),
            payment.id
        ),
        PaymentStatus::NeedsAttention => {
            let attention = payment.attention.as_ref();
            format!(
                "Payout paused: {}\n{}\n{}",
                attention
                    .map(|a| a.message.as_str())
                    .unwrap_or("Settlement needs attention."),
                attention
                    .map(|a| a.action.as_str())
                    .unwrap_or("Contact support."),
                payment.id
            )
        }
    }
}

fn format_payment_line(payment: &PaymentResponse) -> String {
    format!(
        "{}  {:<17}  {} {}",
        payment.id,
        payment.status.as_str(),
        payment.amount,
        payment.currency
    )
}

fn short(value: &str) -> String {
    if value.len() > 12 {
        format!("{}…{}", &value[..6], &value[value.len() - 4..])
    } else {
        value.into()
    }
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
    let (email, token) = authenticate(&client).await?;
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
    let display_key = if args.show {
        issued.api_key.clone()
    } else {
        key_hint(&issued.api_key)
    };
    Ok(format!(
        "  ✓ Signed in as {email}\n  ✓ API key saved to {} ({display_key})\n\n  Create your first payment:\n    payday create --help",
        path.display()
    ))
}

fn account_client(cli: &Cli) -> Result<AccountClient, CliError> {
    let production = credentials::normalized_api_url(&cli.api_url)?
        == credentials::normalized_api_url("https://api.payday.sh")?;
    let issuer = auth_setting(
        cli.auth0_issuer.as_deref(),
        if production {
            PRODUCTION_AUTH0_ISSUER
        } else {
            ""
        },
        "issuer",
    )?;
    let client_id = auth_setting(
        cli.auth0_client_id.as_deref(),
        if production {
            PRODUCTION_AUTH0_CLIENT_ID
        } else {
            ""
        },
        "client_id",
    )?;
    let audience = auth_setting(
        cli.auth0_audience.as_deref(),
        if production {
            PRODUCTION_AUTH0_AUDIENCE
        } else {
            ""
        },
        "audience",
    )?;
    AccountClient::new(&cli.api_url, issuer, client_id, audience)
}

async fn authenticate(client: &AccountClient) -> Result<(String, String), CliError> {
    let email = prompt_line("  Email  › ")?;
    validate_email(&email)?;
    client.send_otp(&email).await?;
    eprintln!(
        "  ✓ Code sent to {} · expires in 3 min · r to resend",
        mask_email(&email)
    );
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
    let (_, token) = authenticate(&client).await?;
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
                "  ✓ API key rotated (generation {})\n  ✓ Saved to {} ({key})\n  ! Previous key remains valid for 24 hours.",
                issued.generation,
                path.display()
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
        "Account {}\n  key          {}\n  generation   {}\n  created      {}\n  rotated      {}\n  previous key {}",
        metadata.account_id,
        metadata.key_hint.as_deref().unwrap_or("revoked"),
        metadata.generation,
        metadata.created_at,
        metadata.rotated_at.as_deref().unwrap_or("never"),
        metadata
            .previous_key_expires_at
            .as_deref()
            .map(|expiry| format!("valid until {expiry}"))
            .unwrap_or_else(|| "none".into())
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
    use super::{account_client, mask_email, payment_next, short, valid_code, validate_email};
    use crate::cli::{Cli, Command};
    use gateway_core::PaymentResponse;
    #[test]
    fn abbreviates_addresses_without_hiding_identity() {
        assert_eq!(
            short("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"),
            "0x7099…79C8"
        );
    }

    #[test]
    fn partially_paid_hint_requests_only_the_remaining_balance() {
        let payment: PaymentResponse = serde_json::from_value(serde_json::json!({
            "id": "pay_0198f80c-8d2f-7dc1-a369-90556a64f700",
            "address": "0x0000000000000000000000000000000000000001",
            "payout_address": "0x0000000000000000000000000000000000000002",
            "refund_address": "0x0000000000000000000000000000000000000003",
            "expires_at": "2030-03-17T17:46:40Z",
            "amount": "1.000000", "amount_base_units": "1000000",
            "received": "0.400000", "received_base_units": "400000",
            "remaining": "0.600000", "remaining_base_units": "600000",
            "currency": "USDC", "status": "partially_paid",
            "token": {"symbol": "USDC", "address": "0x0000000000000000000000000000000000000004", "decimals": 6},
            "chain": {"id": "143", "name": "Monad"},
            "settlement_tx_hash": null, "settled_at": null, "settled_block": null,
            "self_settlement": {"factory": "0x0000000000000000000000000000000000000005", "salt": "0x00"},
            "attention": null
        })).unwrap();
        let hint = payment_next(&payment);
        assert!(hint.contains("0.600000 USDC"));
        assert!(!hint.contains("1.000000 USDC"));
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
            api_url: "http://localhost:3000".into(),
            auth0_issuer: None,
            auth0_client_id: None,
            auth0_audience: None,
            json: false,
            verbose: false,
            command: Command::Logout,
        };
        assert!(account_client(&cli).is_err());

        cli.auth0_issuer = Some("http://localhost:3001".into());
        cli.auth0_client_id = Some("local-client".into());
        cli.auth0_audience = Some("local-api".into());
        assert!(account_client(&cli).is_ok());
    }
}
