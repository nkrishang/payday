//! Typed CLI errors, mapped to stable process exit codes at the boundary.

use thiserror::Error;

/// The API's stable error envelope: `{ "error": { "code", "message" } }`.
#[derive(Debug, serde::Deserialize)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

#[derive(Debug, serde::Deserialize)]
pub struct ApiErrorDetail {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Error)]
pub enum CliError {
    /// Could not reach the gateway or the transport failed.
    #[error("failed to reach gateway at {url}: {source}")]
    Transport {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    /// The gateway returned a non-success status with a structured error body.
    #[error("gateway returned {status} [{code}]: {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },

    /// The gateway returned a non-success status we could not parse.
    #[error("gateway returned {status}: {body}")]
    UnexpectedResponse { status: u16, body: String },

    /// Local input the CLI rejected before making a request.
    #[error("{0}")]
    InvalidInput(String),
}

impl CliError {
    /// Stable process exit code. Distinguishes local misuse from remote failures
    /// so scripts can branch on the outcome.
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::InvalidInput(_) => 2,
            CliError::Transport { .. } => 3,
            CliError::Api { .. } | CliError::UnexpectedResponse { .. } => 1,
        }
    }
}
