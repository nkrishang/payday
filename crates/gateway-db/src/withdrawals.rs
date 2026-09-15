//! Merchant withdrawals and their legs.
//!
//! A withdrawal snapshots the Payday wallet's USDC on every chain into legs.
//! The merchant signs each leg's EIP-3009 authorization; `gateway-indexer`
//! relays the signed legs on the chains it serves. A leg's relay step keeps
//! the same durable shape as a sweep batch (one nonce of one pool signer,
//! every same-nonce replacement, the receipt once seen) and the same rule:
//! one step in flight per chain and signer, enforced by
//! `withdrawal_steps_open` next to `sweep_batches_open`.
//!
//! Leg states:
//!
//! ```text
//! awaiting_signature ─▶ authorized ─▶ relaying ─▶ completed          (transfer leg)
//!                                   └▶ relaying ─▶ burned ─▶ attested ─▶ minting ─▶ completed   (bridge leg)
//! any non-terminal ─▶ failed | expired | cancelled
//! ```

use std::time::Duration;

use alloy_primitives::{Address, B256, Bytes, U256};
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::accounts::AccountId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegKind {
    /// Funds already on the destination chain: one `transferWithAuthorization`.
    Transfer,
    /// Funds on another chain: `WithdrawalForwarder.bridge`, then the mint.
    Bridge,
}

impl LegKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transfer => "transfer",
            Self::Bridge => "bridge",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "transfer" => Some(Self::Transfer),
            "bridge" => Some(Self::Bridge),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegState {
    AwaitingSignature,
    Authorized,
    Relaying,
    Burned,
    Attested,
    Minting,
    Completed,
    Failed,
    Expired,
    Cancelled,
}

impl LegState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingSignature => "awaiting_signature",
            Self::Authorized => "authorized",
            Self::Relaying => "relaying",
            Self::Burned => "burned",
            Self::Attested => "attested",
            Self::Minting => "minting",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "awaiting_signature" => Self::AwaitingSignature,
            "authorized" => Self::Authorized,
            "relaying" => Self::Relaying,
            "burned" => Self::Burned,
            "attested" => Self::Attested,
            "minting" => Self::Minting,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "expired" => Self::Expired,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Expired | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct DbWithdrawal {
    pub id: Uuid,
    pub account_id: Uuid,
    pub idempotency_key: String,
    pub wallet_address: String,
    pub destination_chain_id: i64,
    pub destination_address: String,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub failed_at: Option<DateTime<Utc>>,
}

/// The relayer's in-flight transaction for a leg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayStep {
    pub chain_id: u64,
    /// The pool signer whose nonce this step owns.
    pub signer: Address,
    pub nonce: u64,
    pub gas_limit: u64,
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
    /// Every signed transaction hash prepared for this nonce, oldest first.
    pub tx_hashes: Vec<B256>,
    /// Signed EIP-2718 bytes corresponding one-for-one with `tx_hashes`.
    pub raw_transactions: Vec<Bytes>,
    /// When the newest transaction was broadcast (or persisted, before that).
    pub submitted_at: DateTime<Utc>,
    /// None means the newest signed transaction is durable but not yet known
    /// to have been broadcast.
    pub broadcast_at: Option<DateTime<Utc>>,
    /// Receipt seen for one of the submissions, awaiting finality.
    pub mined: Option<MinedStep>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinedStep {
    pub tx_hash: B256,
    pub block: u64,
    pub block_hash: B256,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbWithdrawalLeg {
    pub id: Uuid,
    pub withdrawal_id: Uuid,
    pub position: i16,
    pub kind: LegKind,
    pub source_chain_id: u64,
    pub destination_chain_id: u64,
    pub amount: U256,
    pub state: LegState,
    pub token_address: String,
    pub domain_name: String,
    pub domain_version: String,
    pub authorization_to: String,
    pub nonce: B256,
    pub salt: Option<B256>,
    pub valid_before: u64,
    pub signature: Option<Vec<u8>>,
    pub authorized_at: Option<DateTime<Utc>>,
    pub step: Option<RelayStep>,
    pub transfer_tx_hash: Option<B256>,
    pub burn_tx_hash: Option<B256>,
    pub mint_tx_hash: Option<B256>,
    pub attestation_message: Option<Vec<u8>>,
    pub attestation: Option<Vec<u8>>,
    pub attestation_next_check_at: Option<DateTime<Utc>>,
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct LegRow {
    id: Uuid,
    withdrawal_id: Uuid,
    position: i16,
    kind: String,
    source_chain_id: i64,
    destination_chain_id: i64,
    amount: String,
    state: String,
    token_address: String,
    domain_name: String,
    domain_version: String,
    authorization_to: String,
    nonce: Vec<u8>,
    salt: Option<Vec<u8>>,
    valid_before: i64,
    signature: Option<Vec<u8>>,
    authorized_at: Option<DateTime<Utc>>,
    step_chain_id: Option<i64>,
    step_signer: Option<Vec<u8>>,
    step_nonce: Option<i64>,
    step_gas_limit: Option<i64>,
    step_max_fee_per_gas: Option<String>,
    step_max_priority_fee_per_gas: Option<String>,
    step_tx_hashes: Option<Vec<Vec<u8>>>,
    step_raw_transactions: Option<Vec<Vec<u8>>>,
    step_submitted_at: Option<DateTime<Utc>>,
    step_broadcast_at: Option<DateTime<Utc>>,
    step_mined_tx_hash: Option<Vec<u8>>,
    step_mined_block: Option<i64>,
    step_mined_block_hash: Option<Vec<u8>>,
    transfer_tx_hash: Option<Vec<u8>>,
    burn_tx_hash: Option<Vec<u8>>,
    mint_tx_hash: Option<Vec<u8>>,
    attestation_message: Option<Vec<u8>>,
    attestation: Option<Vec<u8>>,
    attestation_next_check_at: Option<DateTime<Utc>>,
    failure_reason: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn decode_error(message: String) -> sqlx::Error {
    sqlx::Error::Decode(message.into())
}

fn b256(name: &str, value: Option<Vec<u8>>) -> Result<Option<B256>, sqlx::Error> {
    value
        .map(|bytes| {
            B256::try_from(bytes.as_slice())
                .map_err(|_| decode_error(format!("invalid {name} length in withdrawal leg")))
        })
        .transpose()
}

impl TryFrom<LegRow> for DbWithdrawalLeg {
    type Error = sqlx::Error;

    fn try_from(row: LegRow) -> Result<Self, Self::Error> {
        let fee = |name: &str, value: Option<String>| {
            value
                .ok_or_else(|| decode_error(format!("missing {name} in withdrawal leg step")))?
                .parse::<u128>()
                .map_err(|_| decode_error(format!("invalid {name} in withdrawal leg step")))
        };
        let step = match row.step_chain_id {
            None => None,
            Some(chain_id) => {
                let mined = match (
                    b256("step_mined_tx_hash", row.step_mined_tx_hash)?,
                    row.step_mined_block,
                    b256("step_mined_block_hash", row.step_mined_block_hash)?,
                ) {
                    (Some(tx_hash), Some(block), Some(block_hash)) => Some(MinedStep {
                        tx_hash,
                        block: block as u64,
                        block_hash,
                    }),
                    _ => None,
                };
                let signer = row
                    .step_signer
                    .as_deref()
                    .ok_or_else(|| {
                        decode_error("missing step_signer in withdrawal leg step".into())
                    })
                    .and_then(|bytes| {
                        Address::try_from(bytes).map_err(|_| {
                            decode_error("invalid step_signer length in withdrawal leg step".into())
                        })
                    })?;
                Some(RelayStep {
                    chain_id: chain_id as u64,
                    signer,
                    nonce: row.step_nonce.unwrap_or_default() as u64,
                    gas_limit: row.step_gas_limit.unwrap_or_default() as u64,
                    max_fee_per_gas: fee("step_max_fee_per_gas", row.step_max_fee_per_gas)?,
                    max_priority_fee_per_gas: fee(
                        "step_max_priority_fee_per_gas",
                        row.step_max_priority_fee_per_gas,
                    )?,
                    tx_hashes: row
                        .step_tx_hashes
                        .unwrap_or_default()
                        .into_iter()
                        .map(|hash| b256("step_tx_hash", Some(hash)).map(Option::unwrap))
                        .collect::<Result<_, _>>()?,
                    raw_transactions: row
                        .step_raw_transactions
                        .unwrap_or_default()
                        .into_iter()
                        .map(Bytes::from)
                        .collect(),
                    submitted_at: row.step_submitted_at.ok_or_else(|| {
                        decode_error("missing step_submitted_at in withdrawal leg".into())
                    })?,
                    broadcast_at: row.step_broadcast_at,
                    mined,
                })
            }
        };
        Ok(Self {
            id: row.id,
            withdrawal_id: row.withdrawal_id,
            position: row.position,
            kind: LegKind::parse(&row.kind)
                .ok_or_else(|| decode_error(format!("unknown withdrawal leg kind {}", row.kind)))?,
            source_chain_id: row.source_chain_id as u64,
            destination_chain_id: row.destination_chain_id as u64,
            amount: U256::from_str_radix(&row.amount, 10)
                .map_err(|_| decode_error("invalid withdrawal leg amount".into()))?,
            state: LegState::parse(&row.state).ok_or_else(|| {
                decode_error(format!("unknown withdrawal leg state {}", row.state))
            })?,
            token_address: row.token_address,
            domain_name: row.domain_name,
            domain_version: row.domain_version,
            authorization_to: row.authorization_to,
            nonce: b256("nonce", Some(row.nonce))?.expect("present"),
            salt: b256("salt", row.salt)?,
            valid_before: row.valid_before as u64,
            signature: row.signature,
            authorized_at: row.authorized_at,
            step,
            transfer_tx_hash: b256("transfer_tx_hash", row.transfer_tx_hash)?,
            burn_tx_hash: b256("burn_tx_hash", row.burn_tx_hash)?,
            mint_tx_hash: b256("mint_tx_hash", row.mint_tx_hash)?,
            attestation_message: row.attestation_message,
            attestation: row.attestation,
            attestation_next_check_at: row.attestation_next_check_at,
            failure_reason: row.failure_reason,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

/// What `POST /v1/withdrawals` computed: the destination and one leg per
/// chain the wallet holds USDC on.
#[derive(Debug, Clone)]
pub struct NewWithdrawal {
    pub id: Uuid,
    pub account_id: AccountId,
    pub idempotency_key: String,
    pub wallet_address: String,
    pub destination_chain_id: u64,
    pub destination_address: String,
    pub legs: Vec<NewWithdrawalLeg>,
}

#[derive(Debug, Clone)]
pub struct NewWithdrawalLeg {
    pub id: Uuid,
    pub position: i16,
    pub kind: LegKind,
    pub source_chain_id: u64,
    pub amount: U256,
    pub token_address: String,
    pub domain_name: String,
    pub domain_version: String,
    pub authorization_to: String,
    pub nonce: B256,
    pub salt: Option<B256>,
    pub valid_before: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateWithdrawalError {
    #[error("the account already has a withdrawal in progress")]
    InProgress,
    #[error("the idempotency key was already used")]
    IdempotencyKeyReused,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizeOutcome {
    Authorized,
    /// Already authorized with this exact signature.
    Unchanged,
    /// Already authorized with a different signature, or past signing.
    NotAwaitingSignature,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome {
    Cancelled,
    AlreadyCancelled,
    /// Completed or failed; nothing left to cancel.
    Finished,
    /// A leg has been relayed; its funds are moving and cannot be recalled.
    InProgress,
    NotFound,
}

/// How a relay step ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOutcome {
    /// A transfer leg's `transferWithAuthorization` mined and succeeded.
    Transferred { tx_hash: B256 },
    /// A bridge leg's burn mined and succeeded; Circle attests it next.
    Burned {
        tx_hash: B256,
        next_check_at: DateTime<Utc>,
    },
    /// The mint landed: ours (`Some`), or somebody else's, since
    /// `receiveMessage` is permissionless (`None`).
    Minted { tx_hash: Option<B256> },
}

/// How a retried step was resolved: put back in its queue, or failed for
/// good once the reverts ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryStep {
    Retried,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RelayStats {
    /// Authorized legs waiting to be relayed from this chain.
    pub authorized: i64,
    /// Burned legs waiting for Circle's attestation, by source chain.
    pub awaiting_attestation: i64,
    /// Attested legs waiting to be minted on this chain.
    pub attested: i64,
    /// Whether a step is in flight on this chain.
    pub in_flight: bool,
}

#[derive(Clone)]
pub struct WithdrawalRepository {
    pool: PgPool,
}

/// Terminal leg states, for the settlement queries.
const TERMINAL: [&str; 4] = ["completed", "failed", "expired", "cancelled"];

impl WithdrawalRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // --- Merchant side -----------------------------------------------------

    pub async fn create(&self, input: &NewWithdrawal) -> Result<(), CreateWithdrawalError> {
        let mut tx = self.pool.begin().await?;
        let inserted = sqlx::query(
            r#"INSERT INTO withdrawals
                   (id, account_id, idempotency_key, wallet_address, destination_chain_id, destination_address)
               VALUES ($1, $2, $3, $4, $5, $6)"#,
        )
        .bind(input.id)
        .bind(input.account_id.0)
        .bind(&input.idempotency_key)
        .bind(&input.wallet_address)
        .bind(input.destination_chain_id as i64)
        .bind(&input.destination_address)
        .execute(&mut *tx)
        .await;
        if let Err(sqlx::Error::Database(error)) = &inserted {
            match error.constraint() {
                Some("withdrawals_open") => return Err(CreateWithdrawalError::InProgress),
                Some("withdrawals_idempotency") => {
                    return Err(CreateWithdrawalError::IdempotencyKeyReused);
                }
                _ => {}
            }
        }
        inserted?;
        for leg in &input.legs {
            let kind = leg.kind.as_str();
            sqlx::query(
                r#"INSERT INTO withdrawal_legs
                       (id, withdrawal_id, position, kind, source_chain_id, destination_chain_id, amount,
                        state, token_address, domain_name, domain_version, authorization_kind,
                        authorization_to, nonce, salt, valid_before)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, 'awaiting_signature', $8, $9, $10, $11, $12, $13, $14, $15)"#,
            )
            .bind(leg.id)
            .bind(input.id)
            .bind(leg.position)
            .bind(kind)
            .bind(leg.source_chain_id as i64)
            .bind(input.destination_chain_id as i64)
            .bind(leg.amount.to_string())
            .bind(&leg.token_address)
            .bind(&leg.domain_name)
            .bind(&leg.domain_version)
            .bind(match leg.kind {
                LegKind::Transfer => "transfer",
                LegKind::Bridge => "receive",
            })
            .bind(&leg.authorization_to)
            .bind(leg.nonce.as_slice())
            .bind(leg.salt.map(|salt| salt.to_vec()))
            .bind(leg.valid_before as i64)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn find_by_idempotency_key(
        &self,
        account: AccountId,
        key: &str,
    ) -> Result<Option<DbWithdrawal>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM withdrawals WHERE account_id = $1 AND idempotency_key = $2")
            .bind(account.0)
            .bind(key)
            .fetch_optional(&self.pool)
            .await
    }

    /// A withdrawal by id alone, for the relayer.
    pub async fn by_id(&self, id: Uuid) -> Result<Option<DbWithdrawal>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM withdrawals WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn get(
        &self,
        account: AccountId,
        id: Uuid,
    ) -> Result<Option<DbWithdrawal>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM withdrawals WHERE account_id = $1 AND id = $2")
            .bind(account.0)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    /// Newest first, plus one row beyond `limit` so the caller can tell
    /// whether another page exists. A foreign cursor matches nothing.
    pub async fn list(
        &self,
        account: AccountId,
        limit: u32,
        starting_after: Option<Uuid>,
    ) -> Result<Vec<DbWithdrawal>, sqlx::Error> {
        sqlx::query_as(
            r#"SELECT candidate.* FROM withdrawals candidate
               LEFT JOIN withdrawals cursor ON cursor.account_id = $1 AND cursor.id = $2
               WHERE candidate.account_id = $1
                 AND ($2 IS NULL OR (candidate.created_at, candidate.id) < (cursor.created_at, cursor.id))
               ORDER BY candidate.created_at DESC, candidate.id DESC LIMIT $3"#,
        )
        .bind(account.0)
        .bind(starting_after)
        .bind(i64::from(limit) + 1)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn legs(&self, withdrawal_id: Uuid) -> Result<Vec<DbWithdrawalLeg>, sqlx::Error> {
        self.legs_for(&[withdrawal_id]).await
    }

    /// Every leg of every listed withdrawal, ordered by withdrawal then position.
    pub async fn legs_for(
        &self,
        withdrawal_ids: &[Uuid],
    ) -> Result<Vec<DbWithdrawalLeg>, sqlx::Error> {
        sqlx::query_as::<_, LegRow>(
            "SELECT * FROM withdrawal_legs WHERE withdrawal_id = ANY($1) ORDER BY withdrawal_id, position",
        )
        .bind(withdrawal_ids)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(DbWithdrawalLeg::try_from)
        .collect()
    }

    /// Record the merchant's signature over one leg. The caller has verified
    /// it; this only refuses a leg that is not waiting for one.
    pub async fn authorize(
        &self,
        account: AccountId,
        withdrawal_id: Uuid,
        leg_id: Uuid,
        signature: &[u8],
    ) -> Result<AuthorizeOutcome, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let current: Option<(String, Option<Vec<u8>>)> = sqlx::query_as(
            r#"SELECT l.state, l.signature FROM withdrawal_legs l
               JOIN withdrawals w ON w.id = l.withdrawal_id
               WHERE w.account_id = $1 AND w.id = $2 AND l.id = $3 AND w.cancelled_at IS NULL
               FOR UPDATE OF l"#,
        )
        .bind(account.0)
        .bind(withdrawal_id)
        .bind(leg_id)
        .fetch_optional(&mut *tx)
        .await?;
        let outcome = match current {
            None => AuthorizeOutcome::NotFound,
            Some((state, _)) if state == "awaiting_signature" => {
                sqlx::query(
                    r#"UPDATE withdrawal_legs
                       SET state = 'authorized', signature = $2, authorized_at = now(), updated_at = now()
                       WHERE id = $1"#,
                )
                .bind(leg_id)
                .bind(signature)
                .execute(&mut *tx)
                .await?;
                AuthorizeOutcome::Authorized
            }
            Some((_, Some(existing))) if existing == signature => AuthorizeOutcome::Unchanged,
            Some(_) => AuthorizeOutcome::NotAwaitingSignature,
        };
        tx.commit().await?;
        Ok(outcome)
    }

    /// Cancel while nothing has been relayed. Authorized-but-unsent legs are
    /// cancelled too; their signatures are never used.
    pub async fn cancel(
        &self,
        account: AccountId,
        withdrawal_id: Uuid,
    ) -> Result<CancelOutcome, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let row: Option<DbWithdrawal> = sqlx::query_as(
            "SELECT * FROM withdrawals WHERE account_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(account.0)
        .bind(withdrawal_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(CancelOutcome::NotFound);
        };
        if row.cancelled_at.is_some() {
            return Ok(CancelOutcome::AlreadyCancelled);
        }
        if row.completed_at.is_some() || row.failed_at.is_some() {
            return Ok(CancelOutcome::Finished);
        }
        let relayed: bool = sqlx::query_scalar(
            r#"SELECT EXISTS (SELECT 1 FROM withdrawal_legs
               WHERE withdrawal_id = $1 AND state NOT IN ('awaiting_signature', 'authorized', 'expired')
               FOR UPDATE)"#,
        )
        .bind(withdrawal_id)
        .fetch_one(&mut *tx)
        .await?;
        if relayed {
            return Ok(CancelOutcome::InProgress);
        }
        sqlx::query(
            r#"UPDATE withdrawal_legs SET state = 'cancelled', updated_at = now()
               WHERE withdrawal_id = $1 AND state IN ('awaiting_signature', 'authorized')"#,
        )
        .bind(withdrawal_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE withdrawals SET cancelled_at = now() WHERE id = $1")
            .bind(withdrawal_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(CancelOutcome::Cancelled)
    }

    // --- Relayer side -----------------------------------------------------

    /// The chain's in-flight steps, at most one per pool signer, oldest first.
    pub async fn open_steps(&self, chain_id: u64) -> Result<Vec<DbWithdrawalLeg>, sqlx::Error> {
        sqlx::query_as::<_, LegRow>(
            "SELECT * FROM withdrawal_legs WHERE step_chain_id = $1 ORDER BY step_submitted_at, id",
        )
        .bind(chain_id as i64)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(DbWithdrawalLeg::try_from)
        .collect()
    }

    /// The next leg this chain's signer should act on: an attested leg to
    /// mint here first (its burn already happened), then the oldest
    /// authorized leg whose authorization still has `min_validity` left.
    /// `excluded_ids` keeps legs another concurrent preparation already
    /// claimed out of the selection until their submission is durable.
    pub async fn next_relayable(
        &self,
        chain_id: u64,
        min_validity: Duration,
        excluded_ids: &[Uuid],
    ) -> Result<Option<DbWithdrawalLeg>, sqlx::Error> {
        sqlx::query_as::<_, LegRow>(
            r#"SELECT l.* FROM withdrawal_legs l
               JOIN withdrawals w ON w.id = l.withdrawal_id
               WHERE w.cancelled_at IS NULL
                 AND NOT (l.id = ANY($3))
                 AND (l.step_retry_at IS NULL OR l.step_retry_at <= now())
                 AND (
                   (l.state = 'attested' AND l.destination_chain_id = $1)
                   OR (l.state = 'authorized' AND l.source_chain_id = $1
                       AND l.valid_before > extract(epoch FROM now()) + $2))
               ORDER BY (l.state = 'attested') DESC, l.created_at, l.position
               LIMIT 1"#,
        )
        .bind(chain_id as i64)
        .bind(min_validity.as_secs_f64())
        .bind(excluded_ids)
        .fetch_optional(&self.pool)
        .await?
        .map(DbWithdrawalLeg::try_from)
        .transpose()
    }

    /// Persist a signed step before it is broadcast. Moves an authorized leg
    /// to `relaying` or an attested leg to `minting`; false if the leg is no
    /// longer in either state (cancelled or expired meanwhile), in which case
    /// the caller must not broadcast.
    ///
    /// Locks the withdrawal row first, like every mutation here: cancellation
    /// and step recording then cannot interleave, so a leg whose step is
    /// recorded is never cancelled with its transaction in flight.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_step_submission(
        &self,
        leg_id: Uuid,
        chain_id: u64,
        signer: Address,
        nonce: u64,
        gas_limit: u64,
        max_fee_per_gas: u128,
        max_priority_fee_per_gas: u128,
        tx_hash: B256,
        raw_transaction: &[u8],
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let open: Option<bool> = sqlx::query_scalar(
            r#"SELECT w.cancelled_at IS NULL FROM withdrawal_legs l
               JOIN withdrawals w ON w.id = l.withdrawal_id
               WHERE l.id = $1 FOR UPDATE OF w"#,
        )
        .bind(leg_id)
        .fetch_optional(&mut *tx)
        .await?;
        match open {
            None | Some(false) => return Ok(false),
            Some(true) => {}
        }
        let recorded = sqlx::query(
            r#"UPDATE withdrawal_legs
               SET state = CASE state WHEN 'authorized' THEN 'relaying' ELSE 'minting' END,
                   step_chain_id = $2, step_signer = $9, step_nonce = $3, step_gas_limit = $4,
                   step_max_fee_per_gas = $5, step_max_priority_fee_per_gas = $6,
                   step_tx_hashes = ARRAY[$7::bytea], step_raw_transactions = ARRAY[$8::bytea],
                   step_submitted_at = now(), step_broadcast_at = NULL, updated_at = now()
               WHERE id = $1 AND step_chain_id IS NULL AND state IN ('authorized', 'attested')"#,
        )
        .bind(leg_id)
        .bind(chain_id as i64)
        .bind(nonce as i64)
        .bind(gas_limit as i64)
        .bind(max_fee_per_gas.to_string())
        .bind(max_priority_fee_per_gas.to_string())
        .bind(tx_hash.as_slice())
        .bind(raw_transaction)
        .bind(signer.as_slice())
        .execute(&mut *tx)
        .await
        .map(|result| result.rows_affected() > 0);
        match recorded? {
            true => {
                tx.commit().await?;
                Ok(true)
            }
            false => Ok(false),
        }
    }

    /// Persist a signed same-nonce replacement before broadcasting it. The
    /// pending-timeout clock restarts with the replacement: an unacknowledged
    /// predecessor's expired timeout must not immediately consume the
    /// replacement's replacement budget.
    pub async fn record_step_replacement(
        &self,
        leg_id: Uuid,
        tx_hash: B256,
        max_fee_per_gas: u128,
        max_priority_fee_per_gas: u128,
        raw_transaction: &[u8],
    ) -> Result<bool, sqlx::Error> {
        sqlx::query(
            r#"UPDATE withdrawal_legs
               SET step_tx_hashes = array_append(step_tx_hashes, $2),
                   step_raw_transactions = array_append(step_raw_transactions, $3),
                   step_max_fee_per_gas = $4, step_max_priority_fee_per_gas = $5,
                   step_broadcast_at = NULL, step_submitted_at = now(), updated_at = now()
               WHERE id = $1 AND step_chain_id IS NOT NULL"#,
        )
        .bind(leg_id)
        .bind(tx_hash.as_slice())
        .bind(raw_transaction)
        .bind(max_fee_per_gas.to_string())
        .bind(max_priority_fee_per_gas.to_string())
        .execute(&self.pool)
        .await
        .map(|result| result.rows_affected() > 0)
    }

    /// Mark the newest durable transaction as broadcast.
    pub async fn record_step_broadcast(
        &self,
        leg_id: Uuid,
        tx_hash: B256,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query(
            r#"UPDATE withdrawal_legs
               SET step_broadcast_at = now(), step_submitted_at = now(), updated_at = now()
               WHERE id = $1 AND step_chain_id IS NOT NULL AND step_broadcast_at IS NULL
                 AND step_tx_hashes[cardinality(step_tx_hashes)] = $2"#,
        )
        .bind(leg_id)
        .bind(tx_hash.as_slice())
        .execute(&self.pool)
        .await
        .map(|result| result.rows_affected() > 0)
    }

    pub async fn record_step_mined(
        &self,
        leg_id: Uuid,
        mined: MinedStep,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query(
            r#"UPDATE withdrawal_legs
               SET step_mined_tx_hash = $2, step_mined_block = $3, step_mined_block_hash = $4, updated_at = now()
               WHERE id = $1 AND step_chain_id IS NOT NULL"#,
        )
        .bind(leg_id)
        .bind(mined.tx_hash.as_slice())
        .bind(mined.block as i64)
        .bind(mined.block_hash.as_slice())
        .execute(&self.pool)
        .await
        .map(|result| result.rows_affected() > 0)
    }

    pub async fn clear_step_mined(&self, leg_id: Uuid) -> Result<bool, sqlx::Error> {
        sqlx::query(
            r#"UPDATE withdrawal_legs
               SET step_mined_tx_hash = NULL, step_mined_block = NULL, step_mined_block_hash = NULL,
                   step_submitted_at = now(), updated_at = now()
               WHERE id = $1 AND step_chain_id IS NOT NULL"#,
        )
        .bind(leg_id)
        .execute(&self.pool)
        .await
        .map(|result| result.rows_affected() > 0)
    }

    /// An attested leg's message was already minted by another party:
    /// complete the leg without any step of our own, parent-locked and
    /// settled like every other transition.
    pub async fn complete_minted_elsewhere(&self, leg_id: Uuid) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        lock_withdrawal_of(&mut tx, leg_id).await?;
        let updated: Option<Uuid> = sqlx::query_scalar(
            r#"UPDATE withdrawal_legs
               SET state = 'completed', step_reverts = 0, step_retry_at = NULL, updated_at = now()
               WHERE id = $1 AND state = 'attested'
               RETURNING withdrawal_id"#,
        )
        .bind(leg_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(withdrawal_id) = updated else {
            return Ok(false);
        };
        settle(&mut tx, withdrawal_id).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// The step's transaction is final and succeeded.
    pub async fn complete_step(
        &self,
        leg_id: Uuid,
        outcome: StepOutcome,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // The parent first: two legs reaching terminal states concurrently
        // then serialize, and whichever runs second settles the withdrawal
        // against the first one's outcome.
        lock_withdrawal_of(&mut tx, leg_id).await?;
        // `outcome_kind` selects which hash column the step's transaction
        // lands in; the others keep their values.
        let (state, outcome_kind, tx_hash, next_check_at) = match outcome {
            StepOutcome::Transferred { tx_hash } => ("completed", "transfer", Some(tx_hash), None),
            StepOutcome::Burned {
                tx_hash,
                next_check_at,
            } => ("burned", "burn", Some(tx_hash), Some(next_check_at)),
            StepOutcome::Minted { tx_hash } => ("completed", "mint", tx_hash, None),
        };
        let expected_state = match outcome {
            StepOutcome::Minted { .. } => "minting",
            _ => "relaying",
        };
        let updated: Option<Uuid> = sqlx::query_scalar(
            r#"UPDATE withdrawal_legs
               SET state = $2,
                   transfer_tx_hash = CASE WHEN $6 = 'transfer' THEN $3 ELSE transfer_tx_hash END,
                   burn_tx_hash = CASE WHEN $6 = 'burn' THEN $3 ELSE burn_tx_hash END,
                   mint_tx_hash = CASE WHEN $6 = 'mint' THEN $3 ELSE mint_tx_hash END,
                   attestation_next_check_at = $4,
                   step_chain_id = NULL, step_signer = NULL, step_nonce = NULL, step_gas_limit = NULL,
                   step_max_fee_per_gas = NULL, step_max_priority_fee_per_gas = NULL,
                   step_tx_hashes = NULL, step_raw_transactions = NULL, step_submitted_at = NULL,
                   step_broadcast_at = NULL, step_mined_tx_hash = NULL, step_mined_block = NULL,
                   step_mined_block_hash = NULL, step_reverts = 0, step_retry_at = NULL,
                   updated_at = now()
               WHERE id = $1 AND step_chain_id IS NOT NULL AND state = $5
               RETURNING withdrawal_id"#,
        )
        .bind(leg_id)
        .bind(state)
        .bind(tx_hash.map(|hash| hash.to_vec()))
        .bind(next_check_at)
        .bind(expected_state)
        .bind(outcome_kind)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(withdrawal_id) = updated else {
            return Ok(false);
        };
        settle(&mut tx, withdrawal_id).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// The leg cannot proceed: a reverted transaction, a burn Circle will not
    /// attest, or anything else no retry fixes. Its funds are wherever the
    /// last successful step left them, which `failure_reason` explains.
    pub async fn fail_leg(&self, leg_id: Uuid, reason: &str) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        lock_withdrawal_of(&mut tx, leg_id).await?;
        let updated: Option<Uuid> = sqlx::query_scalar(
            r#"UPDATE withdrawal_legs
               SET state = 'failed', failure_reason = $2,
                   step_chain_id = NULL, step_signer = NULL, step_nonce = NULL, step_gas_limit = NULL,
                   step_max_fee_per_gas = NULL, step_max_priority_fee_per_gas = NULL,
                   step_tx_hashes = NULL, step_raw_transactions = NULL, step_submitted_at = NULL,
                   step_broadcast_at = NULL, step_mined_tx_hash = NULL, step_mined_block = NULL,
                   step_mined_block_hash = NULL, updated_at = now()
               WHERE id = $1 AND state NOT IN ('completed', 'failed', 'expired', 'cancelled')
               RETURNING withdrawal_id"#,
        )
        .bind(leg_id)
        .bind(reason)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(withdrawal_id) = updated else {
            return Ok(false);
        };
        settle(&mut tx, withdrawal_id).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Expire legs whose authorization will not be usable `margin` from now
    /// and were never relayed. Returns how many legs expired.
    pub async fn expire_stale(&self, margin: Duration) -> Result<u64, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Lock each withdrawal before touching its legs, like every mutation
        // here: a leg cannot expire while its step is being recorded or
        // resolved, and the settlement below reads a settled state.
        let stale: Vec<Uuid> = sqlx::query_scalar(
            r#"SELECT id FROM withdrawals WHERE id IN (
                   SELECT l.withdrawal_id FROM withdrawal_legs l
                   WHERE l.state IN ('awaiting_signature', 'authorized')
                     AND l.valid_before <= extract(epoch FROM now()) + $1)
               ORDER BY id FOR UPDATE"#,
        )
        .bind(margin.as_secs_f64())
        .fetch_all(&mut *tx)
        .await?;
        let withdrawals: Vec<Uuid> = if stale.is_empty() {
            Vec::new()
        } else {
            sqlx::query_scalar(
                r#"UPDATE withdrawal_legs SET state = 'expired', updated_at = now()
                   WHERE withdrawal_id = ANY($1)
                     AND state IN ('awaiting_signature', 'authorized')
                     AND valid_before <= extract(epoch FROM now()) + $2
                   RETURNING withdrawal_id"#,
            )
            .bind(&stale)
            .bind(margin.as_secs_f64())
            .fetch_all(&mut *tx)
            .await?
        };
        let expired = withdrawals.len() as u64;
        let mut distinct = withdrawals;
        distinct.sort();
        distinct.dedup();
        for withdrawal_id in distinct {
            settle(&mut tx, withdrawal_id).await?;
        }
        tx.commit().await?;
        Ok(expired)
    }

    /// The step's transaction reverted, but the leg's obligation survives —
    /// an unused authorization, or an attestation Circle has already signed.
    /// Clear the step and put the leg back in its queue, not to be picked
    /// again until the base `backoff`, doubled for every revert already on
    /// the leg, has passed. After `max_reverts` reverts the failure is
    /// permanent instead: the leg is failed with `reason`, which also
    /// settles the withdrawal. A leg that resolved in a concurrent
    /// transaction reads as retried; there is nothing left to do for it.
    pub async fn retry_step(
        &self,
        leg_id: Uuid,
        backoff: Duration,
        max_reverts: u32,
        reason: &str,
    ) -> Result<RetryStep, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        lock_withdrawal_of(&mut tx, leg_id).await?;
        let reverted: Option<(Uuid, i16)> = sqlx::query_as(
            r#"UPDATE withdrawal_legs
               SET state = CASE state WHEN 'relaying' THEN 'authorized' ELSE 'attested' END,
                   step_chain_id = NULL, step_signer = NULL, step_nonce = NULL, step_gas_limit = NULL,
                   step_max_fee_per_gas = NULL, step_max_priority_fee_per_gas = NULL,
                   step_tx_hashes = NULL, step_raw_transactions = NULL, step_submitted_at = NULL,
                   step_broadcast_at = NULL, step_mined_tx_hash = NULL, step_mined_block = NULL,
                   step_mined_block_hash = NULL, step_reverts = step_reverts + 1,
                   step_retry_at = now() + ($2 * power(2, LEAST(step_reverts, 6)) * interval '1 second'),
                   updated_at = now()
               WHERE id = $1 AND step_chain_id IS NOT NULL AND state IN ('relaying', 'minting')
               RETURNING withdrawal_id, step_reverts"#,
        )
        .bind(leg_id)
        .bind(backoff.as_secs_f64())
        .fetch_optional(&mut *tx)
        .await?;
        let Some((withdrawal_id, reverts)) = reverted else {
            return Ok(RetryStep::Retried);
        };
        if reverts as u32 >= max_reverts {
            sqlx::query(
                r#"UPDATE withdrawal_legs
                   SET state = 'failed', failure_reason = $2, step_retry_at = NULL, updated_at = now()
                   WHERE id = $1 AND state IN ('authorized', 'attested')"#,
            )
            .bind(leg_id)
            .bind(reason)
            .execute(&mut *tx)
            .await?;
            settle(&mut tx, withdrawal_id).await?;
            tx.commit().await?;
            return Ok(RetryStep::Failed);
        }
        tx.commit().await?;
        Ok(RetryStep::Retried)
    }

    /// Burned legs from this chain whose attestation is due to be polled.
    pub async fn legs_awaiting_attestation(
        &self,
        chain_id: u64,
    ) -> Result<Vec<DbWithdrawalLeg>, sqlx::Error> {
        sqlx::query_as::<_, LegRow>(
            r#"SELECT * FROM withdrawal_legs
               WHERE state = 'burned' AND source_chain_id = $1
                 AND (attestation_next_check_at IS NULL OR attestation_next_check_at <= now())
               ORDER BY created_at, position"#,
        )
        .bind(chain_id as i64)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(DbWithdrawalLeg::try_from)
        .collect()
    }

    pub async fn record_attestation(
        &self,
        leg_id: Uuid,
        message: &[u8],
        attestation: &[u8],
    ) -> Result<bool, sqlx::Error> {
        sqlx::query(
            r#"UPDATE withdrawal_legs
               SET state = 'attested', attestation_message = $2, attestation = $3,
                   attestation_next_check_at = NULL, updated_at = now()
               WHERE id = $1 AND state = 'burned'"#,
        )
        .bind(leg_id)
        .bind(message)
        .bind(attestation)
        .execute(&self.pool)
        .await
        .map(|result| result.rows_affected() > 0)
    }

    pub async fn defer_attestation(
        &self,
        leg_id: Uuid,
        next_check_at: DateTime<Utc>,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query(
            r#"UPDATE withdrawal_legs SET attestation_next_check_at = $2, updated_at = now()
               WHERE id = $1 AND state = 'burned'"#,
        )
        .bind(leg_id)
        .bind(next_check_at)
        .execute(&self.pool)
        .await
        .map(|result| result.rows_affected() > 0)
    }

    pub async fn relay_stats(&self, chain_id: u64) -> Result<RelayStats, sqlx::Error> {
        let (authorized, awaiting_attestation, attested, in_flight): (i64, i64, i64, bool) =
            sqlx::query_as(
                r#"SELECT
                       count(*) FILTER (WHERE state = 'authorized' AND source_chain_id = $1),
                       count(*) FILTER (WHERE state = 'burned' AND source_chain_id = $1),
                       count(*) FILTER (WHERE state = 'attested' AND destination_chain_id = $1),
                       bool_or(step_chain_id = $1)
                   FROM withdrawal_legs
                   WHERE state IN ('authorized', 'burned', 'attested', 'relaying', 'minting')"#,
            )
            .bind(chain_id as i64)
            .fetch_optional(&self.pool)
            .await?
            .map(|(a, b, c, d): (i64, i64, i64, Option<bool>)| (a, b, c, d.unwrap_or(false)))
            .unwrap_or_default();
        Ok(RelayStats {
            authorized,
            awaiting_attestation,
            attested,
            in_flight,
        })
    }
}

/// Lock the withdrawal a leg belongs to, first, so that two mutations of
/// the same withdrawal's legs cannot interleave: whichever commits second
/// then re-reads the first one's outcome (`SET lock_timeout` is not set on
/// purpose — a relayer waiting out a slow cancellation is correct here).
async fn lock_withdrawal_of(
    tx: &mut Transaction<'_, Postgres>,
    leg_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"SELECT w.id FROM withdrawal_legs l
           JOIN withdrawals w ON w.id = l.withdrawal_id
           WHERE l.id = $1 FOR UPDATE OF w"#,
    )
    .bind(leg_id)
    .execute(&mut **tx)
    .await
    .map(drop)
}

/// Close the withdrawal once every leg is terminal: completed when all of
/// them completed, failed otherwise. A cancelled withdrawal stays cancelled.
async fn settle(
    tx: &mut Transaction<'_, Postgres>,
    withdrawal_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"UPDATE withdrawals w
           SET completed_at = CASE WHEN NOT EXISTS (
                   SELECT 1 FROM withdrawal_legs WHERE withdrawal_id = w.id AND state <> 'completed')
                   THEN now() END,
               failed_at = CASE WHEN EXISTS (
                   SELECT 1 FROM withdrawal_legs WHERE withdrawal_id = w.id AND state <> 'completed')
                   THEN now() END
           WHERE w.id = $1 AND w.completed_at IS NULL AND w.cancelled_at IS NULL AND w.failed_at IS NULL
             AND NOT EXISTS (
                 SELECT 1 FROM withdrawal_legs WHERE withdrawal_id = w.id AND state <> ALL($2))"#,
    )
    .bind(withdrawal_id)
    .bind(TERMINAL.as_slice())
    .execute(&mut **tx)
    .await
    .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn new_account(pool: &PgPool, id: u128) -> AccountId {
        let id = Uuid::from_u128(id);
        sqlx::query(
            "INSERT INTO accounts (id, api_key_hash, api_key_hint) VALUES ($1, $2, 'hint')",
        )
        .bind(id)
        .bind(id.as_bytes().repeat(2))
        .execute(pool)
        .await
        .unwrap();
        AccountId(id)
    }

    const WALLET: &str = "0x1111111111111111111111111111111111111111";
    const DESTINATION: &str = "0x2222222222222222222222222222222222222222";
    const FORWARDER: &str = "0x3333333333333333333333333333333333333333";
    const USDC: &str = "0x754704Bc059F8C67012fEd69BC8A327a5aafb603";

    fn leg(position: i16, kind: LegKind, source: u64, amount: u64) -> NewWithdrawalLeg {
        NewWithdrawalLeg {
            id: Uuid::now_v7(),
            position,
            kind,
            source_chain_id: source,
            amount: U256::from(amount),
            token_address: USDC.into(),
            domain_name: "USDC".into(),
            domain_version: "2".into(),
            authorization_to: match kind {
                LegKind::Transfer => DESTINATION.into(),
                LegKind::Bridge => FORWARDER.into(),
            },
            nonce: B256::repeat_byte(position as u8),
            salt: matches!(kind, LegKind::Bridge).then(|| B256::repeat_byte(0x55)),
            valid_before: 4_000_000_000,
        }
    }

    fn withdrawal(account: AccountId, key: &str) -> NewWithdrawal {
        NewWithdrawal {
            id: Uuid::now_v7(),
            account_id: account,
            idempotency_key: key.into(),
            wallet_address: WALLET.into(),
            destination_chain_id: 8453,
            destination_address: DESTINATION.into(),
            legs: vec![
                leg(0, LegKind::Bridge, 143, 5_000_000),
                leg(1, LegKind::Transfer, 8453, 1_000_000),
            ],
        }
    }

    const SIGNER: Address = Address::repeat_byte(0xA0);

    /// A withdrawal of `key` for a fresh account whose transfer leg on 8453
    /// is authorized; returns the account and that leg's id.
    async fn authorized_transfer_leg(
        pool: &PgPool,
        repo: &WithdrawalRepository,
        key: &str,
    ) -> (AccountId, Uuid) {
        let account = new_account(pool, key.bytes().map(u128::from).sum::<u128>() + 1000).await;
        let input = withdrawal(account, key);
        repo.create(&input).await.unwrap();
        let transfer = input.legs[1].id;
        repo.authorize(account, input.id, transfer, &[0x11u8; 65])
            .await
            .unwrap();
        (account, transfer)
    }

    async fn relay_transfer_leg(repo: &WithdrawalRepository, leg_id: Uuid) {
        assert!(
            repo.record_step_submission(
                leg_id,
                8453,
                SIGNER,
                7,
                90_000,
                100,
                10,
                B256::repeat_byte(0xA1),
                b"raw"
            )
            .await
            .unwrap()
        );
        assert!(
            repo.record_step_broadcast(leg_id, B256::repeat_byte(0xA1))
                .await
                .unwrap()
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn create_get_list_and_the_one_open_withdrawal_rule(pool: PgPool) {
        let repo = WithdrawalRepository::new(pool.clone());
        let account = new_account(&pool, 1).await;
        let first = withdrawal(account, "k1");
        repo.create(&first).await.unwrap();

        let stored = repo.get(account, first.id).await.unwrap().unwrap();
        assert_eq!(stored.destination_chain_id, 8453);
        assert_eq!(stored.wallet_address, WALLET);
        let legs = repo.legs(first.id).await.unwrap();
        assert_eq!(legs.len(), 2);
        assert_eq!(legs[0].kind, LegKind::Bridge);
        assert_eq!(legs[0].state, LegState::AwaitingSignature);
        assert_eq!(legs[0].amount, U256::from(5_000_000u64));
        assert_eq!(legs[0].salt, Some(B256::repeat_byte(0x55)));
        assert_eq!(legs[1].kind, LegKind::Transfer);
        assert_eq!(legs[1].salt, None);
        assert!(legs[1].step.is_none());

        assert!(matches!(
            repo.create(&withdrawal(account, "k2")).await,
            Err(CreateWithdrawalError::InProgress)
        ));
        assert_eq!(
            repo.find_by_idempotency_key(account, "k1")
                .await
                .unwrap()
                .unwrap()
                .id,
            first.id
        );
        assert!(repo.get(account, Uuid::now_v7()).await.unwrap().is_none());
        let other = new_account(&pool, 2).await;
        assert!(repo.get(other, first.id).await.unwrap().is_none());

        assert_eq!(
            repo.cancel(account, first.id).await.unwrap(),
            CancelOutcome::Cancelled
        );
        assert!(matches!(
            repo.create(&withdrawal(account, "k1")).await,
            Err(CreateWithdrawalError::IdempotencyKeyReused)
        ));
        let second = withdrawal(account, "k3");
        repo.create(&second).await.unwrap();

        let page = repo.list(account, 1, None).await.unwrap();
        assert_eq!(page.len(), 2, "one beyond the limit");
        assert_eq!(page[0].id, second.id);
        let rest = repo.list(account, 1, Some(page[0].id)).await.unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].id, first.id);
        assert!(repo.list(other, 10, None).await.unwrap().is_empty());
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn authorize_and_cancel_semantics(pool: PgPool) {
        let repo = WithdrawalRepository::new(pool.clone());
        let account = new_account(&pool, 1).await;
        let input = withdrawal(account, "k1");
        repo.create(&input).await.unwrap();
        let [bridge, transfer] = [input.legs[0].id, input.legs[1].id];
        let signature = [0x11u8; 65];

        assert_eq!(
            repo.authorize(account, input.id, bridge, &signature)
                .await
                .unwrap(),
            AuthorizeOutcome::Authorized
        );
        assert_eq!(
            repo.authorize(account, input.id, bridge, &signature)
                .await
                .unwrap(),
            AuthorizeOutcome::Unchanged
        );
        assert_eq!(
            repo.authorize(account, input.id, bridge, &[0x22u8; 65])
                .await
                .unwrap(),
            AuthorizeOutcome::NotAwaitingSignature
        );
        assert_eq!(
            repo.authorize(account, input.id, Uuid::now_v7(), &signature)
                .await
                .unwrap(),
            AuthorizeOutcome::NotFound
        );
        let other = new_account(&pool, 2).await;
        assert_eq!(
            repo.authorize(other, input.id, transfer, &signature)
                .await
                .unwrap(),
            AuthorizeOutcome::NotFound
        );
        let legs = repo.legs(input.id).await.unwrap();
        assert_eq!(legs[0].state, LegState::Authorized);
        assert_eq!(legs[0].signature.as_deref(), Some(&signature[..]));
        assert!(legs[0].authorized_at.is_some());

        // Authorized but unsent: still cancellable, and the leg is retired.
        assert_eq!(
            repo.cancel(other, input.id).await.unwrap(),
            CancelOutcome::NotFound
        );
        assert_eq!(
            repo.cancel(account, input.id).await.unwrap(),
            CancelOutcome::Cancelled
        );
        assert_eq!(
            repo.cancel(account, input.id).await.unwrap(),
            CancelOutcome::AlreadyCancelled
        );
        let legs = repo.legs(input.id).await.unwrap();
        assert!(legs.iter().all(|leg| leg.state == LegState::Cancelled));
        assert!(
            repo.next_relayable(143, Duration::ZERO, &[])
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            repo.authorize(account, input.id, transfer, &signature)
                .await
                .unwrap(),
            AuthorizeOutcome::NotFound,
            "a cancelled withdrawal takes no more signatures"
        );

        // Once a leg is relayed, cancellation is refused.
        let next = withdrawal(account, "k2");
        repo.create(&next).await.unwrap();
        repo.authorize(account, next.id, next.legs[1].id, &signature)
            .await
            .unwrap();
        relay_transfer_leg(&repo, next.legs[1].id).await;
        assert_eq!(
            repo.cancel(account, next.id).await.unwrap(),
            CancelOutcome::InProgress
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn relay_steps_carry_a_leg_to_completion(pool: PgPool) {
        let repo = WithdrawalRepository::new(pool.clone());
        let account = new_account(&pool, 1).await;
        let input = withdrawal(account, "k1");
        repo.create(&input).await.unwrap();
        let [bridge, transfer] = [input.legs[0].id, input.legs[1].id];
        let signature = [0x11u8; 65];

        // Nothing is relayable before a signature, and an expiring
        // authorization is not relayable either.
        assert!(
            repo.next_relayable(143, Duration::ZERO, &[])
                .await
                .unwrap()
                .is_none()
        );
        repo.authorize(account, input.id, bridge, &signature)
            .await
            .unwrap();
        repo.authorize(account, input.id, transfer, &signature)
            .await
            .unwrap();
        assert_eq!(
            repo.next_relayable(143, Duration::ZERO, &[])
                .await
                .unwrap()
                .unwrap()
                .id,
            bridge
        );
        assert_eq!(
            repo.next_relayable(8453, Duration::ZERO, &[])
                .await
                .unwrap()
                .unwrap()
                .id,
            transfer
        );
        assert!(
            repo.next_relayable(143, Duration::from_secs(10_000_000_000), &[])
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            repo.next_relayable(42_161, Duration::ZERO, &[])
                .await
                .unwrap()
                .is_none()
        );

        // Transfer leg: submit, broadcast, mine, complete.
        relay_transfer_leg(&repo, transfer).await;
        let open = repo.open_steps(8453).await.unwrap().remove(0);
        assert_eq!(open.id, transfer);
        assert_eq!(open.state, LegState::Relaying);
        let step = open.step.unwrap();
        assert_eq!(step.signer, SIGNER);
        assert_eq!(step.nonce, 7);
        assert_eq!(step.tx_hashes, vec![B256::repeat_byte(0xA1)]);
        assert_eq!(step.raw_transactions, vec![Bytes::from_static(b"raw")]);
        assert!(step.broadcast_at.is_some());
        assert!(
            !repo
                .record_step_submission(
                    bridge,
                    8453,
                    SIGNER,
                    8,
                    1,
                    1,
                    1,
                    B256::repeat_byte(0xA2),
                    b"x"
                )
                .await
                .is_ok_and(|ok| ok),
            "a second step on the chain's signer is refused by the open-step index"
        );
        assert!(
            repo.record_step_replacement(transfer, B256::repeat_byte(0xA3), 200, 20, b"raw2")
                .await
                .unwrap()
        );
        assert!(
            repo.record_step_broadcast(transfer, B256::repeat_byte(0xA3))
                .await
                .unwrap()
        );
        let mined = MinedStep {
            tx_hash: B256::repeat_byte(0xA3),
            block: 10,
            block_hash: B256::repeat_byte(0xB1),
        };
        assert!(repo.record_step_mined(transfer, mined).await.unwrap());
        assert_eq!(
            repo.open_steps(8453)
                .await
                .unwrap()
                .remove(0)
                .step
                .unwrap()
                .mined,
            Some(mined)
        );
        assert!(repo.clear_step_mined(transfer).await.unwrap());
        assert!(repo.record_step_mined(transfer, mined).await.unwrap());
        assert!(
            repo.complete_step(
                transfer,
                StepOutcome::Transferred {
                    tx_hash: mined.tx_hash
                }
            )
            .await
            .unwrap()
        );
        assert!(repo.open_steps(8453).await.unwrap().is_empty());
        let legs = repo.legs(input.id).await.unwrap();
        assert_eq!(legs[1].state, LegState::Completed);
        assert_eq!(legs[1].transfer_tx_hash, Some(B256::repeat_byte(0xA3)));
        assert!(legs[1].step.is_none());
        assert!(
            repo.get(account, input.id)
                .await
                .unwrap()
                .unwrap()
                .completed_at
                .is_none()
        );

        // Bridge leg: burn, attest, mint.
        assert!(
            repo.record_step_submission(
                bridge,
                143,
                SIGNER,
                3,
                250_000,
                50,
                5,
                B256::repeat_byte(0xC1),
                b"burn"
            )
            .await
            .unwrap()
        );
        assert!(
            matches!(
                repo.retry_step(bridge, Duration::from_secs(0), 5, "test")
                    .await
                    .unwrap(),
                RetryStep::Retried
            ),
            "a reverted step sends the leg back"
        );
        assert_eq!(
            repo.legs(input.id).await.unwrap()[0].state,
            LegState::Authorized
        );
        assert!(
            repo.record_step_submission(
                bridge,
                143,
                SIGNER,
                4,
                250_000,
                50,
                5,
                B256::repeat_byte(0xC2),
                b"burn"
            )
            .await
            .unwrap()
        );
        let due = Utc::now();
        assert!(
            repo.complete_step(
                bridge,
                StepOutcome::Burned {
                    tx_hash: B256::repeat_byte(0xC2),
                    next_check_at: due,
                }
            )
            .await
            .unwrap()
        );
        let waiting = repo.legs_awaiting_attestation(143).await.unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].burn_tx_hash, Some(B256::repeat_byte(0xC2)));
        assert!(
            repo.legs_awaiting_attestation(8453)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            repo.defer_attestation(bridge, Utc::now() + chrono::Duration::hours(1))
                .await
                .unwrap()
        );
        assert!(
            repo.legs_awaiting_attestation(143)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            repo.record_attestation(bridge, b"message", b"attestation")
                .await
                .unwrap()
        );
        assert!(
            repo.next_relayable(143, Duration::ZERO, &[])
                .await
                .unwrap()
                .is_none()
        );
        let mintable = repo
            .next_relayable(8453, Duration::ZERO, &[])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mintable.id, bridge);
        assert_eq!(mintable.state, LegState::Attested);
        assert_eq!(mintable.attestation.as_deref(), Some(&b"attestation"[..]));
        let stats = repo.relay_stats(8453).await.unwrap();
        assert_eq!(stats.attested, 1);
        assert!(!stats.in_flight);
        assert!(
            repo.record_step_submission(
                bridge,
                8453,
                SIGNER,
                8,
                200_000,
                50,
                5,
                B256::repeat_byte(0xD1),
                b"mint"
            )
            .await
            .unwrap()
        );
        assert_eq!(
            repo.legs(input.id).await.unwrap()[0].state,
            LegState::Minting
        );
        assert!(repo.relay_stats(8453).await.unwrap().in_flight);
        assert!(
            repo.complete_step(
                bridge,
                StepOutcome::Minted {
                    tx_hash: Some(B256::repeat_byte(0xD1))
                }
            )
            .await
            .unwrap()
        );
        let legs = repo.legs(input.id).await.unwrap();
        assert_eq!(legs[0].state, LegState::Completed);
        assert_eq!(legs[0].mint_tx_hash, Some(B256::repeat_byte(0xD1)));
        let done = repo.get(account, input.id).await.unwrap().unwrap();
        assert!(done.completed_at.is_some());
        assert!(done.failed_at.is_none());
        assert_eq!(
            repo.cancel(account, input.id).await.unwrap(),
            CancelOutcome::Finished
        );
        // The account may withdraw again.
        repo.create(&withdrawal(account, "k2")).await.unwrap();
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn a_failed_or_expired_leg_fails_the_withdrawal_once_the_rest_settle(pool: PgPool) {
        let repo = WithdrawalRepository::new(pool.clone());
        let account = new_account(&pool, 1).await;
        let mut input = withdrawal(account, "k1");
        input.legs[0].valid_before = 1; // long past
        repo.create(&input).await.unwrap();
        let [bridge, transfer] = [input.legs[0].id, input.legs[1].id];
        let signature = [0x11u8; 65];
        repo.authorize(account, input.id, transfer, &signature)
            .await
            .unwrap();

        assert_eq!(
            repo.expire_stale(Duration::from_secs(300)).await.unwrap(),
            1
        );
        let legs = repo.legs(input.id).await.unwrap();
        assert_eq!(legs[0].state, LegState::Expired);
        assert_eq!(legs[1].state, LegState::Authorized);
        assert!(
            repo.get(account, input.id)
                .await
                .unwrap()
                .unwrap()
                .failed_at
                .is_none()
        );
        assert_eq!(
            repo.authorize(account, input.id, bridge, &signature)
                .await
                .unwrap(),
            AuthorizeOutcome::NotAwaitingSignature
        );

        relay_transfer_leg(&repo, transfer).await;
        assert!(repo.fail_leg(transfer, "reverted").await.unwrap());
        assert!(!repo.fail_leg(transfer, "again").await.unwrap());
        let legs = repo.legs(input.id).await.unwrap();
        assert_eq!(legs[1].state, LegState::Failed);
        assert_eq!(legs[1].failure_reason.as_deref(), Some("reverted"));
        assert!(legs[1].step.is_none());
        let failed = repo.get(account, input.id).await.unwrap().unwrap();
        assert!(failed.failed_at.is_some());
        assert!(failed.completed_at.is_none());
        assert_eq!(
            repo.cancel(account, input.id).await.unwrap(),
            CancelOutcome::Finished
        );
        repo.create(&withdrawal(account, "k2")).await.unwrap();
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn concurrent_terminal_transitions_still_settle_the_withdrawal(pool: PgPool) {
        let repo = WithdrawalRepository::new(pool.clone());
        let account = new_account(&pool, 1).await;
        let input = withdrawal(account, "k1");
        repo.create(&input).await.unwrap();
        let [bridge, transfer] = [input.legs[0].id, input.legs[1].id];
        let signature = [0x11u8; 65];
        repo.authorize(account, input.id, bridge, &signature)
            .await
            .unwrap();
        repo.authorize(account, input.id, transfer, &signature)
            .await
            .unwrap();
        relay_transfer_leg(&repo, transfer).await;
        assert!(
            repo.record_step_submission(
                bridge,
                143,
                SIGNER,
                3,
                250_000,
                50,
                5,
                B256::repeat_byte(0xC1),
                b"burn"
            )
            .await
            .unwrap()
        );

        // Both legs resolve "at once". Each mutation locks the withdrawal
        // row first, so the second runs against the first one's outcome and
        // settles: the withdrawal cannot stay open with every leg terminal.
        let a = WithdrawalRepository::new(pool.clone());
        let failing =
            tokio::spawn(async move { a.fail_leg(bridge, "the burn transaction reverted").await });
        assert!(
            repo.complete_step(
                transfer,
                StepOutcome::Transferred {
                    tx_hash: B256::repeat_byte(0xA1)
                }
            )
            .await
            .unwrap()
        );
        assert!(failing.await.unwrap().unwrap());

        let legs = repo.legs(input.id).await.unwrap();
        assert_eq!(legs[0].state, LegState::Failed);
        assert_eq!(legs[1].state, LegState::Completed);
        let row = repo.get(account, input.id).await.unwrap().unwrap();
        assert!(
            row.failed_at.is_some(),
            "a failed leg fails the withdrawal even when the last leg completed concurrently"
        );
        assert!(row.completed_at.is_none());
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn a_cancelled_withdrawal_takes_no_new_step(pool: PgPool) {
        let repo = WithdrawalRepository::new(pool.clone());
        let account = new_account(&pool, 1).await;
        let input = withdrawal(account, "k1");
        repo.create(&input).await.unwrap();
        let transfer = input.legs[1].id;
        repo.authorize(account, input.id, transfer, &[0x11u8; 65])
            .await
            .unwrap();

        // A step recorded while the cancellation is deciding would leave the
        // withdrawal cancelled with a transaction in flight; the parent lock
        // on both sides rules the interleaving out in either order.
        assert_eq!(
            repo.cancel(account, input.id).await.unwrap(),
            CancelOutcome::Cancelled
        );
        assert!(
            !repo
                .record_step_submission(
                    transfer,
                    8453,
                    SIGNER,
                    7,
                    90_000,
                    100,
                    10,
                    B256::repeat_byte(0xA1),
                    b"raw"
                )
                .await
                .unwrap()
        );
        assert_eq!(
            repo.legs(input.id).await.unwrap()[1].state,
            LegState::Cancelled
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn a_reverted_step_is_retried_with_backoff_then_failed(pool: PgPool) {
        let repo = WithdrawalRepository::new(pool.clone());
        let account = new_account(&pool, 1).await;
        let mut input = withdrawal(account, "k1");
        input.legs[0].valid_before = 1; // long past: the bridge leg expires below
        repo.create(&input).await.unwrap();
        let [_bridge, transfer] = [input.legs[0].id, input.legs[1].id];
        repo.authorize(account, input.id, transfer, &[0x11u8; 65])
            .await
            .unwrap();
        assert_eq!(
            repo.expire_stale(Duration::from_secs(300)).await.unwrap(),
            1
        );
        let backoff = Duration::from_secs(600);

        assert!(
            repo.record_step_submission(
                transfer,
                8453,
                SIGNER,
                7,
                90_000,
                100,
                10,
                B256::repeat_byte(0xA1),
                b"raw"
            )
            .await
            .unwrap()
        );
        assert_eq!(
            repo.retry_step(transfer, backoff, 3, "the transfer transaction reverted")
                .await
                .unwrap(),
            RetryStep::Retried
        );
        let leg = repo.legs(input.id).await.unwrap().remove(1);
        assert_eq!(leg.state, LegState::Authorized, "back in the queue");
        assert!(leg.step.is_none());
        let (reverts, retry_at) = sqlx::query_as::<_, (i16, Option<chrono::DateTime<Utc>>)>(
            "SELECT step_reverts, step_retry_at FROM withdrawal_legs WHERE id = $1",
        )
        .bind(transfer)
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        assert_eq!(reverts, 1);
        let retry_at = retry_at.expect("the retry carries a backoff");
        assert!(retry_at > Utc::now() + chrono::TimeDelta::try_seconds(500).unwrap());
        assert!(
            repo.next_relayable(8453, Duration::ZERO, &[])
                .await
                .unwrap()
                .is_none(),
            "the backoff keeps the leg out of the queue"
        );

        // Once the backoff has passed the leg is relayed again; the third
        // revert in a row is permanent.
        sqlx::query("UPDATE withdrawal_legs SET step_retry_at = now() - interval '1 second'")
            .execute(&repo.pool)
            .await
            .unwrap();
        assert!(
            repo.record_step_submission(
                transfer,
                8453,
                SIGNER,
                7,
                90_000,
                100,
                10,
                B256::repeat_byte(0xA2),
                b"raw"
            )
            .await
            .unwrap()
        );
        assert_eq!(
            repo.retry_step(transfer, backoff, 3, "the transfer transaction reverted")
                .await
                .unwrap(),
            RetryStep::Retried
        );
        sqlx::query("UPDATE withdrawal_legs SET step_retry_at = now() - interval '1 second'")
            .execute(&repo.pool)
            .await
            .unwrap();
        assert!(
            repo.record_step_submission(
                transfer,
                8453,
                SIGNER,
                7,
                90_000,
                100,
                10,
                B256::repeat_byte(0xA3),
                b"raw"
            )
            .await
            .unwrap()
        );
        assert_eq!(
            repo.retry_step(transfer, backoff, 3, "the transfer transaction reverted")
                .await
                .unwrap(),
            RetryStep::Failed
        );
        let leg = repo.legs(input.id).await.unwrap().remove(1);
        assert_eq!(leg.state, LegState::Failed);
        assert_eq!(
            leg.failure_reason.as_deref(),
            Some("the transfer transaction reverted")
        );
        let row = repo.get(account, input.id).await.unwrap().unwrap();
        assert!(row.failed_at.is_some());
    }

    #[sqlx::test]
    async fn open_steps_are_unique_per_chain_and_signer(pool: PgPool) {
        let repo = WithdrawalRepository::new(pool.clone());
        let other = Address::repeat_byte(0xA1);
        let (account_a, transfer_a) = authorized_transfer_leg(&pool, &repo, "pool-a").await;
        let (_account_b, transfer_b) = authorized_transfer_leg(&pool, &repo, "pool-b").await;

        relay_transfer_leg(&repo, transfer_a).await;
        assert!(
            !repo
                .record_step_submission(
                    transfer_b,
                    8453,
                    SIGNER,
                    7,
                    90_000,
                    100,
                    10,
                    B256::repeat_byte(0xB1),
                    b"raw"
                )
                .await
                .is_ok_and(|ok| ok),
            "one signer cannot own two open steps on a chain"
        );
        assert!(
            repo.record_step_submission(
                transfer_b,
                8453,
                other,
                0,
                90_000,
                100,
                10,
                B256::repeat_byte(0xB1),
                b"raw"
            )
            .await
            .unwrap()
        );
        let open = repo.open_steps(8453).await.unwrap();
        assert_eq!(
            open.iter()
                .map(|leg| (leg.id, leg.step.as_ref().unwrap().signer))
                .collect::<Vec<_>>(),
            vec![(transfer_a, SIGNER), (transfer_b, other)]
        );

        assert!(
            repo.complete_step(
                transfer_a,
                StepOutcome::Transferred {
                    tx_hash: B256::repeat_byte(0xA1)
                }
            )
            .await
            .unwrap()
        );
        let open = repo.open_steps(8453).await.unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, transfer_b);
        let _ = account_a;
    }

    /// An upgrade test, not a fresh-schema one: the database starts at
    /// migration 0001 with a relay step already open, exactly the state a
    /// production deploy has when 0002 applies. The rewritten
    /// `withdrawal_legs_step_complete` constraint requires `step_signer` to
    /// be null exactly when `step_chain_id` is, so without the backfill the
    /// migration fails on every open step and takes both services down.
    #[sqlx::test(migrations = false)]
    async fn migration_0002_backfills_the_signer_of_a_step_open_before_the_pool(
        pool: sqlx::PgPool,
    ) {
        crate::MIGRATOR.run_to(1, &pool).await.unwrap();

        let repo = WithdrawalRepository::new(pool.clone());
        let (account, transfer) = authorized_transfer_leg(&pool, &repo, "pre-pool").await;
        // Open the relay step the way the pre-pool code did: no signer column
        // exists yet to record the owner in.
        sqlx::query(
            r#"UPDATE withdrawal_legs SET state = 'relaying', step_chain_id = 8453,
               step_nonce = 7, step_gas_limit = 90000,
               step_max_fee_per_gas = '100', step_max_priority_fee_per_gas = '10',
               step_tx_hashes = ARRAY['\xa1'::bytea],
               step_raw_transactions = ARRAY['\xa1'::bytea],
               step_submitted_at = now()
               WHERE id = $1"#,
        )
        .bind(transfer)
        .execute(&pool)
        .await
        .unwrap();

        crate::MIGRATOR.run(&pool).await.unwrap();
        let signer: Vec<u8> =
            sqlx::query_scalar("SELECT step_signer FROM withdrawal_legs WHERE id = $1")
                .bind(transfer)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            signer, [0u8; 20],
            "the open step gets the zero-address sentinel, which halts the worker until an operator names the signer that submitted it"
        );
        let _ = account;
    }
}
