//! Durable, at-least-once messaging between Gum's services, on Postgres.
//!
//! # Why Postgres and not a broker
//!
//! Every message Gum sends is caused by a database transaction and is
//! consumed by another database transaction. The property that matters is
//! atomicity between "the ledger changed" and "the message exists", and
//! between "the message was processed" and "the consumer's state changed".
//! With a separate broker that needs an outbox relay on the producer side
//! and an inbox table on the consumer side anyway; the broker in the middle
//! adds a third system to operate, secure and reason about during recovery,
//! for a workload measured in payments per day, not per millisecond.
//! Putting the log in the database gives both properties directly:
//!
//! * [`Publisher::publish`] runs inside the producer's own transaction. If
//!   the transaction rolls back, the message never existed.
//! * [`Consumer::ack`] runs inside the consumer's own transaction. If the
//!   handler's writes roll back, the delivery stays pending and is
//!   redelivered after its lease expires.
//!
//! # Delivery semantics
//!
//! At least once. A consumer claims a delivery under a lease (`FOR UPDATE
//! SKIP LOCKED`), works, and acknowledges. A crash between claim and ack
//! lets the lease expire and the delivery be claimed again. Handlers must
//! therefore be idempotent, which every Gum handler achieves through the
//! message's deduplication key or a domain-level "already resolved" check.
//!
//! A failed attempt reschedules the delivery with exponential backoff and
//! jitter; after [`ConsumerOptions::max_attempts`] failures (or on the first
//! permanent failure) the delivery is parked as `dead` for an operator.
//!
//! # Schema evolution
//!
//! Each topic's payload carries a `schema_version`. A consumer refuses a
//! version above the one it compiled against (dead-letter, not a crash) and
//! decodes older ones through the enum's `serde` defaults.

mod backoff;
mod consumer;
mod publisher;

pub use backoff::{BackoffPolicy, backoff_delay};
pub use consumer::{
    Consumer, ConsumerOptions, DeadDelivery, Delivery, Disposition, HandledDelivery, Handler,
    HandlerError, handle_batch, run_consumer,
};
pub use publisher::{PublishOutcome, Publisher};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BusError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error(
        "message {message_id} encodes as {actual} but was published under the same key as {expected}"
    )]
    ConflictingPayload {
        message_id: uuid::Uuid,
        expected: String,
        actual: String,
    },
    #[error("lease on delivery {message_id} for {consumer} was lost before acknowledgement")]
    LeaseLost {
        message_id: uuid::Uuid,
        consumer: &'static str,
    },
    #[error("payload encoding failed: {0}")]
    Encode(#[from] serde_json::Error),
}
