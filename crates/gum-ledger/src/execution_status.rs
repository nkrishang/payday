//! Read-only view of what `gum-signers` reports about itself.
//!
//! The `execution` schema is owned and written by the signers alone; the
//! server reads two of its tables for `/v1/status` so operators see one
//! picture. Nothing here writes, and nothing in the server may depend on
//! these rows for a decision: they are heartbeats, not state.

use alloy_primitives::{Address, U256};
use sqlx::PgPool;
use sqlx::types::chrono::{DateTime, Utc};

/// The last heartbeat of one chain's executor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutorStatus {
    /// `running`, `halted` or `failing`.
    pub state: String,
    pub detail: Option<String>,
    pub heartbeat_at: DateTime<Utc>,
}

/// The last balance reading of one signer on one chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignerStatus {
    pub signer: Address,
    pub balance_wei: Option<U256>,
    pub low_balance: bool,
    pub error: Option<String>,
    pub observed_at: DateTime<Utc>,
}

/// `(signer, balance_wei, low_balance, error, observed_at)` as stored.
type SignerStatusRow = (Vec<u8>, Option<String>, bool, Option<String>, DateTime<Utc>);

#[derive(Clone)]
pub struct ExecutionStatusView {
    pool: PgPool,
}

impl ExecutionStatusView {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn executor(&self, chain_id: u64) -> Result<Option<ExecutorStatus>, sqlx::Error> {
        let row: Option<(String, Option<String>, DateTime<Utc>)> = sqlx::query_as(
            "SELECT state, detail, heartbeat_at FROM execution.executor_status WHERE chain_id = $1",
        )
        .bind(chain_id as i64)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(state, detail, heartbeat_at)| ExecutorStatus {
            state,
            detail,
            heartbeat_at,
        }))
    }

    pub async fn signers(&self, chain_id: u64) -> Result<Vec<SignerStatus>, sqlx::Error> {
        let rows: Vec<SignerStatusRow> = sqlx::query_as(
            r#"SELECT signer, balance_wei, low_balance, error, observed_at
                   FROM execution.signer_status WHERE chain_id = $1 ORDER BY signer"#,
        )
        .bind(chain_id as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(signer, balance, low_balance, error, observed_at)| {
                Ok(SignerStatus {
                    signer: Address::try_from(signer.as_slice())
                        .map_err(|_| sqlx::Error::Decode("invalid signer address length".into()))?,
                    balance_wei: balance
                        .map(|value| {
                            U256::from_str_radix(&value, 10)
                                .map_err(|_| sqlx::Error::Decode("invalid signer balance".into()))
                        })
                        .transpose()?,
                    low_balance,
                    error,
                    observed_at,
                })
            })
            .collect()
    }
}
