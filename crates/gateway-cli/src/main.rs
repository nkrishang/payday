mod account;
mod cli;
mod client;
mod error;

use std::io::{self, Write};

use clap::Parser;
use gateway_core::{CreateInvoiceRequest, InvoiceResponse};
use uuid::Uuid;

use account::AccountClient;
use cli::{AccountCommand, Cli, Command, CreateArgs, GetArgs, InvoiceCommand};
use client::GatewayClient;
use error::CliError;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match cli.command.clone() {
        Command::Invoice(command) => run_invoice(&cli, command).await,
        Command::Account(command) => run_account(&cli, command).await,
    };

    match result {
        Ok(output) => println!("{output}"),
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(err.exit_code());
        }
    }
}

async fn run_invoice(cli: &Cli, command: InvoiceCommand) -> Result<String, CliError> {
    let api_key = cli.api_key.as_deref().ok_or_else(|| {
        CliError::InvalidInput("API key is required; set GATEWAY_API_KEY or pass --api-key".into())
    })?;
    let client = GatewayClient::new(&cli.api_url, api_key)?;
    let invoice = match command {
        InvoiceCommand::Create(args) => create(&client, args).await?,
        InvoiceCommand::Get(args) => get(&client, args).await?,
    };
    Ok(format_invoice(&invoice, cli.json))
}

async fn run_account(cli: &Cli, command: AccountCommand) -> Result<String, CliError> {
    let issuer = cli.auth0_issuer.as_deref().ok_or_else(|| {
        CliError::InvalidInput("GATEWAY_AUTH0_ISSUER is required for account commands".into())
    })?;
    let client_id = cli.auth0_client_id.as_deref().ok_or_else(|| {
        CliError::InvalidInput("GATEWAY_AUTH0_CLIENT_ID is required for account commands".into())
    })?;
    let audience = cli.auth0_audience.as_deref().ok_or_else(|| {
        CliError::InvalidInput("GATEWAY_AUTH0_AUDIENCE is required for account commands".into())
    })?;
    let client = AccountClient::new(&cli.api_url, issuer, client_id, audience)?;
    let email = prompt_line("Email: ")?;
    client.send_otp(&email).await?;
    eprintln!("A one-time code was sent to {email}. Check your email.");
    let otp = prompt_line("One-time code: ")?;
    let token = client.authenticate(&email, &otp).await?;
    match command {
        AccountCommand::Create(args) => {
            let expected_generation = if let Some(metadata) = client.metadata_optional(&token).await? {
                eprintln!(
                    "WARNING: issuing a new API key immediately invalidates the existing key {} (generation {}).",
                    metadata.hint, metadata.generation
                );
                if !args.yes && !confirm("Proceed? [y/N]: ")? {
                    return Ok("API key replacement cancelled.".into());
                }
                Some(metadata.generation)
            } else {
                None
            };
            let issued = client.create(&token, expected_generation).await?;
            if cli.json {
                serde_json::to_string_pretty(&issued)
            } else if issued.replaced_previous_key {
                Ok(format!(
                    "API key replaced (generation {}).\nAPI key: {}\nAdvisory: your previous API key is now invalid. Store this key securely; it cannot be retrieved later.",
                    issued.generation, issued.api_key
                ))
            } else {
                Ok(format!(
                    "API key created (generation {}).\nAPI key: {}\nStore this key securely; it cannot be retrieved later.",
                    issued.generation, issued.api_key
                ))
            }
        }
        AccountCommand::Get => {
            let metadata = client.metadata(&token).await?;
            if cli.json {
                serde_json::to_string_pretty(&metadata)
            } else {
                Ok(format!(
                    "API key {}\n  generation  {}\n  created     {}\n  replaced    {}",
                    metadata.hint,
                    metadata.generation,
                    metadata.created_at,
                    metadata.rotated_at.as_deref().unwrap_or("never")
                ))
            }
        }
    }
    .map_err(|error| CliError::InvalidInput(format!("failed to serialize response: {error}")))
}

fn prompt_line(prompt: &str) -> Result<String, CliError> {
    eprint!("{prompt}");
    io::stderr()
        .flush()
        .map_err(|error| CliError::InvalidInput(format!("failed to write prompt: {error}")))?;
    let mut value = String::new();
    let read = io::stdin()
        .read_line(&mut value)
        .map_err(|error| CliError::InvalidInput(format!("failed to read input: {error}")))?;
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
        .map_err(|error| CliError::InvalidInput(format!("failed to write prompt: {error}")))?;
    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .map_err(|error| CliError::InvalidInput(format!("failed to read input: {error}")))?;
    Ok(is_confirmation(value.trim()))
}

fn is_confirmation(value: &str) -> bool {
    matches!(value.to_ascii_lowercase().as_str(), "y" | "yes")
}

async fn create(client: &GatewayClient, args: CreateArgs) -> Result<InvoiceResponse, CliError> {
    // A generated key gives each invocation at-most-once semantics on retries;
    // callers can pin their own to make a retry replay the same create.
    // Reject blank fields locally so scripts get a fast, distinct exit code
    // instead of a network round-trip and a server-side rejection.
    require_nonblank("token", &args.token)?;
    require_nonblank("beneficiary", &args.beneficiary)?;
    require_nonblank("recovery", &args.recovery)?;
    require_nonblank("amount", &args.amount)?;

    let idempotency_key = args
        .idempotency_key
        .unwrap_or_else(|| Uuid::now_v7().to_string());

    let req = CreateInvoiceRequest {
        chain_id: args.chain_id.to_string(),
        token_address: args.token,
        beneficiary_address: args.beneficiary,
        amount: args.amount,
        expiration_timestamp: args.expiration_timestamp.to_string(),
        recovery_address: args.recovery,
    };

    eprintln!("using idempotency key: {idempotency_key}");
    client.create_invoice(&req, &idempotency_key).await
}

async fn get(client: &GatewayClient, args: GetArgs) -> Result<InvoiceResponse, CliError> {
    require_nonblank("id", &args.id)?;
    client.get_invoice(&args.id).await
}

/// Reject an empty or whitespace-only argument value.
fn require_nonblank(field: &str, value: &str) -> Result<(), CliError> {
    if value.trim().is_empty() {
        Err(CliError::InvalidInput(format!(
            "--{field} must not be empty"
        )))
    } else {
        Ok(())
    }
}

/// Render an invoice as either JSON (script-friendly) or an aligned summary.
fn format_invoice(inv: &InvoiceResponse, as_json: bool) -> String {
    if as_json {
        return serde_json::to_string_pretty(inv).expect("invoice response must serialize");
    }
    format!(
        "Invoice {}\n  status           {}\n  chain id         {}\n  payment address  {}\n  amount           {} ({} base units)\n  token            {} [{}, {} decimals]\n  beneficiary      {}\n  expires at       {}\n  recovery         {}\n  factory          {}\n  salt             {}",
        inv.id,
        inv.status,
        inv.chain_id,
        inv.payment_address,
        inv.amount,
        inv.amount_base_units,
        inv.token.address,
        inv.token.kind,
        inv.token.decimals,
        inv.beneficiary_address,
        inv.expiration_timestamp,
        inv.recovery_address,
        inv.factory_address,
        inv.salt
    )
}

#[cfg(test)]
mod tests {
    use super::is_confirmation;

    #[test]
    fn replacement_confirmation_is_explicit_and_defaults_to_no() {
        assert!(is_confirmation("y"));
        assert!(is_confirmation("YES"));
        assert!(!is_confirmation(""));
        assert!(!is_confirmation("n"));
        assert!(!is_confirmation("anything else"));
    }
}
