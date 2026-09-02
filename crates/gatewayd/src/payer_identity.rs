//! The payer's proof of mailbox ownership (product plan §6.1).
//!
//! The gateway drives Auth0's passwordless flow on the payer's behalf against
//! a dedicated payer audience: it asks Auth0 to email a code to the
//! merchant-asserted address, exchanges the code the payer types, and verifies
//! the resulting token with the same JWKS discipline as merchant tokens. The
//! payer never names an email; the request only ever carries the invoice and
//! the code.
//!
//! Codes, tokens, and the mailbox itself are never logged.

use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::B256;
use async_trait::async_trait;
use axum::http::StatusCode;
use gateway_db::payer_ref;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::{Auth0Verifier, Identity};

const PASSWORDLESS_OTP_GRANT: &str = "http://auth0.com/oauth/grant-type/passwordless/otp";

#[derive(Debug)]
pub enum OtpError {
    /// The provider refused the code (wrong, expired, or already used).
    Rejected,
    /// The provider could not be reached or answered outside its contract.
    Unavailable(String),
}

/// Auth0's two passwordless endpoints, behind a trait so the route tests can
/// stand in for the tenant. This is not a vendor abstraction: Auth0 is the
/// only implementation that ships.
#[async_trait]
pub trait PayerOtpProvider: Send + Sync {
    /// Email a one-time code to `email`.
    async fn start(&self, email: &str) -> Result<(), OtpError>;
    /// Exchange the code for an access token for the payer audience.
    async fn exchange(&self, email: &str, otp: &str) -> Result<String, OtpError>;
}

pub struct Auth0Passwordless {
    http: reqwest::Client,
    /// Normalized with a trailing slash, like the verifier's issuer.
    issuer: String,
    client_id: String,
    audience: String,
}

impl Auth0Passwordless {
    pub fn new(issuer: &str, client_id: String, audience: String) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| format!("failed to build payer Auth0 HTTP client: {error}"))?;
        Ok(Self {
            http,
            issuer: format!("{}/", issuer.trim_end_matches('/')),
            client_id,
            audience,
        })
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[async_trait]
impl PayerOtpProvider for Auth0Passwordless {
    async fn start(&self, email: &str) -> Result<(), OtpError> {
        let response = self
            .http
            .post(format!("{}passwordless/start", self.issuer))
            .json(&serde_json::json!({
                "client_id": self.client_id,
                "connection": "email",
                "email": email,
                "send": "code",
            }))
            .send()
            .await
            .map_err(|error| OtpError::Unavailable(format!("passwordless start: {error}")))?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(OtpError::Unavailable(format!(
                "passwordless start answered {status}"
            )))
        }
    }

    async fn exchange(&self, email: &str, otp: &str) -> Result<String, OtpError> {
        let response = self
            .http
            .post(format!("{}oauth/token", self.issuer))
            .json(&serde_json::json!({
                "grant_type": PASSWORDLESS_OTP_GRANT,
                "client_id": self.client_id,
                "username": email,
                "otp": otp,
                "realm": "email",
                "audience": self.audience,
            }))
            .send()
            .await
            .map_err(|error| OtpError::Unavailable(format!("token exchange: {error}")))?;
        match response.status() {
            status if status.is_success() => response
                .json::<TokenResponse>()
                .await
                .map(|token| token.access_token)
                .map_err(|_| OtpError::Unavailable("token exchange returned no token".into())),
            // Auth0 answers `invalid_grant` for a wrong or spent code; the
            // local development provider says 401 for the same thing.
            StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED => Err(OtpError::Rejected),
            status => Err(OtpError::Unavailable(format!(
                "token exchange answered {status}"
            ))),
        }
    }
}

/// Everything the email verification routes need: the provider, the payer
/// audience's verifier, and the key behind payer references.
#[derive(Clone)]
pub struct PayerVerification {
    otp: Arc<dyn PayerOtpProvider>,
    verifier: Auth0Verifier,
    payer_ref_master_key: [u8; 32],
}

impl PayerVerification {
    pub fn new(
        otp: Arc<dyn PayerOtpProvider>,
        verifier: Auth0Verifier,
        payer_ref_master_key: [u8; 32],
    ) -> Self {
        Self {
            otp,
            verifier,
            payer_ref_master_key,
        }
    }

    /// Email a code to the merchant-asserted address.
    pub async fn send_code(&self, expected_email: &str) -> Result<(), ApiError> {
        self.otp.start(expected_email).await.map_err(|error| {
            if let OtpError::Unavailable(reason) = &error {
                tracing::warn!(reason, "payer email verification could not start");
            }
            ApiError::identity_provider_unavailable()
        })
    }

    /// Exchange the code, verify the token against the payer audience, and
    /// insist the mailbox Auth0 proved is the one the merchant asserted.
    pub async fn confirm_code(
        &self,
        expected_email: &str,
        otp: &str,
    ) -> Result<Identity, ApiError> {
        let token = self
            .otp
            .exchange(expected_email, otp)
            .await
            .map_err(|error| match error {
                OtpError::Rejected => ApiError::otp_invalid(),
                OtpError::Unavailable(reason) => {
                    tracing::warn!(reason, "payer email verification could not be confirmed");
                    ApiError::identity_provider_unavailable()
                }
            })?;
        let identity = self.verifier.verify(&token).await.map_err(|error| {
            // A JWKS outage is worth telling apart; any other failure means
            // the token does not prove what the flow requires.
            if error.status == StatusCode::SERVICE_UNAVAILABLE {
                error
            } else {
                ApiError::otp_invalid()
            }
        })?;
        if identity.email.trim().to_lowercase() != expected_email {
            return Err(ApiError::otp_invalid());
        }
        Ok(identity)
    }

    pub fn payer_ref(&self, account_id: Uuid, normalized_email: &str) -> B256 {
        payer_ref(&self.payer_ref_master_key, account_id, normalized_email)
    }
}

#[cfg(test)]
pub mod testing {
    //! A stand-in tenant: it remembers which mailboxes were asked for a code
    //! and mints a signed payer token for the fixed code.

    use std::sync::Mutex;

    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use serde::Serialize;

    use super::*;

    pub const OTP: &str = "123456";

    #[derive(Serialize)]
    struct PayerClaims<'a> {
        sub: String,
        iss: &'a str,
        aud: &'a str,
        exp: u64,
        azp: &'a str,
        #[serde(rename = "https://api.payday.sh/auth/method")]
        authentication_method: &'a str,
        #[serde(rename = "https://api.payday.sh/auth/client_id")]
        authentication_client_id: &'a str,
        #[serde(rename = "https://api.payday.sh/auth/authenticated_at")]
        authenticated_at: u64,
        #[serde(rename = "https://api.payday.sh/auth/event_id")]
        authentication_event_id: String,
        #[serde(rename = "https://api.payday.sh/auth/email")]
        email: &'a str,
    }

    pub struct FakeTenant {
        pub issuer: String,
        pub audience: String,
        pub client_id: String,
        pub kid: String,
        pub key: EncodingKey,
        /// Every mailbox a code was sent to, in order.
        pub started: Mutex<Vec<String>>,
        /// When set, the tenant is down.
        pub outage: Mutex<bool>,
    }

    #[async_trait]
    impl PayerOtpProvider for FakeTenant {
        async fn start(&self, email: &str) -> Result<(), OtpError> {
            if *self.outage.lock().unwrap() {
                return Err(OtpError::Unavailable("outage".into()));
            }
            self.started.lock().unwrap().push(email.to_owned());
            Ok(())
        }

        async fn exchange(&self, email: &str, otp: &str) -> Result<String, OtpError> {
            if otp != OTP {
                return Err(OtpError::Rejected);
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let mut header = Header::new(Algorithm::RS256);
            header.kid = Some(self.kid.clone());
            encode(
                &header,
                &PayerClaims {
                    sub: format!("email|{email}"),
                    iss: &self.issuer,
                    aud: &self.audience,
                    exp: now + 300,
                    azp: &self.client_id,
                    authentication_method: "email_otp",
                    authentication_client_id: &self.client_id,
                    authenticated_at: now,
                    authentication_event_id: Uuid::now_v7().to_string(),
                    email,
                },
                &self.key,
            )
            .map_err(|error| OtpError::Unavailable(error.to_string()))
        }
    }
}
