//! Body and query extractors that fail the API's way.
//!
//! axum's own `Json` and `Query` answer a body or query string that does not
//! fit its type with a plain-text 422 or 400 whose text the request-id
//! middleware then replaces with the status's canonical reason, so a caller
//! who misspelled a field learned only "Unprocessable Entity". These wrappers
//! keep the deserializer's message (`unknown field 'rotate', expected
//! 'expected_generation'`, `missing field 'amount'`) and answer with the
//! same `400 invalid_request` envelope every other validation failure uses.
//! Only an oversized body keeps its own status, `413 payload_too_large`.

use axum::extract::rejection::{BytesRejection, JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::api::error::ApiError;

/// `axum::Json` with the API's error envelope on rejection. It also serves
/// as the response type, so a handler file imports one `Json`.
pub struct Json<T>(pub T);

impl<S, T> FromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(Json(value)),
            Err(rejection) => Err(json_rejection(rejection)),
        }
    }
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// `axum::Query` with the API's error envelope on rejection.
pub struct Query<T>(pub T);

impl<S, T> FromRequestParts<S> for Query<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Query(value)),
            Err(rejection) => Err(query_rejection(rejection)),
        }
    }
}

fn json_rejection(rejection: JsonRejection) -> ApiError {
    match rejection {
        JsonRejection::JsonDataError(error) => {
            ApiError::invalid_request(strip_prefix(&error.body_text()))
        }
        JsonRejection::JsonSyntaxError(error) => {
            ApiError::invalid_request(strip_prefix(&error.body_text()))
        }
        JsonRejection::MissingJsonContentType(_) => {
            ApiError::invalid_request("Content-Type must be application/json")
        }
        JsonRejection::BytesRejection(BytesRejection::FailedToBufferBody(error))
            if error.status() == StatusCode::PAYLOAD_TOO_LARGE =>
        {
            ApiError {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                code: "payload_too_large",
                message: "Request body exceeds the route's limit".into(),
            }
        }
        other => ApiError::invalid_request(other.body_text()),
    }
}

fn query_rejection(rejection: QueryRejection) -> ApiError {
    ApiError::invalid_request(strip_prefix(&rejection.body_text()))
}

/// axum prefixes the deserializer's message with what it was doing; the
/// message alone names the field, which is what the caller needs.
fn strip_prefix(text: &str) -> String {
    text.split_once(": ")
        .filter(|(prefix, _)| prefix.starts_with("Failed to "))
        .map_or(text, |(_, rest)| rest)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::header;
    use axum::routing::{get, post};
    use serde::Deserialize;
    use tower::ServiceExt;

    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Body_ {
        expected_generation: i64,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Params {
        limit: Option<u32>,
    }

    fn app() -> Router {
        Router::new()
            .route(
                "/body",
                post(|Json(body): Json<Body_>| async move { Json(body) }),
            )
            .route(
                "/query",
                get(|Query(params): Query<Params>| async move {
                    Json(params.limit.unwrap_or_default())
                }),
            )
    }

    async fn call(request: Request) -> (StatusCode, serde_json::Value) {
        let response = app().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    fn post_json(body: &str, content_type: &str) -> Request {
        Request::post("/body")
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body.to_owned()))
            .unwrap()
    }

    #[tokio::test]
    async fn body_shape_errors_name_the_field_in_the_api_envelope() {
        let (status, body) = call(post_json(
            r#"{"expected_generation":1,"rotate":true}"#,
            "application/json",
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "invalid_request");
        // The deserializer's message, with the path to the offending field
        // in front of it, minus axum's own preamble.
        let message = body["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("unknown field `rotate`") && !message.starts_with("Failed to"),
            "{message}"
        );

        let (status, body) = call(post_json("{}", "application/json")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let message = body["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("missing field `expected_generation`"),
            "{message}"
        );

        let (status, body) = call(post_json("{not json", "application/json")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "invalid_request");
        assert!(!body["error"]["message"].as_str().unwrap().is_empty());

        let (status, body) = call(post_json(r#"{"expected_generation":1}"#, "text/plain")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body["error"]["message"],
            "Content-Type must be application/json"
        );

        let (status, body) = call(post_json(
            r#"{"expected_generation":1}"#,
            "application/json",
        ))
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["expected_generation"], 1);
    }

    #[tokio::test]
    async fn query_shape_errors_name_the_parameter() {
        let (status, body) = call(
            Request::get("/query?sort=name")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "invalid_request");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("unknown field `sort`")
        );
        let (status, body) =
            call(Request::get("/query?limit=x").body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"]["message"].as_str().unwrap().contains("limit"));
    }
}
