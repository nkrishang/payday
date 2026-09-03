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

    /// The malware scan did not report within the wait the CLI is willing to
    /// block for; the upload is intact and can be finalized later.
    #[error(
        "Attachment {id} was not scanned within {seconds} seconds; no invoice was created\n→ Retry the command once the scan has reported."
    )]
    AttachmentScanTimeout { id: uuid::Uuid, seconds: u64 },

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
    pub fn json(&self) -> String {
        let code = match self {
            CliError::InvalidInput(_) => "invalid_input",
            CliError::Transport { .. } => "transport_error",
            CliError::Api { code, .. } => code,
            CliError::AttachmentScanTimeout { .. } => "attachment_scan_timeout",
            CliError::UnexpectedResponse { .. } => "unexpected_response",
            CliError::Config(_) => "configuration_error",
            CliError::Auth { .. } => "authentication_error",
        };
        // Hints after the first line are written for a reader, not a script.
        let message = self.to_string();
        let summary = message.lines().next().unwrap_or_default();
        serde_json::json!({ "error": { "code": code, "message": summary } }).to_string()
    }

    /// Stable process exit code. Distinguishes local misuse from remote failures
    /// so scripts can branch on the outcome.
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::InvalidInput(_) | CliError::Config(_) => 2,
            CliError::Transport { .. } => 3,
            CliError::Api { .. }
            | CliError::AttachmentScanTimeout { .. }
            | CliError::UnexpectedResponse { .. }
            | CliError::Auth { .. } => 1,
        }
    }

    pub fn diagnostic(&self) -> Option<&str> {
        match self {
            CliError::Auth { detail, .. } => detail.as_deref(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_errors_keep_stable_codes_and_messages() {
        let value: serde_json::Value =
            serde_json::from_str(&CliError::InvalidInput("bad expiry".into()).json()).unwrap();
        assert_eq!(value["error"]["code"], "invalid_input");
        assert_eq!(value["error"]["message"], "bad expiry");
    }

    #[test]
    fn json_errors_carry_one_line_and_leave_hints_to_the_terminal() {
        let value: serde_json::Value = serde_json::from_str(
            &CliError::InvalidInput("bad reference\n→ Run `payday list`.".into()).json(),
        )
        .unwrap();
        assert_eq!(value["error"]["message"], "bad reference");
    }
}
