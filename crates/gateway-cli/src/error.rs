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
    /// Could not reach Payday or the transport failed.
    #[error("failed to reach Payday at {url}: {source}")]
    Transport {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    /// Payday returned a non-success status with a structured error body.
    #[error("Payday returned {status} [{code}]: {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },

    /// Payday returned a non-success status we could not parse.
    #[error("Payday returned {status}: {body}")]
    UnexpectedResponse { status: u16, body: String },

    /// Local input the CLI rejected before making a request.
    #[error("{0}")]
    InvalidInput(String),

    #[error("{0}")]
    Config(String),

    #[error("{message}")]
    Auth {
        message: String,
        detail: Option<String>,
    },
}

impl CliError {
    /// Stable process exit code. Distinguishes local misuse from remote failures
    /// so scripts can branch on the outcome.
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::InvalidInput(_) | CliError::Config(_) => 2,
            CliError::Transport { .. } => 3,
            CliError::Api { .. } | CliError::UnexpectedResponse { .. } | CliError::Auth { .. } => 1,
        }
    }

    pub fn diagnostic(&self) -> Option<&str> {
        match self {
            CliError::Auth { detail, .. } => detail.as_deref(),
            _ => None,
        }
    }
}
