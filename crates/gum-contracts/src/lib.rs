//! The typed contracts between Gum's services.
//!
//! Three processes make up the backend:
//!
//! * `gum-server` owns the application ledger (deposit requests, accounts,
//!   withdrawals, webhooks) and serves the public API plus a small internal
//!   RPC surface for the indexer.
//! * `gum-indexer` observes chains and reports finalized facts to the server
//!   over that RPC. It holds no database credentials and no keys.
//! * `gum-signers` holds the transaction keys, executes commands published by
//!   the server and reports evidence back as events.
//!
//! Everything that crosses a process boundary is defined here so that a
//! change to any message or RPC is a change to one crate every service
//! compiles against. Two kinds of contract live here:
//!
//! * [`bus`] messages: durable, at-least-once, asynchronous. A *command*
//!   asks exactly one consumer to do a piece of work (`ExecutionCommand`);
//!   an *event* states a fact that any number of consumers may react to
//!   (`ExecutionEvent`, `ChainControl`).
//! * [`rpc`] request/response types for the synchronous internal HTTP API.
//!
//! Wire encoding is JSON. Addresses and hashes are `0x`-prefixed hex, `U256`
//! values are `0x` hex quantities, block numbers and timestamps are plain
//! integers. Every payload carries a `schema_version` through the envelope
//! so an old message can still be decoded after a consumer upgrade.

pub mod bus;
pub mod correlation;
pub mod rpc;

pub use bus::{
    AbandonReason, BusMessage, CHAIN_CONTROL_TOPIC, ChainControl, EXECUTION_COMMANDS_TOPIC,
    EXECUTION_EVENTS_TOPIC, ExecutionCommand, ExecutionEvent, SettlementEvidence,
    StepPrecondition, StepResult,
    SweepBatchCommand, SweepFailureCause, SweepItem, SweepItemOutcome, SweepItemResult,
    SweepItemStatus, WithdrawalStepCommand, WithdrawalStepKind,
};
pub use correlation::{CAUSATION_HEADER, CORRELATION_HEADER, CorrelationId};
pub use rpc::{
    ChainFaultReport, FinalizedHeadReport, IndexerCursor, PaymentObservation, RangeApplied,
    RecordFinalizedRange, RpcError, RpcErrorCode, WatchList, WatchListQuery,
};
