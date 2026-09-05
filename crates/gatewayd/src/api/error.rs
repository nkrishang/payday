use axum::Json;
use axum::http::header::WWW_AUTHENTICATE;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// Stable, machine-readable error codes for the API.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: "A valid bearer API key or dashboard access token is required".into(),
        }
    }

    pub fn identity_unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "identity_unauthorized",
            message: "A valid Auth0 access token is required".into(),
        }
    }
    pub fn admin_unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "admin_unauthorized",
            message: "A valid operator bearer credential is required".into(),
        }
    }
    pub fn invoice_not_blocked() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "invoice_not_blocked",
            message: "Invoice is not blocked".into(),
        }
    }

    pub fn payer_unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "invalid_payment_link",
            message: "This payment link is invalid or expired".into(),
        }
    }

    pub fn identity_unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "identity_unavailable",
            message: "Account authentication is temporarily unavailable".into(),
        }
    }

    pub fn authentication_event_already_used() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "authentication_event_already_used",
            message: "This authentication event was already used; authenticate again".into(),
        }
    }

    pub fn account_not_provisioned() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "account_not_provisioned",
            message: "This identity does not have an account".into(),
        }
    }

    pub fn account_contact_required() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "account_contact_required",
            message: "A verified merchant email is required before creating a payment; run `payday login` to authenticate again".into(),
        }
    }

    pub fn api_key_generation_conflict() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "api_key_generation_conflict",
            message: "The API key changed after confirmation; authenticate and try again".into(),
        }
    }

    pub fn account_disabled() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "account_disabled",
            message: "This account is disabled".into(),
        }
    }

    pub fn missing_idempotency_key() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "missing_idempotency_key",
            message: "Idempotency-Key header is required".into(),
        }
    }

    pub fn invalid_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            message: msg.into(),
        }
    }

    pub fn invalid_amount(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_amount",
            message: msg.into(),
        }
    }

    pub fn unsupported_chain() -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "unsupported_chain",
            message: "This chain is not supported".into(),
        }
    }

    pub fn unsupported_token() -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "unsupported_token",
            message: "Only the configured Circle-issued USDC token is supported".into(),
        }
    }

    pub fn idempotency_conflict() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "idempotency_conflict",
            message: "Idempotency-Key already used with different request parameters".into(),
        }
    }

    pub fn payment_not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "payment_not_found",
            message: "Payment not found".into(),
        }
    }

    pub fn invalid_payment_reference() -> Self {
        Self::invalid_request(
            "reference must be a complete payment ID (pay_…) or payment address (0x…)",
        )
    }

    pub fn payment_not_payable() -> Self {
        Self {
            status: StatusCode::GONE,
            code: "payment_not_payable",
            message: "This payment is no longer accepting funds".into(),
        }
    }

    pub fn database_unavailable(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "database_unavailable",
            message: msg.into(),
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: msg.into(),
        }
    }

    pub fn customer_not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "customer_not_found",
            message: "Customer not found".into(),
        }
    }

    pub fn issuer_not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "issuer_not_found",
            message: "Issuer identity not found".into(),
        }
    }

    pub fn payout_address_not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "payout_address_not_found",
            message: "Payout address not found".into(),
        }
    }

    pub fn issuer_name_taken() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "issuer_name_taken",
            message: "Another of your issuer identities already uses this name".into(),
        }
    }

    pub fn issuer_in_use() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "issuer_in_use",
            message: "Requests were issued under this identity; it cannot be deleted".into(),
        }
    }

    pub fn issuer_email_already_verified() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "issuer_email_already_verified",
            message: "This contact address is already verified".into(),
        }
    }

    pub fn attachment_not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "attachment_not_found",
            message: "Attachment not found".into(),
        }
    }

    pub fn attachment_scan_pending() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "attachment_scan_pending",
            message: "The malware scan has not reported yet; retry finalize with backoff".into(),
        }
    }

    /// `reason` is a short token or the scanner's verdict, never derived
    /// from the uploaded bytes.
    pub fn attachment_rejected(reason: &str) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "attachment_rejected",
            message: format!("The upload was rejected ({reason}); upload a new file"),
        }
    }

    pub fn attachment_not_ready() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "attachment_not_ready",
            message: "Finalize the upload before attaching it to an invoice".into(),
        }
    }

    /// The bucket expired a finalized upload before an invoice was issued
    /// with it; the merchant starts over with a new upload.
    pub fn attachment_expired() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "attachment_not_ready",
            message: "The upload expired before it was attached; upload the PDF again".into(),
        }
    }

    /// Finalize was called before anything reached the presigned URL.
    pub fn attachment_not_uploaded() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "attachment_not_ready",
            message: "Upload the PDF to upload_url before finalizing".into(),
        }
    }

    pub fn attachment_already_attached() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "attachment_already_attached",
            message: "This attachment already belongs to an issued invoice".into(),
        }
    }

    pub fn payment_not_settled() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "payment_not_settled",
            message: "Proof of Payment is available once the payment has settled".into(),
        }
    }

    /// A gated invoice's content and payment mechanics stay hidden until the
    /// payer has satisfied the policy (product plan §4.3).
    pub fn verification_required() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "verification_required",
            message: "Complete verification to view this invoice's payment details".into(),
        }
    }

    /// The deployment has no payer Auth0 audience configured.
    pub fn verification_unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "verification_unavailable",
            message: "Email verification is not available on this deployment".into(),
        }
    }

    /// The passwordless provider did not answer the code exchange.
    pub fn identity_provider_unavailable() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: "identity_provider_unavailable",
            message: "The verification provider did not respond; try again shortly".into(),
        }
    }

    pub fn verification_not_required() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "verification_not_required",
            message: "This invoice does not require verification".into(),
        }
    }

    pub fn payer_session_invalid() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "payer_session_invalid",
            message: "The payer session is missing, invalid, or expired; start verification again"
                .into(),
        }
    }

    pub fn otp_resend_cooldown(retry_after_secs: u64) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "otp_resend_cooldown",
            message: format!(
                "A code was sent recently; request another in {retry_after_secs} seconds"
            ),
        }
    }

    pub fn otp_invalid() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "otp_invalid",
            message: "The code was not accepted; check it or request a new one".into(),
        }
    }

    pub fn verification_not_started() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "verification_not_started",
            message: "Request a code before confirming one".into(),
        }
    }

    /// The deployment has no onboarding payer wallet configured.
    pub fn onboarding_payment_unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "onboarding_payment_unavailable",
            message: "The onboarding demo payment is not available on this deployment".into(),
        }
    }

    /// Bounds the endpoint to the one reserved, self-issued deposit request
    /// shape — never a general "settle any invoice" affordance.
    pub fn onboarding_payment_not_eligible() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "onboarding_payment_not_eligible",
            message: "This payment is not the onboarding walkthrough's demo request".into(),
        }
    }

    /// At most one onboarding demo payment per account, ever.
    pub fn onboarding_payment_already_claimed() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "onboarding_payment_already_claimed",
            message: "This account has already completed its onboarding demo payment".into(),
        }
    }

    pub fn rate_limited() -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "rate_limited",
            message: "Per-account request limit exceeded".into(),
        }
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: String,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status;
        let body = ErrorBody {
            error: ErrorDetail {
                code: self.code.into(),
                message: self.message,
            },
        };
        let mut response = (status, Json(body)).into_response();
        if status == StatusCode::UNAUTHORIZED {
            response
                .headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = ?e, "database error");
        Self::database_unavailable("database error")
    }
}

impl From<gateway_db::DbInvoiceError> for ApiError {
    fn from(e: gateway_db::DbInvoiceError) -> Self {
        // A stored row outside the schema contract is a server-side data fault,
        // not a client error.
        tracing::error!(error = ?e, "failed to decode invoice row");
        Self::internal("failed to decode stored invoice")
    }
}
