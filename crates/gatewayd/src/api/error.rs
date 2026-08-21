use axum::Json;
use axum::http::StatusCode;
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
            message: "Only the native token is supported in this milestone".into(),
        }
    }

    pub fn idempotency_conflict() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "idempotency_conflict",
            message: "Idempotency-Key already used with different request parameters".into(),
        }
    }

    pub fn invoice_not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "invoice_not_found",
            message: "Invoice not found".into(),
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
        let body = ErrorBody {
            error: ErrorDetail {
                code: self.code.into(),
                message: self.message,
            },
        };
        (self.status, Json(body)).into_response()
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
