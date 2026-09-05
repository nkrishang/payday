//! Who is calling.
//!
//! Two identity providers, deliberately:
//!
//! - **Privy** signs merchants in. The dashboard's email login mints an
//!   identity token — an ES256 JWT carrying the user's DID, mailbox, and the
//!   embedded EVM wallet Privy created for them — and that token is the
//!   dashboard session. [`PrivyVerifier`] checks it against the app's JWKS.
//! - **Auth0** proves mailboxes that are *not* merchants: a payer opening a
//!   gated invoice, or an issuer identity's contact address. Those flows are
//!   many times more numerous than merchant sign-ups and never need an
//!   account, so they stay on the cheap passwordless OTP rather than creating
//!   a Privy user each. [`Auth0Verifier`] checks the token the exchange
//!   returns.
//!
//! Both verifiers share one JWKS cache with the same rotation and outage
//! behaviour; only the key type, the claims, and what they mean differ.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use alloy_primitives::Address;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use gateway_core::valid_email;
use gateway_db::{AccountId, ProvisionAccountError};
use hmac::{Hmac, Mac};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use sha2::Sha256;
use tokio::sync::{Mutex, RwLock};

use crate::api::error::ApiError;
use crate::state::AppState;

const JWKS_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const JWKS_OUTAGE_GRACE: Duration = Duration::from_secs(60 * 60);
const JWKS_RETRY_BACKOFF: Duration = Duration::from_secs(30);
const AUTHENTICATION_MAX_AGE: Duration = Duration::from_secs(5 * 60);
const CLOCK_SKEW: Duration = Duration::from_secs(30);
const EMAIL_OTP_METHOD: &str = "email_otp";

/// The `iss` of every token Privy signs, whatever the app.
pub const PRIVY_ISSUER: &str = "privy.io";
/// Privy DIDs are the only subjects a merchant session may carry.
const PRIVY_SUBJECT_PREFIX: &str = "did:privy:";

/// A fresh email-OTP authentication from the Auth0 payer application: the
/// proof that the merchant-asserted mailbox (a payer's, or an issuer
/// identity's contact address) was just opened.
#[derive(Clone, Debug)]
pub struct Identity {
    pub authentication_event_id: String,
    pub email: String,
}

/// The merchant behind a dashboard session, as Privy's identity token states
/// them: the DID that owns the account, the mailbox they signed in with, and
/// the embedded wallet Privy holds for them — `None` only in the moments
/// between a first login and the wallet's creation.
#[derive(Clone, Debug)]
pub struct MerchantIdentity {
    pub issuer: String,
    pub subject: String,
    pub email: String,
    /// EIP-55 checksummed.
    pub wallet_address: Option<String>,
}

/// The merchant identity a session-authenticated request carries. Routes
/// that must not be reachable with an API key — issuing and revoking keys —
/// take this instead of a bare `AccountId`; a request that authenticated
/// with a key has none and is refused.
pub struct Session(pub MerchantIdentity);

impl<S: Send + Sync> FromRequestParts<S> for Session {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<MerchantIdentity>()
            .cloned()
            .map(Session)
            .ok_or_else(ApiError::identity_unauthorized)
    }
}

// ---------------------------------------------------------------------------
// JWKS cache, shared by both providers
// ---------------------------------------------------------------------------

struct JwksCache {
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

impl JwksCache {
    /// Fetches the key set once, so a deployment with an unreachable
    /// provider fails at startup rather than on its first login.
    async fn fetch(jwks_url: String) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|error| format!("failed to build JWKS HTTP client: {error}"))?;
        let values = fetch_keys(&http, &jwks_url).await?;
        Ok(Self {
            jwks_url,
            http,
            keys: RwLock::new(CachedKeys {
                values,
                refreshed_at: Instant::now(),
                last_attempt: Instant::now() - JWKS_RETRY_BACKOFF,
            }),
            refresh_lock: Mutex::new(()),
        })
    }

    #[cfg(test)]
    fn preloaded(jwks_url: String, kid: &str, key: DecodingKey, refreshed_at: Instant) -> Self {
        Self {
            jwks_url,
            http: reqwest::Client::new(),
            keys: RwLock::new(CachedKeys {
                values: HashMap::from([(kid.into(), key)]),
                refreshed_at,
                last_attempt: Instant::now() - JWKS_RETRY_BACKOFF,
            }),
            refresh_lock: Mutex::new(()),
        }
    }

    /// The key a token names, refreshing the set when it is stale or the key
    /// is unknown; fails closed once the set has been unrefreshable for an
    /// hour, so a long provider outage cannot keep admitting tokens forever.
    async fn key(&self, kid: &str) -> Result<DecodingKey, ApiError> {
        self.refresh_if_needed(kid).await?;
        let cached = self.keys.read().await;
        if cached.refreshed_at.elapsed() > JWKS_OUTAGE_GRACE {
            return Err(ApiError::identity_unavailable());
        }
        cached
            .values
            .get(kid)
            .cloned()
            .ok_or_else(ApiError::identity_unauthorized)
    }

    async fn refresh_if_needed(&self, kid: &str) -> Result<(), ApiError> {
        {
            let cached = self.keys.read().await;
            if !refresh_is_due(&cached, kid) {
                return Ok(());
            }
        }

        let _refresh = self.refresh_lock.lock().await;
        {
            let cached = self.keys.read().await;
            if !refresh_is_due(&cached, kid) {
                return Ok(());
            }
        }

        self.keys.write().await.last_attempt = Instant::now();
        match fetch_keys(&self.http, &self.jwks_url).await {
            Ok(values) => {
                let mut cached = self.keys.write().await;
                cached.values = values;
                cached.refreshed_at = Instant::now();
                Ok(())
            }
            Err(error) => {
                tracing::warn!(%error, url = %self.jwks_url, "failed to refresh JWKS");
                if self.keys.read().await.refreshed_at.elapsed() > JWKS_OUTAGE_GRACE {
                    Err(ApiError::identity_unavailable())
                } else {
                    Ok(())
                }
            }
        }
    }
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
        .map_err(|error| format!("failed to fetch JWKS from {url}: {error}"))?
        .error_for_status()
        .map_err(|error| format!("JWKS at {url} returned an error: {error}"))?;
    let set: JwkSet = response
        .json()
        .await
        .map_err(|error| format!("invalid JWKS at {url}: {error}"))?;
    let mut keys = HashMap::new();
    for jwk in set.keys {
        if let Some(kid) = jwk.common.key_id.clone()
            && let Ok(key) = DecodingKey::from_jwk(&jwk)
        {
            keys.insert(kid, key);
        }
    }
    if keys.is_empty() {
        return Err(format!("JWKS at {url} contains no usable keys"));
    }
    Ok(keys)
}

// ---------------------------------------------------------------------------
// Auth0: the payer audience
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Auth0Verifier {
    inner: Arc<Auth0VerifierInner>,
}

struct Auth0VerifierInner {
    issuer: String,
    audience: String,
    client_id: String,
    jwks: JwksCache,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
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
        let issuer = format!("{}/", issuer.trim_end_matches('/'));
        let jwks = JwksCache::fetch(format!("{issuer}.well-known/jwks.json")).await?;
        Ok(Self {
            inner: Arc::new(Auth0VerifierInner {
                issuer,
                audience,
                client_id,
                jwks,
            }),
        })
    }

    /// A fresh proof: signature, registered claims, the one client this
    /// audience admits, the email-OTP method, and an authentication no older
    /// than five minutes — the whole point being that the mailbox was opened
    /// *just now*, not at some earlier session.
    pub async fn verify(&self, token: &str) -> Result<Identity, ApiError> {
        let claims = self.decode(token).await?;
        let now = unix_now()?;
        if claims.azp != self.inner.client_id
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
            authentication_event_id: claims.authentication_event_id,
            email: claims.email,
        })
    }

    async fn decode(&self, token: &str) -> Result<Claims, ApiError> {
        let header = decode_header(token).map_err(|_| ApiError::identity_unauthorized())?;
        if header.alg != Algorithm::RS256 {
            return Err(ApiError::identity_unauthorized());
        }
        let kid = header.kid.ok_or_else(ApiError::identity_unauthorized)?;
        let key = self.inner.jwks.key(&kid).await?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[&self.inner.audience]);
        validation.set_issuer(&[&self.inner.issuer]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub", "azp"]);
        validation.leeway = 30;
        decode::<Claims>(token, &key, &validation)
            .map(|data| data.claims)
            .map_err(|_| ApiError::identity_unauthorized())
    }

    #[cfg(test)]
    pub fn for_test(
        issuer: &str,
        audience: &str,
        client_id: &str,
        kid: &str,
        key: DecodingKey,
    ) -> Self {
        Self {
            inner: Arc::new(Auth0VerifierInner {
                issuer: format!("{}/", issuer.trim_end_matches('/')),
                audience: audience.into(),
                client_id: client_id.into(),
                jwks: JwksCache::preloaded(String::new(), kid, key, Instant::now()),
            }),
        }
    }
}

/// Payday's Auth0 Action stamps these claims only for its own email-OTP
/// flow; a token from any other connection carries no proven mailbox.
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

// ---------------------------------------------------------------------------
// Privy: merchant sessions
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct PrivyVerifier {
    inner: Arc<PrivyVerifierInner>,
}

struct PrivyVerifierInner {
    app_id: String,
    jwks: JwksCache,
}

/// The identity token's claims. `linked_accounts` is a JSON *string* — a
/// stringified array — exactly as Privy's own server SDK reads it.
#[derive(Deserialize)]
struct PrivyClaims {
    sub: String,
    iss: String,
    #[serde(default)]
    linked_accounts: Option<String>,
}

/// One entry of that array, in the lightweight shape the identity token
/// carries. Only the fields this service reads are named; anything else
/// (verification times, wallet ids, other account types) is ignored.
#[derive(Deserialize)]
struct LinkedAccount {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    chain_type: Option<String>,
    #[serde(default)]
    wallet_client_type: Option<String>,
}

impl PrivyVerifier {
    /// Where Privy publishes an app's verification key set.
    pub fn jwks_url(app_id: &str) -> String {
        format!("https://auth.privy.io/api/v1/apps/{app_id}/jwks.json")
    }

    pub async fn new(app_id: String) -> Result<Self, String> {
        validate_app_id(&app_id)?;
        let jwks = JwksCache::fetch(Self::jwks_url(&app_id)).await?;
        Ok(Self {
            inner: Arc::new(PrivyVerifierInner { app_id, jwks }),
        })
    }

    /// Signature, issuer, audience, expiry, and a subject that is a Privy
    /// DID; then the mailbox, which every merchant has because email is the
    /// only way in, and the embedded wallet, which they have once Privy has
    /// made it.
    pub async fn verify(&self, token: &str) -> Result<MerchantIdentity, ApiError> {
        let header = decode_header(token).map_err(|_| ApiError::identity_unauthorized())?;
        if header.alg != Algorithm::ES256 {
            return Err(ApiError::identity_unauthorized());
        }
        let kid = header.kid.ok_or_else(ApiError::identity_unauthorized)?;
        let key = self.inner.jwks.key(&kid).await?;

        let mut validation = Validation::new(Algorithm::ES256);
        validation.set_audience(&[&self.inner.app_id]);
        validation.set_issuer(&[PRIVY_ISSUER]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.leeway = 30;
        let claims = decode::<PrivyClaims>(token, &key, &validation)
            .map(|data| data.claims)
            .map_err(|_| ApiError::identity_unauthorized())?;
        merchant_from_claims(claims)
    }

    #[cfg(test)]
    pub fn for_test(app_id: &str, kid: &str, key: DecodingKey) -> Self {
        Self {
            inner: Arc::new(PrivyVerifierInner {
                app_id: app_id.into(),
                jwks: JwksCache::preloaded(String::new(), kid, key, Instant::now()),
            }),
        }
    }
}

/// Privy app ids are opaque identifiers that end up in a URL path; anything
/// outside this alphabet is a configuration mistake, not an app.
fn validate_app_id(app_id: &str) -> Result<(), String> {
    if app_id.is_empty()
        || app_id.len() > 64
        || !app_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("PAYDAY_PRIVY_APP_ID must be a Privy app id (letters, digits, - and _)".into());
    }
    Ok(())
}

fn merchant_from_claims(claims: PrivyClaims) -> Result<MerchantIdentity, ApiError> {
    if !claims.sub.starts_with(PRIVY_SUBJECT_PREFIX) || claims.sub.len() > 255 {
        return Err(ApiError::identity_unauthorized());
    }
    let accounts: Vec<LinkedAccount> = claims
        .linked_accounts
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default();
    let email = accounts
        .iter()
        .filter(|account| account.kind == "email")
        .filter_map(|account| account.address.as_deref())
        .map(|address| address.trim().to_lowercase())
        .find(|address| valid_email(address))
        .ok_or_else(ApiError::identity_unauthorized)?;
    // The embedded wallet, not any external wallet the user may also have
    // linked: it is the one Payday can call the merchant's own by default.
    let wallet_address = accounts
        .iter()
        .filter(|account| {
            account.kind == "wallet"
                && account.chain_type.as_deref() == Some("ethereum")
                && account.wallet_client_type.as_deref() == Some("privy")
        })
        .filter_map(|account| account.address.as_deref())
        .find_map(|address| Address::from_str(address.trim()).ok())
        .filter(|address| !address.is_zero())
        .map(|address| address.to_checksum(None));
    Ok(MerchantIdentity {
        issuer: claims.iss,
        subject: claims.sub,
        email,
        wallet_address,
    })
}

fn unix_now() -> Result<u64, ApiError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .map_err(|_| ApiError::identity_unauthorized())
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Authenticate a merchant request with either an API key or a dashboard
/// session (a Privy identity token). The bearer's prefix decides which: keys
/// are always `payday_live_…`/`payday_test_…`, tokens never are. Either way
/// the request runs as one account, rate limited as that account; a session
/// additionally carries its [`MerchantIdentity`] for the routes that need
/// more than an account id.
pub async fn require_account(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let supplied = bearer_from_request(&request).ok_or_else(ApiError::unauthorized)?;
    let (account, merchant) = if looks_like_api_key(supplied) {
        let account = state
            .accounts
            .authenticate(supplied)
            .await?
            .ok_or_else(ApiError::unauthorized)?;
        (account, None)
    } else {
        let (account, merchant) = session_account(&state, supplied).await?;
        (account, Some(merchant))
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
    if let Some(merchant) = merchant {
        request.extensions_mut().insert(merchant);
    }
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

/// The account behind a session, provisioned on first sight so a merchant
/// who signs in to the dashboard exists before they ever hold a key — and
/// with the wallet Privy reports kept current on every sight after.
async fn session_account(
    state: &AppState,
    token: &str,
) -> Result<(AccountId, MerchantIdentity), ApiError> {
    let verifier = state
        .merchant_verifier
        .as_ref()
        .ok_or_else(ApiError::unauthorized)?;
    let merchant = verifier.verify(token).await.map_err(|error| {
        // A JWKS outage is worth telling apart; every other failure is just
        // an invalid credential, exactly like a wrong key.
        if error.status == StatusCode::SERVICE_UNAVAILABLE {
            error
        } else {
            ApiError::unauthorized()
        }
    })?;
    let account = state
        .accounts
        .find_or_provision_by_identity(
            &merchant.issuer,
            &merchant.subject,
            &merchant.email,
            merchant.wallet_address.as_deref(),
        )
        .await
        .map_err(|error| match error {
            ProvisionAccountError::AccountDisabled => ApiError::account_disabled(),
            ProvisionAccountError::Database(error) => error.into(),
        })?;
    Ok((account, merchant))
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
pub mod testing {
    //! A Privy app with its own signing key, for tests that need to mint
    //! merchant sessions.

    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    use serde::Serialize;

    use super::*;

    pub const APP_ID: &str = "cltestappid0000000000000";
    pub const KID: &str = "privy-test-key";

    #[derive(Serialize)]
    struct TokenClaims<'a> {
        sub: &'a str,
        iss: &'a str,
        aud: &'a str,
        iat: u64,
        exp: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        linked_accounts: Option<String>,
    }

    pub struct PrivyApp {
        pub verifier: PrivyVerifier,
        key: EncodingKey,
    }

    impl PrivyApp {
        pub fn new() -> Self {
            let secret = p256::SecretKey::random(&mut rand_08::thread_rng());
            let private_pem = secret.to_pkcs8_pem(LineEnding::LF).unwrap();
            let public_pem = secret
                .public_key()
                .to_public_key_pem(LineEnding::LF)
                .unwrap();
            Self {
                verifier: PrivyVerifier::for_test(
                    APP_ID,
                    KID,
                    DecodingKey::from_ec_pem(public_pem.as_bytes()).unwrap(),
                ),
                key: EncodingKey::from_ec_pem(private_pem.as_bytes()).unwrap(),
            }
        }

        /// A session for `email` whose embedded wallet is `wallet`, alongside
        /// an external wallet the user linked themselves, which must be
        /// ignored.
        pub fn token(&self, subject: &str, email: &str, wallet: Option<&str>) -> String {
            let mut accounts = vec![
                serde_json::json!({"type": "email", "address": email, "lv": 1_700_000_000}),
                serde_json::json!({
                    "type": "wallet",
                    "address": "0x90F79bf6EB2c4f870365E785982E1f101E93b906",
                    "chain_type": "ethereum",
                    "wallet_client_type": "metamask",
                    "connector_type": "injected",
                    "lv": 1_700_000_000
                }),
            ];
            if let Some(wallet) = wallet {
                accounts.push(serde_json::json!({
                    "type": "wallet",
                    "id": "wallet-1",
                    "address": wallet,
                    "chain_type": "ethereum",
                    "wallet_client_type": "privy",
                    "connector_type": "embedded",
                    "lv": 1_700_000_000
                }));
            }
            self.token_with(
                subject,
                PRIVY_ISSUER,
                APP_ID,
                u64::MAX,
                Some(serde_json::Value::Array(accounts).to_string()),
            )
        }

        pub fn token_with(
            &self,
            subject: &str,
            issuer: &str,
            audience: &str,
            expires_at: u64,
            linked_accounts: Option<String>,
        ) -> String {
            let mut header = Header::new(Algorithm::ES256);
            header.kid = Some(KID.into());
            encode(
                &header,
                &TokenClaims {
                    sub: subject,
                    iss: issuer,
                    aud: audience,
                    iat: 1,
                    exp: expires_at,
                    linked_accounts,
                },
                &self.key,
            )
            .unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{APP_ID, PrivyApp};
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

    const WALLET: &str = "0x70997970c51812dc3a010c7d01b50e0d17dc79c8";
    const WALLET_CHECKSUMMED: &str = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";

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
                false,
            )
            .await
            .is_err()
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

    #[tokio::test]
    async fn privy_configuration_requires_a_plausible_app_id() {
        // Refused before any network access: an app id with a slash would
        // change which URL the key set is fetched from.
        for bad in ["", "app/../other", "app id", &"x".repeat(65)] {
            assert!(
                PrivyVerifier::new(bad.into()).await.is_err(),
                "{bad:?} should be refused"
            );
        }
        assert!(validate_app_id("cmt9wxn7h011h0cjsma7fzytr").is_ok());
        assert_eq!(
            PrivyVerifier::jwks_url("cmt9wxn7h011h0cjsma7fzytr"),
            "https://auth.privy.io/api/v1/apps/cmt9wxn7h011h0cjsma7fzytr/jwks.json"
        );
    }

    #[tokio::test]
    async fn privy_sessions_carry_the_mailbox_and_the_embedded_wallet() {
        let app = PrivyApp::new();
        let token = app.token("did:privy:abc123", "Merchant@Example.com", Some(WALLET));
        let merchant = app.verifier.verify(&token).await.unwrap();
        assert_eq!(merchant.issuer, "privy.io");
        assert_eq!(merchant.subject, "did:privy:abc123");
        assert_eq!(merchant.email, "merchant@example.com", "normalized");
        assert_eq!(
            merchant.wallet_address.as_deref(),
            Some(WALLET_CHECKSUMMED),
            "the embedded wallet, checksummed; the linked MetaMask wallet is ignored"
        );

        // Before Privy has created the wallet the session still stands.
        let without = app.token("did:privy:abc123", "merchant@example.com", None);
        let merchant = app.verifier.verify(&without).await.unwrap();
        assert!(merchant.wallet_address.is_none());
    }

    #[tokio::test]
    async fn privy_sessions_are_refused_without_the_right_claims() {
        let app = PrivyApp::new();
        let accounts = |email: Option<&str>| {
            let mut list = Vec::new();
            if let Some(email) = email {
                list.push(serde_json::json!({"type": "email", "address": email}));
            }
            Some(serde_json::Value::Array(list).to_string())
        };
        let good = accounts(Some("merchant@example.com"));
        for (name, token) in [
            (
                "another app",
                app.token_with(
                    "did:privy:x",
                    PRIVY_ISSUER,
                    "other-app",
                    u64::MAX,
                    good.clone(),
                ),
            ),
            (
                "another issuer",
                app.token_with(
                    "did:privy:x",
                    "https://issuer.example/",
                    APP_ID,
                    u64::MAX,
                    good.clone(),
                ),
            ),
            (
                "expired",
                app.token_with("did:privy:x", PRIVY_ISSUER, APP_ID, 1, good.clone()),
            ),
            (
                "not a Privy DID",
                app.token_with("email|user", PRIVY_ISSUER, APP_ID, u64::MAX, good.clone()),
            ),
            (
                "no mailbox",
                app.token_with(
                    "did:privy:x",
                    PRIVY_ISSUER,
                    APP_ID,
                    u64::MAX,
                    accounts(None),
                ),
            ),
            (
                "no linked accounts at all",
                app.token_with("did:privy:x", PRIVY_ISSUER, APP_ID, u64::MAX, None),
            ),
            (
                "unparseable linked accounts",
                app.token_with(
                    "did:privy:x",
                    PRIVY_ISSUER,
                    APP_ID,
                    u64::MAX,
                    Some("not json".into()),
                ),
            ),
            (
                "not an email",
                app.token_with(
                    "did:privy:x",
                    PRIVY_ISSUER,
                    APP_ID,
                    u64::MAX,
                    accounts(Some("nope")),
                ),
            ),
        ] {
            assert!(app.verifier.verify(&token).await.is_err(), "{name}");
        }

        // A token signed by someone else's key, and one for a key this app
        // never published, are both just invalid credentials.
        let other = PrivyApp::new();
        assert!(
            app.verifier
                .verify(&other.token("did:privy:x", "merchant@example.com", None))
                .await
                .is_err()
        );
    }

    fn verifier_and_key() -> (Auth0Verifier, EncodingKey) {
        let private = rsa::RsaPrivateKey::new(&mut rand_08::thread_rng(), 2048).unwrap();
        let private_pem = private.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_pem = private
            .to_public_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let decoding = DecodingKey::from_rsa_pem(public_pem.as_bytes()).unwrap();
        let verifier = Auth0Verifier::for_test(
            "https://issuer.example/",
            "https://api.payday.sh/payer",
            "payday-payer",
            "test-key",
            decoding,
        );
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
                azp: "payday-payer",
                authentication_method: EMAIL_OTP_METHOD,
                authentication_client_id: "payday-payer",
                authenticated_at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                authentication_event_id: "authentication-event",
                email: "payer@example.com",
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
            "https://api.payday.sh/payer",
            u64::MAX,
        );
        let identity = verifier.verify(&valid).await.unwrap();
        assert_eq!(identity.email, "payer@example.com");
        assert_eq!(identity.authentication_event_id, "authentication-event");

        for invalid in [
            token(
                &key,
                "https://wrong.example/",
                "https://api.payday.sh/payer",
                u64::MAX,
            ),
            token(&key, "https://issuer.example/", "wrong-audience", u64::MAX),
            token(
                &key,
                "https://issuer.example/",
                "https://api.payday.sh/payer",
                1,
            ),
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
                "payday-payer",
                EMAIL_OTP_METHOD,
                "payday-payer",
                now,
                "event",
            ),
            (
                "email|user",
                "other-client",
                EMAIL_OTP_METHOD,
                "payday-payer",
                now,
                "event",
            ),
            (
                "email|user",
                "payday-payer",
                EMAIL_OTP_METHOD,
                "other-client",
                now,
                "event",
            ),
            (
                "email|user",
                "payday-payer",
                "social",
                "payday-payer",
                now,
                "event",
            ),
            (
                "email|user",
                "payday-payer",
                EMAIL_OTP_METHOD,
                "payday-payer",
                now - 301,
                "event",
            ),
            (
                "email|user",
                "payday-payer",
                EMAIL_OTP_METHOD,
                "payday-payer",
                now + 31,
                "event",
            ),
            (
                "email|user",
                "payday-payer",
                EMAIL_OTP_METHOD,
                "payday-payer",
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
                    aud: "https://api.payday.sh/payer",
                    exp: u64::MAX,
                    nbf: 1,
                    azp,
                    authentication_method: method,
                    authentication_client_id: client_id,
                    authenticated_at,
                    authentication_event_id: event_id,
                    email: "payer@example.com",
                },
                &key,
            )
            .unwrap();
            assert!(verifier.verify(&token).await.is_err());
        }
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

    /// A verifier whose cached key set is a second past its refresh interval.
    fn stale_verifier(jwks_url: String, kid: &str, key: DecodingKey) -> Auth0Verifier {
        Auth0Verifier {
            inner: Arc::new(Auth0VerifierInner {
                issuer: "https://issuer.example/".into(),
                audience: "https://api.payday.sh/payer".into(),
                client_id: "payday-payer".into(),
                jwks: JwksCache::preloaded(
                    jwks_url,
                    kid,
                    key,
                    Instant::now() - JWKS_CACHE_TTL - Duration::from_secs(1),
                ),
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
            "https://api.payday.sh/payer",
            u64::MAX,
        );
        assert!(verifier.verify(&old_token).await.is_err());
        let new_token = token_with_kid(
            &new_encoding,
            "new-key",
            "https://issuer.example/",
            "https://api.payday.sh/payer",
            u64::MAX,
        );
        assert_eq!(
            verifier.verify(&new_token).await.unwrap().email,
            "payer@example.com"
        );
        assert_eq!(requests.await.unwrap(), 1);
    }

    #[tokio::test]
    async fn unknown_key_id_refreshes_a_fresh_jwks_cache() {
        let (_, _, old_decoding) = rsa_key("old-key");
        let (new_jwk, new_encoding, _) = rsa_key("new-key");
        let (url, requests) = jwks_server(new_jwk).await;
        let verifier = stale_verifier(url, "old-key", old_decoding);
        verifier.inner.jwks.keys.write().await.refreshed_at = Instant::now();
        let new_token = token_with_kid(
            &new_encoding,
            "new-key",
            "https://issuer.example/",
            "https://api.payday.sh/payer",
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
            "https://api.payday.sh/payer",
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
            "https://api.payday.sh/payer",
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
            .jwks
            .keys
            .get_mut()
            .refreshed_at = Instant::now() - JWKS_OUTAGE_GRACE - Duration::from_secs(1);
        let token = token_with_kid(
            &encoding,
            "old-key",
            "https://issuer.example/",
            "https://api.payday.sh/payer",
            u64::MAX,
        );
        let error = verifier.verify(&token).await.unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }
}
