//! Merchant withdrawals and their legs.
//!
//! A withdrawal snapshots the Gum wallet's balance in one currency into legs,
//! one per chain it sits on (one chain at all, unless the currency has a 1:1
//! bridge).
//! The merchant signs each leg's EIP-3009 authorization; `gum-server`'s
//! withdrawal orchestrator turns each signed leg into `WithdrawalStep`
//! commands that `gum-signers` executes, one step at a time per leg. The
//! ledger owns the leg's state machine and records only *which* job runs the
//! step in flight; the transaction behind it (signer, nonce, replacements,
//! receipt) is the signers' own durable state. Circle's attestation of a
//! burn is polled by the orchestrator between the burn and the mint step.
//!
//! Leg states:
//!
//! ```text
//! awaiting_signature ─▶ authorized ─▶ relaying ─▶ completed          (transfer leg)
//!                                   └▶ relaying ─▶ burned ─▶ attested ─▶ minting ─▶ completed   (bridge leg)
//! any non-terminal ─▶ failed | expired | cancelled
//! ```
//!
//! A step that reverts or cannot be executed returns the leg to
//! `authorized`/`attested` behind a backoff (`StepPolicy`); a run of them
//! fails the leg. Every mutation locks the withdrawal row first so that
//! cancellation, step results and expiry never interleave.

use std::time::Duration;

use alloy_primitives::{B256, U256};
use gum_contracts::{AbandonReason, StepResult, WithdrawalStepKind};
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
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
    pub currency: String,
    pub destination_chain_id: i64,
    pub destination_address: String,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub failed_at: Option<DateTime<Utc>>,
}
/// A leg with its execution bookkeeping. The transaction behind an
/// in-flight step (signer, nonce, submissions, receipt) belongs to
/// `gum-signers`; the ledger only knows *which* job is running the step.
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
    /// The `WithdrawalStep` command in flight for this leg, when one is.
    pub step: Option<OpenStep>,
    /// Reverts of the current step so far; reset when a step succeeds.
    pub step_reverts: u16,
    /// The leg is not relayable before this instant (revert backoff).
    pub step_retry_at: Option<DateTime<Utc>>,
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

impl DbWithdrawalLeg {
    /// The step the leg's state calls for next, or is running.
    pub fn step_kind(&self) -> Option<WithdrawalStepKind> {
        match (self.state, self.kind) {
            (LegState::Attested | LegState::Minting, _) => Some(WithdrawalStepKind::Mint),
            (LegState::Authorized | LegState::Relaying, LegKind::Transfer) => {
                Some(WithdrawalStepKind::Transfer)
            }
            (LegState::Authorized | LegState::Relaying, LegKind::Bridge) => {
                Some(WithdrawalStepKind::Burn)
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenStep {
    pub job_id: Uuid,
    pub chain_id: u64,
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
    step_job_id: Option<Uuid>,
    step_chain_id: Option<i64>,
    step_reverts: i16,
    step_retry_at: Option<DateTime<Utc>>,
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
        let step = match (row.step_job_id, row.step_chain_id) {
            (Some(job_id), Some(chain_id)) => Some(OpenStep {
                job_id,
                chain_id: chain_id as u64,
            }),
            _ => None,
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
            step_reverts: row.step_reverts.max(0) as u16,
            step_retry_at: row.step_retry_at,
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
    /// The currency every leg moves.
    pub currency: String,
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
/// How the server treats the signers' evidence about a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepPolicy {
    /// A reverted step whose obligation survives (an unused authorization,
    /// an attestation Circle has already signed) is retried on this base
    /// backoff, doubled for every consecutive revert.
    pub revert_backoff: Duration,
    /// After this many consecutive reverts the leg fails for good.
    pub max_reverts: u32,
    /// How long after a burn Circle is first asked for the attestation.
    pub attestation_poll: Duration,
}

impl Default for StepPolicy {
    fn default() -> Self {
        Self {
            revert_backoff: Duration::from_secs(30),
            max_reverts: 5,
            attestation_poll: Duration::from_secs(20),
        }
    }
}

/// What applying a step result did to the leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepApplied {
    /// The step's effect is recorded; the leg moved to `state`.
    Advanced { leg_id: Uuid, state: LegState },
    /// The leg returned to its queue behind a backoff.
    Retried { leg_id: Uuid, reverts: u16 },
    /// The leg failed for good.
    Failed { leg_id: Uuid },
    /// The step was released without effect (its deadline passed); the
    /// leg is back in its queue and expiry housekeeping decides its fate.
    Released { leg_id: Uuid },
}

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    /// No leg runs this job: a duplicate delivery of a result the ledger
    /// already applied. The caller acknowledges it as done.
    #[error("no withdrawal leg has step job {0} open")]
    NotOpen(Uuid),
    /// The result names another step than the leg is running: a bug in
    /// one of the two services, never something a retry fixes.
    #[error("step job {job_id} runs a {expected:?} step but the result is for a {reported:?} step")]
    Mismatch {
        job_id: Uuid,
        expected: WithdrawalStepKind,
        reported: WithdrawalStepKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RelayStats {
    /// Authorized legs waiting to be relayed from this chain.
    pub authorized: i64,
    /// Burned legs waiting for Circle's attestation, by source chain.
    pub awaiting_attestation: i64,
    /// Attested legs waiting to be minted on this chain.
    pub attested: i64,
    /// Steps in flight on this chain.
    pub in_flight: i64,
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
                   (id, account_id, idempotency_key, wallet_address, currency, destination_chain_id, destination_address)
               VALUES ($1, $2, $3, $4, $7, $5, $6)"#,
        )
        .bind(input.id)
        .bind(input.account_id.0)
        .bind(&input.idempotency_key)
        .bind(&input.wallet_address)
        .bind(input.destination_chain_id as i64)
        .bind(&input.destination_address)
        .bind(&input.currency)
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
    // --- Execution side ------------------------------------------------------
    //
    // The server's withdrawal orchestrator picks relayable legs, opens a
    // step for each (`begin_step`) in the same transaction that publishes
    // the `WithdrawalStep` command, and applies the signers' `StepResult`
    // (`apply_step_result`) in the same transaction that acknowledges the
    // event. Both take the caller's connection for that reason.

    /// Legs with a step in flight on this chain, oldest first.
    pub async fn open_steps(&self, chain_id: u64) -> Result<Vec<DbWithdrawalLeg>, sqlx::Error> {
        sqlx::query_as::<_, LegRow>(
            "SELECT * FROM withdrawal_legs WHERE step_chain_id = $1 ORDER BY updated_at, id",
        )
        .bind(chain_id as i64)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(DbWithdrawalLeg::try_from)
        .collect()
    }

    /// The legs this chain should act on next, in order: attested legs to
    /// mint here first (their burn already happened), then the oldest
    /// authorized legs whose authorization still has `min_validity` left.
    /// Legs in revert backoff and legs of cancelled withdrawals are skipped.
    pub async fn relayable(
        &self,
        chain_id: u64,
        min_validity: Duration,
        limit: i64,
    ) -> Result<Vec<DbWithdrawalLeg>, sqlx::Error> {
        sqlx::query_as::<_, LegRow>(
            r#"SELECT l.* FROM withdrawal_legs l
               JOIN withdrawals w ON w.id = l.withdrawal_id
               WHERE w.cancelled_at IS NULL
                 AND l.step_job_id IS NULL
                 AND (l.step_retry_at IS NULL OR l.step_retry_at <= now())
                 AND (
                   (l.state = 'attested' AND l.destination_chain_id = $1)
                   OR (l.state = 'authorized' AND l.source_chain_id = $1
                       AND l.valid_before > extract(epoch FROM now()) + $2))
               ORDER BY (l.state = 'attested') DESC, l.created_at, l.position
               LIMIT $3"#,
        )
        .bind(chain_id as i64)
        .bind(min_validity.as_secs_f64())
        .bind(limit)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(DbWithdrawalLeg::try_from)
        .collect()
    }

    /// Open a step for the leg under `job_id`, inside `conn`'s transaction.
    /// Moves an authorized leg to `relaying` or an attested leg to
    /// `minting`. Returns false when the leg is no longer in either state or
    /// its withdrawal was cancelled meanwhile, in which case the caller must
    /// not publish the command.
    ///
    /// Locks the withdrawal row first, like every mutation here, so a
    /// cancellation cannot interleave: a leg whose step is open is never
    /// cancelled with its transaction in flight.
    pub async fn begin_step(
        &self,
        conn: &mut PgConnection,
        leg_id: Uuid,
        job_id: Uuid,
        chain_id: u64,
    ) -> Result<bool, sqlx::Error> {
        let open: Option<bool> = sqlx::query_scalar(
            r#"SELECT w.cancelled_at IS NULL FROM withdrawal_legs l
               JOIN withdrawals w ON w.id = l.withdrawal_id
               WHERE l.id = $1 FOR UPDATE OF w"#,
        )
        .bind(leg_id)
        .fetch_optional(&mut *conn)
        .await?;
        if open != Some(true) {
            return Ok(false);
        }
        sqlx::query(
            r#"UPDATE withdrawal_legs
               SET state = CASE state WHEN 'authorized' THEN 'relaying' ELSE 'minting' END,
                   step_job_id = $2, step_chain_id = $3, updated_at = now()
               WHERE id = $1 AND step_job_id IS NULL AND state IN ('authorized', 'attested')"#,
        )
        .bind(leg_id)
        .bind(job_id)
        .bind(chain_id as i64)
        .execute(conn)
        .await
        .map(|result| result.rows_affected() > 0)
    }

    /// Apply what the signers proved about the step `job_id` runs, inside
    /// `conn`'s transaction. `reported` is the step kind the event names;
    /// it must be the one the leg is running.
    pub async fn apply_step_result(
        &self,
        conn: &mut PgConnection,
        job_id: Uuid,
        reported: WithdrawalStepKind,
        result: &StepResult,
        policy: &StepPolicy,
    ) -> Result<StepApplied, StepError> {
        // The parent first: two legs reaching terminal states concurrently
        // then serialize, and whichever runs second settles the withdrawal
        // against the first one's outcome.
        let leg = sqlx::query_as::<_, LegRow>(
            r#"SELECT l.* FROM withdrawal_legs l
               JOIN withdrawals w ON w.id = l.withdrawal_id
               WHERE l.step_job_id = $1 FOR UPDATE OF w, l"#,
        )
        .bind(job_id)
        .fetch_optional(&mut *conn)
        .await?
        .map(DbWithdrawalLeg::try_from)
        .transpose()?
        .ok_or(StepError::NotOpen(job_id))?;
        let expected = leg.step_kind().ok_or(StepError::NotOpen(job_id))?;
        if expected != reported {
            return Err(StepError::Mismatch {
                job_id,
                expected,
                reported,
            });
        }
        let next_check_at =
            Utc::now() + chrono::Duration::from_std(policy.attestation_poll).unwrap_or_default();
        match result {
            StepResult::Succeeded { tx_hash, .. } => {
                advance(conn, &leg, expected, Some(*tx_hash), next_check_at).await
            }
            StepResult::ExecutedElsewhere { tx_hash } => match (expected, tx_hash) {
                // `receiveMessage` is permissionless: the funds are minted
                // wherever the message says, whoever paid the gas.
                (WithdrawalStepKind::Mint, _) => {
                    advance(conn, &leg, expected, None, next_check_at).await
                }
                // The payee is the destination itself; the transfer landed.
                (WithdrawalStepKind::Transfer, hash) => {
                    advance(conn, &leg, expected, *hash, next_check_at).await
                }
                // A burn Circle must attest: without its transaction hash
                // the attestation cannot be fetched, so the leg cannot
                // proceed on its own.
                (WithdrawalStepKind::Burn, Some(hash)) => {
                    advance(conn, &leg, expected, Some(*hash), next_check_at).await
                }
                (WithdrawalStepKind::Burn, None) => {
                    fail(
                        conn,
                        &leg,
                        "the authorization was consumed by a transaction outside the searched history; the burn cannot be attested",
                    )
                    .await
                }
            },
            StepResult::Reverted { .. } => {
                let reason = match expected {
                    WithdrawalStepKind::Transfer => "the transfer transaction reverted",
                    WithdrawalStepKind::Burn => "the burn transaction reverted",
                    WithdrawalStepKind::Mint => "the mint transaction reverted",
                };
                retry_or_fail(conn, &leg, policy, reason).await
            }
            StepResult::Abandoned { reason } => {
                let reason = format!("the step could not be executed: {}", describe_abandon(reason));
                retry_or_fail(conn, &leg, policy, &reason).await
            }
            StepResult::Expired => {
                sqlx::query(
                    r#"UPDATE withdrawal_legs
                       SET state = CASE state WHEN 'relaying' THEN 'authorized' ELSE 'attested' END,
                           step_job_id = NULL, step_chain_id = NULL, updated_at = now()
                       WHERE id = $1"#,
                )
                .bind(leg.id)
                .execute(conn)
                .await?;
                Ok(StepApplied::Released { leg_id: leg.id })
            }
        }
    }

    /// The signers refused the step `job_id` runs outright
    /// (`ExecutionRejected`): the command was malformed or named a chain
    /// they do not serve, which no retry of the same command fixes. The leg
    /// fails inside `conn`'s transaction, with the reason. Returns false
    /// when no leg runs the job (a duplicate delivery).
    pub async fn reject_step(
        &self,
        conn: &mut PgConnection,
        job_id: Uuid,
        reason: &str,
    ) -> Result<bool, sqlx::Error> {
        let leg = sqlx::query_as::<_, LegRow>(
            r#"SELECT l.* FROM withdrawal_legs l
               JOIN withdrawals w ON w.id = l.withdrawal_id
               WHERE l.step_job_id = $1 FOR UPDATE OF w, l"#,
        )
        .bind(job_id)
        .fetch_optional(&mut *conn)
        .await?
        .map(DbWithdrawalLeg::try_from)
        .transpose()?;
        let Some(leg) = leg else {
            return Ok(false);
        };
        let reason = format!("the signers rejected the step: {reason}");
        match fail(conn, &leg, &reason).await {
            Ok(_) => Ok(true),
            Err(StepError::Database(error)) => Err(error),
            Err(StepError::NotOpen(_) | StepError::Mismatch { .. }) => {
                unreachable!("fail() reports database errors only")
            }
        }
    }

    /// The leg cannot proceed: a burn Circle will not attest, or anything
    /// else no retry fixes. Its funds are wherever the last successful step
    /// left them, which `failure_reason` explains.
    pub async fn fail_leg(&self, leg_id: Uuid, reason: &str) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        lock_withdrawal_of(&mut tx, leg_id).await?;
        let updated: Option<Uuid> = sqlx::query_scalar(
            r#"UPDATE withdrawal_legs
               SET state = 'failed', failure_reason = $2,
                   step_job_id = NULL, step_chain_id = NULL, step_retry_at = NULL, updated_at = now()
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
        // here: a leg cannot expire while its step is being opened or
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
        let (authorized, awaiting_attestation, attested, in_flight): (i64, i64, i64, i64) =
            sqlx::query_as(
                r#"SELECT
                       count(*) FILTER (WHERE state = 'authorized' AND source_chain_id = $1),
                       count(*) FILTER (WHERE state = 'burned' AND source_chain_id = $1),
                       count(*) FILTER (WHERE state = 'attested' AND destination_chain_id = $1),
                       count(*) FILTER (WHERE step_chain_id = $1)
                   FROM withdrawal_legs
                   WHERE state IN ('authorized', 'burned', 'attested', 'relaying', 'minting')"#,
            )
            .bind(chain_id as i64)
            .fetch_one(&self.pool)
            .await?;
        Ok(RelayStats {
            authorized,
            awaiting_attestation,
            attested,
            in_flight,
        })
    }
}

fn describe_abandon(reason: &AbandonReason) -> String {
    match reason {
        AbandonReason::Reverted { tx_hash } => format!("transaction {tx_hash} reverted"),
        AbandonReason::NonceConsumed { signer, nonce } => {
            format!("signer {signer} nonce {nonce} was consumed by another transaction")
        }
        AbandonReason::Rejected { reason } => reason.clone(),
    }
}

/// The step's effect is final: record its transaction, move the leg on,
/// clear the step, settle the withdrawal if every leg is terminal.
async fn advance(
    conn: &mut PgConnection,
    leg: &DbWithdrawalLeg,
    step: WithdrawalStepKind,
    tx_hash: Option<B256>,
    next_check_at: DateTime<Utc>,
) -> Result<StepApplied, StepError> {
    let (state, column, next_check) = match step {
        WithdrawalStepKind::Transfer => (LegState::Completed, "transfer", None),
        WithdrawalStepKind::Burn => (LegState::Burned, "burn", Some(next_check_at)),
        WithdrawalStepKind::Mint => (LegState::Completed, "mint", None),
    };
    sqlx::query(
        r#"UPDATE withdrawal_legs
           SET state = $2,
               transfer_tx_hash = CASE WHEN $4 = 'transfer' THEN $3 ELSE transfer_tx_hash END,
               burn_tx_hash = CASE WHEN $4 = 'burn' THEN $3 ELSE burn_tx_hash END,
               mint_tx_hash = CASE WHEN $4 = 'mint' THEN $3 ELSE mint_tx_hash END,
               attestation_next_check_at = $5,
               step_job_id = NULL, step_chain_id = NULL, step_reverts = 0, step_retry_at = NULL,
               updated_at = now()
           WHERE id = $1"#,
    )
    .bind(leg.id)
    .bind(state.as_str())
    .bind(tx_hash.map(|hash| hash.to_vec()))
    .bind(column)
    .bind(next_check)
    .execute(&mut *conn)
    .await?;
    settle_in(conn, leg.withdrawal_id).await?;
    Ok(StepApplied::Advanced {
        leg_id: leg.id,
        state,
    })
}

async fn fail(
    conn: &mut PgConnection,
    leg: &DbWithdrawalLeg,
    reason: &str,
) -> Result<StepApplied, StepError> {
    sqlx::query(
        r#"UPDATE withdrawal_legs
           SET state = 'failed', failure_reason = $2,
               step_job_id = NULL, step_chain_id = NULL, step_retry_at = NULL, updated_at = now()
           WHERE id = $1"#,
    )
    .bind(leg.id)
    .bind(reason)
    .execute(&mut *conn)
    .await?;
    settle_in(conn, leg.withdrawal_id).await?;
    Ok(StepApplied::Failed { leg_id: leg.id })
}

/// A step that produced no effect but whose obligation survives (an unused
/// authorization, an attestation Circle has already signed) goes back to
/// its queue behind a backoff; a run of them is a real failure.
async fn retry_or_fail(
    conn: &mut PgConnection,
    leg: &DbWithdrawalLeg,
    policy: &StepPolicy,
    reason: &str,
) -> Result<StepApplied, StepError> {
    let reverts = u32::from(leg.step_reverts) + 1;
    if reverts >= policy.max_reverts {
        return fail(conn, leg, reason).await;
    }
    sqlx::query(
        r#"UPDATE withdrawal_legs
           SET state = CASE state WHEN 'relaying' THEN 'authorized' ELSE 'attested' END,
               step_job_id = NULL, step_chain_id = NULL,
               step_reverts = $2,
               step_retry_at = now() + ($3 * power(2, LEAST($2 - 1, 6)) * interval '1 second'),
               updated_at = now()
           WHERE id = $1"#,
    )
    .bind(leg.id)
    .bind(reverts as i16)
    .bind(policy.revert_backoff.as_secs_f64())
    .execute(&mut *conn)
    .await?;
    Ok(StepApplied::Retried {
        leg_id: leg.id,
        reverts: reverts as u16,
    })
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

async fn settle(
    tx: &mut Transaction<'_, Postgres>,
    withdrawal_id: Uuid,
) -> Result<(), sqlx::Error> {
    settle_in(tx, withdrawal_id).await
}

/// Close the withdrawal once every leg is terminal: completed when all of
/// them completed, failed otherwise. A cancelled withdrawal stays cancelled.
async fn settle_in(conn: &mut PgConnection, withdrawal_id: Uuid) -> Result<(), sqlx::Error> {
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
    .execute(conn)
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
            currency: "USDC".into(),
            destination_chain_id: 8453,
            destination_address: DESTINATION.into(),
            legs: vec![
                leg(0, LegKind::Bridge, 143, 5_000_000),
                leg(1, LegKind::Transfer, 8453, 1_000_000),
            ],
        }
    }

    /// Open a step for the leg on `chain_id`, as the orchestrator does.
    async fn relay_transfer_leg(repo: &WithdrawalRepository, leg_id: Uuid) -> Uuid {
        begin(repo, leg_id, 8453).await
    }

    async fn begin(repo: &WithdrawalRepository, leg_id: Uuid, chain_id: u64) -> Uuid {
        let job_id = Uuid::now_v7();
        let mut tx = repo.pool.begin().await.unwrap();
        assert!(
            repo.begin_step(&mut tx, leg_id, job_id, chain_id)
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();
        job_id
    }

    async fn apply(
        repo: &WithdrawalRepository,
        job_id: Uuid,
        step: WithdrawalStepKind,
        result: StepResult,
        policy: &StepPolicy,
    ) -> Result<StepApplied, StepError> {
        let mut tx = repo.pool.begin().await.unwrap();
        let applied = repo
            .apply_step_result(&mut tx, job_id, step, &result, policy)
            .await;
        tx.commit().await.unwrap();
        applied
    }

    async fn leg_by_id(repo: &WithdrawalRepository, leg_id: Uuid) -> DbWithdrawalLeg {
        sqlx::query_as::<_, LegRow>("SELECT * FROM withdrawal_legs WHERE id = $1")
            .bind(leg_id)
            .fetch_one(&repo.pool)
            .await
            .map(DbWithdrawalLeg::try_from)
            .unwrap()
            .unwrap()
    }

    fn succeeded(byte: u8) -> StepResult {
        StepResult::Succeeded {
            tx_hash: B256::repeat_byte(byte),
            block: 100,
            block_hash: B256::repeat_byte(0xBB),
        }
    }

    /// A withdrawal whose bridge leg (143 → 8453) and transfer leg (8453)
    /// are both authorized.
    async fn fully_authorized(
        pool: &PgPool,
        repo: &WithdrawalRepository,
        key: &str,
        seed: u128,
    ) -> (AccountId, NewWithdrawal) {
        let account = new_account(pool, seed).await;
        let input = withdrawal(account, key);
        repo.create(&input).await.unwrap();
        for leg in &input.legs {
            repo.authorize(account, input.id, leg.id, &[0x11u8; 65])
                .await
                .unwrap();
        }
        (account, input)
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn transfer_step_completes_the_leg_and_settles_the_withdrawal(pool: PgPool) {
        let repo = WithdrawalRepository::new(pool.clone());
        let (account, input) = fully_authorized(&pool, &repo, "k1", 10).await;
        let transfer = input.legs[1].id;

        // Only the transfer leg is relayable on 8453; the bridge leg's
        // burn happens on 143.
        let on_8453 = repo
            .relayable(8453, Duration::from_secs(60), 10)
            .await
            .unwrap();
        assert_eq!(
            on_8453.iter().map(|l| l.id).collect::<Vec<_>>(),
            vec![transfer]
        );
        assert_eq!(on_8453[0].step_kind(), Some(WithdrawalStepKind::Transfer));

        let job = relay_transfer_leg(&repo, transfer).await;
        let leg = leg_by_id(&repo, transfer).await;
        assert_eq!(leg.state, LegState::Relaying);
        assert_eq!(
            leg.step,
            Some(OpenStep {
                job_id: job,
                chain_id: 8453
            })
        );
        assert!(
            repo.relayable(8453, Duration::from_secs(60), 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(repo.open_steps(8453).await.unwrap().len(), 1);
        assert_eq!(repo.relay_stats(8453).await.unwrap().in_flight, 1);

        let applied = apply(
            &repo,
            job,
            WithdrawalStepKind::Transfer,
            succeeded(0xA1),
            &StepPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            applied,
            StepApplied::Advanced {
                leg_id: transfer,
                state: LegState::Completed
            }
        );
        let leg = leg_by_id(&repo, transfer).await;
        assert_eq!(leg.state, LegState::Completed);
        assert_eq!(leg.transfer_tx_hash, Some(B256::repeat_byte(0xA1)));
        assert!(leg.step.is_none());

        // The bridge leg is still open: the withdrawal is not settled.
        let stored = repo.get(account, input.id).await.unwrap().unwrap();
        assert!(stored.completed_at.is_none() && stored.failed_at.is_none());

        // A second delivery of the same result is a no-op the caller acks.
        assert!(matches!(
            apply(&repo, job, WithdrawalStepKind::Transfer, succeeded(0xA1), &StepPolicy::default()).await,
            Err(StepError::NotOpen(id)) if id == job
        ));
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn bridge_leg_walks_burn_attestation_and_mint(pool: PgPool) {
        let repo = WithdrawalRepository::new(pool.clone());
        let (account, input) = fully_authorized(&pool, &repo, "k1", 11).await;
        let bridge = input.legs[0].id;
        let transfer = input.legs[1].id;
        let policy = StepPolicy {
            attestation_poll: Duration::from_secs(20),
            ..StepPolicy::default()
        };

        let on_143 = repo
            .relayable(143, Duration::from_secs(60), 10)
            .await
            .unwrap();
        assert_eq!(
            on_143.iter().map(|l| l.id).collect::<Vec<_>>(),
            vec![bridge]
        );
        assert_eq!(on_143[0].step_kind(), Some(WithdrawalStepKind::Burn));

        let burn_job = begin(&repo, bridge, 143).await;
        // The result must name the step the leg runs.
        assert!(matches!(
            apply(
                &repo,
                burn_job,
                WithdrawalStepKind::Transfer,
                succeeded(0xB1),
                &policy
            )
            .await,
            Err(StepError::Mismatch {
                expected: WithdrawalStepKind::Burn,
                reported: WithdrawalStepKind::Transfer,
                ..
            })
        ));
        let before = Utc::now();
        apply(
            &repo,
            burn_job,
            WithdrawalStepKind::Burn,
            succeeded(0xB1),
            &policy,
        )
        .await
        .unwrap();
        let leg = leg_by_id(&repo, bridge).await;
        assert_eq!(leg.state, LegState::Burned);
        assert_eq!(leg.burn_tx_hash, Some(B256::repeat_byte(0xB1)));
        let due = leg
            .attestation_next_check_at
            .expect("attestation poll scheduled");
        assert!(
            due >= before + chrono::Duration::seconds(19)
                && due <= Utc::now() + chrono::Duration::seconds(21)
        );

        // Not due yet: the poller does not see it. Deferring to the past does.
        assert!(
            repo.legs_awaiting_attestation(143)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            repo.defer_attestation(bridge, Utc::now() - chrono::Duration::seconds(1))
                .await
                .unwrap()
        );
        assert_eq!(repo.legs_awaiting_attestation(143).await.unwrap().len(), 1);
        assert_eq!(repo.relay_stats(143).await.unwrap().awaiting_attestation, 1);

        assert!(
            repo.record_attestation(bridge, b"msg", b"att")
                .await
                .unwrap()
        );
        assert!(
            !repo
                .record_attestation(bridge, b"msg", b"att")
                .await
                .unwrap(),
            "only a burned leg takes an attestation"
        );
        let leg = leg_by_id(&repo, bridge).await;
        assert_eq!(leg.state, LegState::Attested);
        assert_eq!(leg.step_kind(), Some(WithdrawalStepKind::Mint));

        // The mint happens on the destination chain, ahead of that chain's
        // own transfer leg.
        let on_8453 = repo
            .relayable(8453, Duration::from_secs(60), 10)
            .await
            .unwrap();
        assert_eq!(
            on_8453.iter().map(|l| l.id).collect::<Vec<_>>(),
            vec![bridge, transfer]
        );

        let mint_job = begin(&repo, bridge, 8453).await;
        assert_eq!(leg_by_id(&repo, bridge).await.state, LegState::Minting);
        // Someone else relayed the message: the mint is still done.
        let applied = apply(
            &repo,
            mint_job,
            WithdrawalStepKind::Mint,
            StepResult::ExecutedElsewhere { tx_hash: None },
            &policy,
        )
        .await
        .unwrap();
        assert_eq!(
            applied,
            StepApplied::Advanced {
                leg_id: bridge,
                state: LegState::Completed
            }
        );
        let leg = leg_by_id(&repo, bridge).await;
        assert_eq!(leg.state, LegState::Completed);
        assert_eq!(leg.mint_tx_hash, None);

        let transfer_job = relay_transfer_leg(&repo, transfer).await;
        apply(
            &repo,
            transfer_job,
            WithdrawalStepKind::Transfer,
            succeeded(0xA1),
            &policy,
        )
        .await
        .unwrap();
        let stored = repo.get(account, input.id).await.unwrap().unwrap();
        assert!(stored.completed_at.is_some());
        assert!(stored.failed_at.is_none());
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn burn_consumed_outside_history_fails_the_leg(pool: PgPool) {
        let repo = WithdrawalRepository::new(pool.clone());
        let (account, input) = fully_authorized(&pool, &repo, "k1", 12).await;
        let bridge = input.legs[0].id;
        let job = begin(&repo, bridge, 143).await;
        let applied = apply(
            &repo,
            job,
            WithdrawalStepKind::Burn,
            StepResult::ExecutedElsewhere { tx_hash: None },
            &StepPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(applied, StepApplied::Failed { leg_id: bridge });
        let leg = leg_by_id(&repo, bridge).await;
        assert_eq!(leg.state, LegState::Failed);
        assert!(leg.failure_reason.unwrap().contains("cannot be attested"));
        // The transfer leg is still open; the withdrawal is not settled yet.
        let stored = repo.get(account, input.id).await.unwrap().unwrap();
        assert!(stored.failed_at.is_none());

        // With a known hash the burn is as good as ours.
        let (_, other) = fully_authorized(&pool, &repo, "k2", 13).await;
        let job = begin(&repo, other.legs[0].id, 143).await;
        let applied = apply(
            &repo,
            job,
            WithdrawalStepKind::Burn,
            StepResult::ExecutedElsewhere {
                tx_hash: Some(B256::repeat_byte(0xC1)),
            },
            &StepPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            applied,
            StepApplied::Advanced {
                leg_id: other.legs[0].id,
                state: LegState::Burned
            }
        );
        assert_eq!(
            leg_by_id(&repo, other.legs[0].id).await.burn_tx_hash,
            Some(B256::repeat_byte(0xC1))
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn reverts_back_off_then_fail_at_the_limit(pool: PgPool) {
        let repo = WithdrawalRepository::new(pool.clone());
        let (account, input) = fully_authorized(&pool, &repo, "k1", 14).await;
        let transfer = input.legs[1].id;
        let policy = StepPolicy {
            revert_backoff: Duration::from_secs(30),
            max_reverts: 3,
            ..StepPolicy::default()
        };

        let job = relay_transfer_leg(&repo, transfer).await;
        let applied = apply(
            &repo,
            job,
            WithdrawalStepKind::Transfer,
            StepResult::Reverted {
                tx_hash: B256::repeat_byte(0xD1),
            },
            &policy,
        )
        .await
        .unwrap();
        assert_eq!(
            applied,
            StepApplied::Retried {
                leg_id: transfer,
                reverts: 1
            }
        );
        let leg = leg_by_id(&repo, transfer).await;
        assert_eq!(leg.state, LegState::Authorized);
        assert!(leg.step.is_none());
        assert_eq!(leg.step_reverts, 1);
        let retry_at = leg.step_retry_at.expect("backoff scheduled");
        let wait = (retry_at - Utc::now()).num_seconds();
        assert!(
            (28..=31).contains(&wait),
            "first backoff is the base: {wait}s"
        );
        // Backing off keeps it out of the relayable set.
        assert!(
            repo.relayable(8453, Duration::from_secs(60), 10)
                .await
                .unwrap()
                .is_empty()
        );

        // Force the backoff to elapse and revert again: doubled.
        sqlx::query(
            "UPDATE withdrawal_legs SET step_retry_at = now() - interval '1 second' WHERE id = $1",
        )
        .bind(transfer)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            repo.relayable(8453, Duration::from_secs(60), 10)
                .await
                .unwrap()
                .len(),
            1
        );
        let job = relay_transfer_leg(&repo, transfer).await;
        let applied = apply(
            &repo,
            job,
            WithdrawalStepKind::Transfer,
            StepResult::Abandoned {
                reason: AbandonReason::Rejected {
                    reason: "kms unavailable".into(),
                },
            },
            &policy,
        )
        .await
        .unwrap();
        assert_eq!(
            applied,
            StepApplied::Retried {
                leg_id: transfer,
                reverts: 2
            }
        );
        let leg = leg_by_id(&repo, transfer).await;
        let wait = (leg.step_retry_at.unwrap() - Utc::now()).num_seconds();
        assert!((58..=61).contains(&wait), "second backoff doubles: {wait}s");

        sqlx::query("UPDATE withdrawal_legs SET step_retry_at = NULL WHERE id = $1")
            .bind(transfer)
            .execute(&pool)
            .await
            .unwrap();
        let job = relay_transfer_leg(&repo, transfer).await;
        let applied = apply(
            &repo,
            job,
            WithdrawalStepKind::Transfer,
            StepResult::Reverted {
                tx_hash: B256::repeat_byte(0xD3),
            },
            &policy,
        )
        .await
        .unwrap();
        assert_eq!(applied, StepApplied::Failed { leg_id: transfer });
        let leg = leg_by_id(&repo, transfer).await;
        assert_eq!(leg.state, LegState::Failed);
        assert_eq!(
            leg.failure_reason.as_deref(),
            Some("the transfer transaction reverted")
        );

        // A success resets the revert count for the next step of a leg.
        let bridge = input.legs[0].id;
        let job = begin(&repo, bridge, 143).await;
        apply(
            &repo,
            job,
            WithdrawalStepKind::Burn,
            StepResult::Reverted {
                tx_hash: B256::repeat_byte(0xD4),
            },
            &policy,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE withdrawal_legs SET step_retry_at = NULL WHERE id = $1")
            .bind(bridge)
            .execute(&pool)
            .await
            .unwrap();
        let job = begin(&repo, bridge, 143).await;
        apply(
            &repo,
            job,
            WithdrawalStepKind::Burn,
            succeeded(0xB1),
            &policy,
        )
        .await
        .unwrap();
        assert_eq!(leg_by_id(&repo, bridge).await.step_reverts, 0);

        // Failing the burned leg from the attestation poller settles the
        // withdrawal as failed now that every leg is terminal.
        assert!(
            repo.fail_leg(bridge, "circle refused the message")
                .await
                .unwrap()
        );
        assert!(!repo.fail_leg(bridge, "again").await.unwrap());
        let stored = repo.get(account, input.id).await.unwrap().unwrap();
        assert!(stored.failed_at.is_some());
        assert!(stored.completed_at.is_none());
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn expired_step_releases_the_leg_without_effect(pool: PgPool) {
        let repo = WithdrawalRepository::new(pool.clone());
        let (_, input) = fully_authorized(&pool, &repo, "k1", 15).await;
        let transfer = input.legs[1].id;
        let job = relay_transfer_leg(&repo, transfer).await;
        let applied = apply(
            &repo,
            job,
            WithdrawalStepKind::Transfer,
            StepResult::Expired,
            &StepPolicy::default(),
        )
        .await
        .unwrap();
        assert_eq!(applied, StepApplied::Released { leg_id: transfer });
        let leg = leg_by_id(&repo, transfer).await;
        assert_eq!(leg.state, LegState::Authorized);
        assert!(leg.step.is_none());
        assert_eq!(leg.step_reverts, 0);
        assert!(leg.step_retry_at.is_none());

        // Expiry housekeeping then takes the leg once its authorization is
        // (about to be) unusable, and settles the withdrawal.
        assert_eq!(repo.expire_stale(Duration::from_secs(60)).await.unwrap(), 0);
        sqlx::query("UPDATE withdrawal_legs SET valid_before = extract(epoch FROM now())::bigint + 30 WHERE withdrawal_id = $1")
            .bind(input.id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(repo.expire_stale(Duration::from_secs(60)).await.unwrap(), 2);
        assert_eq!(leg_by_id(&repo, transfer).await.state, LegState::Expired);
        let stored = repo.by_id(input.id).await.unwrap().unwrap();
        assert!(stored.failed_at.is_some());
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn steps_never_open_on_cancelled_or_unauthorized_legs(pool: PgPool) {
        let repo = WithdrawalRepository::new(pool.clone());
        let account = new_account(&pool, 16).await;
        let input = withdrawal(account, "k1");
        repo.create(&input).await.unwrap();
        let transfer = input.legs[1].id;

        // Not yet authorized: no step.
        let mut tx = pool.begin().await.unwrap();
        assert!(
            !repo
                .begin_step(&mut tx, transfer, Uuid::now_v7(), 8453)
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();

        repo.authorize(account, input.id, transfer, &[0x11u8; 65])
            .await
            .unwrap();
        let job = relay_transfer_leg(&repo, transfer).await;
        // Already in flight: a second step is refused.
        let mut tx = pool.begin().await.unwrap();
        assert!(
            !repo
                .begin_step(&mut tx, transfer, Uuid::now_v7(), 8453)
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();
        apply(
            &repo,
            job,
            WithdrawalStepKind::Transfer,
            StepResult::Expired,
            &StepPolicy::default(),
        )
        .await
        .unwrap();

        assert_eq!(
            repo.cancel(account, input.id).await.unwrap(),
            CancelOutcome::Cancelled
        );
        let mut tx = pool.begin().await.unwrap();
        assert!(
            !repo
                .begin_step(&mut tx, transfer, Uuid::now_v7(), 8453)
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();
        assert!(
            repo.relayable(8453, Duration::from_secs(60), 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            apply(
                &repo,
                Uuid::now_v7(),
                WithdrawalStepKind::Transfer,
                succeeded(0xA1),
                &StepPolicy::default()
            )
            .await,
            Err(StepError::NotOpen(_))
        ));
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn concurrent_terminal_legs_settle_the_withdrawal_exactly_once(pool: PgPool) {
        let repo = WithdrawalRepository::new(pool.clone());
        let (account, input) = fully_authorized(&pool, &repo, "k1", 17).await;
        let bridge = input.legs[0].id;
        let transfer = input.legs[1].id;
        let burn_job = begin(&repo, bridge, 143).await;
        apply(
            &repo,
            burn_job,
            WithdrawalStepKind::Burn,
            succeeded(0xB1),
            &StepPolicy::default(),
        )
        .await
        .unwrap();
        repo.record_attestation(bridge, b"m", b"a").await.unwrap();
        let mint_job = begin(&repo, bridge, 8453).await;
        let transfer_job = relay_transfer_leg(&repo, transfer).await;

        // Two consumers apply the two final results at the same time; the
        // withdrawal-row lock serializes them and the second one settles.
        let a = {
            let repo = repo.clone();
            tokio::spawn(async move {
                apply(
                    &repo,
                    mint_job,
                    WithdrawalStepKind::Mint,
                    succeeded(0xC1),
                    &StepPolicy::default(),
                )
                .await
            })
        };
        let b = {
            let repo = repo.clone();
            tokio::spawn(async move {
                apply(
                    &repo,
                    transfer_job,
                    WithdrawalStepKind::Transfer,
                    succeeded(0xA1),
                    &StepPolicy::default(),
                )
                .await
            })
        };
        a.await.unwrap().unwrap();
        b.await.unwrap().unwrap();
        let stored = repo.get(account, input.id).await.unwrap().unwrap();
        assert!(stored.completed_at.is_some());
        assert!(stored.failed_at.is_none());
        assert!(repo.open_steps(8453).await.unwrap().is_empty());
    }
}
