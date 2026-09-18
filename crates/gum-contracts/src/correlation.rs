//! Correlation across process boundaries.
//!
//! One deposit request is traceable end to end because every message and
//! RPC carries the same correlation id: the API request that created the
//! deposit request mints it, the server stamps it on every command it
//! publishes for that request, the signers copy it onto every event they
//! emit, and every log line inside a handler is emitted within a span that
//! carries `correlation_id` as a field.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// HTTP header carrying the correlation id on internal RPC calls.
pub const CORRELATION_HEADER: &str = "x-gum-correlation-id";
/// HTTP header carrying the id of the message or request that caused this one.
pub const CAUSATION_HEADER: &str = "x-gum-causation-id";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CorrelationId(pub Uuid);

impl CorrelationId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for CorrelationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CorrelationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for CorrelationId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid> for CorrelationId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}
