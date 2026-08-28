//! HTTP client for the gateway API.
//!
//! The CLI talks to the gateway strictly over its HTTP API — it never touches the
//! database. The request/response wire types come from `gateway-core`, the same
//! definitions the server uses, so the two cannot drift apart.

use std::net::IpAddr;
use std::time::Duration;

use gateway_core::{
    CancelPaymentResponse, CreatePaymentRequest, PaymentListResponse, PaymentResponse,
};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};

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

/// Thin wrapper around a `reqwest::Client` bound to one gateway base URL.
pub struct GatewayClient {
    base_url: String,
    http: reqwest::Client,
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

        Ok(Self { base_url, http })
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

    /// Hold the request until the payment changes or the server's 30-second
    /// long-poll window elapses. This keeps `--watch` responsive without a
    /// client-side two-second polling loop.
    pub async fn wait_for_payment_change(&self, id: &str) -> Result<PaymentResponse, CliError> {
        let url = format!("{}/v1/payments/{id}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .query(&[("wait_for", "change"), ("timeout", "30")])
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

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    async fn capture_server(
        responses: Vec<String>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut requests = Vec::with_capacity(responses.len());
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let read = stream.read(&mut buffer).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                requests.push(String::from_utf8(request).unwrap());
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (format!("http://{address}"), handle)
    }

    fn response(status: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn assert_bearer_header(request: &str) {
        let authorization = request.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("authorization")
                .then_some(value.trim())
        });
        let expected = format!("Bearer {KEY}");
        assert_eq!(authorization, Some(expected.as_str()));
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
            unauthorized,
        ])
        .await;
        let client = GatewayClient::new(base_url, KEY).unwrap();
        let create = CreatePaymentRequest {
            chain_id: Some("31337".into()),
            token_address: Some("0x0000000000000000000000000000000000000001".into()),
            payout_address: "0x0000000000000000000000000000000000000002".into(),
            amount: "1".into(),
            expires_in: Some(3_600),
            expires_at: None,
            refund_address: Some("0x0000000000000000000000000000000000000003".into()),
            memo: None,
            reference: None,
            metadata: serde_json::json!({}),
        };

        assert!(
            client
                .create_payment(&create, "test-request")
                .await
                .is_err()
        );
        assert!(client.get_payment("pay_test").await.is_err());
        assert!(
            client
                .list_payments(None, 7, Some("pay_0198f80c-8d2f-7dc1-a369-90556a64f700"),)
                .await
                .is_err()
        );

        let requests = requests.await.unwrap();
        assert!(requests[0].starts_with("POST /v1/payments HTTP/1.1"));
        assert!(requests[1].starts_with("GET /v1/payments/pay_test HTTP/1.1"));
        assert!(requests[2].starts_with(
            "GET /v1/payments?limit=7&starting_after=pay_0198f80c-8d2f-7dc1-a369-90556a64f700 HTTP/1.1"
        ));
        assert_bearer_header(&requests[0]);
        assert_bearer_header(&requests[1]);
        assert_bearer_header(&requests[2]);
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
