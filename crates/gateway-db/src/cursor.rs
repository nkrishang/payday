//! Persistent finalized USDC log cursor.

use alloy_primitives::{Address, B256};
use sqlx::PgPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexerCursor {
    pub block: u64,
    pub block_hash: B256,
    pub block_timestamp: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinalizedHead {
    pub block: u64,
    pub block_hash: B256,
    pub block_timestamp: Option<u64>,
}

#[derive(Clone)]
pub struct CursorRepository {
    pool: PgPool,
}

impl CursorRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Last finalized range committed for this chain and USDC contract.
    pub async fn get(
        &self,
        chain_id: u64,
        token: Address,
    ) -> Result<Option<IndexerCursor>, sqlx::Error> {
        let row: Option<(i64, Vec<u8>, Option<i64>)> = sqlx::query_as(
            r#"
            SELECT last_block, last_block_hash, last_block_timestamp
            FROM indexer_cursor
            WHERE chain_id = $1 AND token_address = $2
            "#,
        )
        .bind(chain_id as i64)
        .bind(token.as_slice())
        .fetch_optional(&self.pool)
        .await?;

        row.map(|(block, hash, timestamp)| {
            let block_hash = B256::try_from(hash.as_slice()).map_err(|_| {
                sqlx::Error::Decode("invalid indexer cursor block hash length".into())
            })?;
            Ok(IndexerCursor {
                block: block as u64,
                block_hash,
                block_timestamp: timestamp.map(|value| value as u64),
            })
        })
        .transpose()
    }

    pub async fn finalized_head(
        &self,
        chain_id: u64,
        token: Address,
    ) -> Result<Option<FinalizedHead>, sqlx::Error> {
        let row: Option<(i64, Vec<u8>, Option<i64>)> = sqlx::query_as(
            "SELECT finalized_block, finalized_block_hash, finalized_block_timestamp FROM indexer_status WHERE chain_id = $1 AND token_address = $2"
        ).bind(chain_id as i64).bind(token.as_slice()).fetch_optional(&self.pool).await?;
        row.map(|(block, hash, timestamp)| {
            Ok(FinalizedHead {
                block: block as u64,
                block_hash: B256::try_from(hash.as_slice()).map_err(|_| {
                    sqlx::Error::Decode("invalid indexer cursor block hash length".into())
                })?,
                block_timestamp: timestamp.map(|value| value as u64),
            })
        })
        .transpose()
    }

    pub async fn record_finalized_head(
        &self,
        chain_id: u64,
        token: Address,
        block: u64,
        hash: B256,
        timestamp: u64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO indexer_status (chain_id, token_address, finalized_block, finalized_block_hash, finalized_block_timestamp) VALUES ($1,$2,$3,$4,$5) ON CONFLICT (chain_id, token_address) DO UPDATE SET finalized_block=EXCLUDED.finalized_block, finalized_block_hash=EXCLUDED.finalized_block_hash, finalized_block_timestamp=EXCLUDED.finalized_block_timestamp, observed_at=now()")
            .bind(chain_id as i64).bind(token.as_slice()).bind(block as i64).bind(hash.as_slice()).bind(timestamp as i64)
            .execute(&self.pool).await.map(drop)
    }
}
