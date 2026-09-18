use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gum_contracts::{BusMessage, CorrelationId};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use tokio::sync::watch;
use tracing::{Instrument, error, info, info_span, warn};
use uuid::Uuid;

use crate::{BackoffPolicy, BusError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsumerOptions {
    /// How long a claimed delivery stays invisible to other claimers. Must
    /// exceed the longest handler; handlers that outlive it are redelivered
    /// concurrently, which idempotency makes safe but wasteful.
    pub lease: Duration,
    /// Attempts before a delivery is parked as dead.
    pub max_attempts: u32,
    pub backoff: BackoffPolicy,
    /// How often an idle consumer looks for work.
    pub poll_interval: Duration,
    /// Deliveries claimed per poll.
    pub batch_size: i64,
}

impl Default for ConsumerOptions {
    fn default() -> Self {
        Self {
            lease: Duration::from_secs(30),
            max_attempts: 20,
            backoff: BackoffPolicy::new(Duration::from_secs(2), Duration::from_secs(300)),
            poll_interval: Duration::from_secs(1),
            batch_size: 32,
        }
    }
}

/// A claimed message with the lease that proves the claim.
#[derive(Debug, Clone)]
pub struct Delivery<M> {
    pub message_id: Uuid,
    pub message: M,
    pub kind: String,
    pub correlation_id: CorrelationId,
    pub causation_id: Option<Uuid>,
    pub published_at: DateTime<Utc>,
    /// 1 on the first delivery.
    pub attempt: u32,
    pub lease_token: Uuid,
}

/// What a failed attempt became.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Retry { available_at: DateTime<Utc> },
    Dead,
}

/// How a handler failed. `Permanent` skips the remaining attempts.
#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    #[error("{0}")]
    Retryable(String),
    #[error("{0}")]
    Permanent(String),
}

impl HandlerError {
    pub fn retryable(error: impl std::fmt::Display) -> Self {
        Self::Retryable(error.to_string())
    }

    pub fn permanent(error: impl std::fmt::Display) -> Self {
        Self::Permanent(error.to_string())
    }
}

impl From<sqlx::Error> for HandlerError {
    fn from(error: sqlx::Error) -> Self {
        Self::Retryable(error.to_string())
    }
}

impl From<BusError> for HandlerError {
    fn from(error: BusError) -> Self {
        match error {
            BusError::ConflictingPayload { .. } | BusError::Encode(_) => {
                Self::Permanent(error.to_string())
            }
            BusError::Database(_) | BusError::LeaseLost { .. } => Self::Retryable(error.to_string()),
        }
    }
}

/// A dead-lettered delivery, for operators.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeadDelivery {
    pub message_id: Uuid,
    pub topic: String,
    pub kind: String,
    pub correlation_id: Uuid,
    pub attempt_count: i32,
    pub last_error: Option<String>,
    pub last_attempt_at: Option<DateTime<Utc>>,
}

#[derive(Debug, sqlx::FromRow)]
struct RawMessage {
    id: Uuid,
    kind: String,
    schema_version: i16,
    payload: serde_json::Value,
    correlation_id: Uuid,
    causation_id: Option<Uuid>,
    published_at: DateTime<Utc>,
}

/// One named consumer of one or more topics.
#[derive(Debug, Clone)]
pub struct Consumer {
    name: &'static str,
    pool: PgPool,
    options: ConsumerOptions,
}

impl Consumer {
    pub fn new(name: &'static str, pool: PgPool, options: ConsumerOptions) -> Self {
        Self {
            name,
            pool,
            options,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn options(&self) -> &ConsumerOptions {
        &self.options
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Claim up to `batch_size` claimable deliveries of `M`'s topic. A
    /// message whose payload this binary cannot decode (newer schema
    /// version, unknown variant) is dead-lettered here rather than
    /// returned, since no retry would decode it.
    pub async fn claim<M: BusMessage>(&self) -> Result<Vec<Delivery<M>>, BusError> {
        let claimed: Vec<(Uuid, i32, Uuid)> = sqlx::query_as(
            r#"
            WITH picked AS (
                SELECT delivery.message_id
                FROM bus.deliveries delivery
                JOIN bus.messages message ON message.id = delivery.message_id
                WHERE delivery.consumer = $1
                  AND delivery.state = 'pending'
                  AND delivery.available_at <= now()
                  AND (delivery.leased_until IS NULL OR delivery.leased_until < now())
                  AND message.topic = $2
                ORDER BY message.published_at
                LIMIT $3
                FOR UPDATE OF delivery SKIP LOCKED
            )
            UPDATE bus.deliveries delivery
            SET lease_token = gen_random_uuid(),
                leased_until = now() + $4,
                attempt_count = delivery.attempt_count + 1,
                last_attempt_at = now()
            FROM picked
            WHERE delivery.message_id = picked.message_id AND delivery.consumer = $1
            RETURNING delivery.message_id, delivery.attempt_count, delivery.lease_token
            "#,
        )
        .bind(self.name)
        .bind(M::TOPIC)
        .bind(self.options.batch_size)
        .bind(self.options.lease)
        .fetch_all(&self.pool)
        .await?;
        if claimed.is_empty() {
            return Ok(Vec::new());
        }

        let ids: Vec<Uuid> = claimed.iter().map(|(id, _, _)| *id).collect();
        let messages: Vec<RawMessage> = sqlx::query_as(
            r#"
            SELECT id, kind, schema_version, payload, correlation_id, causation_id, published_at
            FROM bus.messages WHERE id = ANY($1) ORDER BY published_at
            "#,
        )
        .bind(&ids)
        .fetch_all(&self.pool)
        .await?;

        let mut deliveries = Vec::with_capacity(messages.len());
        for raw in messages {
            let (_, attempt_count, lease_token) = claimed
                .iter()
                .find(|(id, _, _)| *id == raw.id)
                .copied()
                .expect("claimed delivery has its message");
            if raw.schema_version as u16 > M::SCHEMA_VERSION {
                let reason = format!(
                    "message schema version {} is newer than the {} this binary supports",
                    raw.schema_version,
                    M::SCHEMA_VERSION
                );
                self.park_dead(raw.id, lease_token, &reason).await?;
                error!(message_id = %raw.id, message_type = %raw.kind, error = %reason, "delivery dead-lettered");
                continue;
            }
            let message: M = match serde_json::from_value(raw.payload) {
                Ok(message) => message,
                Err(error) => {
                    let reason = format!("payload does not decode: {error}");
                    self.park_dead(raw.id, lease_token, &reason).await?;
                    error!(message_id = %raw.id, message_type = %raw.kind, error = %reason, "delivery dead-lettered");
                    continue;
                }
            };
            deliveries.push(Delivery {
                message_id: raw.id,
                message,
                kind: raw.kind,
                correlation_id: CorrelationId(raw.correlation_id),
                causation_id: raw.causation_id,
                published_at: raw.published_at,
                attempt: attempt_count as u32,
                lease_token,
            });
        }
        Ok(deliveries)
    }

    /// Mark a delivery done within the caller's transaction, so the
    /// acknowledgement commits with the handler's own writes or not at all.
    /// Fails with [`BusError::LeaseLost`] when the lease expired and another
    /// claimer took over; the caller must then roll back.
    pub async fn ack<M>(
        &self,
        conn: &mut PgConnection,
        delivery: &Delivery<M>,
    ) -> Result<(), BusError> {
        let updated = sqlx::query(
            r#"
            UPDATE bus.deliveries
            SET state = 'done', done_at = now(), lease_token = NULL, leased_until = NULL, last_error = NULL
            WHERE message_id = $1 AND consumer = $2 AND lease_token = $3 AND state = 'pending'
            "#,
        )
        .bind(delivery.message_id)
        .bind(self.name)
        .bind(delivery.lease_token)
        .execute(conn)
        .await?
        .rows_affected();
        if updated == 1 {
            Ok(())
        } else {
            Err(BusError::LeaseLost {
                message_id: delivery.message_id,
                consumer: self.name,
            })
        }
    }

    /// Record a failed attempt: reschedule with backoff, or park as dead
    /// after the last allowed attempt or on a permanent error. A lost lease
    /// is not an error here; the delivery already belongs to someone else.
    pub async fn fail<M>(
        &self,
        delivery: &Delivery<M>,
        error: &HandlerError,
    ) -> Result<Disposition, BusError> {
        let permanent = matches!(error, HandlerError::Permanent(_));
        let exhausted = delivery.attempt >= self.options.max_attempts;
        let message = error.to_string();
        if permanent || exhausted {
            self.park_dead(delivery.message_id, delivery.lease_token, &message)
                .await?;
            return Ok(Disposition::Dead);
        }
        let delay = self.options.backoff.delay(delivery.attempt);
        let available_at: Option<DateTime<Utc>> = sqlx::query_scalar(
            r#"
            UPDATE bus.deliveries
            SET available_at = now() + $4, lease_token = NULL, leased_until = NULL, last_error = $5
            WHERE message_id = $1 AND consumer = $2 AND lease_token = $3 AND state = 'pending'
            RETURNING available_at
            "#,
        )
        .bind(delivery.message_id)
        .bind(self.name)
        .bind(delivery.lease_token)
        .bind(delay)
        .bind(&message)
        .fetch_optional(&self.pool)
        .await?;
        Ok(Disposition::Retry {
            available_at: available_at.unwrap_or_else(Utc::now),
        })
    }

    async fn park_dead(&self, message_id: Uuid, lease_token: Uuid, error: &str) -> Result<(), BusError> {
        sqlx::query(
            r#"
            UPDATE bus.deliveries
            SET state = 'dead', lease_token = NULL, leased_until = NULL, last_error = $4
            WHERE message_id = $1 AND consumer = $2 AND lease_token = $3 AND state = 'pending'
            "#,
        )
        .bind(message_id)
        .bind(self.name)
        .bind(lease_token)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Dead-lettered deliveries of this consumer, oldest first.
    pub async fn dead(&self, limit: i64) -> Result<Vec<DeadDelivery>, BusError> {
        Ok(sqlx::query_as(
            r#"
            SELECT delivery.message_id, message.topic, message.kind, message.correlation_id,
                   delivery.attempt_count, delivery.last_error, delivery.last_attempt_at
            FROM bus.deliveries delivery
            JOIN bus.messages message ON message.id = delivery.message_id
            WHERE delivery.consumer = $1 AND delivery.state = 'dead'
            ORDER BY delivery.last_attempt_at
            LIMIT $2
            "#,
        )
        .bind(self.name)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Return a dead delivery to the queue with a fresh attempt budget.
    pub async fn retry_dead(&self, message_id: Uuid) -> Result<bool, BusError> {
        let updated = sqlx::query(
            r#"
            UPDATE bus.deliveries
            SET state = 'pending', attempt_count = 0, available_at = now(), last_error = NULL
            WHERE message_id = $1 AND consumer = $2 AND state = 'dead'
            "#,
        )
        .bind(message_id)
        .bind(self.name)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(updated == 1)
    }

    /// Pending deliveries older than `age` that nobody has completed: the
    /// backlog metric a readiness check or alert watches.
    pub async fn backlog(&self, age: Duration) -> Result<i64, BusError> {
        Ok(sqlx::query_scalar(
            r#"
            SELECT count(*) FROM bus.deliveries delivery
            JOIN bus.messages message ON message.id = delivery.message_id
            WHERE delivery.consumer = $1 AND delivery.state = 'pending'
              AND message.published_at < now() - $2
            "#,
        )
        .bind(self.name)
        .bind(age)
        .fetch_one(&self.pool)
        .await?)
    }
}

/// A message handler. It receives the transaction the acknowledgement will
/// commit in; every state change it makes must go through `tx`, and any
/// message it publishes in reaction must be published through `tx` too.
#[async_trait]
pub trait Handler<M: BusMessage>: Send + Sync {
    async fn handle(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        delivery: &Delivery<M>,
    ) -> Result<(), HandlerError>;
}

/// Run `handler` over every delivery of `M` until `shutdown` turns true.
/// Each delivery is handled in its own transaction: handler writes and the
/// acknowledgement commit together. Failures are recorded through
/// [`Consumer::fail`]; the loop itself never stops on a handler error.
pub async fn run_consumer<M, H>(consumer: Consumer, handler: H, mut shutdown: watch::Receiver<bool>)
where
    M: BusMessage,
    H: Handler<M>,
{
    info!(consumer = consumer.name(), topic = M::TOPIC, "bus consumer started");
    loop {
        if *shutdown.borrow() {
            break;
        }
        let batch = match consumer.claim::<M>().await {
            Ok(batch) => batch,
            Err(error) => {
                warn!(consumer = consumer.name(), error = %error, "claiming deliveries failed");
                Vec::new()
            }
        };
        let claimed = batch.len();
        for delivery in batch {
            let span = info_span!(
                "bus.handle",
                consumer = consumer.name(),
                message_type = %delivery.kind,
                message_id = %delivery.message_id,
                correlation_id = %delivery.correlation_id,
                attempt = delivery.attempt,
            );
            handle_one(&consumer, &handler, &delivery)
                .instrument(span)
                .await;
        }
        if claimed as i64 >= consumer.options().batch_size {
            continue;
        }
        tokio::select! {
            _ = tokio::time::sleep(consumer.options().poll_interval) => {}
            _ = shutdown.changed() => {}
        }
    }
    info!(consumer = consumer.name(), topic = M::TOPIC, "bus consumer stopped");
}

async fn handle_one<M: BusMessage, H: Handler<M>>(
    consumer: &Consumer,
    handler: &H,
    delivery: &Delivery<M>,
) {
    let started = std::time::Instant::now();
    let outcome: Result<(), HandlerError> = async {
        let mut tx = consumer.pool().begin().await?;
        handler.handle(&mut tx, delivery).await?;
        consumer.ack(&mut tx, delivery).await?;
        tx.commit().await?;
        Ok(())
    }
    .await;
    let latency_ms = started.elapsed().as_millis() as u64;
    match outcome {
        Ok(()) => info!(latency_ms, "message handled"),
        Err(error) => match consumer.fail(delivery, &error).await {
            Ok(Disposition::Retry { available_at }) => {
                warn!(error = %error, latency_ms, retry_at = %available_at, "message handling failed; will retry")
            }
            Ok(Disposition::Dead) => {
                error!(error = %error, latency_ms, "message handling failed permanently; dead-lettered")
            }
            Err(bus_error) => {
                error!(error = %error, bus_error = %bus_error, "message handling failed and the failure could not be recorded")
            }
        },
    }
}
