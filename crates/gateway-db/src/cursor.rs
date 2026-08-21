//! Persistent indexer cursor: the last block height processed per chain.
//!
//! Height-only for this milestone (no block hash / reorg detection). The cursor
//! lets the indexer skip work when no new block has arrived and resume at the
//! right height across restarts. Because payment detection reconciles against
//! current canonical balances every pass, the cursor is an optimization and
//! observability anchor, not a correctness dependency.

use sqlx::PgPool;

#[derive(Clone)]
pub struct CursorRepository {
    pool: PgPool,
}

impl CursorRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Last processed block height for `chain_id`, or `None` if never recorded.
    pub async fn get(&self, chain_id: u64) -> Result<Option<u64>, sqlx::Error> {
        let row: Option<(i64,)> =
            sqlx::query_as(r#"SELECT last_block FROM indexer_cursor WHERE chain_id = $1"#)
                .bind(chain_id as i64)
                .fetch_optional(&self.pool)
                .await?;

        Ok(row.map(|(last_block,)| last_block as u64))
    }

    /// Record `block` as the last processed height for `chain_id`, inserting or
    /// updating the single per-chain row. Idempotent: re-running with the same
    /// value is a no-op transition.
    pub async fn upsert(&self, chain_id: u64, block: u64) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO indexer_cursor (chain_id, last_block, updated_at)
            VALUES ($1, $2, now())
            ON CONFLICT (chain_id)
            DO UPDATE SET last_block = EXCLUDED.last_block, updated_at = now()
            "#,
        )
        .bind(chain_id as i64)
        .bind(block as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
