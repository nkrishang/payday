mod cli;
mod client;
mod error;

use clap::Parser;
use gateway_core::{CreateInvoiceRequest, InvoiceResponse};
use uuid::Uuid;

use cli::{Cli, Command, CreateArgs, GetArgs, InvoiceCommand};
use client::GatewayClient;
use error::CliError;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let api_key = match cli.api_key.as_deref() {
        Some(api_key) => api_key,
        None => {
            let err = CliError::InvalidInput(
                "API key is required; set GATEWAY_API_KEY or pass --api-key".into(),
            );
            eprintln!("error: {err}");
            std::process::exit(err.exit_code());
        }
    };
    let client = match GatewayClient::new(&cli.api_url, api_key) {
        Ok(client) => client,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(err.exit_code());
        }
    };

    let result = match cli.command {
        Command::Invoice(InvoiceCommand::Create(args)) => create(&client, args).await,
        Command::Invoice(InvoiceCommand::Get(args)) => get(&client, args).await,
    };

    match result {
        Ok(invoice) => print_invoice(&invoice, cli.json),
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(err.exit_code());
        }
    }
}

async fn create(client: &GatewayClient, args: CreateArgs) -> Result<InvoiceResponse, CliError> {
    // A generated key gives each invocation at-most-once semantics on retries;
    // callers can pin their own to make a retry replay the same create.
    // Reject blank fields locally so scripts get a fast, distinct exit code
    // instead of a network round-trip and a server-side rejection.
    require_nonblank("token", &args.token)?;
    require_nonblank("beneficiary", &args.beneficiary)?;
    require_nonblank("amount", &args.amount)?;

    let idempotency_key = args
        .idempotency_key
        .unwrap_or_else(|| Uuid::now_v7().to_string());

    let req = CreateInvoiceRequest {
        chain_id: args.chain_id.to_string(),
        token_address: args.token,
        beneficiary_address: args.beneficiary,
        amount: args.amount,
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
fn print_invoice(inv: &InvoiceResponse, as_json: bool) {
    if as_json {
        // Re-serialize through serde_json for stable, pretty output.
        match serde_json::to_string_pretty(inv) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("error: failed to serialize invoice: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    println!("Invoice {}", inv.id);
    println!("  status           {}", inv.status);
    println!("  chain id         {}", inv.chain_id);
    println!("  payment address  {}", inv.payment_address);
    println!(
        "  amount           {} ({} base units)",
        inv.amount, inv.amount_base_units
    );
    println!(
        "  token            {} [{}, {} decimals]",
        inv.token.address, inv.token.kind, inv.token.decimals
    );
    println!("  beneficiary      {}", inv.beneficiary_address);
    println!("  factory          {}", inv.factory_address);
    println!("  salt             {}", inv.salt);
}
