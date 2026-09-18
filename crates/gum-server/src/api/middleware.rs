use axum::body::to_bytes;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use uuid::Uuid;

pub const REQUEST_ID: &str = "x-request-id";

#[derive(Clone)]
pub struct RequestId(pub String);

/// Accept an upstream correlation ID only when it is a conservative printable
/// token; otherwise generate a UUIDv7. The value is never used as log syntax.
pub async fn request_id(mut request: Request, next: Next) -> Response {
    let supplied = request
        .headers()
        .get(REQUEST_ID)
        .and_then(|v| v.to_str().ok());
    let id = supplied
        .filter(|v| !v.is_empty() && v.len() <= 128 && v.bytes().all(|b| b.is_ascii_graphic()))
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    request.extensions_mut().insert(RequestId(id.clone()));
    let mut response = next.run(request).await;

    // Normalize framework-generated failures (JSON extractor, body limit,
    // unknown route) and correlate all JSON errors with the response header.
    if response.status().is_client_error() || response.status().is_server_error() {
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap_or_default();
        let mut body: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({
            "error": {"code": code(status), "message": status.canonical_reason().unwrap_or("Request failed")}
        }));
        if body.get("error").is_some() {
            body["request_id"] = Value::String(id.clone());
        }
        let encoded = serde_json::to_vec(&body).expect("JSON value is serializable");
        response = (status, encoded.clone()).into_response();
        // Keep every handler/framework header (not only a fragile allow-list).
        // Only representation metadata changes when we replace the body.
        *response.headers_mut() = headers;
        response.headers_mut().remove(header::CONTENT_ENCODING);
        response.headers_mut().remove(header::TRANSFER_ENCODING);
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        response.headers_mut().insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&encoded.len().to_string()).unwrap(),
        );
    }
    response
        .headers_mut()
        .insert(REQUEST_ID, HeaderValue::from_str(&id).unwrap());
    response
}

fn code(status: StatusCode) -> &'static str {
    match status {
        StatusCode::PAYLOAD_TOO_LARGE => "payload_too_large",
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::METHOD_NOT_ALLOWED => "method_not_allowed",
        _ if status.is_client_error() => "invalid_request",
        _ => "internal_error",
    }
}

#[cfg(test)]
mod tests {
    use axum::{Router, body::Body, http::Request, middleware, routing::get};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn normalization_preserves_allow_and_sets_representation_headers() {
        let response = Router::new()
            .route("/resource", get(|| async { "ok" }))
            .layer(middleware::from_fn(request_id))
            .oneshot(Request::post("/resource").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(response.headers()[header::ALLOW], "GET,HEAD");
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        let declared = response.headers()[header::CONTENT_LENGTH]
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap();
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(declared, body.len());
    }
}
