//! Persistent finalized USDC log cursor.

use alloy_primitives::{Address, B256};
use sqlx::PgPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexerCursor {
    pub block: u64,
    pub block_hash: B256,
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
        let row: Option<(i64, Vec<u8>)> = sqlx::query_as(
            r#"
            SELECT last_block, last_block_hash
            FROM indexer_cursor
            WHERE chain_id = $1 AND token_address = $2
            "#,
        )
        .bind(chain_id as i64)
        .bind(token.as_slice())
        .fetch_optional(&self.pool)
        .await?;

        row.map(|(block, hash)| {
            let block_hash = B256::try_from(hash.as_slice()).map_err(|_| {
                sqlx::Error::Decode("invalid indexer cursor block hash length".into())
            })?;
            Ok(IndexerCursor {
                block: block as u64,
                block_hash,
            })
        })
        .transpose()
    }
}
