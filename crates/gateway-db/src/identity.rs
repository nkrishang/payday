//! Identity verification attempts, the merchant-scoped credential ledger,
//! manual review, and provider webhook receipts (product plan §4.6, §6.2,
//! §6.3).
//!
//! Every row here is status-only. The provider's extracted names, document
//! numbers, dates of birth, and images never reach this crate: the gateway's
//! provider types cannot carry them, and nothing below has a column for them.

use std::str::FromStr;
use std::time::Duration;

use alloy_primitives::B256;
use gateway_core::VerificationFactStatus;
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use thiserror::Error;
use uuid::Uuid;

use crate::verifications::DbPayerSession;

/// How long a credential earned today satisfies the same merchant's later
/// invoices when the provider names no expiry of its own (product plan §13:
/// the minimum reasonable lifetime; open item 3 tunes it).
pub const DEFAULT_CREDENTIAL_LIFETIME: Duration = Duration::from_secs(180 * 24 * 3600);
/// How often an actively pending hosted session is polled.
pub const IDENTITY_POLL_INTERVAL: Duration = Duration::from_secs(15);
/// The longest an in-review session or a failing provider waits between polls.
pub const MAX_POLL_BACKOFF: Duration = Duration::from_secs(15 * 60);
/// Automated resubmissions allowed after a decline: one (product plan §13).
pub const MAX_AUTOMATED_ATTEMPTS: i16 = 2;

/// `payer_verifications.status`; the DB CHECK agrees with `as_str`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityStatus {
    Pending,
    Approved,
    Declined,
    InReview,
    Expired,
    Abandoned,
    ReviewRequired,
}

impl IdentityStatus {
    pub const ALL: [IdentityStatus; 7] = [
        IdentityStatus::Pending,
        IdentityStatus::Approved,
        IdentityStatus::Declined,
        IdentityStatus::InReview,
        IdentityStatus::Expired,
        IdentityStatus::Abandoned,
        IdentityStatus::ReviewRequired,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            IdentityStatus::Pending => "pending",
            IdentityStatus::Approved => "approved",
            IdentityStatus::Declined => "declined",
            IdentityStatus::InReview => "in_review",
            IdentityStatus::Expired => "expired",
            IdentityStatus::Abandoned => "abandoned",
            IdentityStatus::ReviewRequired => "review_required",
        }
    }

    /// Whether the provider may still change its mind.
    pub fn is_open(self) -> bool {
        matches!(self, IdentityStatus::Pending | IdentityStatus::InReview)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("unknown verification status: {0}")]
pub struct IdentityStatusParseError(pub String);

impl FromStr for IdentityStatus {
    type Err = IdentityStatusParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        IdentityStatus::ALL
            .into_iter()
            .find(|status| status.as_str() == s)
            .ok_or_else(|| IdentityStatusParseError(s.to_string()))
    }
}

/// `payer_credentials.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    /// A real person passed document, liveness, and face-match checks.
    DocumentLiveness,
    /// Additionally, the document matched one asserted identity, whose hash
    /// the credential is bound to.
    MatchedIdentity,
}

impl CredentialKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CredentialKind::DocumentLiveness => "document_liveness",
            CredentialKind::MatchedIdentity => "matched_identity",
        }
    }
}

fn fact_str(status: VerificationFactStatus) -> &'static str {
    status.as_str()
}

/// One verification attempt as stored: an email code or a hosted identity
/// session, whichever kind.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DbVerificationAttempt {
    pub id: Uuid,
    pub invoice_id: Uuid,
    pub account_id: Uuid,
    pub payer_session_id: Uuid,
    pub kind: String,
    pub status: String,
    pub provider: String,
    pub provider_reference: Option<String>,
    pub payer_ref: Option<Vec<u8>>,
    pub expected_identity_hash: Option<Vec<u8>>,
    pub document_status: Option<String>,
    pub liveness_status: Option<String>,
    pub identity_match_status: Option<String>,
    pub risk_codes: Vec<String>,
    pub country_code: Option<String>,
    pub attempt_number: i16,
    pub verified_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub next_poll_at: Option<DateTime<Utc>>,
    pub poll_count: i32,
    pub provider_failures: i32,
    pub created_at: DateTime<Utc>,
}

/// The projection every attempt query returns, spliced into static SQL.
macro_rules! attempt_columns {
    () => {
        "id, invoice_id, account_id, payer_session_id, kind, status, provider, \
         provider_reference, payer_ref, expected_identity_hash, document_status, liveness_status, \
         identity_match_status, risk_codes, country_code, attempt_number, verified_at, expires_at, \
         next_poll_at, poll_count, provider_failures, created_at"
    };
}

impl DbVerificationAttempt {
    pub fn status(&self) -> IdentityStatus {
        self.status
            .parse()
            .expect("the CHECK constraint admits only known statuses")
    }

    /// Whether this attempt asserted an identity to match against.
    pub fn matched(&self) -> bool {
        self.expected_identity_hash.is_some()
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DbPayerCredential {
    pub id: Uuid,
    pub account_id: Uuid,
    pub payer_ref: Vec<u8>,
    pub kind: String,
    pub status: String,
    pub expected_identity_hash: Option<Vec<u8>>,
    pub verified_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DbVerificationReview {
    pub id: Uuid,
    pub verification_id: Uuid,
    pub requested_by_account_id: Option<Uuid>,
    pub decision: Option<String>,
    pub reviewer: Option<String>,
    pub note: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}

/// `(id, provider_reference, status, expected_identity_hash, poll_count, provider_failures)`.
type ClaimedRow = (Uuid, String, String, Option<Vec<u8>>, i32, i32);

/// An attempt the reconciler holds a lease on.
#[derive(Debug, Clone)]
pub struct ClaimedVerification {
    pub id: Uuid,
    pub provider_reference: String,
    pub status: IdentityStatus,
    pub matched: bool,
    pub poll_count: i32,
    pub provider_failures: i32,
}

/// What the provider (or a reviewer) decided, reduced to what Payday keeps.
#[derive(Debug, Clone)]
pub struct DecisionRecord {
    pub status: IdentityStatus,
    pub document: VerificationFactStatus,
    pub liveness: VerificationFactStatus,
    pub identity_match: VerificationFactStatus,
    pub risk_codes: Vec<String>,
    pub country_code: Option<String>,
    /// When the provider's own approval lapses, if it says.
    pub expires_at: Option<DateTime<Utc>>,
}

/// What recording an approval changed.
#[derive(Debug, Clone)]
pub struct IdentityCompletion {
    pub attempt: DbVerificationAttempt,
    pub session: DbPayerSession,
    /// Set when this approval completed the invoice's verification, which
    /// only happens while the invoice is still live.
    pub invoice_completed_at: Option<DateTime<Utc>>,
}

/// The identity picture for one payer on one invoice: the latest attempt
/// this payer reference made, and whether automation has stopped for them.
#[derive(Debug, Clone, Default)]
pub struct IdentityState {
    pub latest: Option<DbVerificationAttempt>,
    /// A second decline, a merchant review request, or a reviewer's decision:
    /// no further automated attempt may start.
    pub review_required: bool,
    /// The provider is still deciding an earlier attempt.
    pub in_review: bool,
}

impl IdentityState {
    /// Whether a new hosted session may be started now.
    pub fn start_available(&self) -> bool {
        !self.review_required && !self.in_review
    }
}

#[derive(Debug, Error)]
pub enum StartIdentityError {
    /// Automated verification has stopped for this payer on this invoice.
    #[error("identity verification requires human review")]
    ReviewRequired,
    /// An earlier attempt is with the provider's reviewers.
    #[error("an earlier identity attempt is still in review")]
    InReview,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Error)]
pub enum ReviewError {
    #[error("no verification attempt is eligible for review")]
    NotAvailable,
    #[error("verification attempt not found")]
    NotFound,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    Approve,
    Decline,
}

/// How long to wait before polling again after `polls` fetches of an
/// in-review session, or `failures` consecutive provider failures.
pub fn poll_backoff(count: i32) -> Duration {
    let base = IDENTITY_POLL_INTERVAL.as_secs();
    let exponent = count.clamp(0, 16) as u32;
    Duration::from_secs(base.saturating_mul(2u64.saturating_pow(exponent))).min(MAX_POLL_BACKOFF)
}

#[derive(Clone)]
pub struct VerificationRepository {
    pool: PgPool,
}

impl VerificationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The active credential of `kind` this merchant holds for the payer,
    /// bound to `expected_identity_hash` for matched credentials. Another
    /// merchant's credential is never found: the reference itself is
    /// merchant-scoped and the query is keyed by account.
    pub async fn find_active_credential(
        &self,
        account_id: Uuid,
        payer_ref: B256,
        kind: CredentialKind,
        expected_identity_hash: Option<B256>,
    ) -> Result<Option<DbPayerCredential>, sqlx::Error> {
        sqlx::query_as::<_, DbPayerCredential>(
            r#"
            SELECT id, account_id, payer_ref, kind, status, expected_identity_hash,
                   verified_at, expires_at
            FROM payer_credentials
            WHERE account_id = $1
              AND payer_ref = $2
              AND kind = $3
              AND status = 'active'
              AND expires_at > now()
              AND expected_identity_hash IS NOT DISTINCT FROM $4
            ORDER BY verified_at DESC
            LIMIT 1
            "#,
        )
        .bind(account_id)
        .bind(payer_ref.as_slice())
        .bind(kind.as_str())
        .bind(expected_identity_hash.map(|hash| hash.to_vec()))
        .fetch_optional(&self.pool)
        .await
    }

    /// Where this payer stands on the invoice behind `session`.
    pub async fn identity_state(
        &self,
        session: &DbPayerSession,
    ) -> Result<IdentityState, sqlx::Error> {
        let Some(payer_ref) = session.payer_ref.as_deref() else {
            return Ok(IdentityState::default());
        };
        let attempts = sqlx::query_as::<_, DbVerificationAttempt>(concat!(
            r#"
            SELECT "#,
            attempt_columns!(),
            r#"
            FROM payer_verifications
            WHERE invoice_id = $1 AND kind = 'identity' AND payer_ref = $2
            ORDER BY created_at DESC, id DESC
            "#
        ))
        .bind(session.invoice_id)
        .bind(payer_ref)
        .fetch_all(&self.pool)
        .await?;
        let reviewed: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM verification_reviews AS review
                JOIN payer_verifications AS attempt ON attempt.id = review.verification_id
                WHERE attempt.invoice_id = $1 AND attempt.kind = 'identity' AND attempt.payer_ref = $2
            )
            "#,
        )
        .bind(session.invoice_id)
        .bind(payer_ref)
        .fetch_one(&self.pool)
        .await?;
        Ok(state_from(attempts, reviewed))
    }

    /// Open an identity attempt for `session`'s invoice. Earlier pending
    /// attempts by the same payer are abandoned so the reconciler stops
    /// polling sessions the payer walked away from; a decline earns exactly
    /// one resubmission, and anything past that is a matter for review.
    pub async fn begin_identity_verification(
        &self,
        session_id: Uuid,
        payer_ref: B256,
        expected_identity_hash: Option<B256>,
    ) -> Result<DbVerificationAttempt, StartIdentityError> {
        let mut tx = self.pool.begin().await?;
        let (invoice_id, account_id): (Uuid, Uuid) = sqlx::query_as(
            r#"
            SELECT invoice.id, invoice.account_id
            FROM payer_sessions AS session
            JOIN invoices AS invoice ON invoice.id = session.invoice_id
            WHERE session.id = $1
            FOR UPDATE OF invoice
            "#,
        )
        .bind(session_id)
        .fetch_one(&mut *tx)
        .await?;
        let attempts = sqlx::query_as::<_, DbVerificationAttempt>(concat!(
            r#"
            SELECT "#,
            attempt_columns!(),
            r#"
            FROM payer_verifications
            WHERE invoice_id = $1 AND kind = 'identity' AND payer_ref = $2
            ORDER BY created_at DESC, id DESC
            "#
        ))
        .bind(invoice_id)
        .bind(payer_ref.as_slice())
        .fetch_all(&mut *tx)
        .await?;
        let reviewed: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM verification_reviews AS review
                JOIN payer_verifications AS attempt ON attempt.id = review.verification_id
                WHERE attempt.invoice_id = $1 AND attempt.kind = 'identity' AND attempt.payer_ref = $2
            )
            "#,
        )
        .bind(invoice_id)
        .bind(payer_ref.as_slice())
        .fetch_one(&mut *tx)
        .await?;
        let declines = attempts
            .iter()
            .filter(|attempt| attempt.status() == IdentityStatus::Declined)
            .count() as i16;
        let state = state_from(attempts, reviewed);
        if state.review_required || declines >= MAX_AUTOMATED_ATTEMPTS {
            return Err(StartIdentityError::ReviewRequired);
        }
        if state.in_review {
            return Err(StartIdentityError::InReview);
        }
        sqlx::query(
            r#"
            UPDATE payer_verifications
            SET status = 'abandoned', next_poll_at = NULL, leased_until = NULL
            WHERE invoice_id = $1 AND kind = 'identity' AND payer_ref = $2 AND status = 'pending'
            "#,
        )
        .bind(invoice_id)
        .bind(payer_ref.as_slice())
        .execute(&mut *tx)
        .await?;
        let id = Uuid::now_v7();
        let attempt = sqlx::query_as::<_, DbVerificationAttempt>(concat!(
            r#"
            INSERT INTO payer_verifications
                (id, invoice_id, account_id, payer_session_id, kind, status, provider,
                 payer_ref, expected_identity_hash, attempt_number)
            VALUES ($1, $2, $3, $4, 'identity', 'pending', 'didit', $5, $6, $7)
            RETURNING "#,
            attempt_columns!(),
            r#"
            "#
        ))
        .bind(id)
        .bind(invoice_id)
        .bind(account_id)
        .bind(session_id)
        .bind(payer_ref.as_slice())
        .bind(expected_identity_hash.map(|hash| hash.to_vec()))
        .bind(declines + 1)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(attempt)
    }

    /// The provider accepted the session: remember its reference and start
    /// polling. Nothing else about the hosted session is kept.
    pub async fn record_provider_session(
        &self,
        attempt_id: Uuid,
        provider_reference: &str,
        status: IdentityStatus,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE payer_verifications
            SET provider_reference = $2,
                status = $3,
                next_poll_at = CASE WHEN $4 THEN now() + make_interval(secs => $5) END
            WHERE id = $1 AND status = 'pending'
            "#,
        )
        .bind(attempt_id)
        .bind(provider_reference)
        .bind(status.as_str())
        .bind(status.is_open())
        .bind(IDENTITY_POLL_INTERVAL.as_secs_f64())
        .execute(&self.pool)
        .await
        .map(drop)
    }

    /// The provider refused to open a session: the attempt never happened.
    pub async fn abandon(&self, attempt_id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE payer_verifications SET status = 'abandoned', next_poll_at = NULL WHERE id = $1 AND status = 'pending'",
        )
        .bind(attempt_id)
        .execute(&self.pool)
        .await
        .map(drop)
    }

    /// An eligible credential satisfies this session without a new hosted
    /// session. The reuse is recorded as an approved attempt so the merchant
    /// sees what unlocked the invoice, and it completes the invoice exactly
    /// as a fresh approval would.
    pub async fn reuse_credential(
        &self,
        session_id: Uuid,
        credential: &DbPayerCredential,
    ) -> Result<IdentityCompletion, sqlx::Error> {
        let matched = credential.kind == CredentialKind::MatchedIdentity.as_str();
        let mut tx = self.pool.begin().await?;
        let (invoice_id, account_id): (Uuid, Uuid) = sqlx::query_as(
            r#"
            SELECT invoice.id, invoice.account_id
            FROM payer_sessions AS session
            JOIN invoices AS invoice ON invoice.id = session.invoice_id
            WHERE session.id = $1
            FOR UPDATE OF invoice
            "#,
        )
        .bind(session_id)
        .fetch_one(&mut *tx)
        .await?;
        let approved = fact_str(VerificationFactStatus::Approved);
        let attempt = sqlx::query_as::<_, DbVerificationAttempt>(concat!(
            r#"
            INSERT INTO payer_verifications
                (id, invoice_id, account_id, payer_session_id, kind, status, provider,
                 payer_ref, expected_identity_hash, document_status, liveness_status,
                 identity_match_status, verified_at, expires_at)
            VALUES ($1, $2, $3, $4, 'identity', 'approved', 'didit', $5, $6, $7, $7, $8, $9, $10)
            RETURNING "#,
            attempt_columns!(),
            r#"
            "#
        ))
        .bind(Uuid::now_v7())
        .bind(invoice_id)
        .bind(account_id)
        .bind(session_id)
        .bind(&credential.payer_ref)
        .bind(&credential.expected_identity_hash)
        .bind(approved)
        .bind(matched.then_some(approved))
        .bind(credential.verified_at)
        .bind(credential.expires_at)
        .fetch_one(&mut *tx)
        .await?;
        let now = Utc::now();
        let session = unlock_session(&mut tx, session_id, matched, now).await?;
        let invoice_completed_at = complete_invoice(&mut tx, invoice_id, now).await?;
        tx.commit().await?;
        Ok(IdentityCompletion {
            attempt,
            session,
            invoice_completed_at,
        })
    }

    /// Record what the provider decided about an open attempt. A closed
    /// attempt is left alone, so a late webhook after polling (or the other
    /// way round) changes nothing. Returns the completion for an approval.
    pub async fn apply_decision(
        &self,
        attempt_id: Uuid,
        record: &DecisionRecord,
    ) -> Result<Option<IdentityCompletion>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let Some(attempt) = sqlx::query_as::<_, DbVerificationAttempt>(concat!(
            r#"
            SELECT "#,
            attempt_columns!(),
            r#" FROM payer_verifications
            WHERE id = $1 AND kind = 'identity' AND status IN ('pending', 'in_review')
            FOR UPDATE
            "#
        ))
        .bind(attempt_id)
        .fetch_optional(&mut *tx)
        .await?
        else {
            return Ok(None);
        };
        sqlx::query(
            r#"
            UPDATE payer_verifications
            SET document_status = $2, liveness_status = $3, identity_match_status = $4,
                risk_codes = $5, country_code = $6, poll_count = poll_count + 1,
                provider_failures = 0, leased_until = NULL
            WHERE id = $1
            "#,
        )
        .bind(attempt_id)
        .bind(fact_str(record.document))
        .bind(fact_str(record.liveness))
        .bind(fact_str(record.identity_match))
        .bind(&record.risk_codes)
        .bind(&record.country_code)
        .execute(&mut *tx)
        .await?;
        let completion = match record.status {
            IdentityStatus::Approved => {
                let now = Utc::now();
                Some(approve(&mut tx, &attempt, record.expires_at, now).await?)
            }
            IdentityStatus::Declined => {
                let status = if attempt.attempt_number >= MAX_AUTOMATED_ATTEMPTS {
                    IdentityStatus::ReviewRequired
                } else {
                    IdentityStatus::Declined
                };
                sqlx::query(
                    "UPDATE payer_verifications SET status = $2, next_poll_at = NULL WHERE id = $1",
                )
                .bind(attempt_id)
                .bind(status.as_str())
                .execute(&mut *tx)
                .await?;
                enqueue_declined_webhook(&mut tx, attempt.invoice_id).await?;
                None
            }
            IdentityStatus::InReview | IdentityStatus::Pending => {
                let backoff = if record.status == IdentityStatus::InReview {
                    poll_backoff(attempt.poll_count)
                } else {
                    IDENTITY_POLL_INTERVAL
                };
                sqlx::query(
                    r#"
                    UPDATE payer_verifications
                    SET status = $2, next_poll_at = now() + make_interval(secs => $3)
                    WHERE id = $1
                    "#,
                )
                .bind(attempt_id)
                .bind(record.status.as_str())
                .bind(backoff.as_secs_f64())
                .execute(&mut *tx)
                .await?;
                None
            }
            IdentityStatus::Expired
            | IdentityStatus::Abandoned
            | IdentityStatus::ReviewRequired => {
                sqlx::query(
                    "UPDATE payer_verifications SET status = $2, next_poll_at = NULL WHERE id = $1",
                )
                .bind(attempt_id)
                .bind(record.status.as_str())
                .execute(&mut *tx)
                .await?;
                None
            }
        };
        tx.commit().await?;
        Ok(completion)
    }

    /// Lease up to `limit` attempts whose poll is due. A lease that lapses
    /// without a result makes the attempt claimable again.
    pub async fn claim_due(
        &self,
        limit: i64,
        lease: Duration,
    ) -> Result<Vec<ClaimedVerification>, sqlx::Error> {
        let rows: Vec<ClaimedRow> = sqlx::query_as(
            r#"
            UPDATE payer_verifications AS claimed
            SET leased_until = now() + make_interval(secs => $2)
            WHERE claimed.id IN (
                SELECT id FROM payer_verifications
                WHERE kind = 'identity'
                  AND status IN ('pending', 'in_review')
                  AND provider_reference IS NOT NULL
                  AND next_poll_at <= now()
                  AND (leased_until IS NULL OR leased_until < now())
                ORDER BY next_poll_at
                FOR UPDATE SKIP LOCKED
                LIMIT $1
            )
            RETURNING claimed.id, claimed.provider_reference, claimed.status,
                      claimed.expected_identity_hash, claimed.poll_count, claimed.provider_failures
            "#,
        )
        .bind(limit)
        .bind(lease.as_secs_f64())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(id, provider_reference, status, hash, poll_count, provider_failures)| {
                    ClaimedVerification {
                        id,
                        provider_reference,
                        status: status
                            .parse()
                            .expect("the CHECK constraint admits only known statuses"),
                        matched: hash.is_some(),
                        poll_count,
                        provider_failures,
                    }
                },
            )
            .collect())
    }

    /// The provider could not be asked: try again later, with backoff, and
    /// never treat the outage as a decision about the payer.
    pub async fn defer_after_failure(&self, attempt_id: Uuid) -> Result<(), sqlx::Error> {
        let failures: Option<i32> =
            sqlx::query_scalar("SELECT provider_failures FROM payer_verifications WHERE id = $1")
                .bind(attempt_id)
                .fetch_optional(&self.pool)
                .await?;
        let backoff = poll_backoff(failures.unwrap_or(0));
        sqlx::query(
            r#"
            UPDATE payer_verifications
            SET provider_failures = provider_failures + 1, leased_until = NULL,
                next_poll_at = now() + make_interval(secs => $2)
            WHERE id = $1 AND status IN ('pending', 'in_review')
            "#,
        )
        .bind(attempt_id)
        .bind(backoff.as_secs_f64())
        .execute(&self.pool)
        .await
        .map(drop)
    }

    /// A verified provider callback: record the event once and bring the
    /// matching attempt's poll forward. Returns whether the event was new.
    pub async fn record_webhook(
        &self,
        provider: &str,
        event_id: &str,
        provider_reference: &str,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let inserted = sqlx::query(
            r#"
            INSERT INTO identity_webhook_receipts (provider, event_id, provider_reference)
            VALUES ($1, $2, $3)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(provider)
        .bind(event_id)
        .bind(provider_reference)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if inserted {
            sqlx::query(
                r#"
                UPDATE payer_verifications
                SET next_poll_at = now()
                WHERE provider = $1 AND provider_reference = $2 AND status IN ('pending', 'in_review')
                "#,
            )
            .bind(provider)
            .bind(provider_reference)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(inserted)
    }

    /// Every attempt on a merchant's invoice, oldest first, with its reviews.
    pub async fn attempts_for_invoice(
        &self,
        account_id: Uuid,
        invoice_id: Uuid,
    ) -> Result<Vec<(DbVerificationAttempt, Vec<DbVerificationReview>)>, sqlx::Error> {
        let attempts = sqlx::query_as::<_, DbVerificationAttempt>(concat!(
            r#"
            SELECT "#,
            attempt_columns!(),
            r#" FROM payer_verifications
            WHERE account_id = $1 AND invoice_id = $2
            ORDER BY created_at, id
            "#
        ))
        .bind(account_id)
        .bind(invoice_id)
        .fetch_all(&self.pool)
        .await?;
        let reviews = sqlx::query_as::<_, DbVerificationReview>(
            r#"
            SELECT review.id, review.verification_id, review.requested_by_account_id,
                   review.decision, review.reviewer, review.note, review.requested_at,
                   review.decided_at
            FROM verification_reviews AS review
            JOIN payer_verifications AS attempt ON attempt.id = review.verification_id
            WHERE attempt.account_id = $1 AND attempt.invoice_id = $2
            ORDER BY review.requested_at, review.id
            "#,
        )
        .bind(account_id)
        .bind(invoice_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(attempts
            .into_iter()
            .map(|attempt| {
                let own = reviews
                    .iter()
                    .filter(|review| review.verification_id == attempt.id)
                    .cloned()
                    .collect();
                (attempt, own)
            })
            .collect())
    }

    /// The merchant asks a human to look at the payer's latest declined
    /// identity attempt. Automation stops for that payer from here on.
    pub async fn request_review(
        &self,
        account_id: Uuid,
        invoice_id: Uuid,
    ) -> Result<DbVerificationAttempt, ReviewError> {
        let mut tx = self.pool.begin().await?;
        let Some(attempt) = sqlx::query_as::<_, DbVerificationAttempt>(concat!(
            r#"
            SELECT "#,
            attempt_columns!(),
            r#" FROM payer_verifications
            WHERE account_id = $1 AND invoice_id = $2 AND kind = 'identity'
              AND status IN ('declined', 'review_required')
            ORDER BY created_at DESC, id DESC
            LIMIT 1
            FOR UPDATE
            "#
        ))
        .bind(account_id)
        .bind(invoice_id)
        .fetch_optional(&mut *tx)
        .await?
        else {
            return Err(ReviewError::NotAvailable);
        };
        let open: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM verification_reviews WHERE verification_id = $1 AND decided_at IS NULL)",
        )
        .bind(attempt.id)
        .fetch_one(&mut *tx)
        .await?;
        if !open {
            sqlx::query(
                r#"
                INSERT INTO verification_reviews (id, verification_id, requested_by_account_id)
                VALUES ($1, $2, $3)
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(attempt.id)
            .bind(account_id)
            .execute(&mut *tx)
            .await?;
        }
        let attempt = sqlx::query_as::<_, DbVerificationAttempt>(concat!(
            r#"
            UPDATE payer_verifications SET status = 'review_required', next_poll_at = NULL
            WHERE id = $1
            RETURNING "#,
            attempt_columns!(),
            r#"
            "#
        ))
        .bind(attempt.id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(attempt)
    }

    /// An operator decides an attempt that automation handed over. Approval
    /// unlocks the payer's session and creates credentials bound to the very
    /// same expected-identity hash an automated approval would have used.
    pub async fn decide_review(
        &self,
        attempt_id: Uuid,
        reviewer: &str,
        decision: ReviewDecision,
        note: Option<&str>,
    ) -> Result<(DbVerificationAttempt, DbVerificationReview), ReviewError> {
        let mut tx = self.pool.begin().await?;
        let Some(attempt) = sqlx::query_as::<_, DbVerificationAttempt>(concat!(
            r#"
            SELECT "#,
            attempt_columns!(),
            r#" FROM payer_verifications
            WHERE id = $1 AND kind = 'identity'
            FOR UPDATE
            "#
        ))
        .bind(attempt_id)
        .fetch_optional(&mut *tx)
        .await?
        else {
            return Err(ReviewError::NotFound);
        };
        if attempt.status() != IdentityStatus::ReviewRequired {
            return Err(ReviewError::NotAvailable);
        }
        let open: Option<Uuid> = sqlx::query_scalar(
            r#"
            SELECT id FROM verification_reviews
            WHERE verification_id = $1 AND decided_at IS NULL
            ORDER BY requested_at DESC LIMIT 1
            "#,
        )
        .bind(attempt.id)
        .fetch_optional(&mut *tx)
        .await?;
        // A payer may appeal directly, without the merchant asking first.
        let review_id = match open {
            Some(id) => id,
            None => {
                let id = Uuid::now_v7();
                sqlx::query(
                    "INSERT INTO verification_reviews (id, verification_id) VALUES ($1, $2)",
                )
                .bind(id)
                .bind(attempt.id)
                .execute(&mut *tx)
                .await?;
                id
            }
        };
        let decided = match decision {
            ReviewDecision::Approve => "approved",
            ReviewDecision::Decline => "declined",
        };
        let review = sqlx::query_as::<_, DbVerificationReview>(
            r#"
            UPDATE verification_reviews
            SET decision = $2, reviewer = $3, note = $4, decided_at = now()
            WHERE id = $1
            RETURNING id, verification_id, requested_by_account_id, decision, reviewer, note,
                      requested_at, decided_at
            "#,
        )
        .bind(review_id)
        .bind(decided)
        .bind(reviewer)
        .bind(note)
        .fetch_one(&mut *tx)
        .await?;
        let approved = fact_str(VerificationFactStatus::Approved);
        let attempt = match decision {
            ReviewDecision::Approve => {
                sqlx::query(
                    r#"
                    UPDATE payer_verifications
                    SET provider = 'manual', document_status = $2, liveness_status = $2,
                        identity_match_status = CASE WHEN expected_identity_hash IS NOT NULL THEN $2 ELSE identity_match_status END
                    WHERE id = $1
                    "#,
                )
                .bind(attempt.id)
                .bind(approved)
                .execute(&mut *tx)
                .await?;
                approve(&mut tx, &attempt, None, Utc::now()).await?.attempt
            }
            ReviewDecision::Decline => {
                sqlx::query_as::<_, DbVerificationAttempt>(concat!(
                    r#"
                    UPDATE payer_verifications SET status = 'declined', provider = 'manual'
                    WHERE id = $1
                    RETURNING "#,
                    attempt_columns!(),
                    r#"
                    "#
                ))
                .bind(attempt.id)
                .fetch_one(&mut *tx)
                .await?
            }
        };
        tx.commit().await?;
        Ok((attempt, review))
    }

    pub async fn find_attempt(
        &self,
        attempt_id: Uuid,
    ) -> Result<Option<DbVerificationAttempt>, sqlx::Error> {
        sqlx::query_as::<_, DbVerificationAttempt>(concat!(
            "SELECT ",
            attempt_columns!(),
            " FROM payer_verifications WHERE id = $1"
        ))
        .bind(attempt_id)
        .fetch_optional(&self.pool)
        .await
    }
}

fn state_from(attempts: Vec<DbVerificationAttempt>, reviewed: bool) -> IdentityState {
    let review_required = reviewed
        || attempts
            .iter()
            .any(|attempt| attempt.status() == IdentityStatus::ReviewRequired);
    let in_review = attempts
        .iter()
        .any(|attempt| attempt.status() == IdentityStatus::InReview);
    IdentityState {
        latest: attempts.into_iter().next(),
        review_required,
        in_review,
    }
}

/// Approval: stamp the attempt, mint or refresh the credentials, unlock the
/// attempt's own session, and complete the invoice if it is still live.
async fn approve(
    tx: &mut Transaction<'_, Postgres>,
    attempt: &DbVerificationAttempt,
    provider_expires_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<IdentityCompletion, sqlx::Error> {
    let default_expiry = now + DEFAULT_CREDENTIAL_LIFETIME;
    let expires_at = provider_expires_at
        .filter(|expiry| *expiry > now)
        .map_or(default_expiry, |expiry| expiry.min(default_expiry));
    let attempt = sqlx::query_as::<_, DbVerificationAttempt>(concat!(
        r#"
        UPDATE payer_verifications
        SET status = 'approved', verified_at = $2, expires_at = $3, next_poll_at = NULL
        WHERE id = $1
        RETURNING "#,
        attempt_columns!(),
        r#"
        "#
    ))
    .bind(attempt.id)
    .bind(now)
    .bind(expires_at)
    .fetch_one(&mut **tx)
    .await?;
    let payer_ref = attempt
        .payer_ref
        .clone()
        .expect("identity attempts carry the payer reference");
    sqlx::query(
        r#"
        INSERT INTO payer_credentials (id, account_id, payer_ref, kind, status, verified_at, expires_at)
        VALUES ($1, $2, $3, 'document_liveness', 'active', $4, $5)
        ON CONFLICT (account_id, payer_ref, kind) WHERE kind = 'document_liveness' AND status = 'active'
        DO UPDATE SET verified_at = EXCLUDED.verified_at, expires_at = EXCLUDED.expires_at
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(attempt.account_id)
    .bind(&payer_ref)
    .bind(now)
    .bind(expires_at)
    .execute(&mut **tx)
    .await?;
    if let Some(hash) = &attempt.expected_identity_hash {
        sqlx::query(
            r#"
            INSERT INTO payer_credentials
                (id, account_id, payer_ref, kind, status, expected_identity_hash, verified_at, expires_at)
            VALUES ($1, $2, $3, 'matched_identity', 'active', $4, $5, $6)
            ON CONFLICT (account_id, payer_ref, expected_identity_hash)
                WHERE kind = 'matched_identity' AND status = 'active'
            DO UPDATE SET verified_at = EXCLUDED.verified_at, expires_at = EXCLUDED.expires_at
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(attempt.account_id)
        .bind(&payer_ref)
        .bind(hash)
        .bind(now)
        .bind(expires_at)
        .execute(&mut **tx)
        .await?;
    }
    let session = unlock_session(tx, attempt.payer_session_id, attempt.matched(), now).await?;
    let invoice_completed_at = complete_invoice(tx, attempt.invoice_id, now).await?;
    Ok(IdentityCompletion {
        attempt,
        session,
        invoice_completed_at,
    })
}

/// Set the identity facts on exactly one session.
async fn unlock_session(
    tx: &mut Transaction<'_, Postgres>,
    session_id: Uuid,
    matched: bool,
    now: DateTime<Utc>,
) -> Result<DbPayerSession, sqlx::Error> {
    sqlx::query_as::<_, DbPayerSession>(
        r#"
        UPDATE payer_sessions
        SET document_verified_at = COALESCE(document_verified_at, $2),
            liveness_verified_at = COALESCE(liveness_verified_at, $2),
            identity_matched_at = CASE WHEN $3 THEN COALESCE(identity_matched_at, $2)
                                       ELSE identity_matched_at END
        WHERE id = $1
        RETURNING id, invoice_id, payer_ref, email_verified_at, document_verified_at,
                  liveness_verified_at, identity_matched_at, created_at, expires_at
        "#,
    )
    .bind(session_id)
    .bind(now)
    .bind(matched)
    .fetch_one(&mut **tx)
    .await
}

/// Complete an identity-mode invoice's verification, but only while it is
/// still live: a completion after the deadline or after settlement must not
/// create a claim the sweep gate would honour (product plan §4.8).
async fn complete_invoice(
    tx: &mut Transaction<'_, Postgres>,
    invoice_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        UPDATE invoices
        SET verification_completed_at = $2, updated_at = now()
        WHERE id = $1
          AND payer_policy_mode IN ('verified_identity', 'verified_identity_unattributed')
          AND verification_completed_at IS NULL
          AND status IN ('created', 'funded', 'deploying')
          AND expiration_timestamp >= $3
        RETURNING verification_completed_at
        "#,
    )
    .bind(invoice_id)
    .bind(now)
    .bind(now.timestamp())
    .fetch_optional(&mut **tx)
    .await
}

/// `verification.declined`, once per invoice, through the same envelope the
/// lifecycle trigger writes.
async fn enqueue_declined_webhook(
    tx: &mut Transaction<'_, Postgres>,
    invoice_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        SELECT enqueue_invoice_webhook_event(
            invoice.account_id, invoice.id, 'verification.declined', now(),
            jsonb_build_object('payment', webhook_payment_object(invoice)))
        FROM invoices AS invoice
        WHERE invoice.id = $1
        "#,
    )
    .bind(invoice_id)
    .execute(&mut **tx)
    .await
    .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invoices::tests::{account, issuance_input};
    use crate::verifications::{PAYER_SESSION_TTL, PayerSessionRepository, payer_ref};
    use crate::{DbInvoice, InvoiceRepository};
    use gateway_core::{ExpectedIdentity, PayerPolicy, PayerPolicyMode, expected_identity_hash};

    const MASTER_KEY: [u8; 32] = [1u8; 32];

    async fn invoice(pool: &PgPool, owner: u128, key: &str, policy: PayerPolicy) -> DbInvoice {
        let owner = account(pool, owner).await;
        let mut input = issuance_input(owner, key, None);
        input.issuance_snapshot.payer_policy = policy.clone();
        input.payer_policy = policy;
        InvoiceRepository::new(pool.clone())
            .insert_issued(&input, None)
            .await
            .unwrap()
            .row
    }

    fn unattributed() -> PayerPolicy {
        PayerPolicy::VerifiedIdentityUnattributed {
            expected_email: "alice@example.com".into(),
        }
    }

    fn alice() -> ExpectedIdentity {
        ExpectedIdentity {
            first_name: "Alice".into(),
            last_name: "Smith".into(),
        }
    }

    fn matched(identity: ExpectedIdentity) -> PayerPolicy {
        PayerPolicy::VerifiedIdentity {
            expected_email: "alice@example.com".into(),
            expected_identity: identity,
        }
    }

    /// A session on `row` whose mailbox is proven, as identity start requires.
    async fn verified_session(pool: &PgPool, row: &DbInvoice) -> (DbPayerSession, B256) {
        let sessions = PayerSessionRepository::new(pool.clone());
        let created = sessions.create(row.id, PAYER_SESSION_TTL).await.unwrap();
        let reference = payer_ref(&MASTER_KEY, row.account_id, "alice@example.com");
        let completion = sessions
            .approve_email(
                created.id,
                reference,
                Utc::now(),
                &format!("event-{}", created.id),
            )
            .await
            .unwrap();
        (completion.session, reference)
    }

    fn approved() -> DecisionRecord {
        DecisionRecord {
            status: IdentityStatus::Approved,
            document: VerificationFactStatus::Approved,
            liveness: VerificationFactStatus::Approved,
            identity_match: VerificationFactStatus::Approved,
            risk_codes: vec![],
            country_code: Some("DEU".into()),
            expires_at: None,
        }
    }

    fn declined() -> DecisionRecord {
        DecisionRecord {
            status: IdentityStatus::Declined,
            document: VerificationFactStatus::Declined,
            liveness: VerificationFactStatus::Approved,
            identity_match: VerificationFactStatus::NotRequired,
            risk_codes: vec!["DOCUMENT_EXPIRED".into()],
            country_code: None,
            expires_at: None,
        }
    }

    async fn started(
        repo: &VerificationRepository,
        session: &DbPayerSession,
        reference: B256,
        hash: Option<B256>,
        provider_reference: &str,
    ) -> DbVerificationAttempt {
        let attempt = repo
            .begin_identity_verification(session.id, reference, hash)
            .await
            .unwrap();
        repo.record_provider_session(attempt.id, provider_reference, IdentityStatus::Pending)
            .await
            .unwrap();
        repo.find_attempt(attempt.id).await.unwrap().unwrap()
    }

    async fn completed_at(pool: &PgPool, invoice_id: Uuid) -> Option<DateTime<Utc>> {
        sqlx::query_scalar("SELECT verification_completed_at FROM invoices WHERE id = $1")
            .bind(invoice_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[test]
    fn statuses_round_trip_and_backoff_is_bounded() {
        for status in IdentityStatus::ALL {
            assert_eq!(status.as_str().parse::<IdentityStatus>().unwrap(), status);
        }
        assert!("pending".parse::<IdentityStatus>().unwrap().is_open());
        assert!(!"approved".parse::<IdentityStatus>().unwrap().is_open());
        assert!("unknown".parse::<IdentityStatus>().is_err());
        assert_eq!(poll_backoff(0), IDENTITY_POLL_INTERVAL);
        assert_eq!(poll_backoff(2), IDENTITY_POLL_INTERVAL * 4);
        assert_eq!(poll_backoff(40), MAX_POLL_BACKOFF);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn approval_unlocks_only_its_session_and_mints_merchant_scoped_credentials(pool: PgPool) {
        let repo = VerificationRepository::new(pool.clone());
        let row = invoice(&pool, 1, "unattributed", unattributed()).await;
        let (session, reference) = verified_session(&pool, &row).await;
        let (bystander, _) = verified_session(&pool, &row).await;
        let attempt = started(&repo, &session, reference, None, "didit-1").await;
        assert_eq!(attempt.status(), IdentityStatus::Pending);
        assert_eq!(attempt.attempt_number, 1);
        assert!(attempt.next_poll_at.is_some());

        let completion = repo
            .apply_decision(attempt.id, &approved())
            .await
            .unwrap()
            .expect("an approval completes");
        assert_eq!(completion.attempt.status(), IdentityStatus::Approved);
        assert_eq!(completion.attempt.country_code.as_deref(), Some("DEU"));
        assert!(completion.session.facts().document);
        assert!(completion.session.facts().liveness);
        assert!(!completion.session.facts().identity_match);
        assert!(
            completion
                .session
                .satisfies(PayerPolicyMode::VerifiedIdentityUnattributed)
        );
        assert!(completion.invoice_completed_at.is_some());
        assert!(completed_at(&pool, row.id).await.is_some());
        // The bystander session on the same invoice learns nothing.
        let sessions = PayerSessionRepository::new(pool.clone());
        let other: DbPayerSession = sqlx::query_as(
            "SELECT id, invoice_id, payer_ref, email_verified_at, document_verified_at, liveness_verified_at, identity_matched_at, created_at, expires_at FROM payer_sessions WHERE id = $1",
        )
        .bind(bystander.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!other.facts().document);
        drop(sessions);

        // A second decision about a closed attempt changes nothing.
        assert!(
            repo.apply_decision(attempt.id, &declined())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            repo.find_attempt(attempt.id)
                .await
                .unwrap()
                .unwrap()
                .status(),
            IdentityStatus::Approved
        );

        // The generic credential is reusable by this merchant for this payer,
        // and by nobody else.
        let credential = repo
            .find_active_credential(
                row.account_id,
                reference,
                CredentialKind::DocumentLiveness,
                None,
            )
            .await
            .unwrap()
            .expect("a generic credential was minted");
        assert!(credential.expires_at > Utc::now() + Duration::from_secs(179 * 24 * 3600));
        assert!(
            repo.find_active_credential(
                row.account_id,
                reference,
                CredentialKind::MatchedIdentity,
                Some(expected_identity_hash(&alice())),
            )
            .await
            .unwrap()
            .is_none()
        );
        let other_merchant = invoice(&pool, 2, "other", unattributed()).await;
        let foreign_reference =
            payer_ref(&MASTER_KEY, other_merchant.account_id, "alice@example.com");
        assert!(
            repo.find_active_credential(
                other_merchant.account_id,
                foreign_reference,
                CredentialKind::DocumentLiveness,
                None,
            )
            .await
            .unwrap()
            .is_none()
        );
        assert!(
            repo.find_active_credential(
                other_merchant.account_id,
                reference,
                CredentialKind::DocumentLiveness,
                None,
            )
            .await
            .unwrap()
            .is_none()
        );

        // Reuse on a later invoice of the same merchant records an approved
        // attempt and completes that invoice too.
        let later = invoice(&pool, 1, "later", unattributed()).await;
        let (later_session, _) = verified_session(&pool, &later).await;
        let reused = repo
            .reuse_credential(later_session.id, &credential)
            .await
            .unwrap();
        assert_eq!(reused.attempt.status(), IdentityStatus::Approved);
        assert!(reused.attempt.provider_reference.is_none());
        assert!(reused.session.facts().document && reused.session.facts().liveness);
        assert!(reused.invoice_completed_at.is_some());

        // Expired and revoked credentials are not eligible.
        sqlx::query(
            "UPDATE payer_credentials SET expires_at = now() - interval '1 day' WHERE id = $1",
        )
        .bind(credential.id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            repo.find_active_credential(
                row.account_id,
                reference,
                CredentialKind::DocumentLiveness,
                None,
            )
            .await
            .unwrap()
            .is_none()
        );
        sqlx::query("UPDATE payer_credentials SET expires_at = now() + interval '1 day', status = 'revoked' WHERE id = $1")
            .bind(credential.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            repo.find_active_credential(
                row.account_id,
                reference,
                CredentialKind::DocumentLiveness,
                None,
            )
            .await
            .unwrap()
            .is_none()
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn matched_credentials_bind_the_exact_assertion(pool: PgPool) {
        let repo = VerificationRepository::new(pool.clone());
        let row = invoice(&pool, 1, "matched", matched(alice())).await;
        let (session, reference) = verified_session(&pool, &row).await;
        let hash = expected_identity_hash(&alice());
        let attempt = started(&repo, &session, reference, Some(hash), "didit-m").await;
        let completion = repo
            .apply_decision(attempt.id, &approved())
            .await
            .unwrap()
            .unwrap();
        assert!(completion.session.facts().identity_match);
        assert!(
            completion
                .session
                .satisfies(PayerPolicyMode::VerifiedIdentity)
        );
        assert!(completion.invoice_completed_at.is_some());

        assert!(
            repo.find_active_credential(
                row.account_id,
                reference,
                CredentialKind::MatchedIdentity,
                Some(hash)
            )
            .await
            .unwrap()
            .is_some()
        );
        // The generic credential rides along; the matched one is bound.
        assert!(
            repo.find_active_credential(
                row.account_id,
                reference,
                CredentialKind::DocumentLiveness,
                None
            )
            .await
            .unwrap()
            .is_some()
        );
        let bob = ExpectedIdentity {
            first_name: "Bob".into(),
            last_name: "Smith".into(),
        };
        assert!(
            repo.find_active_credential(
                row.account_id,
                reference,
                CredentialKind::MatchedIdentity,
                Some(expected_identity_hash(&bob))
            )
            .await
            .unwrap()
            .is_none()
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn one_resubmission_then_review_with_the_reviewer_recorded(pool: PgPool) {
        let repo = VerificationRepository::new(pool.clone());
        let row = invoice(&pool, 1, "retry", matched(alice())).await;
        let (session, reference) = verified_session(&pool, &row).await;
        let hash = expected_identity_hash(&alice());

        let first = started(&repo, &session, reference, Some(hash), "didit-1").await;
        assert!(
            repo.apply_decision(first.id, &declined())
                .await
                .unwrap()
                .is_none()
        );
        let first = repo.find_attempt(first.id).await.unwrap().unwrap();
        assert_eq!(first.status(), IdentityStatus::Declined);
        assert_eq!(first.document_status.as_deref(), Some("declined"));
        assert_eq!(first.risk_codes, ["DOCUMENT_EXPIRED"]);
        let declined_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM webhook_events WHERE invoice_id = $1 AND event_type = 'verification.declined'",
        )
        .bind(row.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(declined_events, 1);
        let state = repo.identity_state(&session).await.unwrap();
        assert!(state.start_available());
        assert_eq!(state.latest.as_ref().unwrap().id, first.id);

        let second = started(&repo, &session, reference, Some(hash), "didit-2").await;
        assert_eq!(second.attempt_number, 2);
        assert!(
            repo.apply_decision(second.id, &declined())
                .await
                .unwrap()
                .is_none()
        );
        let second = repo.find_attempt(second.id).await.unwrap().unwrap();
        assert_eq!(second.status(), IdentityStatus::ReviewRequired);
        assert!(
            !repo
                .identity_state(&session)
                .await
                .unwrap()
                .start_available()
        );
        assert!(matches!(
            repo.begin_identity_verification(session.id, reference, Some(hash))
                .await,
            Err(StartIdentityError::ReviewRequired)
        ));

        // The merchant asks for a review; the reviewer approves it, and the
        // approval binds the same hash an automated one would have.
        let requested = repo.request_review(row.account_id, row.id).await.unwrap();
        assert_eq!(requested.id, second.id);
        assert!(matches!(
            repo.request_review(row.account_id, Uuid::now_v7()).await,
            Err(ReviewError::NotAvailable)
        ));
        let (decided, review) = repo
            .decide_review(
                second.id,
                "operator@payday",
                ReviewDecision::Approve,
                Some("Appeal upheld"),
            )
            .await
            .unwrap();
        assert_eq!(decided.status(), IdentityStatus::Approved);
        assert_eq!(decided.provider, "manual");
        assert_eq!(decided.identity_match_status.as_deref(), Some("approved"));
        assert_eq!(review.reviewer.as_deref(), Some("operator@payday"));
        assert_eq!(review.decision.as_deref(), Some("approved"));
        assert_eq!(review.note.as_deref(), Some("Appeal upheld"));
        assert!(review.decided_at.is_some());
        assert_eq!(review.requested_by_account_id, Some(row.account_id));
        assert!(
            repo.find_active_credential(
                row.account_id,
                reference,
                CredentialKind::MatchedIdentity,
                Some(hash)
            )
            .await
            .unwrap()
            .is_some()
        );
        assert!(completed_at(&pool, row.id).await.is_some());
        assert!(matches!(
            repo.decide_review(second.id, "operator@payday", ReviewDecision::Decline, None)
                .await,
            Err(ReviewError::NotAvailable)
        ));
        assert!(matches!(
            repo.decide_review(
                Uuid::now_v7(),
                "operator@payday",
                ReviewDecision::Decline,
                None
            )
            .await,
            Err(ReviewError::NotFound)
        ));
        let listed = repo
            .attempts_for_invoice(row.account_id, row.id)
            .await
            .unwrap();
        let kinds: Vec<&str> = listed
            .iter()
            .map(|(attempt, _)| attempt.kind.as_str())
            .collect();
        assert_eq!(kinds, ["identity", "identity"]);
        assert!(listed[0].1.is_empty());
        assert_eq!(listed[1].1.len(), 1);
        assert_eq!(listed[1].1[0].reviewer.as_deref(), Some("operator@payday"));
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn claims_lease_webhooks_dedupe_and_failures_defer_without_declining(pool: PgPool) {
        let repo = VerificationRepository::new(pool.clone());
        let row = invoice(&pool, 1, "poll", unattributed()).await;
        let (session, reference) = verified_session(&pool, &row).await;
        let attempt = started(&repo, &session, reference, None, "didit-poll").await;

        // Not due yet: the first poll is a quarter minute out.
        assert!(
            repo.claim_due(10, Duration::from_secs(60))
                .await
                .unwrap()
                .is_empty()
        );
        // A (lost-then-found) webhook brings it forward; the same event twice
        // is one receipt.
        assert!(
            repo.record_webhook("didit", "evt-1", "didit-poll")
                .await
                .unwrap()
        );
        assert!(
            !repo
                .record_webhook("didit", "evt-1", "didit-poll")
                .await
                .unwrap()
        );
        let claimed = repo.claim_due(10, Duration::from_secs(60)).await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id, attempt.id);
        assert_eq!(claimed[0].provider_reference, "didit-poll");
        assert!(!claimed[0].matched);
        // Leased: nobody else claims it meanwhile.
        assert!(
            repo.claim_due(10, Duration::from_secs(60))
                .await
                .unwrap()
                .is_empty()
        );

        // A provider outage defers with backoff and leaves the payer pending.
        repo.defer_after_failure(attempt.id).await.unwrap();
        let deferred = repo.find_attempt(attempt.id).await.unwrap().unwrap();
        assert_eq!(deferred.status(), IdentityStatus::Pending);
        assert_eq!(deferred.provider_failures, 1);
        assert!(deferred.next_poll_at.unwrap() > Utc::now());

        // In review backs off further each poll; approval closes polling.
        sqlx::query("UPDATE payer_verifications SET next_poll_at = now(), leased_until = NULL WHERE id = $1")
            .bind(attempt.id)
            .execute(&pool)
            .await
            .unwrap();
        let in_review = DecisionRecord {
            status: IdentityStatus::InReview,
            ..approved()
        };
        assert!(
            repo.apply_decision(attempt.id, &in_review)
                .await
                .unwrap()
                .is_none()
        );
        let reviewing = repo.find_attempt(attempt.id).await.unwrap().unwrap();
        assert_eq!(reviewing.status(), IdentityStatus::InReview);
        assert_eq!(reviewing.poll_count, 1);
        assert_eq!(reviewing.provider_failures, 0);
        assert!(matches!(
            repo.begin_identity_verification(session.id, reference, None)
                .await,
            Err(StartIdentityError::InReview)
        ));
        assert!(
            repo.apply_decision(attempt.id, &approved())
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            repo.find_attempt(attempt.id)
                .await
                .unwrap()
                .unwrap()
                .next_poll_at
                .is_none()
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn completion_after_expiry_does_not_create_live_eligibility(pool: PgPool) {
        let repo = VerificationRepository::new(pool.clone());
        let row = invoice(&pool, 1, "late", unattributed()).await;
        let (session, reference) = verified_session(&pool, &row).await;
        let attempt = started(&repo, &session, reference, None, "didit-late").await;
        sqlx::query("UPDATE invoices SET status = 'expired', expired_at = now() WHERE id = $1")
            .bind(row.id)
            .execute(&pool)
            .await
            .unwrap();
        let completion = repo
            .apply_decision(attempt.id, &approved())
            .await
            .unwrap()
            .unwrap();
        // The payer's facts and credential are real; the invoice is not revived.
        assert!(completion.session.facts().document);
        assert!(completion.invoice_completed_at.is_none());
        assert!(completed_at(&pool, row.id).await.is_none());
        assert!(
            repo.find_active_credential(
                row.account_id,
                reference,
                CredentialKind::DocumentLiveness,
                None
            )
            .await
            .unwrap()
            .is_some()
        );

        // Starting again abandons a pending attempt by the same payer.
        let pending = invoice(&pool, 1, "restart", unattributed()).await;
        let (restart_session, _) = verified_session(&pool, &pending).await;
        let stale = started(&repo, &restart_session, reference, None, "didit-stale").await;
        let fresh = started(&repo, &restart_session, reference, None, "didit-fresh").await;
        assert_eq!(
            repo.find_attempt(stale.id).await.unwrap().unwrap().status(),
            IdentityStatus::Abandoned
        );
        assert_eq!(fresh.attempt_number, 1);
        repo.abandon(fresh.id).await.unwrap();
        assert_eq!(
            repo.find_attempt(fresh.id).await.unwrap().unwrap().status(),
            IdentityStatus::Abandoned
        );
    }
}
