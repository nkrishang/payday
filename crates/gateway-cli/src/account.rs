use std::time::Duration;

use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::client::require_secure_transport;
use crate::error::{ApiErrorBody, CliError};

pub struct AccountClient {
    api_url: String,
    issuer: String,
    client_id: String,
    audience: String,
    http: reqwest::Client,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct IssuedApiKey {
    pub api_key: String,
    pub generation: i64,
    pub replaced_previous_key: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ApiKeyMetadata {
    pub account_id: String,
    pub key_hint: Option<String>,
    pub generation: i64,
    pub created_at: String,
    pub rotated_at: Option<String>,
    pub previous_key_expires_at: Option<String>,
    pub revoked_at: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

impl AccountClient {
    pub fn new(
        api_url: impl Into<String>,
        issuer: &str,
        client_id: &str,
        audience: &str,
    ) -> Result<Self, CliError> {
        let api_url = api_url.into();
        require_secure_transport(
            &Url::parse(&api_url)
                .map_err(|error| CliError::InvalidInput(format!("invalid API URL: {error}")))?,
        )?;
        let issuer_url = Url::parse(issuer)
            .map_err(|error| CliError::InvalidInput(format!("invalid Auth0 issuer: {error}")))?;
        require_secure_transport(&issuer_url)?;
        for (name, value) in [("Auth0 client ID", client_id), ("Auth0 audience", audience)] {
            if value.trim().is_empty() {
                return Err(CliError::InvalidInput(format!("{name} must not be empty")));
            }
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|source| CliError::Transport {
                url: issuer.into(),
                source,
            })?;
        Ok(Self {
            api_url: api_url.trim_end_matches('/').into(),
            issuer: issuer.trim_end_matches('/').into(),
            client_id: client_id.into(),
            audience: audience.into(),
            http,
        })
    }

    pub async fn send_otp(&self, email: &str) -> Result<(), CliError> {
        let url = format!("{}/passwordless/start", self.issuer);
        let response = self
            .http
            .post(&url)
            .json(&serde_json::json!({
                "client_id": self.client_id,
                "connection": "email",
                "email": email,
                "send": "code"
            }))
            .send()
            .await
            .map_err(|error| CliError::Auth {
                message: "Couldn't reach the sign-in service.".into(),
                detail: Some(error.to_string()),
            })?;
        decode_auth0_empty(response).await
    }

    pub async fn authenticate(&self, email: &str, otp: &str) -> Result<String, CliError> {
        let token_url = format!("{}/oauth/token", self.issuer);
        let response = self
            .http
            .post(&token_url)
            .json(&serde_json::json!({
                "grant_type": "http://auth0.com/oauth/grant-type/passwordless/otp",
                "client_id": self.client_id,
                "username": email,
                "otp": otp,
                "realm": "email",
                "audience": self.audience,
                "scope": "openid"
            }))
            .send()
            .await
            .map_err(|error| CliError::Auth {
                message: "Couldn't reach the sign-in service.".into(),
                detail: Some(error.to_string()),
            })?;
        decode_auth0::<TokenResponse>(response)
            .await
            .map(|value| value.access_token)
    }

    pub async fn create(
        &self,
        token: &str,
        expected_generation: Option<i64>,
    ) -> Result<IssuedApiKey, CliError> {
        self.request(
            reqwest::Method::POST,
            "/v1/account/api-key",
            token,
            Some(serde_json::json!({ "expected_generation": expected_generation })),
        )
        .await
    }

    pub async fn metadata(&self, token: &str) -> Result<ApiKeyMetadata, CliError> {
        self.request(reqwest::Method::GET, "/v1/account/api-key", token, None)
            .await
    }

    pub async fn metadata_optional(&self, token: &str) -> Result<Option<ApiKeyMetadata>, CliError> {
        match self.metadata(token).await {
            Ok(metadata) => Ok(Some(metadata)),
            Err(CliError::Api {
                status: 404, code, ..
            }) if code == "account_not_provisioned" => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub async fn revoke(&self, token: &str, expected_generation: i64) -> Result<(), CliError> {
        let url = format!("{}/v1/account/api-key", self.api_url);
        let response = self
            .http
            .delete(&url)
            .bearer_auth(token)
            .json(&serde_json::json!({ "expected_generation": expected_generation }))
            .send()
            .await
            .map_err(|source| CliError::Transport {
                url: url.clone(),
                source,
            })?;
        if response.status().is_success() {
            Ok(())
        } else {
            decode_success::<serde_json::Value>(response)
                .await
                .map(|_| ())
        }
    }

    async fn request<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, CliError> {
        let url = format!("{}{}", self.api_url, path);
        let mut request = self.http.request(method, &url).bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|source| CliError::Transport {
            url: url.clone(),
            source,
        })?;
        decode_success(response).await
    }
}

#[derive(Deserialize)]
struct Auth0Error {
    error: Option<String>,
}

async fn decode_auth0_empty(response: reqwest::Response) -> Result<(), CliError> {
    if response.status().is_success() {
        Ok(())
    } else {
        decode_auth0::<serde_json::Value>(response)
            .await
            .map(|_| ())
    }
}

async fn decode_auth0<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, CliError> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|source| CliError::Transport {
            url: "Auth0 response".into(),
            source,
        })?;
    if status.is_success() {
        return serde_json::from_str(&body).map_err(|error| CliError::Auth {
            message: "The sign-in service returned an invalid response.".into(),
            detail: Some(format!("{error}; body: {body}")),
        });
    }
    let parsed = serde_json::from_str::<Auth0Error>(&body).ok();
    let code = parsed
        .as_ref()
        .and_then(|e| e.error.as_deref())
        .unwrap_or("");
    if status.as_u16() == 429 || code == "too_many_attempts" {
        Err(CliError::Auth {
            message: "Too many tries. Wait a minute and run `payday login` again.".into(),
            detail: Some(body),
        })
    } else if code == "invalid_grant" {
        Err(CliError::Auth {
            message: "That code didn't match.".into(),
            detail: Some(body),
        })
    } else {
        Err(CliError::Auth {
            message: "The sign-in service rejected the request.".into(),
            detail: Some(format!("HTTP {status}; {body}")),
        })
    }
}

async fn decode_success<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, CliError> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|source| CliError::Transport {
            url: "response body".into(),
            source,
        })?;
    if status.is_success() {
        return serde_json::from_str(&body).map_err(|error| CliError::UnexpectedResponse {
            status: status.as_u16(),
            body: format!("invalid JSON response: {error}"),
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

    async fn capture_server(
        responses: Vec<String>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                loop {
                    let read = stream.read(&mut buffer).await.unwrap();
                    request.extend_from_slice(&buffer[..read]);
                    if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]);
                        let content_length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + content_length {
                            break;
                        }
                    }
                }
                requests.push(String::from_utf8(request).unwrap());
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (format!("http://{address}"), handle)
    }

    fn response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn auth0_errors_are_safe_and_distinct() {
        let bad = "{\"error\":\"invalid_grant\",\"error_description\":\"secret raw detail\"}";
        let limited = "{\"error\":\"too_many_attempts\"}";
        let (url, _) = capture_server(vec![
            format!("HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{bad}", bad.len()),
            format!("HTTP/1.1 429 Too Many Requests\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{limited}", limited.len()),
        ]).await;
        let client = AccountClient::new(&url, &url, "client", "audience").unwrap();
        let first = client
            .authenticate("a@b.co", "000000")
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(first, "That code didn't match.");
        assert!(!first.contains("secret raw detail"));
        assert!(
            client
                .authenticate("a@b.co", "000000")
                .await
                .unwrap_err()
                .to_string()
                .contains("Too many")
        );
    }

    #[tokio::test]
    async fn email_otp_authenticates_and_sends_identity_token_to_gateway() {
        let token = serde_json::json!({ "access_token": "identity-jwt" }).to_string();
        let issued = serde_json::json!({
            "api_key": "payday_live_abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG",
            "generation": 1,
            "replaced_previous_key": false
        })
        .to_string();
        let (base_url, requests) =
            capture_server(vec![response("{}"), response(&token), response(&issued)]).await;
        let client =
            AccountClient::new(&base_url, &base_url, "public-client", "payday-api").unwrap();
        client.send_otp("user@example.com").await.unwrap();
        let identity_token = client
            .authenticate("user@example.com", "123456")
            .await
            .unwrap();
        let key = client.create(&identity_token, None).await.unwrap();
        assert_eq!(key.generation, 1);
        assert!(!key.replaced_previous_key);

        let requests = requests.await.unwrap();
        assert!(requests[0].starts_with("POST /passwordless/start HTTP/1.1"));
        assert!(requests[0].contains(r#""client_id":"public-client""#));
        assert!(requests[0].contains(r#""email":"user@example.com""#));
        assert!(requests[0].contains(r#""connection":"email""#));
        assert!(requests[0].contains(r#""send":"code""#));
        assert!(requests[1].starts_with("POST /oauth/token HTTP/1.1"));
        assert!(
            requests[1]
                .contains(r#""grant_type":"http://auth0.com/oauth/grant-type/passwordless/otp""#)
        );
        assert!(requests[1].contains(r#""otp":"123456""#));
        assert!(requests[1].contains(r#""realm":"email""#));
        assert!(requests[1].contains(r#""audience":"payday-api""#));
        assert!(requests[1].contains(r#""scope":"openid""#));
        assert!(requests[2].starts_with("POST /v1/account/api-key HTTP/1.1"));
        assert!(requests[2].contains(r#""expected_generation":null"#));
        assert!(
            requests[2]
                .to_ascii_lowercase()
                .contains("authorization: bearer identity-jwt")
        );
    }

    #[tokio::test]
    async fn replacement_sends_the_generation_that_was_confirmed() {
        let issued = serde_json::json!({
            "api_key": "replacement-key",
            "generation": 8,
            "replaced_previous_key": true
        })
        .to_string();
        let (base_url, requests) = capture_server(vec![response(&issued)]).await;
        let client =
            AccountClient::new(&base_url, &base_url, "public-client", "payday-api").unwrap();

        let result = client.create("identity-jwt", Some(7)).await.unwrap();
        assert_eq!(result.generation, 8);
        assert!(result.replaced_previous_key);
        let requests = requests.await.unwrap();
        assert!(requests[0].contains(r#""expected_generation":7"#));
    }

    #[tokio::test]
    async fn revocation_accepts_an_empty_no_content_response() {
        let (base_url, requests) = capture_server(vec![
            "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".into(),
        ])
        .await;
        let client =
            AccountClient::new(&base_url, &base_url, "public-client", "payday-api").unwrap();

        client.revoke("identity-jwt", 7).await.unwrap();
        let requests = requests.await.unwrap();
        assert!(requests[0].starts_with("DELETE /v1/account/api-key HTTP/1.1"));
        assert!(requests[0].contains(r#""expected_generation":7"#));
    }

    #[test]
    fn account_flow_requires_secure_transport() {
        assert!(
            AccountClient::new(
                "https://api.payday.sh",
                "http://login.example",
                "client",
                "audience"
            )
            .is_err()
        );
    }
}
