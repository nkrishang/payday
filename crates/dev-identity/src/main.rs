use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rand::Rng;
use rsa::{
    RsaPrivateKey,
    pkcs8::{EncodePrivateKey, LineEnding},
    traits::PublicKeyParts,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{net::TcpListener, sync::Mutex};
use uuid::Uuid;

/// The CLI and the dashboard, mirroring the two Auth0 applications the
/// production merchant Post-Login Action admits. Both exchange an email OTP
/// for a token bound to the same API audience.
const CLI_CLIENT_ID: &str = "payday-cli-local";
const DASHBOARD_CLIENT_ID: &str = "payday-dashboard-local";
const CLIENT_IDS: [&str; 2] = [CLI_CLIENT_ID, DASHBOARD_CLIENT_ID];
const AUDIENCE: &str = "payday-api-local";
/// The payer application and its own audience, mirroring the production
/// payer Action: gatewayd exchanges a payer's code here, and the token it
/// gets back is good for nothing but unlocking an invoice.
const PAYER_CLIENT_ID: &str = "payday-payer-local";
const PAYER_AUDIENCE: &str = "payday-payer-local";

/// How long an emailed code stays usable, matching the window Auth0's
/// passwordless connection is configured for in `auth0/passwordless.tf`. The
/// dashboard counts the same five minutes down before it offers another code,
/// so a code is never dead on the page while it still works here, or the
/// reverse.
const EMAIL_OTP_TTL: Duration = Duration::from_secs(300);

/// The audience a client may request, if any. A merchant client cannot mint
/// a payer token and the payer client cannot reach the merchant API.
fn audience_for(client_id: &str) -> Option<&'static str> {
    if CLIENT_IDS.contains(&client_id) {
        Some(AUDIENCE)
    } else if client_id == PAYER_CLIENT_ID {
        Some(PAYER_AUDIENCE)
    } else {
        None
    }
}

#[derive(Clone)]
struct AppState {
    issuer: String,
    kid: String,
    encoding_key: Arc<EncodingKey>,
    jwks: serde_json::Value,
    // Auth0 passwordless transactions belong to the application that started
    // them. Keep the audience in the key as well so this remains safe if a
    // local client is ever allowed to address more than one API.
    otps: Arc<Mutex<HashMap<(String, String, String), PendingOtp>>>,
    otp_ttl: Duration,
}

/// One emailed code, and the moment it stops being one.
struct PendingOtp {
    code: String,
    expires_at: Instant,
}

#[derive(Deserialize)]
struct StartRequest {
    client_id: String,
    connection: String,
    email: String,
    send: String,
}

#[derive(Deserialize)]
struct TokenRequest {
    grant_type: String,
    client_id: String,
    username: String,
    otp: String,
    realm: String,
    audience: String,
}

#[derive(Serialize)]
struct Claims {
    sub: String,
    iss: String,
    aud: String,
    exp: u64,
    azp: String,
    #[serde(rename = "https://api.payday.sh/auth/method")]
    method: &'static str,
    #[serde(rename = "https://api.payday.sh/auth/client_id")]
    client_id: String,
    #[serde(rename = "https://api.payday.sh/auth/authenticated_at")]
    authenticated_at: u64,
    #[serde(rename = "https://api.payday.sh/auth/event_id")]
    event_id: String,
    #[serde(rename = "https://api.payday.sh/auth/email")]
    email: String,
}

fn router(issuer: String) -> Router {
    router_with_state(new_state(issuer))
}

fn new_state(issuer: String) -> AppState {
    let private =
        RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).expect("generate local RSA key");
    let pem = private
        .to_pkcs8_pem(LineEnding::LF)
        .expect("encode local RSA key");
    let public = private.to_public_key();
    let modulus = URL_SAFE_NO_PAD.encode(public.n().to_bytes_be());
    let exponent = URL_SAFE_NO_PAD.encode(public.e().to_bytes_be());
    let thumbprint = format!(r#"{{"e":"{exponent}","kty":"RSA","n":"{modulus}"}}"#);
    let kid = URL_SAFE_NO_PAD.encode(Sha256::digest(thumbprint));
    let jwks = serde_json::json!({"keys": [{
        "kty": "RSA", "use": "sig", "alg": "RS256", "kid": kid,
        "n": modulus, "e": exponent
    }]});
    AppState {
        issuer: format!("{}/", issuer.trim_end_matches('/')),
        kid,
        encoding_key: Arc::new(
            EncodingKey::from_rsa_pem(pem.as_bytes()).expect("load local RSA key"),
        ),
        jwks,
        otps: Arc::new(Mutex::new(HashMap::new())),
        otp_ttl: EMAIL_OTP_TTL,
    }
}

fn router_with_state(state: AppState) -> Router {
    Router::new()
        .route("/passwordless/start", post(start))
        .route("/oauth/token", post(token))
        .route("/.well-known/jwks.json", get(jwks))
        .layer(middleware::from_fn(cors))
        .with_state(state)
}

/// The dashboard calls the passwordless endpoints from the browser, exactly as
/// it calls Auth0 in production. Auth0 allows the SPA's origins; this provider
/// only ever listens on loopback, so allowing any origin exposes nothing that
/// was not already reachable from the developer's machine.
async fn cors(request: Request, next: Next) -> Response {
    let mut response = if request.method() == Method::OPTIONS {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type"),
    );
    response
}

async fn start(
    State(state): State<AppState>,
    Json(request): Json<StartRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let Some(audience) = audience_for(&request.client_id) else {
        return Err(StatusCode::BAD_REQUEST);
    };
    if request.connection != "email" || request.send != "code" || !request.email.contains('@') {
        return Err(StatusCode::BAD_REQUEST);
    }
    let otp = std::env::var("PAYDAY_DEV_IDENTITY_OTP")
        .unwrap_or_else(|_| format!("{:06}", rand::rng().random_range(0..1_000_000)));
    state.otps.lock().await.insert(
        (
            request.client_id,
            audience.to_owned(),
            request.email.to_ascii_lowercase(),
        ),
        PendingOtp {
            code: otp.clone(),
            expires_at: Instant::now() + state.otp_ttl,
        },
    );
    eprintln!("DEV IDENTITY OTP {} {}", request.email, otp);
    Ok(Json(serde_json::json!({})))
}

async fn token(
    State(state): State<AppState>,
    Json(request): Json<TokenRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if request.grant_type != "http://auth0.com/oauth/grant-type/passwordless/otp"
        || request.realm != "email"
        || audience_for(&request.client_id) != Some(request.audience.as_str())
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    // Like Auth0, a wrong code is refused without spending the right one;
    // the right one is spent on use, and one past its window is spent whether
    // or not the digits were right — an expired code is not a code.
    let username = request.username.to_ascii_lowercase();
    let otp_key = (
        request.client_id.clone(),
        request.audience.clone(),
        username.clone(),
    );
    let mut otps = state.otps.lock().await;
    let now = Instant::now();
    let (accepted, expired) = match otps.get(&otp_key) {
        None => (false, false),
        Some(pending) => (
            pending.expires_at > now && pending.code == request.otp,
            pending.expires_at <= now,
        ),
    };
    if accepted || expired {
        otps.remove(&otp_key);
    }
    drop(otps);
    if !accepted {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let email = request.username.to_ascii_lowercase();
    let subject = URL_SAFE_NO_PAD.encode(Sha256::digest(&email));
    let claims = Claims {
        sub: format!("email|{subject}"),
        iss: state.issuer.clone(),
        aud: request.audience,
        exp: now + 300,
        azp: request.client_id.clone(),
        method: "email_otp",
        client_id: request.client_id,
        authenticated_at: now,
        event_id: Uuid::now_v7().to_string(),
        email,
    };
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(state.kid.clone());
    let access_token = encode(&header, &claims, &state.encoding_key)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        serde_json::json!({"access_token": access_token, "token_type": "Bearer", "expires_in": 300}),
    ))
}

async fn jwks(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(state.jwks)
}

#[tokio::main]
async fn main() {
    let bind =
        std::env::var("PAYDAY_DEV_IDENTITY_BIND").unwrap_or_else(|_| "127.0.0.1:3001".into());
    let bind: SocketAddr = bind
        .parse()
        .expect("PAYDAY_DEV_IDENTITY_BIND must be a socket address");
    assert!(
        bind.ip().is_loopback(),
        "development identity provider must bind to a loopback address"
    );
    let issuer =
        std::env::var("PAYDAY_DEV_IDENTITY_ISSUER").unwrap_or_else(|_| format!("http://{bind}"));
    let listener = TcpListener::bind(&bind)
        .await
        .expect("bind dev identity provider");
    eprintln!("payday development identity provider listening on {bind} (never use in production)");
    axum::serve(listener, router(issuer))
        .await
        .expect("serve dev identity provider");
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{DecodingKey, Validation, decode, decode_header, jwk::JwkSet};

    /// Requests a code for `email` and returns the OTP the provider logged.
    async fn issue_otp(state: &AppState, client_id: &str, email: &str) -> String {
        let _ = start(
            State(state.clone()),
            Json(StartRequest {
                client_id: client_id.into(),
                connection: "email".into(),
                email: email.into(),
                send: "code".into(),
            }),
        )
        .await
        .unwrap();
        state
            .otps
            .lock()
            .await
            .get(&(
                client_id.to_owned(),
                audience_for(client_id).unwrap().to_owned(),
                email.to_ascii_lowercase(),
            ))
            .unwrap()
            .code
            .clone()
    }

    fn token_request(client_id: &str, email: &str, otp: &str) -> TokenRequest {
        token_request_for(client_id, email, otp, AUDIENCE)
    }

    fn token_request_for(client_id: &str, email: &str, otp: &str, audience: &str) -> TokenRequest {
        TokenRequest {
            grant_type: "http://auth0.com/oauth/grant-type/passwordless/otp".into(),
            client_id: client_id.into(),
            username: email.into(),
            otp: otp.into(),
            realm: "email".into(),
            audience: audience.into(),
        }
    }

    /// Decodes a token against the provider's own JWKS with the CLI's validation rules.
    fn verified_claims(state: &AppState, jwt: &str) -> serde_json::Value {
        verified_claims_for(state, jwt, AUDIENCE)
    }

    fn verified_claims_for(state: &AppState, jwt: &str, audience: &str) -> serde_json::Value {
        let set: JwkSet = serde_json::from_value(state.jwks.clone()).unwrap();
        let key =
            DecodingKey::from_jwk(set.find(&decode_header(jwt).unwrap().kid.unwrap()).unwrap())
                .unwrap();
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[audience]);
        validation.set_issuer(&["http://127.0.0.1:3001/"]);
        decode::<serde_json::Value>(jwt, &key, &validation)
            .unwrap()
            .claims
    }

    #[tokio::test]
    async fn otp_is_single_use_and_token_matches_jwks() {
        let state = new_state("http://127.0.0.1:3001".into());
        let otp = issue_otp(&state, CLI_CLIENT_ID, "dev@example.com").await;
        let request = || token_request(CLI_CLIENT_ID, "dev@example.com", &otp);
        // A wrong code does not spend the right one.
        assert_eq!(
            token(
                State(state.clone()),
                Json(token_request(CLI_CLIENT_ID, "dev@example.com", "000000")),
            )
            .await
            .unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
        let response = token(State(state.clone()), Json(request()))
            .await
            .unwrap()
            .0;
        assert_eq!(
            token(State(state.clone()), Json(request()))
                .await
                .unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
        let claims = verified_claims(&state, response["access_token"].as_str().unwrap());
        assert_eq!(claims["azp"], CLI_CLIENT_ID);
        assert_eq!(
            claims["https://api.payday.sh/auth/client_id"],
            CLI_CLIENT_ID
        );
    }

    #[tokio::test]
    async fn an_expired_code_is_refused_and_cannot_be_retried() {
        // Nothing here sleeps for five minutes: the window is state, so a
        // provider whose codes are born expired proves the same rule.
        let mut state = new_state("http://127.0.0.1:3001".into());
        state.otp_ttl = Duration::ZERO;
        let otp = issue_otp(&state, DASHBOARD_CLIENT_ID, "merchant@example.com").await;
        let request = || token_request(DASHBOARD_CLIENT_ID, "merchant@example.com", &otp);

        assert_eq!(
            token(State(state.clone()), Json(request()))
                .await
                .unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
        // And it is gone rather than merely refused, so a later attempt with
        // the same digits cannot succeed either.
        assert!(state.otps.lock().await.is_empty());
        assert_eq!(
            token(State(state.clone()), Json(request()))
                .await
                .unwrap_err(),
            StatusCode::UNAUTHORIZED
        );

        // A fresh code from the same mailbox still works.
        state.otp_ttl = EMAIL_OTP_TTL;
        let fresh = issue_otp(&state, DASHBOARD_CLIENT_ID, "merchant@example.com").await;
        assert!(
            token(
                State(state.clone()),
                Json(token_request(
                    DASHBOARD_CLIENT_ID,
                    "merchant@example.com",
                    &fresh,
                )),
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn dashboard_client_tokens_verify_against_the_jwks_with_dashboard_azp() {
        let state = new_state("http://127.0.0.1:3001".into());
        let otp = issue_otp(&state, DASHBOARD_CLIENT_ID, "merchant@example.com").await;
        let response = token(
            State(state.clone()),
            Json(token_request(
                DASHBOARD_CLIENT_ID,
                "merchant@example.com",
                &otp,
            )),
        )
        .await
        .unwrap()
        .0;

        let claims = verified_claims(&state, response["access_token"].as_str().unwrap());
        assert_eq!(claims["aud"], AUDIENCE);
        assert_eq!(claims["azp"], DASHBOARD_CLIENT_ID);
        assert_eq!(
            claims["https://api.payday.sh/auth/client_id"],
            DASHBOARD_CLIENT_ID
        );
        assert_eq!(claims["https://api.payday.sh/auth/method"], "email_otp");
        assert_eq!(
            claims["https://api.payday.sh/auth/email"],
            "merchant@example.com"
        );
    }

    #[tokio::test]
    async fn unknown_client_cannot_start_or_exchange_a_code() {
        let state = new_state("http://127.0.0.1:3001".into());
        let denied = start(
            State(state.clone()),
            Json(StartRequest {
                client_id: "payday-other-local".into(),
                connection: "email".into(),
                email: "dev@example.com".into(),
                send: "code".into(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(denied, StatusCode::BAD_REQUEST);

        // A code issued to one client is not exchangeable by a client the
        // provider does not know, even with the right OTP.
        let otp = issue_otp(&state, CLI_CLIENT_ID, "dev@example.com").await;
        let denied = token(
            State(state.clone()),
            Json(token_request("payday-other-local", "dev@example.com", &otp)),
        )
        .await
        .unwrap_err();
        assert_eq!(denied, StatusCode::BAD_REQUEST);

        // Known applications sharing the merchant audience still own distinct
        // passwordless transactions.
        let denied = token(
            State(state.clone()),
            Json(token_request(DASHBOARD_CLIENT_ID, "dev@example.com", &otp)),
        )
        .await
        .unwrap_err();
        assert_eq!(denied, StatusCode::UNAUTHORIZED);

        // The failed cross-client exchange did not spend the CLI's code.
        let _ = token(
            State(state),
            Json(token_request(CLI_CLIENT_ID, "dev@example.com", &otp)),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn payer_client_gets_the_payer_audience_and_nothing_else() {
        let state = new_state("http://127.0.0.1:3001".into());
        let otp = issue_otp(&state, PAYER_CLIENT_ID, "payer@example.com").await;
        // The payer client cannot mint a merchant token, even with its code.
        let denied = token(
            State(state.clone()),
            Json(token_request(PAYER_CLIENT_ID, "payer@example.com", &otp)),
        )
        .await
        .unwrap_err();
        assert_eq!(denied, StatusCode::BAD_REQUEST);

        let response = token(
            State(state.clone()),
            Json(token_request_for(
                PAYER_CLIENT_ID,
                "payer@example.com",
                &otp,
                PAYER_AUDIENCE,
            )),
        )
        .await
        .unwrap()
        .0;
        let claims = verified_claims_for(
            &state,
            response["access_token"].as_str().unwrap(),
            PAYER_AUDIENCE,
        );
        assert_eq!(claims["aud"], PAYER_AUDIENCE);
        assert_eq!(claims["azp"], PAYER_CLIENT_ID);
        assert_eq!(
            claims["https://api.payday.sh/auth/client_id"],
            PAYER_CLIENT_ID
        );
        assert_eq!(claims["https://api.payday.sh/auth/method"], "email_otp");
        assert_eq!(
            claims["https://api.payday.sh/auth/email"],
            "payer@example.com"
        );

        // And a merchant client cannot request the payer audience.
        let otp = issue_otp(&state, CLI_CLIENT_ID, "dev@example.com").await;
        let denied = token(
            State(state.clone()),
            Json(token_request_for(
                CLI_CLIENT_ID,
                "dev@example.com",
                &otp,
                PAYER_AUDIENCE,
            )),
        )
        .await
        .unwrap_err();
        assert_eq!(denied, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn browser_preflight_and_responses_allow_any_origin() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = router(base.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let http = reqwest::Client::new();

        let preflight = http
            .request(
                reqwest::Method::OPTIONS,
                format!("{base}/passwordless/start"),
            )
            .header("origin", "http://127.0.0.1:3002")
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "content-type")
            .send()
            .await
            .unwrap();
        assert_eq!(preflight.status(), reqwest::StatusCode::NO_CONTENT);
        assert_eq!(preflight.headers()["access-control-allow-origin"], "*");
        assert_eq!(
            preflight.headers()["access-control-allow-methods"],
            "GET, POST"
        );
        assert_eq!(
            preflight.headers()["access-control-allow-headers"],
            "content-type"
        );

        let started = http
            .post(format!("{base}/passwordless/start"))
            .header("origin", "http://127.0.0.1:3002")
            .json(&serde_json::json!({
                "client_id": DASHBOARD_CLIENT_ID, "connection": "email",
                "email": "merchant@example.com", "send": "code"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(started.status(), reqwest::StatusCode::OK);
        assert_eq!(started.headers()["access-control-allow-origin"], "*");
    }

    #[test]
    fn restarted_provider_advertises_a_new_key_id() {
        let before = new_state("http://127.0.0.1:3001".into());
        let after = new_state("http://127.0.0.1:3001".into());

        assert_ne!(before.kid, after.kid);
        assert_eq!(before.jwks["keys"][0]["kid"], before.kid);
        assert_eq!(after.jwks["keys"][0]["kid"], after.kid);
    }
}
