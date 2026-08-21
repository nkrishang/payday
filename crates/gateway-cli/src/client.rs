//! HTTP client for the gateway API.
//!
//! The CLI talks to the gateway strictly over HTTP — it never touches the
//! database. Response DTOs mirror the server's `InvoiceResponse` wire shape.

use serde::{Deserialize, Serialize};

use crate::error::{ApiErrorBody, CliError};

/// Wire representation of an invoice, matching gatewayd's `InvoiceResponse`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invoice {
    pub id: String,
    pub chain_id: String,
    pub factory_address: String,
    pub token: Token,
    pub beneficiary_address: String,
    pub amount: String,
    pub amount_base_units: String,
    pub salt: String,
    pub payment_address: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub address: String,
    pub kind: String,
    pub decimals: u8,
}

/// Parameters for `POST /v1/invoices`.
#[derive(Debug, Serialize)]
pub struct CreateInvoice {
    pub chain_id: String,
    pub token_address: String,
    pub beneficiary_address: String,
    pub amount: String,
}

/// Thin wrapper around a `reqwest::Client` bound to one gateway base URL.
pub struct GatewayClient {
    base_url: String,
    http: reqwest::Client,
}

impl GatewayClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        // Trim a trailing slash so `base + "/v1/..."` never doubles up.
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            base_url,
            http: reqwest::Client::new(),
        }
    }

    /// Create an invoice. `idempotency_key` is sent as the `Idempotency-Key`
    /// header, which the server requires.
    pub async fn create_invoice(
        &self,
        req: &CreateInvoice,
        idempotency_key: &str,
    ) -> Result<Invoice, CliError> {
        let url = format!("{}/v1/invoices", self.base_url);
        let resp = self
            .http
            .post(&url)
            .header("Idempotency-Key", idempotency_key)
            .json(req)
            .send()
            .await
            .map_err(|source| CliError::Transport {
                url: url.clone(),
                source,
            })?;

        parse_response(resp).await
    }

    /// Fetch an invoice by ID.
    pub async fn get_invoice(&self, id: &str) -> Result<Invoice, CliError> {
        let url = format!("{}/v1/invoices/{id}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|source| CliError::Transport {
                url: url.clone(),
                source,
            })?;

        parse_response(resp).await
    }
}

/// Turn a response into either a decoded body or a typed error, preserving the
/// server's stable error code when present.
async fn parse_response(resp: reqwest::Response) -> Result<Invoice, CliError> {
    let status = resp.status();
    let url = resp.url().to_string();
    let body = resp
        .text()
        .await
        .map_err(|source| CliError::Transport { url, source })?;

    if status.is_success() {
        return serde_json::from_str(&body).map_err(|e| CliError::UnexpectedResponse {
            status: status.as_u16(),
            body: format!("could not decode invoice: {e}; body was: {body}"),
        });
    }

    match serde_json::from_str::<ApiErrorBody>(&body) {
        Ok(parsed) => Err(CliError::Api {
            status: status.as_u16(),
            code: parsed.error.code,
            message: parsed.error.message,
        }),
        Err(_) => Err(CliError::UnexpectedResponse {
            status: status.as_u16(),
            body,
        }),
    }
}
