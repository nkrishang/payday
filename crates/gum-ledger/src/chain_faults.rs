//! Chains the system has stopped trusting.
//!
//! The indexer raises a fault when a block it had treated as finalized
//! changes hash: everything derived from that chain's history is now in
//! doubt, so the ledger schedules no new sweep jobs or withdrawal steps on
//! the chain and tells the signers (`ChainControl::Halted`) to sign nothing
//! new for it while in-flight transactions keep being reconciled. Clearing a
//! fault is an operator action once the chain's state has been reviewed.
//!
//! Raising and clearing publish their `ChainControl` notice in the same
//! transaction as the row change (the outbox): a signer cannot learn of a
//! halt the ledger did not record, or miss one it did.

use gum_bus::{BusError, Publisher};
use gum_contracts::{ChainControl, CorrelationId};
use sqlx::PgPool;
use sqlx::types::chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct DbChainFault {
    pub id: Uuid,
    pub chain_id: i64,
    pub reason: String,
    pub block: Option<i64>,
    pub raised_at: DateTime<Utc>,
    pub cleared_at: Option<DateTime<Utc>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ChainFaultError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Bus(#[from] BusError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaiseOutcome {
    /// A new fault; the chain is now halted.
    Raised(Uuid),
    /// The chain was already halted by this fault; nothing changed.
    AlreadyOpen(Uuid),
}

#[derive(Clone)]
pub struct ChainFaultRepository {
    pool: PgPool,
    publisher: Publisher,
}

impl ChainFaultRepository {
    pub fn new(pool: PgPool, publisher: Publisher) -> Self {
        Self { pool, publisher }
    }

    /// Halt `chain_id`. Idempotent per chain: a second report while a fault
    /// is open returns the open fault and publishes nothing.
    pub async fn raise(
        &self,
        chain_id: u64,
        reason: &str,
        block: Option<u64>,
        correlation_id: CorrelationId,
    ) -> Result<RaiseOutcome, ChainFaultError> {
        let mut tx = self.pool.begin().await?;
        // Serialize raisers of the same chain on the advisory lock so the
        // partial unique index below never turns a race into an error.
        sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
            .bind(CHAIN_FAULT_LOCK)
            .bind(chain_id as i32)
            .execute(&mut *tx)
            .await?;
        let open: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM chain_faults WHERE chain_id = $1 AND cleared_at IS NULL",
        )
        .bind(chain_id as i64)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(id) = open {
            return Ok(RaiseOutcome::AlreadyOpen(id));
        }
        let fault_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO chain_faults (id, chain_id, reason, block) VALUES ($1, $2, $3, $4)",
        )
        .bind(fault_id)
        .bind(chain_id as i64)
        .bind(reason)
        .bind(block.map(|block| block as i64))
        .execute(&mut *tx)
        .await?;
        self.publisher
            .publish(
                &mut tx,
                &ChainControl::Halted {
                    fault_id,
                    chain_id,
                    reason: reason.to_owned(),
                },
                correlation_id,
                None,
            )
            .await?;
        tx.commit().await?;
        tracing::error!(
            chain_id,
            fault_id = %fault_id,
            block,
            reason,
            "chain halted: finality violation reported by the indexer"
        );
        Ok(RaiseOutcome::Raised(fault_id))
    }

    /// Clear the open fault on `chain_id`, if any. Returns the cleared
    /// fault's id.
    pub async fn clear(
        &self,
        chain_id: u64,
        correlation_id: CorrelationId,
    ) -> Result<Option<Uuid>, ChainFaultError> {
        let mut tx = self.pool.begin().await?;
        let cleared: Option<Uuid> = sqlx::query_scalar(
            r#"UPDATE chain_faults SET cleared_at = now()
               WHERE chain_id = $1 AND cleared_at IS NULL RETURNING id"#,
        )
        .bind(chain_id as i64)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(fault_id) = cleared else {
            return Ok(None);
        };
        self.publisher
            .publish(
                &mut tx,
                &ChainControl::Resumed { fault_id, chain_id },
                correlation_id,
                None,
            )
            .await?;
        tx.commit().await?;
        tracing::warn!(chain_id, fault_id = %fault_id, "chain resumed by operator");
        Ok(Some(fault_id))
    }

    /// The open fault on `chain_id`, if the chain is halted.
    pub async fn open(&self, chain_id: u64) -> Result<Option<DbChainFault>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM chain_faults WHERE chain_id = $1 AND cleared_at IS NULL")
            .bind(chain_id as i64)
            .fetch_optional(&self.pool)
            .await
    }

    /// Every open fault, oldest first.
    pub async fn all_open(&self) -> Result<Vec<DbChainFault>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM chain_faults WHERE cleared_at IS NULL ORDER BY raised_at")
            .fetch_all(&self.pool)
            .await
    }
}

/// Advisory lock class for `raise`; the second key is the chain id.
const CHAIN_FAULT_LOCK: i32 = 0x4641_554C; // "FAUL"

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLISHER: Publisher = Publisher::new("test");

    async fn control_messages(pool: &PgPool) -> Vec<(String, String)> {
        sqlx::query_as(
            "SELECT kind, deduplication_key FROM bus.messages WHERE topic = 'chain.control' ORDER BY id",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn raise_is_idempotent_and_publishes_once(pool: PgPool) {
        let repo = ChainFaultRepository::new(pool.clone(), PUBLISHER);
        let first = repo
            .raise(
                143,
                "finalized block 10 changed hash",
                Some(10),
                CorrelationId::new(),
            )
            .await
            .unwrap();
        let RaiseOutcome::Raised(fault_id) = first else {
            panic!("expected a new fault, got {first:?}");
        };
        assert_eq!(
            repo.raise(143, "again", None, CorrelationId::new())
                .await
                .unwrap(),
            RaiseOutcome::AlreadyOpen(fault_id)
        );
        let open = repo.open(143).await.unwrap().unwrap();
        assert_eq!(open.id, fault_id);
        assert_eq!(open.block, Some(10));
        assert!(repo.open(8453).await.unwrap().is_none());
        assert_eq!(
            control_messages(&pool).await,
            vec![("halted".to_owned(), format!("143:halted:{fault_id}"))]
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn clear_resumes_and_a_new_fault_can_follow(pool: PgPool) {
        let repo = ChainFaultRepository::new(pool.clone(), PUBLISHER);
        assert!(
            repo.clear(143, CorrelationId::new())
                .await
                .unwrap()
                .is_none()
        );
        let RaiseOutcome::Raised(first) = repo
            .raise(143, "violation", None, CorrelationId::new())
            .await
            .unwrap()
        else {
            panic!("expected a new fault");
        };
        assert_eq!(
            repo.clear(143, CorrelationId::new()).await.unwrap(),
            Some(first)
        );
        assert!(repo.open(143).await.unwrap().is_none());
        assert!(repo.all_open().await.unwrap().is_empty());
        let RaiseOutcome::Raised(second) = repo
            .raise(143, "violation", None, CorrelationId::new())
            .await
            .unwrap()
        else {
            panic!("expected a new fault after clearing");
        };
        assert_ne!(first, second);
        assert_eq!(repo.all_open().await.unwrap().len(), 1);
        let kinds: Vec<String> = control_messages(&pool)
            .await
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(kinds, ["halted", "resumed", "halted"]);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn concurrent_raises_produce_one_fault(pool: PgPool) {
        let repo = ChainFaultRepository::new(pool.clone(), PUBLISHER);
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let repo = repo.clone();
                tokio::spawn(async move {
                    repo.raise(143, "violation", None, CorrelationId::new())
                        .await
                        .unwrap()
                })
            })
            .collect();
        let mut raised = 0;
        for task in tasks {
            if matches!(task.await.unwrap(), RaiseOutcome::Raised(_)) {
                raised += 1;
            }
        }
        assert_eq!(raised, 1);
        assert_eq!(control_messages(&pool).await.len(), 1);
    }
}
