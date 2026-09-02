//! Payer sessions and email verification attempts (product plan §4.6).
//!
//! A payer session is an opaque bearer token scoped to one invoice. Only its
//! SHA-256 is stored, so a database read never yields a usable token. The
//! session accumulates verification facts; the invoice's own
//! `verification_completed_at` is set separately and gates settlement.

use std::time::Duration;

use alloy_primitives::B256;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use gateway_core::{PayerPolicyMode, VerificationFacts};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use sqlx::types::chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

/// How long a payer session stays usable. It must outlive a hosted identity
/// flow the payer returns from, and it never outlives the day.
pub const PAYER_SESSION_TTL: Duration = Duration::from_secs(24 * 3600);

const PAYER_REF_ACCOUNT_DOMAIN: &[u8] = b"PAYDAY_PAYER_REF_ACCOUNT_V1";

/// The merchant-scoped payer reference (product plan §4.6). The account key is
/// derived first so a reference never links the same mailbox across merchants:
///
/// ```text
/// account_key = HMAC(master_key, "PAYDAY_PAYER_REF_ACCOUNT_V1" || account_id)
/// payer_ref   = HMAC(account_key, normalized_email)
/// ```
pub fn payer_ref(master_key: &[u8; 32], account_id: Uuid, normalized_email: &str) -> B256 {
    let mut account_key =
        Hmac::<Sha256>::new_from_slice(master_key).expect("HMAC accepts any key length");
    account_key.update(PAYER_REF_ACCOUNT_DOMAIN);
    account_key.update(account_id.as_bytes());
    let account_key = account_key.finalize().into_bytes();
    let mut reference =
        Hmac::<Sha256>::new_from_slice(&account_key).expect("HMAC accepts any key length");
    reference.update(normalized_email.as_bytes());
    B256::from_slice(&reference.finalize().into_bytes())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DbPayerSession {
    pub id: Uuid,
    pub invoice_id: Uuid,
    pub payer_ref: Option<Vec<u8>>,
    pub email_verified_at: Option<DateTime<Utc>>,
    pub document_verified_at: Option<DateTime<Utc>>,
    pub liveness_verified_at: Option<DateTime<Utc>>,
    pub identity_matched_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl DbPayerSession {
    pub fn facts(&self) -> VerificationFacts {
        VerificationFacts {
            email: self.email_verified_at.is_some(),
            document: self.document_verified_at.is_some(),
            liveness: self.liveness_verified_at.is_some(),
            identity_match: self.identity_matched_at.is_some(),
        }
    }

    /// Whether this session may see the invoice's content and mechanics.
    pub fn satisfies(&self, mode: PayerPolicyMode) -> bool {
        self.facts().satisfy(mode)
    }
}

/// A freshly minted session. `token` exists only here and in the response
/// that carries it to the payer.
#[derive(Debug, Clone)]
pub struct CreatedPayerSession {
    pub id: Uuid,
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

/// A pending email attempt awaiting its one-time code.
#[derive(Debug, Clone)]
pub struct EmailVerificationAttempt {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Error)]
pub enum StartEmailVerificationError {
    /// A code was sent for this invoice too recently; `retry_after` is how
    /// long until another may be requested.
    #[error("a code was sent recently; retry in {retry_after:?}")]
    Cooldown { retry_after: Duration },
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// What approving an email attempt changed.
#[derive(Debug, Clone)]
pub struct VerificationCompletion {
    pub session: DbPayerSession,
    /// Set when this approval completed the invoice's verification, which
    /// only `verified_email` allows; identity modes complete in Slice 4.
    pub invoice_completed_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct PayerSessionRepository {
    pool: PgPool,
}

fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

impl PayerSessionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Mint a session for one invoice: 32 random bytes as unpadded base64url,
    /// of which only the hash is stored.
    pub async fn create(
        &self,
        invoice_id: Uuid,
        ttl: Duration,
    ) -> Result<CreatedPayerSession, sqlx::Error> {
        let mut secret = [0u8; 32];
        rand::rng().fill_bytes(&mut secret);
        let token = URL_SAFE_NO_PAD.encode(secret);
        let id = Uuid::now_v7();
        let expires_at: DateTime<Utc> = sqlx::query_scalar(
            r#"
            INSERT INTO payer_sessions (id, token_hash, invoice_id, expires_at)
            VALUES ($1, $2, $3, now() + make_interval(secs => $4))
            RETURNING expires_at
            "#,
        )
        .bind(id)
        .bind(token_hash(&token).as_slice())
        .bind(invoice_id)
        .bind(ttl.as_secs_f64())
        .fetch_one(&self.pool)
        .await?;
        Ok(CreatedPayerSession {
            id,
            token,
            expires_at,
        })
    }

    /// The unexpired session behind `token`, only if it belongs to
    /// `invoice_id`: a session never unlocks another invoice.
    pub async fn find_active(
        &self,
        token: &str,
        invoice_id: Uuid,
    ) -> Result<Option<DbPayerSession>, sqlx::Error> {
        sqlx::query_as::<_, DbPayerSession>(
            r#"
            SELECT id, invoice_id, payer_ref, email_verified_at, document_verified_at,
                   liveness_verified_at, identity_matched_at, created_at, expires_at
            FROM payer_sessions
            WHERE token_hash = $1 AND invoice_id = $2 AND expires_at > now()
            "#,
        )
        .bind(token_hash(token).as_slice())
        .bind(invoice_id)
        .fetch_optional(&self.pool)
        .await
    }

    /// Record that a code is being sent for `session_id`'s invoice. Earlier
    /// pending attempts on the session are abandoned; the cooldown is per
    /// invoice, whatever session asks, so the expected mailbox cannot be
    /// flooded by opening new sessions.
    pub async fn begin_email_verification(
        &self,
        session_id: Uuid,
        cooldown: Duration,
    ) -> Result<EmailVerificationAttempt, StartEmailVerificationError> {
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
        let latest: Option<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT max(created_at) FROM payer_verifications WHERE invoice_id = $1 AND kind = 'email'",
        )
        .bind(invoice_id)
        .fetch_one(&mut *tx)
        .await?;
        if let Some(latest) = latest {
            let elapsed = (Utc::now() - latest).to_std().unwrap_or_default();
            if elapsed < cooldown {
                return Err(StartEmailVerificationError::Cooldown {
                    retry_after: cooldown - elapsed,
                });
            }
        }
        sqlx::query(
            r#"
            UPDATE payer_verifications SET status = 'abandoned'
            WHERE payer_session_id = $1 AND kind = 'email' AND status = 'pending'
            "#,
        )
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
        let id = Uuid::now_v7();
        let created_at: DateTime<Utc> = sqlx::query_scalar(
            r#"
            INSERT INTO payer_verifications
                (id, invoice_id, account_id, payer_session_id, kind, status, provider)
            VALUES ($1, $2, $3, $4, 'email', 'pending', 'auth0')
            RETURNING created_at
            "#,
        )
        .bind(id)
        .bind(invoice_id)
        .bind(account_id)
        .bind(session_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(EmailVerificationAttempt { id, created_at })
    }

    /// Whether the session has a code outstanding.
    pub async fn has_pending_email_verification(
        &self,
        session_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM payer_verifications
                WHERE payer_session_id = $1 AND kind = 'email' AND status = 'pending'
            )
            "#,
        )
        .bind(session_id)
        .fetch_one(&self.pool)
        .await
    }

    /// The mailbox is proven: bind the payer reference and the email fact to
    /// the session, approve its pending attempt, and, for `verified_email`,
    /// complete the invoice in the same transaction. Identity modes leave the
    /// invoice incomplete until their identity facts arrive.
    pub async fn approve_email(
        &self,
        session_id: Uuid,
        payer_ref: B256,
        verified_at: DateTime<Utc>,
        provider_event_id: &str,
    ) -> Result<VerificationCompletion, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let session = sqlx::query_as::<_, DbPayerSession>(
            r#"
            UPDATE payer_sessions
            SET payer_ref = $2, email_verified_at = COALESCE(email_verified_at, $3)
            WHERE id = $1
            RETURNING id, invoice_id, payer_ref, email_verified_at, document_verified_at,
                      liveness_verified_at, identity_matched_at, created_at, expires_at
            "#,
        )
        .bind(session_id)
        .bind(payer_ref.as_slice())
        .bind(verified_at)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            UPDATE payer_verifications
            SET status = 'approved', verified_at = $2, payer_ref = $3, provider_event_id = $4
            WHERE payer_session_id = $1 AND kind = 'email' AND status = 'pending'
            "#,
        )
        .bind(session_id)
        .bind(verified_at)
        .bind(payer_ref.as_slice())
        .bind(provider_event_id)
        .execute(&mut *tx)
        .await?;
        let invoice_completed_at: Option<DateTime<Utc>> = sqlx::query_scalar(
            r#"
            UPDATE invoices
            SET verification_completed_at = $2, updated_at = now()
            WHERE id = $1
              AND payer_policy_mode = 'verified_email'
              AND verification_completed_at IS NULL
            RETURNING verification_completed_at
            "#,
        )
        .bind(session.invoice_id)
        .bind(verified_at)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(VerificationCompletion {
            session,
            invoice_completed_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invoices::tests::{account, issuance_input};
    use crate::{DbInvoice, InvoiceRepository};
    use gateway_core::PayerPolicy;

    async fn invoice(pool: &PgPool, key: &str, policy: PayerPolicy) -> DbInvoice {
        let owner = account(pool, 1).await;
        let mut input = issuance_input(owner, key, None);
        input.issuance_snapshot.payer_policy = policy.clone();
        input.payer_policy = policy;
        InvoiceRepository::new(pool.clone())
            .insert_issued(&input, None)
            .await
            .unwrap()
            .row
    }

    fn email_policy() -> PayerPolicy {
        PayerPolicy::VerifiedEmail {
            expected_email: "alice@example.com".into(),
        }
    }

    #[test]
    fn payer_ref_is_merchant_scoped_and_deterministic() {
        let key = [7u8; 32];
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        assert_eq!(
            payer_ref(&key, a, "alice@example.com"),
            payer_ref(&key, a, "alice@example.com")
        );
        assert_ne!(
            payer_ref(&key, a, "alice@example.com"),
            payer_ref(&key, b, "alice@example.com")
        );
        assert_ne!(
            payer_ref(&key, a, "alice@example.com"),
            payer_ref(&key, a, "bob@example.com")
        );
        assert_ne!(
            payer_ref(&key, a, "alice@example.com"),
            payer_ref(&[8u8; 32], a, "alice@example.com")
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn session_tokens_are_hashed_unpadded_base64url_and_expire(pool: PgPool) {
        let repo = PayerSessionRepository::new(pool.clone());
        let row = invoice(&pool, "gated", email_policy()).await;
        let session = repo.create(row.id, PAYER_SESSION_TTL).await.unwrap();

        assert_eq!(URL_SAFE_NO_PAD.decode(&session.token).unwrap().len(), 32);
        assert!(!session.token.contains('='));
        let stored: Vec<u8> =
            sqlx::query_scalar("SELECT token_hash FROM payer_sessions WHERE id = $1")
                .bind(session.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, Sha256::digest(session.token.as_bytes()).to_vec());
        assert_ne!(stored, session.token.as_bytes());
        assert!(session.expires_at > Utc::now() + Duration::from_secs(23 * 3600));

        let found = repo
            .find_active(&session.token, row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, session.id);
        assert!(!found.satisfies(PayerPolicyMode::VerifiedEmail));
        assert!(
            repo.find_active("not-the-token", row.id)
                .await
                .unwrap()
                .is_none()
        );

        sqlx::query(
            "UPDATE payer_sessions SET created_at = now() - interval '2 hours', expires_at = now() - interval '1 hour' WHERE id = $1",
        )
            .bind(session.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            repo.find_active(&session.token, row.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn a_session_cannot_unlock_another_invoice(pool: PgPool) {
        let repo = PayerSessionRepository::new(pool.clone());
        let first = invoice(&pool, "first", email_policy()).await;
        let second = invoice(&pool, "second", email_policy()).await;
        let session = repo.create(first.id, PAYER_SESSION_TTL).await.unwrap();
        repo.begin_email_verification(session.id, Duration::from_secs(60))
            .await
            .unwrap();
        let reference = payer_ref(&[1u8; 32], first.account_id, "alice@example.com");
        let completion = repo
            .approve_email(session.id, reference, Utc::now(), "event-1")
            .await
            .unwrap();
        assert!(completion.invoice_completed_at.is_some());
        assert!(completion.session.satisfies(PayerPolicyMode::VerifiedEmail));
        assert_eq!(
            completion.session.payer_ref.as_deref(),
            Some(reference.as_slice())
        );

        assert!(
            repo.find_active(&session.token, first.id)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            repo.find_active(&session.token, second.id)
                .await
                .unwrap()
                .is_none()
        );
        let completed: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM invoices WHERE verification_completed_at IS NOT NULL",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(completed, vec![first.id]);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn email_attempts_cool_down_per_invoice_and_identity_modes_stay_incomplete(pool: PgPool) {
        let repo = PayerSessionRepository::new(pool.clone());
        let row = invoice(
            &pool,
            "identity",
            PayerPolicy::VerifiedIdentityUnattributed {
                expected_email: "alice@example.com".into(),
            },
        )
        .await;
        let first = repo.create(row.id, PAYER_SESSION_TTL).await.unwrap();
        let second = repo.create(row.id, PAYER_SESSION_TTL).await.unwrap();
        let cooldown = Duration::from_secs(60);
        repo.begin_email_verification(first.id, cooldown)
            .await
            .unwrap();
        assert!(repo.has_pending_email_verification(first.id).await.unwrap());
        // Another session for the same invoice shares the cooldown.
        assert!(matches!(
            repo.begin_email_verification(second.id, cooldown).await,
            Err(StartEmailVerificationError::Cooldown { retry_after }) if retry_after <= cooldown
        ));
        assert!(
            !repo
                .has_pending_email_verification(second.id)
                .await
                .unwrap()
        );
        // Once it lapses a resend replaces the earlier pending attempt.
        sqlx::query("UPDATE payer_verifications SET created_at = now() - interval '2 minutes'")
            .execute(&pool)
            .await
            .unwrap();
        repo.begin_email_verification(first.id, cooldown)
            .await
            .unwrap();
        let statuses: Vec<String> = sqlx::query_scalar(
            "SELECT status FROM payer_verifications WHERE payer_session_id = $1 ORDER BY created_at",
        )
        .bind(first.id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(statuses, ["abandoned", "pending"]);

        let completion = repo
            .approve_email(
                first.id,
                payer_ref(&[1u8; 32], row.account_id, "alice@example.com"),
                Utc::now(),
                "event-2",
            )
            .await
            .unwrap();
        assert!(completion.invoice_completed_at.is_none());
        assert!(completion.session.facts().email);
        assert!(
            !completion
                .session
                .satisfies(PayerPolicyMode::VerifiedIdentityUnattributed)
        );
        assert!(!repo.has_pending_email_verification(first.id).await.unwrap());
        let (status, event): (String, Option<String>) = sqlx::query_as(
            "SELECT status, provider_event_id FROM payer_verifications WHERE payer_session_id = $1 AND status <> 'abandoned'",
        )
        .bind(first.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "approved");
        assert_eq!(event.as_deref(), Some("event-2"));
    }
}
