use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::response::Response;
use gateway_core::valid_email;
use gateway_db::{AccountId, ProvisionAccountError};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};

use crate::api::error::ApiError;
use crate::state::AppState;

const JWKS_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const JWKS_OUTAGE_GRACE: Duration = Duration::from_secs(60 * 60);
const JWKS_RETRY_BACKOFF: Duration = Duration::from_secs(30);
const AUTHENTICATION_MAX_AGE: Duration = Duration::from_secs(5 * 60);
const CLOCK_SKEW: Duration = Duration::from_secs(30);
const EMAIL_OTP_METHOD: &str = "email_otp";

/// A fresh email-OTP authentication from one of the verifier's own merchant
/// applications — the CLI or the dashboard — the only credentials that may
/// issue or revoke API keys; for the payer audience it is the proof that the
/// merchant-asserted mailbox was just opened.
#[derive(Clone, Debug)]
pub struct Identity {
    pub issuer: String,
    pub subject: String,
    pub authentication_event_id: String,
    pub email: String,
}

/// A verified identity used as a session: it authenticates API calls for
/// the account behind it without any freshness or single-use requirement,
/// because it never mutates key state.
#[derive(Clone, Debug)]
pub struct SessionIdentity {
    pub issuer: String,
    pub subject: String,
    pub email: String,
}

#[derive(Clone)]
pub struct Auth0Verifier {
    inner: Arc<Auth0VerifierInner>,
}

struct Auth0VerifierInner {
    issuer: String,
    audience: String,
    /// The CLI application.
    client_id: String,
    /// The dashboard application; `None` refuses its tokens entirely.
    dashboard_client_id: Option<String>,
    jwks_url: String,
    http: reqwest::Client,
    keys: RwLock<CachedKeys>,
    refresh_lock: Mutex<()>,
}

struct CachedKeys {
    values: HashMap<String, DecodingKey>,
    refreshed_at: Instant,
    last_attempt: Instant,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    iss: String,
    azp: String,
    #[serde(rename = "https://api.payday.sh/auth/method")]
    authentication_method: String,
    #[serde(rename = "https://api.payday.sh/auth/client_id")]
    authentication_client_id: String,
    #[serde(rename = "https://api.payday.sh/auth/authenticated_at")]
    authenticated_at: u64,
    #[serde(rename = "https://api.payday.sh/auth/event_id")]
    authentication_event_id: String,
    #[serde(rename = "https://api.payday.sh/auth/email")]
    email: String,
}

impl Auth0Verifier {
    pub async fn new(
        issuer: String,
        audience: String,
        client_id: String,
        dashboard_client_id: Option<String>,
        allow_dev_identity: bool,
    ) -> Result<Self, String> {
        let parsed_issuer = reqwest::Url::parse(&issuer)
            .map_err(|error| format!("invalid Auth0 issuer URL: {error}"))?;
        if !issuer_transport_allowed(&parsed_issuer, allow_dev_identity)
            || parsed_issuer.query().is_some()
            || parsed_issuer.fragment().is_some()
        {
            return Err(
                "Auth0 issuer must be HTTPS; loopback HTTP requires PAYDAY_DEV_IDENTITY=1".into(),
            );
        }
        for (name, value) in [("audience", &audience), ("client ID", &client_id)] {
            if value.trim().is_empty() {
                return Err(format!("Auth0 {name} must not be empty"));
            }
        }
        if dashboard_client_id
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || *value == client_id)
        {
            return Err("Auth0 dashboard client ID must be a distinct, non-empty client".into());
        }
        let issuer = format!("{}/", issuer.trim_end_matches('/'));
        let jwks_url = format!("{issuer}.well-known/jwks.json");
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|error| format!("failed to build Auth0 HTTP client: {error}"))?;
        let values = fetch_keys(&http, &jwks_url).await?;
        Ok(Self {
            inner: Arc::new(Auth0VerifierInner {
                issuer,
                audience,
                client_id,
                dashboard_client_id,
                jwks_url,
                http,
                keys: RwLock::new(CachedKeys {
                    values,
                    refreshed_at: Instant::now(),
                    last_attempt: Instant::now() - JWKS_RETRY_BACKOFF,
                }),
                refresh_lock: Mutex::new(()),
            }),
        })
    }

    pub async fn verify(&self, token: &str) -> Result<Identity, ApiError> {
        let claims = self.decode(token).await?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ApiError::identity_unauthorized())?
            .as_secs();
        // Either merchant client may hold this, same as a session: the CLI
        // proves itself the same way the dashboard's own sign-in does. What
        // makes this stronger than a session is everything below — freshness
        // and single use — not which application asked.
        let accepted_client = claims.azp == self.inner.client_id
            || self.inner.dashboard_client_id.as_deref() == Some(claims.azp.as_str());
        if !accepted_client
            || claims.authentication_client_id != claims.azp
            || !email_otp_subject(&claims)
            || claims.authentication_event_id.is_empty()
            || claims.authentication_event_id.len() > 255
            || claims.authenticated_at > now + CLOCK_SKEW.as_secs()
            || now.saturating_sub(claims.authenticated_at) > AUTHENTICATION_MAX_AGE.as_secs()
        {
            return Err(ApiError::identity_unauthorized());
        }
        Ok(Identity {
            issuer: claims.iss,
            subject: claims.sub,
            authentication_event_id: claims.authentication_event_id,
            email: claims.email,
        })
    }

    /// A dashboard or CLI token as a session: signature, issuer, audience,
    /// and expiry as for [`Self::verify`], and the same email-OTP provenance,
    /// but no freshness window and no event consumption — the token lives
    /// as long as Auth0 says and never touches key state.
    pub async fn verify_session(&self, token: &str) -> Result<SessionIdentity, ApiError> {
        let claims = self.decode(token).await?;
        let accepted_client = claims.azp == self.inner.client_id
            || self.inner.dashboard_client_id.as_deref() == Some(claims.azp.as_str());
        if !accepted_client
            || claims.authentication_client_id != claims.azp
            || !email_otp_subject(&claims)
        {
            return Err(ApiError::identity_unauthorized());
        }
        Ok(SessionIdentity {
            issuer: claims.iss,
            subject: claims.sub,
            email: claims.email,
        })
    }

    /// Signature, key rotation, and the registered claims common to both
    /// token uses.
    async fn decode(&self, token: &str) -> Result<Claims, ApiError> {
        let header = decode_header(token).map_err(|_| ApiError::identity_unauthorized())?;
        if header.alg != Algorithm::RS256 {
            return Err(ApiError::identity_unauthorized());
        }
        let kid = header.kid.ok_or_else(ApiError::identity_unauthorized)?;

        self.refresh_keys_if_needed(&kid).await?;
        let cached = self.inner.keys.read().await;
        if cached.refreshed_at.elapsed() > JWKS_OUTAGE_GRACE {
            return Err(ApiError::identity_unavailable());
        }
        let key = cached
            .values
            .get(&kid)
            .cloned()
            .ok_or_else(ApiError::identity_unauthorized)?;
        drop(cached);

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[&self.inner.audience]);
        validation.set_issuer(&[&self.inner.issuer]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub", "azp"]);
        validation.leeway = 30;
        decode::<Claims>(token, &key, &validation)
            .map(|data| data.claims)
            .map_err(|_| ApiError::identity_unauthorized())
    }

    async fn refresh_keys_if_needed(&self, kid: &str) -> Result<(), ApiError> {
        {
            let cached = self.inner.keys.read().await;
            if !refresh_is_due(&cached, kid) {
                return Ok(());
            }
        }

        let _refresh = self.inner.refresh_lock.lock().await;
        {
            let cached = self.inner.keys.read().await;
            if !refresh_is_due(&cached, kid) {
                return Ok(());
            }
        }

        self.inner.keys.write().await.last_attempt = Instant::now();
        match fetch_keys(&self.inner.http, &self.inner.jwks_url).await {
            Ok(values) => {
                let mut cached = self.inner.keys.write().await;
                cached.values = values;
                cached.refreshed_at = Instant::now();
                Ok(())
            }
            Err(error) => {
                tracing::warn!(%error, "failed to refresh Auth0 JWKS");
                if self.inner.keys.read().await.refreshed_at.elapsed() > JWKS_OUTAGE_GRACE {
                    Err(ApiError::identity_unavailable())
                } else {
                    Ok(())
                }
            }
        }
    }

    #[cfg(test)]
    pub fn for_test(
        issuer: &str,
        audience: &str,
        client_id: &str,
        dashboard_client_id: Option<&str>,
        kid: &str,
        key: DecodingKey,
    ) -> Self {
        Self {
            inner: Arc::new(Auth0VerifierInner {
                issuer: format!("{}/", issuer.trim_end_matches('/')),
                audience: audience.into(),
                client_id: client_id.into(),
                dashboard_client_id: dashboard_client_id.map(str::to_owned),
                jwks_url: String::new(),
                http: reqwest::Client::new(),
                keys: RwLock::new(CachedKeys {
                    values: HashMap::from([(kid.into(), key)]),
                    refreshed_at: Instant::now(),
                    last_attempt: Instant::now() - JWKS_RETRY_BACKOFF,
                }),
                refresh_lock: Mutex::new(()),
            }),
        }
    }
}

/// Payday's Auth0 Action stamps these claims only for its own email-OTP
/// flow; a token from any other connection carries no merchant email.
fn email_otp_subject(claims: &Claims) -> bool {
    claims.authentication_method == EMAIL_OTP_METHOD
        && claims.sub.starts_with("email|")
        && valid_email(&claims.email)
}

fn issuer_transport_allowed(issuer: &reqwest::Url, allow_dev_identity: bool) -> bool {
    issuer.scheme() == "https"
        || (allow_dev_identity
            && issuer.scheme() == "http"
            && issuer.host_str().is_some_and(|host| {
                let host = host
                    .strip_prefix('[')
                    .and_then(|host| host.strip_suffix(']'))
                    .unwrap_or(host);
                host.eq_ignore_ascii_case("localhost")
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            }))
}

fn refresh_is_due(cached: &CachedKeys, kid: &str) -> bool {
    (cached.refreshed_at.elapsed() >= JWKS_CACHE_TTL || !cached.values.contains_key(kid))
        && cached.last_attempt.elapsed() >= JWKS_RETRY_BACKOFF
}

async fn fetch_keys(
    http: &reqwest::Client,
    url: &str,
) -> Result<HashMap<String, DecodingKey>, String> {
    let response = http
        .get(url)
        .send()
        .await
        .map_err(|error| format!("failed to fetch Auth0 JWKS: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Auth0 JWKS returned an error: {error}"))?;
    let set: JwkSet = response
        .json()
        .await
        .map_err(|error| format!("invalid Auth0 JWKS: {error}"))?;
    let mut keys = HashMap::new();
    for jwk in set.keys {
        if let Some(kid) = jwk.common.key_id.clone()
            && let Ok(key) = DecodingKey::from_jwk(&jwk)
        {
            keys.insert(kid, key);
        }
    }
    if keys.is_empty() {
        return Err("Auth0 JWKS contains no usable keys".into());
    }
    Ok(keys)
}

/// Authenticate a merchant request with either an API key or an identity
/// token (a dashboard session, or the CLI's own token). The bearer's prefix
/// decides which: keys are always `payday_live_…`/`payday_test_…`, tokens
/// never are. Either way the request runs as one account, rate limited as
/// that account.
pub async fn require_account(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let supplied = bearer_from_request(&request).ok_or_else(ApiError::unauthorized)?;
    let account = if looks_like_api_key(supplied) {
        state
            .accounts
            .authenticate(supplied)
            .await?
            .ok_or_else(ApiError::unauthorized)?
    } else {
        session_account(&state, supplied).await?
    };
    // One independently refilled bucket per authenticated account. Authentication
    // failures cannot consume another customer's allowance.
    const LIMIT: f64 = 60.0;
    let now = Instant::now();
    let (allowed, remaining, seconds_until_full) = {
        let mut buckets = state.rate_limits.lock().await;
        let bucket = buckets.entry(account.0).or_insert((LIMIT, now));
        bucket.0 = (bucket.0 + now.duration_since(bucket.1).as_secs_f64()).min(LIMIT);
        bucket.1 = now;
        let allowed = bucket.0 >= 1.0;
        if allowed {
            bucket.0 -= 1.0;
        }
        (
            allowed,
            bucket.0.floor() as u64,
            (LIMIT - bucket.0).ceil() as u64,
        )
    };
    request.extensions_mut().insert(account);
    tracing::Span::current().record("account_id", tracing::field::display(account.0));
    let mut response = if allowed {
        next.run(request).await
    } else {
        ApiError::rate_limited().into_response()
    };
    response
        .headers_mut()
        .insert("x-ratelimit-limit", HeaderValue::from_static("60"));
    response.headers_mut().insert(
        "x-ratelimit-remaining",
        HeaderValue::from_str(&remaining.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        "x-ratelimit-reset",
        HeaderValue::from_str(
            &(SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                + seconds_until_full)
                .to_string(),
        )
        .unwrap(),
    );
    if !allowed {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    }
    Ok(response)
}

fn looks_like_api_key(credential: &str) -> bool {
    ["payday_live_", "payday_test_"]
        .iter()
        .any(|prefix| credential.starts_with(prefix))
}

/// The account behind an identity token, provisioned on first sight so a
/// merchant who signs in to the dashboard exists before they ever hold a key.
async fn session_account(state: &AppState, token: &str) -> Result<AccountId, ApiError> {
    let verifier = state
        .identity_verifier
        .as_ref()
        .ok_or_else(ApiError::unauthorized)?;
    let session = verifier.verify_session(token).await.map_err(|error| {
        // A JWKS outage is worth telling apart; every other failure is just
        // an invalid credential, exactly like a wrong key.
        if error.status == StatusCode::SERVICE_UNAVAILABLE {
            error
        } else {
            ApiError::unauthorized()
        }
    })?;
    state
        .accounts
        .find_or_provision_by_identity(&session.issuer, &session.subject, &session.email)
        .await
        .map_err(|error| match error {
            ProvisionAccountError::AccountDisabled => ApiError::account_disabled(),
            ProvisionAccountError::Database(error) => error.into(),
        })
}

pub async fn require_identity(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let token = bearer_from_request(&request).ok_or_else(ApiError::identity_unauthorized)?;
    let verifier = state
        .identity_verifier
        .as_ref()
        .ok_or_else(ApiError::identity_unavailable)?;
    let identity = verifier.verify(token).await?;
    request.extensions_mut().insert(identity);
    Ok(next.run(request).await)
}

/// Who an operator route acts as: the configured reviewer identity, recorded
/// on every manual verification decision (product plan §6.3).
#[derive(Clone, Debug)]
pub struct Reviewer(pub String);

/// `PAYDAY_ADMIN_REVIEWER_ID`, or `operator` while a deployment has not named
/// its reviewer.
pub fn reviewer_from_env() -> Reviewer {
    Reviewer(
        std::env::var("PAYDAY_ADMIN_REVIEWER_ID")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "operator".into()),
    )
}

pub async fn require_admin(mut request: Request, next: Next) -> Result<Response, ApiError> {
    let supplied = bearer_from_request(&request).ok_or_else(ApiError::admin_unauthorized)?;
    let expected =
        std::env::var("PAYDAY_ADMIN_BEARER_SECRET").map_err(|_| ApiError::admin_unauthorized())?;
    let mut expected_mac =
        Hmac::<Sha256>::new_from_slice(b"Payday admin credential comparison").unwrap();
    expected_mac.update(expected.as_bytes());
    let expected_tag = expected_mac.finalize().into_bytes();
    let mut supplied_mac =
        Hmac::<Sha256>::new_from_slice(b"Payday admin credential comparison").unwrap();
    supplied_mac.update(supplied.as_bytes());
    if expected.len() < 32 || supplied_mac.verify_slice(&expected_tag).is_err() {
        return Err(ApiError::admin_unauthorized());
    }
    request.extensions_mut().insert(reviewer_from_env());
    Ok(next.run(request).await)
}

fn bearer_from_request(request: &Request) -> Option<&str> {
    request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(bearer_credential)
}

fn bearer_credential(value: &str) -> Option<&str> {
    let mut parts = value.split_ascii_whitespace();
    let scheme = parts.next()?;
    let credential = parts.next()?;
    (scheme.eq_ignore_ascii_case("Bearer") && parts.next().is_none()).then_some(credential)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    use rsa::traits::PublicKeyParts;
    use serde::Serialize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Serialize)]
    struct TestClaims<'a> {
        sub: &'a str,
        iss: &'a str,
        aud: &'a str,
        exp: u64,
        nbf: u64,
        azp: &'a str,
        #[serde(rename = "https://api.payday.sh/auth/method")]
        authentication_method: &'a str,
        #[serde(rename = "https://api.payday.sh/auth/client_id")]
        authentication_client_id: &'a str,
        #[serde(rename = "https://api.payday.sh/auth/authenticated_at")]
        authenticated_at: u64,
        #[serde(rename = "https://api.payday.sh/auth/event_id")]
        authentication_event_id: &'a str,
        #[serde(rename = "https://api.payday.sh/auth/email")]
        email: &'a str,
    }

    #[test]
    fn parses_one_bearer_credential() {
        assert_eq!(bearer_credential("Bearer key"), Some("key"));
        assert_eq!(bearer_credential("bearer key"), Some("key"));
        assert_eq!(bearer_credential("key"), None);
        assert_eq!(bearer_credential("Basic key"), None);
        assert_eq!(bearer_credential("Bearer key extra"), None);
    }

    #[tokio::test]
    async fn auth0_configuration_requires_secure_issuer_audience_and_client() {
        assert!(
            Auth0Verifier::new(
                "http://issuer.example".into(),
                "audience".into(),
                "client".into(),
                None,
                false,
            )
            .await
            .is_err()
        );
        assert!(
            Auth0Verifier::new(
                "https://issuer.example".into(),
                " ".into(),
                "client".into(),
                None,
                false,
            )
            .await
            .is_err()
        );
        assert!(
            Auth0Verifier::new(
                "https://issuer.example".into(),
                "audience".into(),
                " ".into(),
                None,
                false,
            )
            .await
            .is_err()
        );
        assert!(
            Auth0Verifier::new(
                "https://issuer.example".into(),
                "audience".into(),
                "client".into(),
                Some("client".into()),
                false,
            )
            .await
            .is_err(),
            "the dashboard client must be distinct"
        );
        assert!(issuer_transport_allowed(
            &reqwest::Url::parse("http://127.0.0.1:3001").unwrap(),
            true
        ));
        assert!(issuer_transport_allowed(
            &reqwest::Url::parse("http://[::1]:3001").unwrap(),
            true
        ));
        assert!(!issuer_transport_allowed(
            &reqwest::Url::parse("http://identity.example").unwrap(),
            true
        ));
    }

    fn verifier_and_key() -> (Auth0Verifier, EncodingKey) {
        let private = rsa::RsaPrivateKey::new(&mut rand_08::thread_rng(), 2048).unwrap();
        let private_pem = private.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_pem = private
            .to_public_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let decoding = DecodingKey::from_rsa_pem(public_pem.as_bytes()).unwrap();
        let verifier = Auth0Verifier {
            inner: Arc::new(Auth0VerifierInner {
                issuer: "https://issuer.example/".into(),
                audience: "https://api.payday.sh".into(),
                client_id: "payday-cli".into(),
                dashboard_client_id: Some("payday-dashboard".into()),
                jwks_url: "https://issuer.example/.well-known/jwks.json".into(),
                http: reqwest::Client::new(),
                keys: RwLock::new(CachedKeys {
                    values: HashMap::from([("test-key".into(), decoding)]),
                    refreshed_at: Instant::now(),
                    last_attempt: Instant::now() - JWKS_RETRY_BACKOFF,
                }),
                refresh_lock: Mutex::new(()),
            }),
        };
        (
            verifier,
            EncodingKey::from_rsa_pem(private_pem.as_bytes()).unwrap(),
        )
    }

    fn token(key: &EncodingKey, issuer: &str, audience: &str, expires_at: u64) -> String {
        token_with_kid(key, "test-key", issuer, audience, expires_at)
    }

    fn token_with_kid(
        key: &EncodingKey,
        kid: &str,
        issuer: &str,
        audience: &str,
        expires_at: u64,
    ) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.into());
        encode(
            &header,
            &TestClaims {
                sub: "email|user",
                iss: issuer,
                aud: audience,
                exp: expires_at,
                nbf: 1,
                azp: "payday-cli",
                authentication_method: EMAIL_OTP_METHOD,
                authentication_client_id: "payday-cli",
                authenticated_at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                authentication_event_id: "authentication-event",
                email: "merchant@example.com",
            },
            key,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn verifies_signature_and_registered_claims() {
        let (verifier, key) = verifier_and_key();
        let valid = token(
            &key,
            "https://issuer.example/",
            "https://api.payday.sh",
            u64::MAX,
        );
        let identity = verifier.verify(&valid).await.unwrap();
        assert_eq!(identity.subject, "email|user");

        for invalid in [
            token(
                &key,
                "https://wrong.example/",
                "https://api.payday.sh",
                u64::MAX,
            ),
            token(&key, "https://issuer.example/", "wrong-audience", u64::MAX),
            token(&key, "https://issuer.example/", "https://api.payday.sh", 1),
        ] {
            assert!(verifier.verify(&invalid).await.is_err());
        }
    }

    #[tokio::test]
    async fn rejects_tokens_not_from_fresh_payday_email_otp_authentication() {
        let (verifier, key) = verifier_and_key();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        for (sub, azp, method, client_id, authenticated_at, event_id) in [
            (
                "google-oauth2|user",
                "payday-cli",
                EMAIL_OTP_METHOD,
                "payday-cli",
                now,
                "event",
            ),
            (
                "email|user",
                "other-client",
                EMAIL_OTP_METHOD,
                "payday-cli",
                now,
                "event",
            ),
            (
                "email|user",
                "payday-cli",
                EMAIL_OTP_METHOD,
                "other-client",
                now,
                "event",
            ),
            (
                "email|user",
                "payday-cli",
                "social",
                "payday-cli",
                now,
                "event",
            ),
            (
                "email|user",
                "payday-cli",
                EMAIL_OTP_METHOD,
                "payday-cli",
                now - 301,
                "event",
            ),
            (
                "email|user",
                "payday-cli",
                EMAIL_OTP_METHOD,
                "payday-cli",
                now + 31,
                "event",
            ),
            (
                "email|user",
                "payday-cli",
                EMAIL_OTP_METHOD,
                "payday-cli",
                now,
                "",
            ),
        ] {
            let mut header = Header::new(Algorithm::RS256);
            header.kid = Some("test-key".into());
            let token = encode(
                &header,
                &TestClaims {
                    sub,
                    iss: "https://issuer.example/",
                    aud: "https://api.payday.sh",
                    exp: u64::MAX,
                    nbf: 1,
                    azp,
                    authentication_method: method,
                    authentication_client_id: client_id,
                    authenticated_at,
                    authentication_event_id: event_id,
                    email: "merchant@example.com",
                },
                &key,
            )
            .unwrap();
            assert!(verifier.verify(&token).await.is_err());
        }
    }

    #[tokio::test]
    async fn session_tokens_accept_both_applications_without_a_freshness_window() {
        let (verifier, key) = verifier_and_key();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let token = |azp: &str, client_id: &str, method: &str, sub: &str, authenticated_at| {
            let mut header = Header::new(Algorithm::RS256);
            header.kid = Some("test-key".into());
            encode(
                &header,
                &TestClaims {
                    sub,
                    iss: "https://issuer.example/",
                    aud: "https://api.payday.sh",
                    exp: u64::MAX,
                    nbf: 1,
                    azp,
                    authentication_method: method,
                    authentication_client_id: client_id,
                    authenticated_at,
                    authentication_event_id: "event",
                    email: "merchant@example.com",
                },
                &key,
            )
            .unwrap()
        };
        let stale = now - 24 * 3600;

        // A day-old dashboard token is a fine session, but stale for a key
        // credential regardless of which merchant client minted it.
        let dashboard = token(
            "payday-dashboard",
            "payday-dashboard",
            EMAIL_OTP_METHOD,
            "email|user",
            stale,
        );
        let session = verifier.verify_session(&dashboard).await.unwrap();
        assert_eq!(session.subject, "email|user");
        assert_eq!(session.email, "merchant@example.com");
        assert!(verifier.verify(&dashboard).await.is_err());

        // Freshly authenticated, that same dashboard client is just as good a
        // key credential as the CLI: the dashboard can step up with its own
        // sign-in to manage the key, without ever holding one day to day.
        let fresh_dashboard = token(
            "payday-dashboard",
            "payday-dashboard",
            EMAIL_OTP_METHOD,
            "email|user",
            now,
        );
        let identity = verifier.verify(&fresh_dashboard).await.unwrap();
        assert_eq!(identity.subject, "email|user");

        // The CLI's own token works as a session too, stale or not.
        let cli = token(
            "payday-cli",
            "payday-cli",
            EMAIL_OTP_METHOD,
            "email|user",
            stale,
        );
        assert!(verifier.verify_session(&cli).await.is_ok());
        assert!(verifier.verify(&cli).await.is_err());
        let fresh_cli = token(
            "payday-cli",
            "payday-cli",
            EMAIL_OTP_METHOD,
            "email|user",
            now,
        );
        assert!(verifier.verify(&fresh_cli).await.is_ok());

        for rejected in [
            token(
                "other-app",
                "other-app",
                EMAIL_OTP_METHOD,
                "email|user",
                now,
            ),
            token(
                "payday-dashboard",
                "payday-cli",
                EMAIL_OTP_METHOD,
                "email|user",
                now,
            ),
            token(
                "payday-dashboard",
                "payday-dashboard",
                "social",
                "email|user",
                now,
            ),
            token(
                "payday-dashboard",
                "payday-dashboard",
                EMAIL_OTP_METHOD,
                "google-oauth2|user",
                now,
            ),
        ] {
            assert!(verifier.verify_session(&rejected).await.is_err());
        }
        assert!(
            verifier
                .verify_session(&token(
                    "payday-dashboard",
                    "payday-dashboard",
                    EMAIL_OTP_METHOD,
                    "email|user",
                    now
                ))
                .await
                .is_ok()
        );
        assert!(
            stale_verifier(
                "http://127.0.0.1:1/jwks".into(),
                "unused",
                DecodingKey::from_secret(b"")
            )
            .verify_session(&dashboard)
            .await
            .is_err(),
            "a deployment without a dashboard client refuses its tokens"
        );
    }

    fn rsa_key(kid: &str) -> (String, EncodingKey, DecodingKey) {
        let private = rsa::RsaPrivateKey::new(&mut rand_08::thread_rng(), 2048).unwrap();
        let public = private.to_public_key();
        let private_pem = private.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_pem = public.to_public_key_pem(LineEnding::LF).unwrap();
        let jwk = serde_json::json!({
            "kty": "RSA",
            "kid": kid,
            "use": "sig",
            "alg": "RS256",
            "n": URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),
            "e": URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())
        })
        .to_string();
        (
            jwk,
            EncodingKey::from_rsa_pem(private_pem.as_bytes()).unwrap(),
            DecodingKey::from_rsa_pem(public_pem.as_bytes()).unwrap(),
        )
    }

    async fn jwks_server(jwk: String) -> (String, tokio::task::JoinHandle<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/jwks", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).await.unwrap();
            let body = format!(r#"{{"keys":[{jwk}]}}"#);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            1
        });
        (url, handle)
    }

    async fn failing_jwks_server() -> (String, tokio::task::JoinHandle<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/jwks", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            1
        });
        (url, handle)
    }

    fn stale_verifier(jwks_url: String, kid: &str, key: DecodingKey) -> Auth0Verifier {
        Auth0Verifier {
            inner: Arc::new(Auth0VerifierInner {
                issuer: "https://issuer.example/".into(),
                audience: "https://api.payday.sh".into(),
                client_id: "payday-cli".into(),
                dashboard_client_id: None,
                jwks_url,
                http: reqwest::Client::new(),
                keys: RwLock::new(CachedKeys {
                    values: HashMap::from([(kid.into(), key)]),
                    refreshed_at: Instant::now() - JWKS_CACHE_TTL - Duration::from_secs(1),
                    last_attempt: Instant::now() - JWKS_RETRY_BACKOFF,
                }),
                refresh_lock: Mutex::new(()),
            }),
        }
    }

    #[tokio::test]
    async fn stale_jwks_refresh_adds_and_removes_keys() {
        let (_, old_encoding, old_decoding) = rsa_key("old-key");
        let (new_jwk, new_encoding, _) = rsa_key("new-key");
        let (url, requests) = jwks_server(new_jwk).await;
        let verifier = stale_verifier(url, "old-key", old_decoding);
        let old_token = token_with_kid(
            &old_encoding,
            "old-key",
            "https://issuer.example/",
            "https://api.payday.sh",
            u64::MAX,
        );
        assert!(verifier.verify(&old_token).await.is_err());
        let new_token = token_with_kid(
            &new_encoding,
            "new-key",
            "https://issuer.example/",
            "https://api.payday.sh",
            u64::MAX,
        );
        assert_eq!(
            verifier.verify(&new_token).await.unwrap().subject,
            "email|user"
        );
        assert_eq!(requests.await.unwrap(), 1);
    }

    #[tokio::test]
    async fn unknown_key_id_refreshes_a_fresh_jwks_cache() {
        let (_, _, old_decoding) = rsa_key("old-key");
        let (new_jwk, new_encoding, _) = rsa_key("new-key");
        let (url, requests) = jwks_server(new_jwk).await;
        let verifier = stale_verifier(url, "old-key", old_decoding);
        verifier.inner.keys.write().await.refreshed_at = Instant::now();
        let new_token = token_with_kid(
            &new_encoding,
            "new-key",
            "https://issuer.example/",
            "https://api.payday.sh",
            u64::MAX,
        );

        assert!(verifier.verify(&new_token).await.is_ok());
        assert_eq!(requests.await.unwrap(), 1);
    }

    #[tokio::test]
    async fn concurrent_refresh_is_single_flight() {
        let (new_jwk, new_encoding, old_decoding) = rsa_key("new-key");
        let (url, requests) = jwks_server(new_jwk).await;
        let verifier = stale_verifier(url, "old-key", old_decoding);
        let token = token_with_kid(
            &new_encoding,
            "new-key",
            "https://issuer.example/",
            "https://api.payday.sh",
            u64::MAX,
        );
        let checks = (0..8).map(|_| {
            let verifier = verifier.clone();
            let token = token.clone();
            tokio::spawn(async move { verifier.verify(&token).await })
        });
        for check in checks {
            assert!(check.await.unwrap().is_ok());
        }
        assert_eq!(requests.await.unwrap(), 1);
    }

    #[tokio::test]
    async fn refresh_failure_is_backed_off_while_cached_key_is_fresh_enough() {
        let (_, encoding, decoding) = rsa_key("old-key");
        let (url, requests) = failing_jwks_server().await;
        let verifier = stale_verifier(url, "old-key", decoding);
        let token = token_with_kid(
            &encoding,
            "old-key",
            "https://issuer.example/",
            "https://api.payday.sh",
            u64::MAX,
        );
        assert!(verifier.verify(&token).await.is_ok());
        assert!(verifier.verify(&token).await.is_ok());
        assert_eq!(requests.await.unwrap(), 1);
    }

    #[tokio::test]
    async fn expired_jwks_cache_fails_closed_during_outage() {
        let (_, encoding, decoding) = rsa_key("old-key");
        let mut verifier = stale_verifier("http://127.0.0.1:1/jwks".into(), "old-key", decoding);
        Arc::get_mut(&mut verifier.inner)
            .unwrap()
            .keys
            .get_mut()
            .refreshed_at = Instant::now() - JWKS_OUTAGE_GRACE - Duration::from_secs(1);
        let token = token_with_kid(
            &encoding,
            "old-key",
            "https://issuer.example/",
            "https://api.payday.sh",
            u64::MAX,
        );
        let error = verifier.verify(&token).await.unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }
}
