mod account;
mod cli;
mod client;
mod error;

use std::io::{self, IsTerminal, Write};

use clap::Parser;
use gateway_core::{CreatePaymentRequest, PaymentResponse, PaymentStatus};
use uuid::Uuid;

use account::AccountClient;
use cli::{AccountCommand, Cli, Command, CreateArgs, GetArgs};
use client::GatewayClient;
use error::CliError;

struct Output {
    body: String,
    signed_in: Option<String>,
    next: Option<String>,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match cli.command.clone() {
        Command::Account(command) => run_account(&cli, command).await,
        command => run_payment(&cli, command).await,
    };

    match result {
        Ok(output) => print_output(output, cli.json, io::stdout().is_terminal()),
        Err(err) => {
            eprintln!("error: {err}");
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

async fn run_payment(cli: &Cli, command: Command) -> Result<Output, CliError> {
    let api_key = cli.api_key.as_deref().ok_or_else(|| {
        CliError::InvalidInput("API key is required; set PAYDAY_API_KEY or pass --api-key".into())
    })?;
    let client = GatewayClient::new(&cli.api_url, api_key)?;
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
        Command::Account(_) => unreachable!(),
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

async fn run_account(cli: &Cli, command: AccountCommand) -> Result<Output, CliError> {
    let issuer = required(cli.auth0_issuer.as_deref(), "PAYDAY_AUTH0_ISSUER")?;
    let client_id = required(cli.auth0_client_id.as_deref(), "PAYDAY_AUTH0_CLIENT_ID")?;
    let audience = required(cli.auth0_audience.as_deref(), "PAYDAY_AUTH0_AUDIENCE")?;
    let client = AccountClient::new(&cli.api_url, issuer, client_id, audience)?;
    let email = prompt_line("Email: ")?;
    client.send_otp(&email).await?;
    eprintln!("A one-time code was sent to {email}. Check your email.");
    let otp = prompt_line("One-time code: ")?;
    let token = client.authenticate(&email, &otp).await?;
    let (body, next) = match command {
        AccountCommand::Create(args) => {
            let expected_generation =
                if let Some(metadata) = client.metadata_optional(&token).await? {
                    eprintln!(
                        "WARNING: a new API key immediately invalidates {} (generation {}).",
                        metadata.hint, metadata.generation
                    );
                    if !args.yes && !confirm("Proceed? [y/N]: ")? {
                        return Ok(Output {
                            body: "API key replacement cancelled.".into(),
                            signed_in: Some(email),
                            next: None,
                        });
                    }
                    Some(metadata.generation)
                } else {
                    None
                };
            let issued = client.create(&token, expected_generation).await?;
            let body = if cli.json {
                serde_json::to_string_pretty(&issued).expect("API key response must serialize")
            } else {
                format!(
                    "API key {} (generation {}).\n\n{}\n\nStore it securely; it cannot be retrieved later.",
                    if issued.replaced_previous_key {
                        "replaced"
                    } else {
                        "created"
                    },
                    issued.generation,
                    issued.api_key
                )
            };
            (
                body,
                Some("Set PAYDAY_API_KEY to the API key shown above.".into()),
            )
        }
        AccountCommand::Get => {
            let metadata = client.metadata(&token).await?;
            let body = if cli.json {
                serde_json::to_string_pretty(&metadata).expect("account metadata must serialize")
            } else {
                format!(
                    "API key {} · generation {} · created {} · replaced {}",
                    metadata.hint,
                    metadata.generation,
                    metadata.created_at,
                    metadata.rotated_at.as_deref().unwrap_or("never")
                )
            };
            (
                body,
                Some("Run `payday create` to accept a payment.".into()),
            )
        }
    };
    Ok(Output {
        body,
        signed_in: Some(email),
        next,
    })
}

fn required<'a>(value: Option<&'a str>, name: &str) -> Result<&'a str, CliError> {
    value.ok_or_else(|| CliError::InvalidInput(format!("{name} is required for account commands")))
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
    use super::{payment_next, short};
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
}
