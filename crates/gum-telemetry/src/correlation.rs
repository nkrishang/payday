//! Correlation ids on HTTP: read them from incoming requests, write them on
//! outgoing ones, mint one when a request arrives without.

use axum::http::{HeaderMap, HeaderValue};
use gum_contracts::{CAUSATION_HEADER, CORRELATION_HEADER, CorrelationId};
use uuid::Uuid;

/// The correlation id an incoming request carries, or a fresh one.
pub fn extract(headers: &HeaderMap) -> CorrelationId {
    headers
        .get(CORRELATION_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or_default()
}

pub fn extract_causation(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get(CAUSATION_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
}

/// Stamp the correlation headers onto an outgoing request.
pub fn inject(headers: &mut HeaderMap, correlation_id: CorrelationId, causation_id: Option<Uuid>) {
    if let Ok(value) = HeaderValue::from_str(&correlation_id.to_string()) {
        headers.insert(CORRELATION_HEADER, value);
    }
    if let Some(value) = causation_id.and_then(|id| HeaderValue::from_str(&id.to_string()).ok()) {
        headers.insert(CAUSATION_HEADER, value);
    }
}
