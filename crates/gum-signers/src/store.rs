//! The `execution` schema: everything gum-signers knows about its work.
//!
//! Every pass of a chain worker reloads its state from here, so a process
//! that dies at any instruction resumes from the last committed row. The
//! functions are small and typed; the executor composes them and decides
//! what goes into one transaction.

use alloy_primitives::{Address, B256, Bytes, U256};
use chrono::{DateTime, Utc};
use gum_contracts::{BusMessage, ChainControl, ExecutionCommand, ExecutionEvent};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

/// A command accepted from the bus, not yet resolved.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: Uuid,
    pub chain_id: u64,
    pub correlation_id: Uuid,
    pub command_message_id: Option<Uuid>,
    pub command: ExecutionCommand,
    pub state: JobState,
    /// Transient failures before a transaction existed.
    pub attempts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Executing,
    Finalized,
    Abandoned,
    Rejected,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Executing => "executing",
            Self::Finalized => "finalized",
            Self::Abandoned => "abandoned",
            Self::Rejected => "rejected",
        }
    }

    fn parse(value: &str) -> Result<Self, sqlx::Error> {
        Ok(match value {
            "queued" => Self::Queued,
            "executing" => Self::Executing,
            "finalized" => Self::Finalized,
            "abandoned" => Self::Abandoned,
            "rejected" => Self::Rejected,
            other => return Err(decode_error(format!("unknown job state {other}"))),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxState {
    Prepared,
    Broadcast,
    Mined,
    Finalized,
    Reverted,
    Abandoned,
}

impl TxState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Broadcast => "broadcast",
            Self::Mined => "mined",
            Self::Finalized => "finalized",
            Self::Reverted => "reverted",
            Self::Abandoned => "abandoned",
        }
    }

    fn parse(value: &str) -> Result<Self, sqlx::Error> {
        Ok(match value {
            "prepared" => Self::Prepared,
            "broadcast" => Self::Broadcast,
            "mined" => Self::Mined,
            "finalized" => Self::Finalized,
            "reverted" => Self::Reverted,
            "abandoned" => Self::Abandoned,
            other => return Err(decode_error(format!("unknown transaction state {other}"))),
        })
    }
}

/// The receipt seen for one attempt, awaiting finality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mined {
    pub tx_hash: B256,
    pub block: u64,
    pub block_hash: B256,
    pub transaction_index: u64,
    pub succeeded: bool,
}

/// One signed transaction for a slot; replacements share the slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub replacement_number: u32,
    pub tx_hash: B256,
    pub raw_transaction: Bytes,
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
    pub broadcast_at: Option<DateTime<Utc>>,
}

/// A (signer, nonce) slot spent on a job, with every attempt oldest first.
#[derive(Debug, Clone)]
pub struct OpenTransaction {
    pub id: Uuid,
    pub job: Job,
    pub signer: Address,
    pub nonce: u64,
    pub target: Address,
    pub calldata: Bytes,
    pub gas_limit: u64,
    pub state: TxState,
    pub submitted_at: DateTime<Utc>,
    pub mined: Option<Mined>,
    pub stall_reported_at: Option<DateTime<Utc>>,
    pub attempts: Vec<Attempt>,
}

impl OpenTransaction {
    pub fn newest_attempt(&self) -> &Attempt {
        self.attempts
            .last()
            .expect("a transaction has at least one attempt")
    }
}

/// What to insert for a fresh slot.
pub struct NewTransaction<'a> {
    pub job_id: Uuid,
    pub chain_id: u64,
    pub signer: Address,
    pub nonce: u64,
    pub target: Address,
    pub calldata: &'a Bytes,
    pub gas_limit: u64,
    pub attempt: &'a Attempt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutorState {
    pub running: bool,
    pub halted: bool,
}

fn decode_error(message: String) -> sqlx::Error {
    sqlx::Error::Decode(message.into())
}

fn address(bytes: &[u8]) -> Result<Address, sqlx::Error> {
    if bytes.len() != 20 {
        return Err(decode_error(format!(
            "address column holds {} bytes",
            bytes.len()
        )));
    }
    Ok(Address::from_slice(bytes))
}

fn hash(bytes: &[u8]) -> Result<B256, sqlx::Error> {
    if bytes.len() != 32 {
        return Err(decode_error(format!(
            "hash column holds {} bytes",
            bytes.len()
        )));
    }
    Ok(B256::from_slice(bytes))
}

fn u128_text(value: &str) -> Result<u128, sqlx::Error> {
    value
        .parse()
        .map_err(|_| decode_error(format!("{value} is not a u128")))
}

fn unsigned(value: i64) -> Result<u64, sqlx::Error> {
    u64::try_from(value).map_err(|_| decode_error(format!("{value} is negative")))
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

/// Accept a command as a job. `true` when the row was inserted; `false`
/// when the job already existed (a redelivered command).
pub async fn insert_job(
    conn: &mut PgConnection,
    command: &ExecutionCommand,
    correlation_id: Uuid,
    command_message_id: Uuid,
    state: JobState,
    resolution: Option<&ExecutionEvent>,
) -> Result<bool, sqlx::Error> {
    let resolved_at = (!matches!(state, JobState::Queued | JobState::Executing)).then(Utc::now);
    let resolution = resolution
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| decode_error(error.to_string()))?;
    let inserted = sqlx::query(
        r#"
        INSERT INTO execution.jobs
            (id, kind, chain_id, correlation_id, command_message_id, command, state,
             resolved_at, resolution)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .bind(command.job_id())
    .bind(command.kind())
    .bind(command.chain_id() as i64)
    .bind(correlation_id)
    .bind(command_message_id)
    .bind(serde_json::to_value(command).map_err(|error| decode_error(error.to_string()))?)
    .bind(state.as_str())
    .bind(resolved_at)
    .bind(resolution)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(inserted == 1)
}

fn job_from_row(row: &sqlx::postgres::PgRow) -> Result<Job, sqlx::Error> {
    use sqlx::Row;
    let command: serde_json::Value = row.try_get("command")?;
    let command: ExecutionCommand =
        serde_json::from_value(command).map_err(|error| decode_error(error.to_string()))?;
    Ok(Job {
        id: row.try_get("id")?,
        chain_id: unsigned(row.try_get("chain_id")?)?,
        correlation_id: row.try_get("correlation_id")?,
        command_message_id: row.try_get("command_message_id")?,
        command,
        state: JobState::parse(row.try_get::<String, _>("state")?.as_str())?,
        attempts: u32::try_from(row.try_get::<i32, _>("attempts")?).unwrap_or(0),
    })
}

/// Jobs waiting for a signer on `chain_id`, oldest first.
pub async fn queued_jobs(pool: &PgPool, chain_id: u64) -> Result<Vec<Job>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT id, chain_id, correlation_id, command_message_id, command, state, attempts
        FROM execution.jobs
        WHERE chain_id = $1 AND state = 'queued' AND next_attempt_at <= now()
        ORDER BY accepted_at
        "#,
    )
    .bind(chain_id as i64)
    .fetch_all(pool)
    .await?;
    rows.iter().map(job_from_row).collect()
}

pub async fn job(pool: &PgPool, job_id: Uuid) -> Result<Option<Job>, sqlx::Error> {
    let row = sqlx::query(
        r#"
        SELECT id, chain_id, correlation_id, command_message_id, command, state, attempts
        FROM execution.jobs WHERE id = $1
        "#,
    )
    .bind(job_id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(job_from_row).transpose()
}

/// A transient failure before any transaction existed: count it and hold
/// the job until `next_attempt_at`.
pub async fn defer_job(
    conn: &mut PgConnection,
    job_id: Uuid,
    next_attempt_at: DateTime<Utc>,
    error: &str,
) -> Result<u32, sqlx::Error> {
    let attempts: i32 = sqlx::query_scalar(
        r#"
        UPDATE execution.jobs
        SET attempts = attempts + 1, next_attempt_at = $2, last_error = $3
        WHERE id = $1 AND state = 'queued'
        RETURNING attempts
        "#,
    )
    .bind(job_id)
    .bind(next_attempt_at)
    .bind(error)
    .fetch_one(conn)
    .await?;
    Ok(u32::try_from(attempts).unwrap_or(0))
}

/// Close a job with the event that describes its conclusion. The caller
/// publishes the same event in the same transaction. `true` when the job
/// was open and is now closed; `false` when it was already resolved, in
/// which case nothing should be published either.
pub async fn resolve_job(
    conn: &mut PgConnection,
    job_id: Uuid,
    state: JobState,
    resolution: &ExecutionEvent,
) -> Result<bool, sqlx::Error> {
    debug_assert!(!matches!(state, JobState::Queued | JobState::Executing));
    let resolution =
        serde_json::to_value(resolution).map_err(|error| decode_error(error.to_string()))?;
    let updated = sqlx::query(
        r#"
        UPDATE execution.jobs
        SET state = $2, resolved_at = now(), resolution = $3
        WHERE id = $1 AND resolved_at IS NULL
        "#,
    )
    .bind(job_id)
    .bind(state.as_str())
    .bind(resolution)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

// ---------------------------------------------------------------------------
// Transactions and attempts
// ---------------------------------------------------------------------------

/// Persist a signed transaction and its first attempt, and move the job to
/// `executing`, in the caller's transaction. Fails with a unique violation
/// when the signer already has an open lane on the chain or the nonce is
/// already spent: both mean the caller's view was stale, never that
/// anything should be forced.
pub async fn begin_transaction(
    conn: &mut PgConnection,
    new: NewTransaction<'_>,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO execution.transactions
            (id, job_id, chain_id, signer, nonce, target, calldata, gas_limit)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(id)
    .bind(new.job_id)
    .bind(new.chain_id as i64)
    .bind(new.signer.as_slice())
    .bind(new.nonce as i64)
    .bind(new.target.as_slice())
    .bind(new.calldata.as_ref())
    .bind(new.gas_limit as i64)
    .execute(&mut *conn)
    .await?;
    insert_attempt(conn, id, new.attempt).await?;
    sqlx::query("UPDATE execution.jobs SET state = 'executing' WHERE id = $1 AND state = 'queued'")
        .bind(new.job_id)
        .execute(conn)
        .await?;
    Ok(id)
}

/// Persist a same-nonce replacement before it is broadcast.
pub async fn insert_attempt(
    conn: &mut PgConnection,
    transaction_id: Uuid,
    attempt: &Attempt,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO execution.transaction_attempts
            (transaction_id, replacement_number, tx_hash, raw_transaction,
             max_fee_per_gas, max_priority_fee_per_gas)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(transaction_id)
    .bind(attempt.replacement_number as i32)
    .bind(attempt.tx_hash.as_slice())
    .bind(attempt.raw_transaction.as_ref())
    .bind(attempt.max_fee_per_gas.to_string())
    .bind(attempt.max_priority_fee_per_gas.to_string())
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE execution.transactions SET submitted_at = now() WHERE id = $1")
        .bind(transaction_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// An attempt left the node: the pending-timeout clock restarts.
pub async fn mark_broadcast(
    conn: &mut PgConnection,
    transaction_id: Uuid,
    replacement_number: u32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE execution.transaction_attempts
        SET broadcast_at = COALESCE(broadcast_at, now())
        WHERE transaction_id = $1 AND replacement_number = $2
        "#,
    )
    .bind(transaction_id)
    .bind(replacement_number as i32)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        r#"
        UPDATE execution.transactions
        SET broadcast_at = COALESCE(broadcast_at, now()),
            submitted_at = now(),
            state = CASE WHEN state = 'prepared' THEN 'broadcast' ELSE state END
        WHERE id = $1
        "#,
    )
    .bind(transaction_id)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn record_mined(
    conn: &mut PgConnection,
    transaction_id: Uuid,
    mined: Mined,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE execution.transactions
        SET state = 'mined', mined_tx_hash = $2, mined_block = $3, mined_block_hash = $4,
            mined_transaction_index = $5, mined_succeeded = $6
        WHERE id = $1 AND resolved_at IS NULL
        "#,
    )
    .bind(transaction_id)
    .bind(mined.tx_hash.as_slice())
    .bind(mined.block as i64)
    .bind(mined.block_hash.as_slice())
    .bind(mined.transaction_index as i64)
    .bind(mined.succeeded)
    .execute(conn)
    .await?;
    Ok(())
}

/// The receipt's block left the canonical chain: forget it and wait for
/// another one.
pub async fn clear_mined(conn: &mut PgConnection, transaction_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE execution.transactions
        SET state = 'broadcast', mined_tx_hash = NULL, mined_block = NULL, mined_block_hash = NULL,
            mined_transaction_index = NULL, mined_succeeded = NULL
        WHERE id = $1 AND resolved_at IS NULL
        "#,
    )
    .bind(transaction_id)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn mark_stall_reported(
    conn: &mut PgConnection,
    transaction_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE execution.transactions SET stall_reported_at = now() WHERE id = $1")
        .bind(transaction_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Close a slot; its signer's lane is free from this commit on.
pub async fn resolve_transaction(
    conn: &mut PgConnection,
    transaction_id: Uuid,
    state: TxState,
) -> Result<(), sqlx::Error> {
    debug_assert!(matches!(
        state,
        TxState::Finalized | TxState::Reverted | TxState::Abandoned
    ));
    sqlx::query(
        r#"
        UPDATE execution.transactions
        SET state = $2, resolved_at = now()
        WHERE id = $1 AND resolved_at IS NULL
        "#,
    )
    .bind(transaction_id)
    .bind(state.as_str())
    .execute(conn)
    .await?;
    Ok(())
}

/// Every unresolved slot on `chain_id` with its job and attempts, oldest first.
pub async fn open_transactions(
    pool: &PgPool,
    chain_id: u64,
) -> Result<Vec<OpenTransaction>, sqlx::Error> {
    use sqlx::Row;
    let rows = sqlx::query(
        r#"
        SELECT t.id, t.signer, t.nonce, t.target, t.calldata, t.gas_limit, t.state,
               t.submitted_at, t.mined_tx_hash, t.mined_block, t.mined_block_hash,
               t.mined_transaction_index, t.mined_succeeded, t.stall_reported_at,
               j.id AS job_id, j.chain_id, j.correlation_id, j.command_message_id, j.command,
               j.state AS job_state, j.attempts
        FROM execution.transactions t
        JOIN execution.jobs j ON j.id = t.job_id
        WHERE t.chain_id = $1 AND t.resolved_at IS NULL
        ORDER BY t.created_at
        "#,
    )
    .bind(chain_id as i64)
    .fetch_all(pool)
    .await?;

    let mut transactions = Vec::with_capacity(rows.len());
    for row in &rows {
        let id: Uuid = row.try_get("id")?;
        let command: serde_json::Value = row.try_get("command")?;
        let job = Job {
            id: row.try_get("job_id")?,
            chain_id: unsigned(row.try_get("chain_id")?)?,
            correlation_id: row.try_get("correlation_id")?,
            command_message_id: row.try_get("command_message_id")?,
            command: serde_json::from_value(command)
                .map_err(|error| decode_error(error.to_string()))?,
            state: JobState::parse(row.try_get::<String, _>("job_state")?.as_str())?,
            attempts: u32::try_from(row.try_get::<i32, _>("attempts")?).unwrap_or(0),
        };
        let mined = match row.try_get::<Option<Vec<u8>>, _>("mined_tx_hash")? {
            Some(tx_hash) => Some(Mined {
                tx_hash: hash(&tx_hash)?,
                block: unsigned(row.try_get("mined_block")?)?,
                block_hash: hash(&row.try_get::<Vec<u8>, _>("mined_block_hash")?)?,
                transaction_index: unsigned(row.try_get("mined_transaction_index")?)?,
                succeeded: row.try_get("mined_succeeded")?,
            }),
            None => None,
        };
        transactions.push(OpenTransaction {
            id,
            job,
            signer: address(&row.try_get::<Vec<u8>, _>("signer")?)?,
            nonce: unsigned(row.try_get("nonce")?)?,
            target: address(&row.try_get::<Vec<u8>, _>("target")?)?,
            calldata: Bytes::from(row.try_get::<Vec<u8>, _>("calldata")?),
            gas_limit: unsigned(row.try_get("gas_limit")?)?,
            state: TxState::parse(row.try_get::<String, _>("state")?.as_str())?,
            submitted_at: row.try_get("submitted_at")?,
            mined,
            stall_reported_at: row.try_get("stall_reported_at")?,
            attempts: attempts(pool, id).await?,
        });
    }
    Ok(transactions)
}

async fn attempts(pool: &PgPool, transaction_id: Uuid) -> Result<Vec<Attempt>, sqlx::Error> {
    use sqlx::Row;
    let rows = sqlx::query(
        r#"
        SELECT replacement_number, tx_hash, raw_transaction, max_fee_per_gas,
               max_priority_fee_per_gas, broadcast_at
        FROM execution.transaction_attempts
        WHERE transaction_id = $1
        ORDER BY replacement_number
        "#,
    )
    .bind(transaction_id)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(Attempt {
                replacement_number: u32::try_from(row.try_get::<i32, _>("replacement_number")?)
                    .unwrap_or(0),
                tx_hash: hash(&row.try_get::<Vec<u8>, _>("tx_hash")?)?,
                raw_transaction: Bytes::from(row.try_get::<Vec<u8>, _>("raw_transaction")?),
                max_fee_per_gas: u128_text(&row.try_get::<String, _>("max_fee_per_gas")?)?,
                max_priority_fee_per_gas: u128_text(
                    &row.try_get::<String, _>("max_priority_fee_per_gas")?,
                )?,
                broadcast_at: row.try_get("broadcast_at")?,
            })
        })
        .collect()
}

/// Signers with an open lane on `chain_id`.
pub async fn busy_signers(pool: &PgPool, chain_id: u64) -> Result<Vec<Address>, sqlx::Error> {
    let rows: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT signer FROM execution.transactions WHERE chain_id = $1 AND resolved_at IS NULL",
    )
    .bind(chain_id as i64)
    .fetch_all(pool)
    .await?;
    rows.iter().map(|bytes| address(bytes)).collect()
}

// ---------------------------------------------------------------------------
// Chain control and status
// ---------------------------------------------------------------------------

pub async fn apply_chain_control(
    conn: &mut PgConnection,
    control: &ChainControl,
) -> Result<(), sqlx::Error> {
    match control {
        ChainControl::Halted {
            fault_id,
            chain_id,
            reason,
        } => {
            sqlx::query(
                r#"
                INSERT INTO execution.chain_halts (chain_id, fault_id, reason)
                VALUES ($1, $2, $3)
                ON CONFLICT (chain_id) DO UPDATE
                SET fault_id = EXCLUDED.fault_id, reason = EXCLUDED.reason, halted_at = now()
                "#,
            )
            .bind(*chain_id as i64)
            .bind(fault_id)
            .bind(reason)
            .execute(conn)
            .await?;
        }
        ChainControl::Resumed { fault_id, chain_id } => {
            sqlx::query("DELETE FROM execution.chain_halts WHERE chain_id = $1 AND fault_id = $2")
                .bind(*chain_id as i64)
                .bind(fault_id)
                .execute(conn)
                .await?;
        }
    }
    Ok(())
}

pub async fn is_halted(pool: &PgPool, chain_id: u64) -> Result<bool, sqlx::Error> {
    let halted: Option<i64> =
        sqlx::query_scalar("SELECT chain_id FROM execution.chain_halts WHERE chain_id = $1")
            .bind(chain_id as i64)
            .fetch_optional(pool)
            .await?;
    Ok(halted.is_some())
}

pub async fn upsert_executor_status(
    pool: &PgPool,
    chain_id: u64,
    state: &str,
    detail: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO execution.executor_status (chain_id, state, detail, heartbeat_at)
        VALUES ($1, $2, $3, now())
        ON CONFLICT (chain_id) DO UPDATE
        SET state = EXCLUDED.state, detail = EXCLUDED.detail, heartbeat_at = now()
        "#,
    )
    .bind(chain_id as i64)
    .bind(state)
    .bind(detail)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn upsert_signer_status(
    pool: &PgPool,
    chain_id: u64,
    signer: Address,
    balance: Result<(U256, bool), &str>,
) -> Result<(), sqlx::Error> {
    let (balance_wei, low_balance, error) = match balance {
        Ok((balance, low)) => (Some(balance.to_string()), low, None),
        Err(error) => (None, false, Some(error)),
    };
    sqlx::query(
        r#"
        INSERT INTO execution.signer_status (chain_id, signer, balance_wei, low_balance, error, observed_at)
        VALUES ($1, $2, $3, $4, $5, now())
        ON CONFLICT (chain_id, signer) DO UPDATE
        SET balance_wei = COALESCE(EXCLUDED.balance_wei, execution.signer_status.balance_wei),
            low_balance = EXCLUDED.low_balance, error = EXCLUDED.error, observed_at = now()
        "#,
    )
    .bind(chain_id as i64)
    .bind(signer.as_slice())
    .bind(balance_wei)
    .bind(low_balance)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}
