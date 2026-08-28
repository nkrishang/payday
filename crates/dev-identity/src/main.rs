use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
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

const CLIENT_ID: &str = "payday-cli-local";
const AUDIENCE: &str = "payday-api-local";

#[derive(Clone)]
struct AppState {
    issuer: String,
    kid: String,
    encoding_key: Arc<EncodingKey>,
    jwks: serde_json::Value,
    otps: Arc<Mutex<HashMap<String, String>>>,
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
    }
}

fn router_with_state(state: AppState) -> Router {
    Router::new()
        .route("/passwordless/start", post(start))
        .route("/oauth/token", post(token))
        .route("/.well-known/jwks.json", get(jwks))
        .with_state(state)
}

async fn start(
    State(state): State<AppState>,
    Json(request): Json<StartRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if request.client_id != CLIENT_ID
        || request.connection != "email"
        || request.send != "code"
        || !request.email.contains('@')
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let otp = std::env::var("PAYDAY_DEV_IDENTITY_OTP")
        .unwrap_or_else(|_| format!("{:06}", rand::rng().random_range(0..1_000_000)));
    state
        .otps
        .lock()
        .await
        .insert(request.email.to_ascii_lowercase(), otp.clone());
    eprintln!("DEV IDENTITY OTP {} {}", request.email, otp);
    Ok(Json(serde_json::json!({})))
}

async fn token(
    State(state): State<AppState>,
    Json(request): Json<TokenRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if request.grant_type != "http://auth0.com/oauth/grant-type/passwordless/otp"
        || request.client_id != CLIENT_ID
        || request.realm != "email"
        || request.audience != AUDIENCE
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let valid = state
        .otps
        .lock()
        .await
        .remove(&request.username.to_ascii_lowercase());
    if valid.as_deref() != Some(&request.otp) {
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
        aud: AUDIENCE.into(),
        exp: now + 300,
        azp: CLIENT_ID.into(),
        method: "email_otp",
        client_id: CLIENT_ID.into(),
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

    #[tokio::test]
    async fn otp_is_single_use_and_token_matches_jwks() {
        let state = new_state("http://127.0.0.1:3001".into());
        let _ = start(
            State(state.clone()),
            Json(StartRequest {
                client_id: CLIENT_ID.into(),
                connection: "email".into(),
                email: "dev@example.com".into(),
                send: "code".into(),
            }),
        )
        .await
        .unwrap();
        let otp = state
            .otps
            .lock()
            .await
            .get("dev@example.com")
            .unwrap()
            .clone();
        let request = || TokenRequest {
            grant_type: "http://auth0.com/oauth/grant-type/passwordless/otp".into(),
            client_id: CLIENT_ID.into(),
            username: "dev@example.com".into(),
            otp: otp.clone(),
            realm: "email".into(),
            audience: AUDIENCE.into(),
        };
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
        let jwt = response["access_token"].as_str().unwrap();
        let set: JwkSet = serde_json::from_value(state.jwks.clone()).unwrap();
        let key =
            DecodingKey::from_jwk(set.find(&decode_header(jwt).unwrap().kid.unwrap()).unwrap())
                .unwrap();
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[AUDIENCE]);
        validation.set_issuer(&["http://127.0.0.1:3001/"]);
        assert!(decode::<serde_json::Value>(jwt, &key, &validation).is_ok());
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
