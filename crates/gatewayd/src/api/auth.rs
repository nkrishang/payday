use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::Response;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::api::error::ApiError;

const MIN_API_KEY_LEN: usize = 32;

/// A one-way representation of the configured bearer credential.
///
/// Keeping only the digest in application state prevents accidental exposure
/// of the credential through future logging or debugging of shared state.
#[derive(Clone)]
pub struct ApiKey([u8; 32]);

impl ApiKey {
    pub fn new(value: &str) -> Result<Self, &'static str> {
        if value.len() < MIN_API_KEY_LEN {
            return Err("GATEWAY_API_KEY must be at least 32 bytes");
        }
        if !value.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err("GATEWAY_API_KEY must contain only visible ASCII characters");
        }

        Ok(Self(Sha256::digest(value.as_bytes()).into()))
    }

    fn matches(&self, candidate: &str) -> bool {
        let candidate: [u8; 32] = Sha256::digest(candidate.as_bytes()).into();
        bool::from(self.0.ct_eq(&candidate))
    }
}

pub async fn require_api_key(
    State(api_key): State<ApiKey>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let supplied = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(bearer_credential);

    match supplied {
        Some(candidate) if api_key.matches(candidate) => Ok(next.run(request).await),
        _ => Err(ApiError::unauthorized()),
    }
}

fn bearer_credential(value: &str) -> Option<&str> {
    let mut parts = value.split_ascii_whitespace();
    let scheme = parts.next()?;
    let credential = parts.next()?;

    if scheme.eq_ignore_ascii_case("Bearer") && parts.next().is_none() {
        Some(credential)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn rejects_short_keys() {
        assert_eq!(
            ApiKey::new("too-short").err(),
            Some("GATEWAY_API_KEY must be at least 32 bytes")
        );
    }

    #[test]
    fn rejects_keys_that_cannot_be_a_single_bearer_credential() {
        let key_with_space = format!("{KEY} ");
        assert_eq!(
            ApiKey::new(&key_with_space).err(),
            Some("GATEWAY_API_KEY must contain only visible ASCII characters")
        );
    }

    #[test]
    fn matches_only_the_configured_key() {
        let key = ApiKey::new(KEY).unwrap();
        assert!(key.matches(KEY));
        assert!(!key.matches("0123456789abcdef0123456789abcdee"));
    }

    #[test]
    fn parses_bearer_scheme_case_insensitively() {
        assert_eq!(bearer_credential(&format!("Bearer {KEY}")), Some(KEY));
        assert_eq!(bearer_credential(&format!("bearer {KEY}")), Some(KEY));
        assert_eq!(bearer_credential(KEY), None);
        assert_eq!(bearer_credential(&format!("Basic {KEY}")), None);
        assert_eq!(bearer_credential(&format!("Bearer {KEY} extra")), None);
    }
}
