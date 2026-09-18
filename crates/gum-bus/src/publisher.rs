use gum_contracts::{BusMessage, CorrelationId};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::BusError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    Published(Uuid),
    /// The same deduplication key was already published with the same
    /// payload; the earlier message stands.
    Duplicate(Uuid),
}

impl PublishOutcome {
    pub fn message_id(self) -> Uuid {
        match self {
            Self::Published(id) | Self::Duplicate(id) => id,
        }
    }
}

/// Publishes messages inside the caller's transaction (the outbox).
#[derive(Debug, Clone, Copy)]
pub struct Publisher {
    producer: &'static str,
}

impl Publisher {
    pub const fn new(producer: &'static str) -> Self {
        Self { producer }
    }

    /// Insert `message` into the log within `conn`'s transaction. Fan-out to
    /// subscribed consumers happens in the database. The producer's
    /// deduplication key makes this idempotent: a replay with an equal
    /// payload is a no-op, a replay with a different payload is an error,
    /// because two different intents under one key means a bug upstream.
    pub async fn publish<M: BusMessage>(
        &self,
        conn: &mut PgConnection,
        message: &M,
        correlation_id: CorrelationId,
        causation_id: Option<Uuid>,
    ) -> Result<PublishOutcome, BusError> {
        let payload = serde_json::to_value(message)?;
        let canonical = serde_json::to_vec(&payload)?;
        let payload_hash = Sha256::digest(&canonical).to_vec();
        let id = Uuid::now_v7();
        let kind = message.kind();
        let deduplication_key = message.deduplication_key();

        let inserted: Option<Uuid> = sqlx::query_scalar(
            r#"
            INSERT INTO bus.messages
                (id, topic, kind, schema_version, payload, payload_hash, producer,
                 deduplication_key, correlation_id, causation_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (topic, deduplication_key) DO NOTHING
            RETURNING id
            "#,
        )
        .bind(id)
        .bind(M::TOPIC)
        .bind(kind)
        .bind(M::SCHEMA_VERSION as i16)
        .bind(&payload)
        .bind(&payload_hash)
        .bind(self.producer)
        .bind(&deduplication_key)
        .bind(correlation_id.0)
        .bind(causation_id)
        .fetch_optional(&mut *conn)
        .await?;

        if let Some(id) = inserted {
            tracing::debug!(
                topic = M::TOPIC,
                message_type = kind,
                message_id = %id,
                correlation_id = %correlation_id,
                "message published"
            );
            return Ok(PublishOutcome::Published(id));
        }

        let (existing_id, existing_hash): (Uuid, Vec<u8>) = sqlx::query_as(
            "SELECT id, payload_hash FROM bus.messages WHERE topic = $1 AND deduplication_key = $2",
        )
        .bind(M::TOPIC)
        .bind(&deduplication_key)
        .fetch_one(&mut *conn)
        .await?;
        if existing_hash != payload_hash {
            return Err(BusError::ConflictingPayload {
                message_id: existing_id,
                expected: hex(&existing_hash),
                actual: hex(&payload_hash),
            });
        }
        Ok(PublishOutcome::Duplicate(existing_id))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
