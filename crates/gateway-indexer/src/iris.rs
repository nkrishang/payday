//! Circle's attestation service (Iris) for CCTP V2.
//!
//! After a burn, `GET /v2/messages/{sourceDomain}?transactionHash=…` answers
//! 404 until the transaction is indexed, then one entry per CCTP message in
//! it whose `status` is `pending_confirmations` until the source chain is
//! final (seconds on Monad, ~15–19 minutes on Base and Arbitrum) and
//! `complete` once the attestation can be submitted to `receiveMessage`.
//! No API key; 40 requests per second per IP, far above the one poll per
//! pending leg the relayer makes.

use alloy_primitives::{B256, Bytes, hex};
use async_trait::async_trait;
use serde::Deserialize;
use thiserror::Error;

pub const DEFAULT_IRIS_URL: &str = "https://iris-api.circle.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttestationStatus {
    /// Iris has not indexed the transaction yet.
    NotIndexed,
    /// Indexed; the source chain is not final yet.
    Pending,
    /// Ready to mint: every complete V2 message in the transaction, each the
    /// bytes `receiveMessage(message, attestation)` takes. One transaction
    /// can carry several CCTP messages, so the caller picks the one that
    /// describes its burn.
    Complete { messages: Vec<(Bytes, Bytes)> },
}

#[derive(Debug, Error)]
pub enum IrisError {
    #[error("attestation service request failed: {0}")]
    Transport(String),
    #[error("attestation service answered {status}: {body}")]
    Status { status: u16, body: String },
    #[error("attestation service answer was malformed: {0}")]
    Malformed(String),
}

/// The seam the relayer polls through, so it is testable without Circle.
#[async_trait]
pub trait AttestationSource: Send + Sync {
    async fn attestation(
        &self,
        source_domain: u32,
        burn_tx_hash: B256,
    ) -> Result<AttestationStatus, IrisError>;
}

pub struct IrisClient {
    base_url: String,
    http: reqwest::Client,
}

impl IrisClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self, IrisError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|error| IrisError::Transport(error.to_string()))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            http,
        })
    }
}

#[derive(Debug, Deserialize)]
struct MessagesResponse {
    messages: Vec<IrisMessage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IrisMessage {
    status: String,
    message: Option<String>,
    attestation: Option<String>,
    cctp_version: Option<u32>,
}

/// Interpret one Iris answer: every complete V2 message the transaction
/// carried, in the order the service listed them. A transaction through the
/// forwarder emits exactly one, but a helper transaction may emit several,
/// and which of them describes the caller's burn only the caller can say.
pub fn parse_messages(body: &str) -> Result<AttestationStatus, IrisError> {
    let parsed: MessagesResponse =
        serde_json::from_str(body).map_err(|error| IrisError::Malformed(error.to_string()))?;
    let v2: Vec<&IrisMessage> = parsed
        .messages
        .iter()
        .filter(|message| message.cctp_version.unwrap_or(2) == 2)
        .collect();
    if v2.is_empty() {
        return Ok(AttestationStatus::NotIndexed);
    }
    let decode = |field: &str, value: &Option<String>| -> Result<Bytes, IrisError> {
        let value = value
            .as_deref()
            .filter(|value| !value.is_empty() && *value != "PENDING" && *value != "0x")
            .ok_or_else(|| IrisError::Malformed(format!("complete message without {field}")))?;
        hex::decode(value)
            .map(Bytes::from)
            .map_err(|_| IrisError::Malformed(format!("{field} is not hex")))
    };
    let mut complete = Vec::new();
    for message in v2 {
        if message.status != "complete" {
            continue;
        }
        complete.push((
            decode("message", &message.message)?,
            decode("attestation", &message.attestation)?,
        ));
    }
    if complete.is_empty() {
        return Ok(AttestationStatus::Pending);
    }
    Ok(AttestationStatus::Complete { messages: complete })
}

#[async_trait]
impl AttestationSource for IrisClient {
    async fn attestation(
        &self,
        source_domain: u32,
        burn_tx_hash: B256,
    ) -> Result<AttestationStatus, IrisError> {
        let url = format!(
            "{}/v2/messages/{source_domain}?transactionHash={burn_tx_hash}",
            self.base_url
        );
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|error| IrisError::Transport(error.to_string()))?;
        let status = response.status();
        if status.as_u16() == 404 {
            return Ok(AttestationStatus::NotIndexed);
        }
        let body = response
            .text()
            .await
            .map_err(|error| IrisError::Transport(error.to_string()))?;
        if !status.is_success() {
            return Err(IrisError::Status {
                status: status.as_u16(),
                body: body.chars().take(200).collect(),
            });
        }
        parse_messages(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iris_answers_are_read_as_documented() {
        let pending = r#"{"messages":[{"attestation":"PENDING","message":"0x","eventNonce":"1","cctpVersion":2,"status":"pending_confirmations"}]}"#;
        assert_eq!(parse_messages(pending).unwrap(), AttestationStatus::Pending);
        let complete = r#"{"messages":[{"attestation":"0xabcd","message":"0x0102","eventNonce":"1","cctpVersion":2,"status":"complete","decodedMessage":{}}]}"#;
        assert_eq!(
            parse_messages(complete).unwrap(),
            AttestationStatus::Complete {
                messages: vec![(
                    Bytes::from_static(&[1, 2]),
                    Bytes::from_static(&[0xab, 0xcd])
                )],
            }
        );
        let two = r#"{"messages":[{"attestation":"0xabcd","message":"0x0102","eventNonce":"1","cctpVersion":2,"status":"complete"},{"attestation":"0xeeff","message":"0x0304","eventNonce":"2","cctpVersion":2,"status":"complete"}]}"#;
        assert_eq!(
            parse_messages(two).unwrap(),
            AttestationStatus::Complete {
                messages: vec![
                    (
                        Bytes::from_static(&[1, 2]),
                        Bytes::from_static(&[0xab, 0xcd])
                    ),
                    (
                        Bytes::from_static(&[3, 4]),
                        Bytes::from_static(&[0xee, 0xff])
                    ),
                ],
            }
        );
        assert_eq!(
            parse_messages(r#"{"messages":[]}"#).unwrap(),
            AttestationStatus::NotIndexed
        );
        let broken =
            r#"{"messages":[{"attestation":"PENDING","message":"0x","status":"complete"}]}"#;
        assert!(matches!(
            parse_messages(broken),
            Err(IrisError::Malformed(_))
        ));
    }
}
