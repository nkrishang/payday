//! Synchronous internal RPC between `gum-indexer` and `gum-server`.
//!
//! The indexer is a pure observer: everything it learns becomes a request
//! to the server, which applies it to the ledger in one transaction. The
//! routes are mounted by the server under `/internal/v1/indexer` and are
//! authenticated with a shared bearer token; they are never exposed on the
//! public listener.
//!
//! | Method | Path                                             | Body / Response                      |
//! |--------|--------------------------------------------------|--------------------------------------|
//! | GET    | `/chains/{chain_id}/cursor`                      | → `Option<IndexerCursor>`            |
//! | POST   | `/chains/{chain_id}/watch-list`                  | `WatchListQuery` → `WatchList`       |
//! | POST   | `/chains/{chain_id}/finalized-head`              | `FinalizedHeadReport` → `()`         |
//! | POST   | `/chains/{chain_id}/finalized-ranges`            | `RecordFinalizedRange` → `RangeApplied` |
//! | POST   | `/chains/{chain_id}/faults`                      | `ChainFaultReport` → `()`            |

use alloy_primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// The last finalized range the ledger committed for a chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexerCursor {
    pub block: u64,
    pub block_hash: B256,
    pub block_timestamp: Option<u64>,
}

/// Ask for the addresses to watch, skipping the list when it has not moved
/// since `known_fingerprint`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchListQuery {
    /// Requests changed after this instant are watched even when settled,
    /// so a late transfer to a settled address still wakes the sweep fast.
    pub recent_since: chrono::DateTime<chrono::Utc>,
    pub known_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchList {
    /// Opaque change detector; equal fingerprints mean an equal list.
    pub fingerprint: String,
    /// `None` when `known_fingerprint` matched.
    pub addresses: Option<Vec<Address>>,
}

/// The node's finalized head as the indexer last saw it; the ledger records
/// it as the chain's clock and closes unbound requests it has outlived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizedHeadReport {
    pub block: u64,
    pub block_hash: B256,
    pub block_timestamp: u64,
}

/// One validated, finalized `Transfer` log to a watched address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentObservation {
    pub token: Address,
    pub block_number: u64,
    pub block_hash: B256,
    pub block_timestamp: u64,
    pub transaction_hash: B256,
    pub transaction_index: u64,
    pub log_index: u64,
    pub sender: Address,
    pub recipient: Address,
    pub amount: U256,
}

/// One contiguous finalized range. `expected_cursor` is the cursor the
/// indexer scanned from; the ledger refuses the range unless its stored
/// cursor still matches (compare-and-set), which is what makes two indexer
/// replicas, or a restart mid-request, safe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordFinalizedRange {
    pub expected_cursor: Option<IndexerCursor>,
    pub end_block: u64,
    pub end_block_hash: B256,
    pub end_block_timestamp: u64,
    pub observations: Vec<PaymentObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RangeApplied {
    Applied {
        funded: Vec<Uuid>,
        expired: Vec<Uuid>,
        cursor: IndexerCursor,
    },
    /// The stored cursor already sits at or past `end_block`: a retry of a
    /// request whose first attempt committed. Nothing changed.
    AlreadyApplied { cursor: IndexerCursor },
    /// The stored cursor is not `expected_cursor`. The indexer reloads it
    /// and scans again from there.
    CursorMismatch { cursor: Option<IndexerCursor> },
}

/// The indexer saw something that means its view of the chain cannot be
/// trusted (a finalized block changed hash). The ledger records the fault,
/// stops scheduling work on the chain and halts the signers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainFaultReport {
    pub reason: String,
    pub block: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcErrorCode {
    Unauthorized,
    UnknownChain,
    InvalidRequest,
    /// Retry after backoff.
    Unavailable,
    Internal,
}

/// The error body every internal route returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[error("{code:?}: {message}")]
pub struct RpcError {
    pub code: RpcErrorCode,
    pub message: String,
}

impl RpcError {
    pub fn new(code: RpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self.code,
            RpcErrorCode::Unavailable | RpcErrorCode::Internal
        )
    }

    pub fn http_status(&self) -> u16 {
        match self.code {
            RpcErrorCode::Unauthorized => 401,
            RpcErrorCode::UnknownChain => 404,
            RpcErrorCode::InvalidRequest => 400,
            RpcErrorCode::Unavailable => 503,
            RpcErrorCode::Internal => 500,
        }
    }
}
