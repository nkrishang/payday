//! HTTP client for the gateway API.
//!
//! The CLI talks to the gateway strictly over HTTP — it never touches the
//! database. The request/response wire types come from `gateway-core`, the same
//! definitions the server uses, so the two cannot drift apart.

use gateway_core::{CreateInvoiceRequest, InvoiceResponse};

use crate::error::{ApiErrorBody, CliError};

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
        req: &CreateInvoiceRequest,
        idempotency_key: &str,
    ) -> Result<InvoiceResponse, CliError> {
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
    pub async fn get_invoice(&self, id: &str) -> Result<InvoiceResponse, CliError> {
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
async fn parse_response(resp: reqwest::Response) -> Result<InvoiceResponse, CliError> {
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
