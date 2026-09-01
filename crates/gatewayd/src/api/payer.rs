use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy_primitives::{B256, U256};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use gateway_core::{
    AttachmentDescriptor, Invoice, InvoiceId, InvoiceStatus, PayerInvoiceDetails,
    PayerPaymentResponse, PayerPolicyResponse, PaymentResponse, VerificationRequirementsResponse,
    masked_email,
};
use gateway_db::DbAttachment;
use qrcode::{QrCode, render::svg};
use uuid::Uuid;

use crate::api::attachments::{no_store, signed_descriptor};
use crate::api::error::ApiError;
use crate::state::AppState;

#[derive(Clone)]
pub struct PayerAccess {
    public_base_url: String,
    /// `public_base_url` as a header value, for the merchant routes' CORS
    /// allow-list; built once so the router never has to fail.
    origin: HeaderValue,
    explorer_base_url: Option<String>,
}

impl PayerAccess {
    pub fn new(
        public_base_url: impl Into<String>,
        explorer_base_url: Option<String>,
    ) -> Result<Self, String> {
        let public_base_url = validate_base_url(public_base_url.into(), "public base URL", true)?;
        let origin = HeaderValue::from_str(&public_base_url)
            .map_err(|_| "public base URL must be a valid header value".to_string())?;
        let explorer_base_url = explorer_base_url
            .map(|url| validate_base_url(url, "explorer base URL", false))
            .transpose()?;
        Ok(Self {
            public_base_url,
            origin,
            explorer_base_url,
        })
    }

    /// The one browser origin the merchant API answers to: the dashboard is
    /// served from the public base URL, whose origin this is (a validated
    /// scheme and host with no path, query, or fragment).
    pub fn origin(&self) -> &str {
        &self.public_base_url
    }

    pub fn origin_header(&self) -> HeaderValue {
        self.origin.clone()
    }

    /// Link to the hosted checkout for one payment. Its origin is a different
    /// service, so this is the one place that knows where the checkout lives.
    pub fn checkout_url(&self, id: &InvoiceId) -> String {
        format!("{}/pay/{id}", self.public_base_url)
    }

    pub fn payment_url(&self, invoice: &Invoice) -> Result<String, ApiError> {
        Ok(self.checkout_url(&invoice.id))
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

/// What one payer may see of an invoice. Payer sessions arrive in Slice 3;
/// until then only permissionless invoices unlock their content.
pub struct PayerInvoiceAccess {
    pub invoice: Invoice,
    pub settlement_tx_hash: Option<String>,
    pub verification_completed: bool,
    pub content_unlocked: bool,
    /// Fetched only when the content is unlocked.
    pub attachment: Option<DbAttachment>,
}

/// One `content_unlocked` decision drives every gated field; no field is
/// guarded on its own. The settlement transaction is gated too: it is a
/// public on-chain record naming the payment address, the amount, and the
/// merchant's payout address, which is exactly what a locked invoice
/// withholds.
fn payer_response(state: &AppState, access: PayerInvoiceAccess) -> PayerPaymentResponse {
    let PayerInvoiceAccess {
        invoice,
        settlement_tx_hash,
        verification_completed,
        content_unlocked: unlocked,
        attachment,
    } = access;
    let now = unix_now();
    let (remaining, payable) = payment_state(&invoice, now);
    let payment_uri = (unlocked && payable).then(|| payment_uri(&invoice, remaining));
    let payer_message = invoice.blocked_reason.as_ref().map(|_| {
        "Payout is paused, but your funds remain safe. The merchant and Payday support are resolving settlement; do not send a second payment.".into()
    });
    let policy = &invoice.issuance_snapshot.payer_policy;
    let mode = policy.mode();
    let payer_policy = PayerPolicyResponse {
        mode,
        expected_email_hint: policy.expected_email().map(masked_email),
    };
    let requirements = VerificationRequirementsResponse::for_mode(mode, verification_completed);
    let response = PaymentResponse::from_invoice(invoice, None);
    let settlement_tx_hash = settlement_tx_hash.filter(|_| unlocked);
    let settlement_explorer_url = settlement_tx_hash
        .as_deref()
        .and_then(|hash| state.payer.transaction_url(hash));
    let gated = |value: String| unlocked.then_some(value);
    PayerPaymentResponse {
        id: response.id,
        issuer_name: response.issuer.name,
        heading: response.heading,
        payer_policy,
        requirements,
        status: response.status,
        payable,
        expires_at: response.expires_at,
        server_timestamp: now.to_string(),
        settlement_tx_hash,
        settlement_explorer_url,
        payer_message,
        content_unlocked: unlocked,
        chain: unlocked.then_some(response.chain),
        token: unlocked.then_some(response.token),
        amount: gated(response.amount.clone()),
        amount_base_units: gated(response.amount_base_units.clone()),
        received: gated(response.received),
        received_base_units: gated(response.received_base_units),
        remaining: gated(response.remaining),
        remaining_base_units: gated(remaining.to_string()),
        address_explorer_url: unlocked
            .then(|| state.payer.address_url(&response.address))
            .flatten(),
        address: gated(response.address),
        payment_uri,
        invoice: unlocked.then(|| PayerInvoiceDetails {
            amount: response.amount,
            amount_base_units: response.amount_base_units,
            bill_to: response.bill_to,
            notes: response.notes,
            reference: response.reference,
            attachment: attachment.as_ref().and_then(DbAttachment::descriptor),
        }),
    }
}

fn parse_invoice_id(id: &str) -> Result<Uuid, ApiError> {
    id.strip_prefix("pay_")
        .and_then(|value| Uuid::from_str(value).ok())
        .ok_or_else(ApiError::payer_unauthorized)
}

pub async fn authorized_invoice(
    state: &AppState,
    id: &str,
) -> Result<PayerInvoiceAccess, ApiError> {
    let uuid = parse_invoice_id(id)?;
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
    let invoice = Invoice::try_from(&row)?;
    let content_unlocked = !invoice.issuance_snapshot.payer_policy.mode().is_gated();
    let attachment = if content_unlocked && invoice.issuance_snapshot.attachment.is_some() {
        state.attachments.find_by_invoice(row.id).await?
    } else {
        None
    };
    Ok(PayerInvoiceAccess {
        invoice,
        settlement_tx_hash,
        verification_completed: row.verification_completed_at.is_some(),
        content_unlocked,
        attachment,
    })
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let access = authorized_invoice(&state, &id).await?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    Ok((headers, Json(payer_response(&state, access))))
}

pub async fn qr(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let access = authorized_invoice(&state, &id).await?;
    if !access.content_unlocked {
        return Err(ApiError::verification_required());
    }
    let (remaining, payable) = payment_state(&access.invoice, unix_now());
    if !payable {
        return Err(ApiError::payment_not_payable());
    }
    let svg = QrCode::new(payment_uri(&access.invoice, remaining).as_bytes())
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

/// The invoice's PDF for a payer who may see the content, with a fresh
/// signed link. Locked content answers exactly like the QR: verify first.
pub async fn attachment(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(HeaderMap, Json<AttachmentDescriptor>), ApiError> {
    let access = authorized_invoice(&state, &id).await?;
    if !access.content_unlocked {
        return Err(ApiError::verification_required());
    }
    let attachment = access
        .attachment
        .ok_or_else(ApiError::attachment_not_found)?;
    Ok((
        no_store(),
        Json(signed_descriptor(&state, &attachment).await?),
    ))
}

/// Permanently redirect a payer link to the hosted checkout.
///
/// The checkout is served from `PAYDAY_PUBLIC_BASE_URL`, which is a different
/// origin from this service, so every link ever shared keeps working after the
/// page moved. The id is parsed strictly before it is interpolated: without
/// that check an arbitrary path segment would end up in a `Location` header.
pub async fn page(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let location = state.payer.checkout_url(&InvoiceId(parse_invoice_id(&id)?));
    let location =
        HeaderValue::from_str(&location).map_err(|_| ApiError::internal("invalid payer link"))?;
    Ok((
        StatusCode::MOVED_PERMANENTLY,
        [
            (header::LOCATION, location),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            ),
            (
                header::REFERRER_POLICY,
                HeaderValue::from_static("no-referrer"),
            ),
        ],
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{U256, address};
    use gateway_core::{
        Amount, BeneficiaryAddress, CanonicalIssuanceSnapshot, ChainId, FactoryAddress, Invoice,
        Party, PayerPolicy, RecoveryAddress, TokenAddress,
    };

    use super::*;

    fn invoice() -> Invoice {
        let factory = FactoryAddress(address!("0x0000000000000000000000000000000000000001"));
        let token = TokenAddress(address!("0x754704Bc059F8C67012fEd69BC8A327a5aafb603"));
        let beneficiary =
            BeneficiaryAddress(address!("0x0000000000000000000000000000000000000002"));
        let amount = Amount(U256::from(1_500_000));
        let recovery = RecoveryAddress(address!("0x0000000000000000000000000000000000000003"));
        let party = |name: &str| Party {
            name: name.into(),
            email: None,
            details: None,
        };
        let snapshot = CanonicalIssuanceSnapshot::new(
            party("Acme"),
            party("Globex"),
            PayerPolicy::Permissionless,
            factory,
            ChainId(143),
            token,
            beneficiary,
            amount,
            u64::MAX / 2,
            recovery,
        );
        Invoice::issue(
            factory,
            ChainId(143),
            token,
            beneficiary,
            amount,
            u64::MAX / 2,
            recovery,
            snapshot,
        )
        .unwrap()
    }

    #[test]
    fn payment_url_is_tokenless_and_payment_uri_is_eip_681() {
        let access = PayerAccess::new(
            "https://pay.payday.sh/",
            Some("https://monadvision.com/".into()),
        )
        .unwrap();
        let invoice = invoice();
        let url = access.payment_url(&invoice).unwrap();
        assert_eq!(url, format!("https://pay.payday.sh/pay/{}", invoice.id));
        assert!(!url.contains("token"));
        assert_eq!(access.origin(), "https://pay.payday.sh");
        assert_eq!(access.origin_header(), "https://pay.payday.sh");
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
    fn configuration_rejects_insecure_remote_urls() {
        assert!(PayerAccess::new("http://pay.payday.sh", None).is_err());
        assert!(PayerAccess::new("https://pay.payday.sh/base", None).is_err());
        assert!(PayerAccess::new("https://user@pay.payday.sh", None).is_err());
        assert!(PayerAccess::new("http://127.0.0.1:3000", None).is_ok());
        assert!(
            PayerAccess::new(
                "https://pay.payday.sh",
                Some("http://monadvision.com".into()),
            )
            .is_err()
        );
    }
}
