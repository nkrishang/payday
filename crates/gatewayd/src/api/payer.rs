use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy_primitives::{B256, U256};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{Html, IntoResponse, Response};
use gateway_core::{Invoice, InvoiceStatus, PayerPaymentResponse, PaymentResponse};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use qrcode::{QrCode, render::svg};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::state::AppState;

const ISSUER: &str = "payday-gateway";
const AUDIENCE: &str = "payer-payment-read";
const READ_GRACE_SECS: u64 = 30 * 24 * 60 * 60;

#[derive(Clone)]
pub struct PayerAccess {
    public_base_url: String,
    explorer_base_url: Option<String>,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    iss: String,
    aud: String,
    exp: u64,
}

#[derive(Deserialize)]
pub struct AccessQuery {
    token: String,
}

impl PayerAccess {
    pub fn new(
        public_base_url: impl Into<String>,
        explorer_base_url: Option<String>,
        secret: &[u8],
    ) -> Result<Self, String> {
        if secret.len() < 32 {
            return Err("PAYDAY_PAYER_TOKEN_SECRET must be at least 32 bytes".into());
        }
        let public_base_url = validate_base_url(public_base_url.into(), "public base URL", true)?;
        let explorer_base_url = explorer_base_url
            .map(|url| validate_base_url(url, "explorer base URL", false))
            .transpose()?;
        Ok(Self {
            public_base_url,
            explorer_base_url,
            encoding_key: EncodingKey::from_secret(secret),
            decoding_key: DecodingKey::from_secret(secret),
        })
    }

    pub fn payment_url(&self, invoice: &Invoice) -> Result<String, ApiError> {
        let token = encode(
            &Header::new(Algorithm::HS256),
            &Claims {
                sub: invoice.id.to_string(),
                iss: ISSUER.into(),
                aud: AUDIENCE.into(),
                exp: invoice.expiration_timestamp.saturating_add(READ_GRACE_SECS),
            },
            &self.encoding_key,
        )
        .map_err(|error| ApiError::internal(format!("failed to sign payment URL: {error}")))?;
        Ok(format!(
            "{}/pay/{}?token={token}",
            self.public_base_url, invoice.id
        ))
    }

    fn verify(&self, token: &str, id: &str) -> Result<(), ApiError> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_issuer(&[ISSUER]);
        validation.set_audience(&[AUDIENCE]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        let claims = decode::<Claims>(token, &self.decoding_key, &validation)
            .map_err(|_| ApiError::payer_unauthorized())?
            .claims;
        if claims.sub != id {
            return Err(ApiError::payer_unauthorized());
        }
        Ok(())
    }

    pub fn address_url(&self, address: &str) -> Option<String> {
        self.explorer_base_url
            .as_ref()
            .map(|base| format!("{base}/address/{address}"))
    }

    pub fn transaction_url(&self, hash: &str) -> Option<String> {
        self.explorer_base_url
            .as_ref()
            .map(|base| format!("{base}/tx/{hash}"))
    }
}

fn validate_base_url(
    mut value: String,
    name: &str,
    allow_local_http: bool,
) -> Result<String, String> {
    value = value.trim_end_matches('/').to_string();
    let parsed = reqwest::Url::parse(&value).map_err(|error| format!("invalid {name}: {error}"))?;
    let local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if parsed.cannot_be_a_base()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || (parsed.scheme() != "https" && !(allow_local_http && local && parsed.scheme() == "http"))
    {
        return Err(format!(
            "{name} must be an HTTPS origin without a query or fragment; local HTTP is allowed for the public URL"
        ));
    }
    Ok(value)
}

fn payment_uri(invoice: &Invoice, amount: U256) -> String {
    format!(
        "ethereum:{}@{}/transfer?address={}&uint256={}",
        invoice.token.0.to_checksum(None),
        invoice.chain_id.0,
        invoice.payment_address.0.to_checksum(None),
        amount,
    )
}

fn payment_state(invoice: &Invoice, now: u64) -> (U256, bool) {
    let remaining = invoice.amount.0.saturating_sub(invoice.received.0);
    let payable = invoice.status == InvoiceStatus::Created
        && now <= invoice.expiration_timestamp
        && !remaining.is_zero();
    (remaining, payable)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

fn payer_response(
    state: &AppState,
    invoice: Invoice,
    settlement_tx_hash: Option<String>,
) -> PayerPaymentResponse {
    let now = unix_now();
    let (remaining, payable) = payment_state(&invoice, now);
    let payment_uri = payable.then(|| payment_uri(&invoice, remaining));
    let payer_message = invoice.blocked_reason.as_ref().map(|_| {
        "Payout is paused, but your funds remain safe. The merchant and Payday support are resolving settlement; do not send a second payment.".into()
    });
    let response = PaymentResponse::from_invoice(invoice, None);
    let settlement_explorer_url = settlement_tx_hash
        .as_deref()
        .and_then(|hash| state.payer.transaction_url(hash));
    PayerPaymentResponse {
        id: response.id,
        chain: response.chain,
        token: response.token,
        amount: response.amount,
        amount_base_units: response.amount_base_units,
        received: response.received,
        received_base_units: response.received_base_units,
        remaining: response.remaining,
        remaining_base_units: remaining.to_string(),
        address_explorer_url: state.payer.address_url(&response.address),
        address: response.address,
        expires_at: response.expires_at,
        server_timestamp: now.to_string(),
        status: response.status,
        payable,
        payment_uri,
        settlement_tx_hash,
        settlement_explorer_url,
        payer_message,
    }
}

async fn authorized_invoice(
    state: &AppState,
    id: &str,
    token: &str,
) -> Result<(Invoice, Option<String>), ApiError> {
    let uuid = id
        .strip_prefix("pay_")
        .and_then(|value| Uuid::from_str(value).ok())
        .ok_or_else(ApiError::payer_unauthorized)?;
    state.payer.verify(token, id)?;
    let row = state
        .repo
        .find_by_id(uuid)
        .await?
        .ok_or_else(ApiError::payer_unauthorized)?;
    let settlement_tx_hash = row
        .settlement_tx_hash
        .as_deref()
        .map(B256::try_from)
        .transpose()
        .map_err(|_| ApiError::internal("invalid settlement transaction hash"))?
        .map(|hash| hash.to_string());
    Ok((Invoice::try_from(&row)?, settlement_tx_hash))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AccessQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let (invoice, settlement_tx_hash) = authorized_invoice(&state, &id, &query.token).await?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    Ok((
        headers,
        Json(payer_response(&state, invoice, settlement_tx_hash)),
    ))
}

pub async fn qr(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AccessQuery>,
) -> Result<Response, ApiError> {
    let (invoice, _) = authorized_invoice(&state, &id, &query.token).await?;
    let (remaining, payable) = payment_state(&invoice, unix_now());
    if !payable {
        return Err(ApiError::payment_not_payable());
    }
    let svg = QrCode::new(payment_uri(&invoice, remaining).as_bytes())
        .map_err(|error| ApiError::internal(format!("failed to render payment QR: {error}")))?
        .render::<svg::Color>()
        .min_dimensions(224, 224)
        .dark_color(svg::Color("#090811"))
        .light_color(svg::Color("#ffffff"))
        .build();
    Ok((
        [
            (header::CONTENT_TYPE, "image/svg+xml; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        svg,
    )
        .into_response())
}

pub async fn page() -> impl IntoResponse {
    (
        [
            (
                header::CACHE_CONTROL,
                "public, max-age=300, stale-while-revalidate=86400",
            ),
            (header::REFERRER_POLICY, "no-referrer"),
            (header::X_FRAME_OPTIONS, "DENY"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'self'; connect-src 'self'; img-src 'self'; style-src 'self'; script-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'",
            ),
        ],
        Html(include_str!("payer.html")),
    )
}

pub async fn css() -> impl IntoResponse {
    asset("text/css; charset=utf-8", include_str!("payer.css"))
}

pub async fn js() -> impl IntoResponse {
    asset("text/javascript; charset=utf-8", include_str!("payer.js"))
}

fn asset(content_type: &'static str, body: &'static str) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=3600"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        body,
    )
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{U256, address};
    use gateway_core::{
        Amount, BeneficiaryAddress, ChainId, FactoryAddress, Invoice, RecoveryAddress, TokenAddress,
    };

    use super::*;

    fn invoice() -> Invoice {
        Invoice::new(
            FactoryAddress(address!("0x0000000000000000000000000000000000000001")),
            ChainId(143),
            TokenAddress(address!("0x754704Bc059F8C67012fEd69BC8A327a5aafb603")),
            BeneficiaryAddress(address!("0x0000000000000000000000000000000000000002")),
            Amount(U256::from(1_500_000)),
            u64::MAX - READ_GRACE_SECS,
            RecoveryAddress(address!("0x0000000000000000000000000000000000000003")),
        )
    }

    #[test]
    fn token_is_scoped_to_one_invoice_and_payment_uri_is_eip_681() {
        let access = PayerAccess::new(
            "https://pay.payday.sh/",
            Some("https://monadvision.com/".into()),
            b"0123456789abcdef0123456789abcdef",
        )
        .unwrap();
        let invoice = invoice();
        let url = access.payment_url(&invoice).unwrap();
        let token = url.split("?token=").nth(1).unwrap();
        assert!(url.starts_with(&format!("https://pay.payday.sh/pay/{}", invoice.id)));
        assert!(access.verify(token, &invoice.id.to_string()).is_ok());
        assert!(
            access
                .verify(token, &format!("pay_{}", Uuid::now_v7()))
                .is_err()
        );
        assert_eq!(
            payment_uri(&invoice, invoice.amount.0),
            format!(
                "ethereum:0x754704Bc059F8C67012fEd69BC8A327a5aafb603@143/transfer?address={}&uint256=1500000",
                invoice.payment_address.0.to_checksum(None)
            )
        );
    }

    #[test]
    fn partial_payment_requests_only_the_remaining_amount_and_deadline_closes_it() {
        let mut invoice = invoice();
        invoice.received = Amount(U256::from(500_000));
        let (remaining, payable) = payment_state(&invoice, invoice.expiration_timestamp);
        assert_eq!(remaining, U256::from(1_000_000));
        assert!(payable);
        assert!(payment_uri(&invoice, remaining).ends_with("uint256=1000000"));

        assert!(!payment_state(&invoice, invoice.expiration_timestamp + 1).1);
        invoice.status = InvoiceStatus::Funded;
        assert!(!payment_state(&invoice, invoice.expiration_timestamp).1);
    }

    #[test]
    fn configuration_rejects_weak_secrets_and_insecure_remote_urls() {
        assert!(PayerAccess::new("https://pay.payday.sh", None, b"short").is_err());
        assert!(PayerAccess::new("http://pay.payday.sh", None, &[b'x'; 32]).is_err());
        assert!(PayerAccess::new("https://pay.payday.sh/base", None, &[b'x'; 32]).is_err());
        assert!(PayerAccess::new("https://user@pay.payday.sh", None, &[b'x'; 32]).is_err());
        assert!(PayerAccess::new("http://127.0.0.1:3000", None, &[b'x'; 32]).is_ok());
        assert!(
            PayerAccess::new(
                "https://pay.payday.sh",
                Some("http://monadvision.com".into()),
                &[b'x'; 32]
            )
            .is_err()
        );
    }
}
