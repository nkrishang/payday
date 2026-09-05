//! HTTP client for the gateway API.
//!
//! The CLI talks to the gateway strictly over its HTTP API — it never touches the
//! database. The request/response wire types come from `gateway-core`, the same
//! definitions the server uses, so the two cannot drift apart.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::Duration;

use gateway_core::{
    AttachmentDescriptor, CancelPaymentResponse, CreatePaymentRequest, PDF_MIME_TYPE,
    PaymentListResponse, PaymentResponse, ProofOfPayment,
};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use uuid::Uuid;

use crate::account::ApiKeyMetadata;
use crate::error::{ApiErrorBody, CliError};

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct WebhookEndpoint {
    pub id: uuid::Uuid,
    pub url: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

/// A reusable counterparty record (`/v1/customers`).
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Customer {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub details: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CreateCustomerRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct CustomerListResponse {
    pub customers: Vec<Customer>,
    pub next_cursor: Option<String>,
}

/// A reserved upload slot: where to PUT the PDF and the headers the presigned
/// URL was signed with, which must be sent verbatim. The slot's `expires_at`
/// is not needed here: the PUT follows immediately.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AttachmentUploadSlot {
    pub id: Uuid,
    pub upload_url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

/// How long a single 5 MiB PUT to object storage may take on a slow link.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Thin wrapper around a `reqwest::Client` bound to one gateway base URL.
pub struct GatewayClient {
    base_url: String,
    http: reqwest::Client,
    /// Carries no credentials: presigned uploads go straight to object
    /// storage, which must never see the API key.
    anonymous: reqwest::Client,
}

impl GatewayClient {
    pub fn new(base_url: impl Into<String>, api_key: &str) -> Result<Self, CliError> {
        let base_url = base_url.into();
        let parsed_url = reqwest::Url::parse(&base_url)
            .map_err(|error| CliError::InvalidInput(format!("invalid API URL: {error}")))?;
        require_secure_transport(&parsed_url)?;

        // Trim a trailing slash so `base + "/v1/..."` never doubles up.
        let base_url = base_url.trim_end_matches('/').to_string();
        if api_key.len() < 32 {
            return Err(CliError::InvalidInput(
                "API key must be at least 32 bytes".into(),
            ));
        }
        if !api_key.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(CliError::InvalidInput(
                "API key must contain only visible ASCII characters".into(),
            ));
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|_| CliError::InvalidInput("API key contains invalid characters".into()))?;
        authorization.set_sensitive(true);

        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|source| CliError::Transport {
                url: base_url.clone(),
                source,
            })?;
        let anonymous = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(UPLOAD_TIMEOUT)
            .build()
            .map_err(|source| CliError::Transport {
                url: base_url.clone(),
                source,
            })?;

        Ok(Self {
            base_url,
            http,
            anonymous,
        })
    }

    /// Create a payment. `idempotency_key` is sent as the `Idempotency-Key`
    /// header, which the server requires.
    pub async fn create_payment(
        &self,
        req: &CreatePaymentRequest,
        idempotency_key: &str,
    ) -> Result<PaymentResponse, CliError> {
        let url = format!("{}/v1/payments", self.base_url);
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

    /// Fetch a payment by ID.
    pub async fn get_payment(&self, id: &str) -> Result<PaymentResponse, CliError> {
        let url = format!("{}/v1/payments/{id}", self.base_url);
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

    /// Hold the request until the payment changes or the requested long-poll
    /// window elapses. This keeps `--watch` responsive without blind polling.
    pub async fn wait_for_payment_change(
        &self,
        id: &str,
        timeout: u64,
    ) -> Result<PaymentResponse, CliError> {
        let url = format!("{}/v1/payments/{id}", self.base_url);
        let timeout = timeout.to_string();
        let resp = self
            .http
            .get(&url)
            .query(&[("wait_for", "change"), ("timeout", timeout.as_str())])
            .send()
            .await
            .map_err(|source| CliError::Transport {
                url: url.clone(),
                source,
            })?;
        parse_response(resp).await
    }

    pub async fn list_payments(
        &self,
        status: Option<&str>,
        limit: u32,
        starting_after: Option<&str>,
    ) -> Result<PaymentListResponse, CliError> {
        let url = format!("{}/v1/payments", self.base_url);
        let mut query = vec![("limit", limit.to_string())];
        if let Some(status) = status {
            query.push(("status", status.to_owned()));
        }
        if let Some(cursor) = starting_after {
            query.push(("starting_after", cursor.to_owned()));
        }
        let resp = self
            .http
            .get(&url)
            .query(&query)
            .send()
            .await
            .map_err(|source| CliError::Transport {
                url: url.clone(),
                source,
            })?;
        parse_typed_response(resp, "payments").await
    }

    pub async fn cancel_payment(&self, reference: &str) -> Result<CancelPaymentResponse, CliError> {
        let url = format!("{}/v1/payments/{reference}/cancel", self.base_url);
        let response = self
            .http
            .post(&url)
            .send()
            .await
            .map_err(|source| CliError::Transport {
                url: url.clone(),
                source,
            })?;
        parse_response(response).await
    }

    pub async fn release_payment(&self, id: &str) -> Result<PaymentResponse, CliError> {
        let url = format!("{}/v1/admin/payments/{id}/release", self.base_url);
        let response = self
            .http
            .post(&url)
            .send()
            .await
            .map_err(|source| CliError::Transport {
                url: url.clone(),
                source,
            })?;
        parse_response(response).await
    }

    /// The Payday-rendered invoice summary PDF.
    pub async fn invoice_pdf(&self, id: &str) -> Result<Vec<u8>, CliError> {
        let url = format!("{}/v1/payments/{id}/invoice.pdf", self.base_url);
        let response = self
            .http
            .get(&url)
            .header(ACCEPT, PDF_MIME_TYPE)
            .send()
            .await
            .map_err(|source| CliError::Transport {
                url: url.clone(),
                source,
            })?;
        let status = response.status();
        if status.is_success() {
            return response
                .bytes()
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(|source| CliError::Transport { url, source });
        }
        Err(parse_error_body(status, response).await)
    }

    /// The settled invoice's Proof of Payment; `409 payment_not_settled`
    /// before settlement.
    pub async fn proof(&self, id: &str) -> Result<ProofOfPayment, CliError> {
        self.request_json(
            self.http
                .get(format!("{}/v1/payments/{id}/proof", self.base_url)),
        )
        .await
    }

    /// Reserve a presigned upload slot for one PDF.
    pub async fn create_attachment(
        &self,
        filename: &str,
    ) -> Result<AttachmentUploadSlot, CliError> {
        self.request_json(
            self.http
                .post(format!("{}/v1/attachments", self.base_url))
                .json(&serde_json::json!({ "filename": filename })),
        )
        .await
    }

    /// PUT the bytes to object storage with exactly the headers the URL was
    /// signed for. Object storage speaks its own error dialect, so a failure
    /// here is reported by status alone.
    pub async fn upload_attachment(
        &self,
        slot: &AttachmentUploadSlot,
        bytes: Vec<u8>,
    ) -> Result<(), CliError> {
        let mut headers = HeaderMap::new();
        for (name, value) in &slot.headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                CliError::UnexpectedResponse {
                    status: 0,
                    body: format!("upload slot carried an invalid header name {name:?}"),
                }
            })?;
            let value = HeaderValue::from_str(value).map_err(|_| CliError::UnexpectedResponse {
                status: 0,
                body: format!("upload slot carried an invalid value for header {name}"),
            })?;
            headers.insert(name, value);
        }
        let response = self
            .anonymous
            .put(&slot.upload_url)
            .headers(headers)
            .body(bytes)
            .send()
            .await
            .map_err(|source| CliError::Transport {
                url: slot.upload_url.clone(),
                source,
            })?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        Err(CliError::UnexpectedResponse {
            status: status.as_u16(),
            body: "object storage refused the attachment upload".into(),
        })
    }

    /// Ask the API to admit the uploaded bytes; `409 attachment_scan_pending`
    /// while the malware scan has not reported.
    pub async fn finalize_attachment(&self, id: Uuid) -> Result<AttachmentDescriptor, CliError> {
        self.request_json(
            self.http
                .post(format!("{}/v1/attachments/{id}/finalize", self.base_url)),
        )
        .await
    }

    pub async fn create_customer(&self, req: &CreateCustomerRequest) -> Result<Customer, CliError> {
        self.request_json(
            self.http
                .post(format!("{}/v1/customers", self.base_url))
                .json(req),
        )
        .await
    }

    pub async fn get_customer(&self, id: Uuid) -> Result<Customer, CliError> {
        self.request_json(
            self.http
                .get(format!("{}/v1/customers/{id}", self.base_url)),
        )
        .await
    }

    pub async fn list_customers(
        &self,
        limit: u32,
        starting_after: Option<Uuid>,
    ) -> Result<CustomerListResponse, CliError> {
        let mut query = vec![("limit", limit.to_string())];
        if let Some(cursor) = starting_after {
            query.push(("starting_after", cursor.to_string()));
        }
        self.request_json(
            self.http
                .get(format!("{}/v1/customers", self.base_url))
                .query(&query),
        )
        .await
    }

    pub async fn account(&self) -> Result<ApiKeyMetadata, CliError> {
        let url = format!("{}/v1/account", self.base_url);
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|source| CliError::Transport {
                url: url.clone(),
                source,
            })?;
        parse_response(response).await
    }

    pub async fn add_webhook(&self, endpoint: &str) -> Result<WebhookEndpoint, CliError> {
        self.request_json(
            self.http
                .post(format!("{}/v1/webhooks", self.base_url))
                .json(&serde_json::json!({"url": endpoint})),
        )
        .await
    }

    pub async fn list_webhooks(&self) -> Result<Vec<WebhookEndpoint>, CliError> {
        self.request_json(self.http.get(format!("{}/v1/webhooks", self.base_url)))
            .await
    }

    pub async fn remove_webhook(&self, id: uuid::Uuid) -> Result<serde_json::Value, CliError> {
        self.request_json(
            self.http
                .delete(format!("{}/v1/webhooks/{id}", self.base_url)),
        )
        .await
    }

    pub async fn test_webhook(&self, id: uuid::Uuid) -> Result<serde_json::Value, CliError> {
        self.request_json(
            self.http
                .post(format!("{}/v1/webhooks/{id}/test", self.base_url)),
        )
        .await
    }

    pub async fn webhook_deliveries(&self) -> Result<Vec<serde_json::Value>, CliError> {
        self.request_json(
            self.http
                .get(format!("{}/v1/webhook-deliveries", self.base_url)),
        )
        .await
    }

    async fn request_json<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, CliError> {
        let url = request
            .try_clone()
            .and_then(|r| r.build().ok())
            .map(|r| r.url().to_string())
            .unwrap_or_else(|| self.base_url.clone());
        let response = request
            .send()
            .await
            .map_err(|source| CliError::Transport { url, source })?;
        parse_response(response).await
    }
}

pub(crate) fn require_secure_transport(url: &reqwest::Url) -> Result<(), CliError> {
    if url.scheme() == "https" || (url.scheme() == "http" && is_loopback(url)) {
        return Ok(());
    }

    Err(CliError::InvalidInput(
        "API URL must use HTTPS; HTTP is allowed only for localhost and loopback addresses".into(),
    ))
}

fn is_loopback(url: &reqwest::Url) -> bool {
    url.host_str().is_some_and(|host| {
        let ip_literal = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host);
        host.eq_ignore_ascii_case("localhost")
            || ip_literal
                .parse::<IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    })
}

/// Turn a response into either a decoded body or a typed error, preserving the
/// server's stable error code when present.
async fn parse_typed_response<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
    noun: &str,
) -> Result<T, CliError> {
    parse_response_with_noun(resp, noun).await
}

async fn parse_response<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T, CliError> {
    parse_response_with_noun(resp, "response").await
}

async fn parse_response_with_noun<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
    noun: &str,
) -> Result<T, CliError> {
    let status = resp.status();
    let url = resp.url().to_string();
    let body = resp
        .text()
        .await
        .map_err(|source| CliError::Transport { url, source })?;

    if status.is_success() {
        return serde_json::from_str(&body).map_err(|e| CliError::UnexpectedResponse {
            status: status.as_u16(),
            body: format!("could not decode {noun}: {e}; body was: {body}"),
        });
    }

    Err(api_error(status, body))
}

/// Read a failed response's body and turn it into the typed error.
async fn parse_error_body(status: reqwest::StatusCode, response: reqwest::Response) -> CliError {
    let url = response.url().to_string();
    match response.text().await {
        Ok(body) => api_error(status, body),
        Err(source) => CliError::Transport { url, source },
    }
}

fn api_error(status: reqwest::StatusCode, body: String) -> CliError {
    match serde_json::from_str::<ApiErrorBody>(&body) {
        Ok(parsed) => CliError::Api {
            status: status.as_u16(),
            code: parsed.error.code,
            message: parsed.error.message,
        },
        Err(_) => CliError::UnexpectedResponse {
            status: status.as_u16(),
            body,
        },
    }
}

/// A one-connection-per-response HTTP stub shared by the client and proof
/// tests: it records each raw request and answers with the next canned reply.
#[cfg(test)]
pub(crate) mod test_support {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    pub(crate) async fn capture_server(
        responses: Vec<String>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut requests = Vec::with_capacity(responses.len());
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 4096];
                loop {
                    let read = stream.read(&mut buffer).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        // Keep reading until the declared body has arrived so
                        // the stub never answers a request it has not fully seen.
                        let head = String::from_utf8_lossy(&request[..end]).to_string();
                        let length = head
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                requests.push(String::from_utf8_lossy(&request).to_string());
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (format!("http://{address}"), handle)
    }

    pub(crate) fn response(status: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    pub(crate) fn header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
        request.lines().find_map(|line| {
            let (candidate, value) = line.split_once(':')?;
            candidate.eq_ignore_ascii_case(name).then_some(value.trim())
        })
    }
}

#[cfg(test)]
mod tests {
    use gateway_core::{Party, PayerPolicy};

    use super::test_support::{capture_server, header, response};
    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    fn assert_bearer_header(request: &str) {
        let expected = format!("Bearer {KEY}");
        assert_eq!(header(request, "authorization"), Some(expected.as_str()));
    }

    #[test]
    fn rejects_api_keys_that_cannot_be_header_values() {
        let invalid = "0123456789abcdef0123456789abc\nde";
        assert!(GatewayClient::new("https://gateway.example", invalid).is_err());
    }

    #[test]
    fn rejects_whitespace_that_the_server_would_reject() {
        for invalid in [format!("{KEY} "), format!("{KEY}\t")] {
            assert!(GatewayClient::new("https://gateway.example", &invalid).is_err());
        }
    }

    #[test]
    fn rejects_short_api_keys() {
        assert!(GatewayClient::new("https://gateway.example", "too-short").is_err());
    }

    #[test]
    fn requires_https_except_for_loopback_development() {
        assert!(GatewayClient::new("https://gateway.example", KEY).is_ok());
        assert!(GatewayClient::new("http://localhost:3000", KEY).is_ok());
        assert!(GatewayClient::new("http://127.0.0.1:3000", KEY).is_ok());
        assert!(GatewayClient::new("http://[::1]:3000", KEY).is_ok());
        assert!(GatewayClient::new("http://gateway.example", KEY).is_err());
    }

    #[tokio::test]
    async fn sends_the_bearer_key_on_create_and_get() {
        let unauthorized = response(
            "401 Unauthorized",
            "Content-Type: application/json\r\n",
            "{}",
        );
        let (base_url, requests) = capture_server(vec![
            unauthorized.clone(),
            unauthorized.clone(),
            unauthorized.clone(),
            unauthorized,
        ])
        .await;
        let client = GatewayClient::new(base_url, KEY).unwrap();
        let party = |name: &str| Party {
            name: name.into(),
            email: None,
            details: None,
        };
        let create = CreatePaymentRequest {
            chain_id: Some("31337".into()),
            token_address: Some("0x0000000000000000000000000000000000000001".into()),
            payout_address: "0x0000000000000000000000000000000000000002".into(),
            amount: "1".into(),
            issuer: party("Acme"),
            bill_to: party("Globex"),
            customer_id: None,
            issuer_id: None,
            notes: None,
            heading: None,
            reference: None,
            metadata: serde_json::json!({}),
            payer_policy: PayerPolicy::Permissionless,
            attachment_id: None,
            expires_in: Some(3_600),
            expires_at: None,
        };

        assert!(
            client
                .create_payment(&create, "test-request")
                .await
                .is_err()
        );
        assert!(client.get_payment("pay_test").await.is_err());
        assert!(client.wait_for_payment_change("pay_test", 7).await.is_err());
        assert!(
            client
                .list_payments(None, 7, Some("pay_0198f80c-8d2f-7dc1-a369-90556a64f700"),)
                .await
                .is_err()
        );

        let requests = requests.await.unwrap();
        assert!(requests[0].starts_with("POST /v1/payments HTTP/1.1"));
        assert!(requests[1].starts_with("GET /v1/payments/pay_test HTTP/1.1"));
        assert!(
            requests[2].starts_with("GET /v1/payments/pay_test?wait_for=change&timeout=7 HTTP/1.1")
        );
        assert!(requests[3].starts_with(
            "GET /v1/payments?limit=7&starting_after=pay_0198f80c-8d2f-7dc1-a369-90556a64f700 HTTP/1.1"
        ));
        assert_bearer_header(&requests[0]);
        assert_bearer_header(&requests[1]);
        assert_bearer_header(&requests[2]);
        assert_bearer_header(&requests[3]);
    }

    #[tokio::test]
    async fn whoami_decodes_account_metadata_with_rotation_grace() {
        let body = serde_json::json!({
            "account_id": "0198ec14-39df-7dd0-9994-92b510a89e85",
            "key_hint": "…abcdef",
            "generation": 2,
            "created_at": "2026-08-27T12:00:00Z",
            "rotated_at": "2026-08-27T12:00:00Z",
            "previous_key_expires_at": "2026-08-28T12:00:00Z",
            "revoked_at": null
        })
        .to_string();
        let ok = response("200 OK", "Content-Type: application/json\r\n", &body);
        let (base_url, requests) = capture_server(vec![ok]).await;
        let metadata = GatewayClient::new(base_url, KEY)
            .unwrap()
            .account()
            .await
            .unwrap();

        assert_eq!(metadata.generation, 2);
        assert_eq!(metadata.key_hint.as_deref(), Some("…abcdef"));
        assert!(metadata.previous_key_expires_at.is_some());
        let requests = requests.await.unwrap();
        assert!(requests[0].starts_with("GET /v1/account HTTP/1.1"));
        assert_bearer_header(&requests[0]);
    }

    #[tokio::test]
    async fn does_not_follow_redirects_with_the_bearer_key() {
        let redirect = response("302 Found", "Location: /redirected\r\n", "");
        let (base_url, requests) = capture_server(vec![redirect]).await;
        let client = GatewayClient::new(base_url, KEY).unwrap();

        let error = client.get_payment("pay_test").await.unwrap_err();
        assert!(matches!(
            error,
            CliError::UnexpectedResponse { status: 302, .. }
        ));
        assert_eq!(requests.await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn customer_client_uses_documented_routes_and_shapes() {
        let customer = serde_json::json!({
            "id": "0198ec14-39df-7dd0-9994-92b510a89e85",
            "name": "Globex",
            "email": null,
            "details": null,
            "created_at": "2026-08-27T12:00:00Z",
            "updated_at": "2026-08-27T12:00:00Z"
        });
        let created = response(
            "201 Created",
            "Content-Type: application/json\r\n",
            &customer.to_string(),
        );
        let page = response(
            "200 OK",
            "Content-Type: application/json\r\n",
            &serde_json::json!({ "customers": [customer], "next_cursor": null }).to_string(),
        );
        let (base_url, requests) = capture_server(vec![created.clone(), page, created]).await;
        let client = GatewayClient::new(base_url, KEY).unwrap();
        let id: Uuid = "0198ec14-39df-7dd0-9994-92b510a89e85".parse().unwrap();

        let customer = client
            .create_customer(&CreateCustomerRequest {
                name: "Globex".into(),
                email: None,
                details: None,
            })
            .await
            .unwrap();
        assert_eq!(customer.id, id);
        assert_eq!(customer.email, None);
        let listed = client.list_customers(7, Some(id)).await.unwrap();
        assert_eq!(listed.customers.len(), 1);
        assert!(listed.next_cursor.is_none());
        client.get_customer(id).await.unwrap();

        let requests = requests.await.unwrap();
        assert!(requests[0].starts_with("POST /v1/customers HTTP/1.1"));
        // Absent optional fields are omitted rather than sent as null.
        assert!(
            requests[0].ends_with(r#"{"name":"Globex"}"#),
            "{}",
            requests[0]
        );
        assert!(requests[1].starts_with(&format!(
            "GET /v1/customers?limit=7&starting_after={id} HTTP/1.1"
        )));
        assert!(requests[2].starts_with(&format!("GET /v1/customers/{id} HTTP/1.1")));
        requests
            .iter()
            .for_each(|request| assert_bearer_header(request));
    }

    #[tokio::test]
    async fn attachment_upload_sends_signed_headers_to_storage_without_the_api_key() {
        let id: Uuid = "0198ec14-39df-7dd0-9994-92b510a89e86".parse().unwrap();
        let descriptor = serde_json::json!({
            "id": id,
            "filename": "invoice.pdf",
            "mime_type": "application/pdf",
            "byte_length": "9",
            "sha256": format!("0x{}", "ab".repeat(32)),
        });
        // Object storage is a separate stub so the PUT's destination and
        // headers can be inspected apart from the API's.
        let (storage_url, storage) = capture_server(vec![response("200 OK", "", "")]).await;
        let slot = serde_json::json!({
            "id": id,
            "upload_url": format!("{storage_url}/payday-attachments-local/uploads/acct/{id}.pdf?X-Amz-Signature=abc"),
            "headers": {
                "content-type": "application/pdf",
                "x-amz-tagging": "payday-upload=pending"
            },
            "expires_at": "2026-08-27T12:15:00Z"
        });
        let (api_url, api) = capture_server(vec![
            response(
                "201 Created",
                "Content-Type: application/json\r\n",
                &slot.to_string(),
            ),
            response(
                "409 Conflict",
                "Content-Type: application/json\r\n",
                r#"{"error":{"code":"attachment_scan_pending","message":"scan pending"}}"#,
            ),
            response(
                "200 OK",
                "Content-Type: application/json\r\n",
                &descriptor.to_string(),
            ),
        ])
        .await;
        let client = GatewayClient::new(api_url, KEY).unwrap();

        let slot = client.create_attachment("invoice.pdf").await.unwrap();
        assert_eq!(slot.id, id);
        client
            .upload_attachment(&slot, b"%PDF-1.7\n".to_vec())
            .await
            .unwrap();
        let pending = client.finalize_attachment(id).await.unwrap_err();
        assert!(
            matches!(pending, CliError::Api { ref code, status: 409, .. } if code == "attachment_scan_pending")
        );
        let finalized = client.finalize_attachment(id).await.unwrap();
        assert_eq!(finalized.byte_length, "9");

        let api = api.await.unwrap();
        assert!(api[0].starts_with("POST /v1/attachments HTTP/1.1"));
        assert!(api[0].ends_with(r#"{"filename":"invoice.pdf"}"#));
        assert!(api[1].starts_with(&format!("POST /v1/attachments/{id}/finalize HTTP/1.1")));
        assert!(api[2].starts_with(&format!("POST /v1/attachments/{id}/finalize HTTP/1.1")));
        api.iter().for_each(|request| assert_bearer_header(request));

        let storage = storage.await.unwrap();
        assert!(storage[0].starts_with(&format!(
            "PUT /payday-attachments-local/uploads/acct/{id}.pdf?X-Amz-Signature=abc HTTP/1.1"
        )));
        assert_eq!(header(&storage[0], "content-type"), Some("application/pdf"));
        assert_eq!(
            header(&storage[0], "x-amz-tagging"),
            Some("payday-upload=pending")
        );
        assert_eq!(header(&storage[0], "content-length"), Some("9"));
        assert!(
            header(&storage[0], "authorization").is_none(),
            "the API key must never reach object storage"
        );
        assert!(storage[0].ends_with("%PDF-1.7\n"));
    }

    #[tokio::test]
    async fn proof_and_pdf_use_documented_routes() {
        let pdf = response("200 OK", "Content-Type: application/pdf\r\n", "%PDF-1.7 x");
        let not_settled = response(
            "409 Conflict",
            "Content-Type: application/json\r\n",
            r#"{"error":{"code":"payment_not_settled","message":"not yet"}}"#,
        );
        let (base_url, requests) =
            capture_server(vec![pdf, not_settled.clone(), not_settled]).await;
        let client = GatewayClient::new(base_url, KEY).unwrap();

        assert_eq!(client.invoice_pdf("pay_test").await.unwrap(), b"%PDF-1.7 x");
        let error = client.proof("pay_test").await.unwrap_err();
        assert!(
            matches!(error, CliError::Api { ref code, status: 409, .. } if code == "payment_not_settled")
        );
        // A failed PDF read still surfaces the API's stable error code.
        let error = client.invoice_pdf("pay_test").await.unwrap_err();
        assert!(matches!(error, CliError::Api { ref code, .. } if code == "payment_not_settled"));

        let requests = requests.await.unwrap();
        assert!(requests[0].starts_with("GET /v1/payments/pay_test/invoice.pdf HTTP/1.1"));
        assert_eq!(header(&requests[0], "accept"), Some("application/pdf"));
        assert!(requests[1].starts_with("GET /v1/payments/pay_test/proof HTTP/1.1"));
        requests
            .iter()
            .for_each(|request| assert_bearer_header(request));
    }

    #[tokio::test]
    async fn webhook_client_uses_documented_routes_and_bearer_auth() {
        let ok = response("200 OK", "Content-Type: application/json\r\n", "[]");
        let (base_url, requests) = capture_server(vec![ok.clone(), ok]).await;
        let client = GatewayClient::new(base_url, KEY).unwrap();
        client.list_webhooks().await.unwrap();
        client.webhook_deliveries().await.unwrap();
        let requests = requests.await.unwrap();
        assert!(requests[0].starts_with("GET /v1/webhooks HTTP/1.1"));
        assert!(requests[1].starts_with("GET /v1/webhook-deliveries HTTP/1.1"));
        requests
            .iter()
            .for_each(|request| assert_bearer_header(request));
    }
}
