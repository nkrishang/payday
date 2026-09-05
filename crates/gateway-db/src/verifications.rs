//! Payer sessions, email verification attempts, and merchant client secrets
//! (product plan §4.6).
//!
//! A payer session is an opaque bearer token scoped to one invoice. Only its
//! SHA-256 is stored, so a database read never yields a usable token. The
//! session accumulates verification facts; the invoice's own
//! `verification_completed_at` is set separately and gates settlement.
//!
//! The session also carries the one-time wallet challenge: a nonce issued
//! once the policy is satisfied, which the payer's wallet signs inside its
//! attestation. It is consumed by the binding that accepts the signature
//! (`InvoiceRepository::bind_payer_wallet`).
//! A client secret is the merchant-session mode's credential: minted for one
//! invoice by the merchant API, returned once, stored hashed, and spent by
//! exactly one exchange, which mints the payer session that carries the
//! merchant-session fact.

use std::time::Duration;

use alloy_primitives::B256;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use gateway_core::{PayerPolicyMode, VerificationFacts};
use hmac::{Hmac, Mac};
use rand::Rng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use sqlx::types::chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

/// How long a payer session stays usable: long enough for a payer to come
/// back to the page later that day, and it never outlives the day.
pub const PAYER_SESSION_TTL: Duration = Duration::from_secs(24 * 3600);

/// How long a client secret may wait to be exchanged: long enough for a
/// redirect and a slow page load, short enough that a leaked link is stale
/// before it travels far. The merchant mints another for a payer who returns.
pub const CLIENT_SECRET_TTL: Duration = Duration::from_secs(15 * 60);
/// Client secrets are recognizable on sight, like API keys, so a merchant
/// spotting one in a log knows what leaked.
pub const CLIENT_SECRET_PREFIX: &str = "cs_";

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
    /// The outstanding wallet challenge, if one was issued and not yet
    /// consumed.
    pub wallet_nonce: Option<Vec<u8>>,
    pub wallet_nonce_expires_at: Option<DateTime<Utc>>,
    /// Set when the session was minted by exchanging a merchant client secret.
    pub merchant_session_verified_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl DbPayerSession {
    /// The session's own facts. The wallet fact belongs to the invoice, not
    /// the session, so it is reported as absent here; callers that know the
    /// invoice's binding fill it in.
    pub fn facts(&self) -> VerificationFacts {
        VerificationFacts {
            email: self.email_verified_at.is_some(),
            wallet: false,
            merchant_session: self.merchant_session_verified_at.is_some(),
        }
    }

    /// Whether this session may see the invoice's content and mechanics.
    pub fn satisfies(&self, mode: PayerPolicyMode) -> bool {
        self.facts().satisfy(mode)
    }

    /// The unexpired challenge this session holds, if any.
    pub fn wallet_challenge(&self, now: DateTime<Utc>) -> Option<WalletChallenge> {
        let nonce = B256::try_from(self.wallet_nonce.as_deref()?).ok()?;
        let expires_at = self.wallet_nonce_expires_at?;
        (expires_at > now).then_some(WalletChallenge { nonce, expires_at })
    }
}

/// A one-time wallet challenge issued to a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalletChallenge {
    pub nonce: B256,
    pub expires_at: DateTime<Utc>,
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

/// A freshly minted client secret. `secret` exists only here and in the
/// merchant response that carries it.
#[derive(Debug, Clone)]
pub struct CreatedClientSecret {
    pub id: Uuid,
    pub secret: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Error)]
pub enum ExchangeClientSecretError {
    /// No such secret for this invoice. A secret for another invoice is
    /// unknown here too: it never opens anything but the invoice it was
    /// minted for.
    #[error("the client secret is not valid for this payment")]
    Unknown,
    /// The secret was already exchanged; the link was opened once already.
    #[error("the client secret was already used")]
    Used,
    #[error("the client secret has expired")]
    Expired,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// What exchanging a client secret produced: the payer session it minted,
/// already carrying the merchant-session fact, and whether that completed
/// the invoice's verification.
#[derive(Debug, Clone)]
pub struct ClientSecretExchange {
    pub session: DbPayerSession,
    pub token: String,
    pub invoice_completed_at: Option<DateTime<Utc>>,
}

/// One verification attempt as the merchant may see it: what was attempted
/// and where it stands. Never the code, the session, or the payer reference.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DbVerificationAttempt {
    pub id: Uuid,
    pub kind: String,
    pub status: String,
    pub verified_at: Option<DateTime<Utc>>,
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
    /// Set when this approval completed the invoice's verification: the
    /// first proof of the expected mailbox while the invoice is still live.
    pub invoice_completed_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct PayerSessionRepository {
    pool: PgPool,
}

fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// 32 random bytes as unpadded base64url: the payer session token, and the
/// body of a client secret.
fn random_token() -> String {
    let mut secret = [0u8; 32];
    rand::rng().fill_bytes(&mut secret);
    URL_SAFE_NO_PAD.encode(secret)
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
        let token = random_token();
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
            SELECT id, invoice_id, payer_ref, email_verified_at, merchant_session_verified_at,
                   wallet_nonce, wallet_nonce_expires_at, created_at, expires_at
            FROM payer_sessions
            WHERE token_hash = $1 AND invoice_id = $2 AND expires_at > now()
            "#,
        )
        .bind(token_hash(token).as_slice())
        .bind(invoice_id)
        .fetch_optional(&self.pool)
        .await
    }

    /// Issue (or replace) the session's wallet challenge: 32 random bytes the
    /// payer's wallet must sign, void after `ttl`. Callers check that the
    /// session satisfies the invoice's policy first; the challenge is what
    /// orders the wallet signature after the identity step.
    pub async fn issue_wallet_challenge(
        &self,
        session_id: Uuid,
        ttl: Duration,
    ) -> Result<WalletChallenge, sqlx::Error> {
        let nonce = B256::from(rand::rng().random::<[u8; 32]>());
        let expires_at: DateTime<Utc> = sqlx::query_scalar(
            r#"
            UPDATE payer_sessions
            SET wallet_nonce = $2,
                wallet_nonce_expires_at = LEAST(expires_at, now() + make_interval(secs => $3))
            WHERE id = $1 AND expires_at > now()
            RETURNING wallet_nonce_expires_at
            "#,
        )
        .bind(session_id)
        .bind(nonce.as_slice())
        .bind(ttl.as_secs_f64())
        .fetch_one(&self.pool)
        .await?;
        Ok(WalletChallenge { nonce, expires_at })
    }

    pub async fn delete(&self, session_id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM payer_sessions WHERE id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn abandon_email_verification(&self, session_id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE payer_verifications SET status = 'abandoned' WHERE payer_session_id = $1 AND kind = 'email' AND status = 'pending'",
        )
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
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
            r#"
            SELECT max(created_at)
            FROM payer_verifications
            WHERE invoice_id = $1 AND kind = 'email' AND status IN ('pending', 'approved')
            "#,
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

    /// Every attempt on a merchant's invoice, oldest first.
    pub async fn attempts_for_invoice(
        &self,
        account_id: Uuid,
        invoice_id: Uuid,
    ) -> Result<Vec<DbVerificationAttempt>, sqlx::Error> {
        sqlx::query_as::<_, DbVerificationAttempt>(
            r#"
            SELECT id, kind, status, verified_at, created_at
            FROM payer_verifications
            WHERE account_id = $1 AND invoice_id = $2
            ORDER BY created_at, id
            "#,
        )
        .bind(account_id)
        .bind(invoice_id)
        .fetch_all(&self.pool)
        .await
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
    /// the session, approve its pending attempt, and complete the invoice in
    /// the same transaction.
    pub async fn approve_email(
        &self,
        session_id: Uuid,
        payer_ref: B256,
        verified_at: DateTime<Utc>,
        provider_event_id: &str,
    ) -> Result<VerificationCompletion, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // A commit acknowledgement can be lost after PostgreSQL committed.
        // Make the provider event an idempotency key so the handler can retry
        // without requiring the already-consumed OTP again.
        if let Some(existing_session_id) = sqlx::query_scalar::<_, Uuid>(
            "SELECT payer_session_id FROM payer_verifications WHERE provider_event_id = $1 AND status = 'approved'",
        )
        .bind(provider_event_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            let session = sqlx::query_as::<_, DbPayerSession>(
                r#"
                SELECT id, invoice_id, payer_ref, email_verified_at, merchant_session_verified_at,
                       wallet_nonce, wallet_nonce_expires_at, created_at, expires_at
                FROM payer_sessions WHERE id = $1
                "#,
            )
            .bind(existing_session_id)
            .fetch_one(&mut *tx)
            .await?;
            let completed = sqlx::query_scalar(
                "SELECT verification_completed_at FROM invoices WHERE id = $1",
            )
            .bind(session.invoice_id)
            .fetch_one(&mut *tx)
            .await?;
            return Ok(VerificationCompletion {
                session,
                invoice_completed_at: completed,
            });
        }
        let attempt = sqlx::query(
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
        if attempt.rows_affected() != 1 {
            return Err(sqlx::Error::RowNotFound);
        }
        let session = sqlx::query_as::<_, DbPayerSession>(
            r#"
            UPDATE payer_sessions
            SET payer_ref = $2, email_verified_at = COALESCE(email_verified_at, $3)
            WHERE id = $1 AND expires_at > $3
            RETURNING id, invoice_id, payer_ref, email_verified_at, merchant_session_verified_at,
                      wallet_nonce, wallet_nonce_expires_at, created_at, expires_at
            "#,
        )
        .bind(session_id)
        .bind(payer_ref.as_slice())
        .bind(verified_at)
        .fetch_one(&mut *tx)
        .await?;
        // A completed terminal invoice may issue a fresh receipt capability
        // after the expected mailbox is proven again: the session above
        // satisfies the policy on its own. This never revives an invoice or
        // extends the old bearer, and the update below leaves terminal
        // invoices alone.
        let invoice_completed_at: Option<DateTime<Utc>> = sqlx::query_scalar(
            r#"
            UPDATE invoices
            SET verification_completed_at = $2, updated_at = now()
            WHERE id = $1
              AND payer_policy_mode = 'verified_email'
              AND verification_completed_at IS NULL
              AND status IN ('created', 'funded', 'deploying')
              AND expiration_timestamp >= EXTRACT(EPOCH FROM $2)::bigint
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

    /// Mint a client secret for one invoice: `cs_` and 32 random bytes as
    /// unpadded base64url, of which only the hash is stored. Earlier secrets
    /// stay valid until they are spent or expire; each is single-use on its
    /// own, so a merchant retrying a redirect never breaks the link it
    /// already sent.
    pub async fn create_client_secret(
        &self,
        invoice_id: Uuid,
        account_id: Uuid,
        ttl: Duration,
    ) -> Result<CreatedClientSecret, sqlx::Error> {
        let secret = format!("{CLIENT_SECRET_PREFIX}{}", random_token());
        let id = Uuid::now_v7();
        let expires_at: DateTime<Utc> = sqlx::query_scalar(
            r#"
            INSERT INTO payer_client_secrets (id, invoice_id, account_id, secret_hash, expires_at)
            VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5))
            RETURNING expires_at
            "#,
        )
        .bind(id)
        .bind(invoice_id)
        .bind(account_id)
        .bind(token_hash(&secret).as_slice())
        .bind(ttl.as_secs_f64())
        .fetch_one(&self.pool)
        .await?;
        Ok(CreatedClientSecret {
            id,
            secret,
            expires_at,
        })
    }

    /// Spend a client secret: in one transaction, mark it used, mint the
    /// payer session it opens (already carrying the merchant-session fact),
    /// record the approved attempt, and complete the invoice's verification
    /// while it is still live. The secret row is locked first, so two
    /// exchanges of the same secret resolve to one session and one `Used`.
    pub async fn exchange_client_secret(
        &self,
        secret: &str,
        invoice_id: Uuid,
        session_ttl: Duration,
    ) -> Result<ClientSecretExchange, ExchangeClientSecretError> {
        #[derive(sqlx::FromRow)]
        struct StoredSecret {
            id: Uuid,
            account_id: Uuid,
            used_at: Option<DateTime<Utc>>,
            expires_at: DateTime<Utc>,
        }
        let mut tx = self.pool.begin().await?;
        let found = sqlx::query_as::<_, StoredSecret>(
            r#"
            SELECT id, account_id, used_at, expires_at
            FROM payer_client_secrets
            WHERE secret_hash = $1 AND invoice_id = $2
            FOR UPDATE
            "#,
        )
        .bind(token_hash(secret).as_slice())
        .bind(invoice_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(StoredSecret {
            id: secret_id,
            account_id,
            used_at,
            expires_at,
        }) = found
        else {
            return Err(ExchangeClientSecretError::Unknown);
        };
        let now = Utc::now();
        if used_at.is_some() {
            return Err(ExchangeClientSecretError::Used);
        }
        if expires_at <= now {
            return Err(ExchangeClientSecretError::Expired);
        }
        let token = random_token();
        let session_id = Uuid::now_v7();
        let session = sqlx::query_as::<_, DbPayerSession>(
            r#"
            INSERT INTO payer_sessions
                (id, token_hash, invoice_id, expires_at, merchant_session_verified_at)
            VALUES ($1, $2, $3, $4 + make_interval(secs => $5), $4)
            RETURNING id, invoice_id, payer_ref, email_verified_at, merchant_session_verified_at,
                      wallet_nonce, wallet_nonce_expires_at, created_at, expires_at
            "#,
        )
        .bind(session_id)
        .bind(token_hash(&token).as_slice())
        .bind(invoice_id)
        .bind(now)
        .bind(session_ttl.as_secs_f64())
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE payer_client_secrets SET used_at = $2, payer_session_id = $3 WHERE id = $1",
        )
        .bind(secret_id)
        .bind(now)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO payer_verifications
                (id, invoice_id, account_id, payer_session_id, kind, status, provider, verified_at)
            VALUES ($1, $2, $3, $4, 'merchant_session', 'approved', 'merchant', $5)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(invoice_id)
        .bind(account_id)
        .bind(session_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        // As for a proven mailbox: the first exchange while the invoice is
        // live completes its verification; a terminal invoice only gains a
        // fresh receipt session.
        let invoice_completed_at: Option<DateTime<Utc>> = sqlx::query_scalar(
            r#"
            UPDATE invoices
            SET verification_completed_at = $2, updated_at = now()
            WHERE id = $1
              AND payer_policy_mode = 'merchant_session'
              AND verification_completed_at IS NULL
              AND status IN ('created', 'funded', 'deploying')
              AND expiration_timestamp >= EXTRACT(EPOCH FROM $2)::bigint
            RETURNING verification_completed_at
            "#,
        )
        .bind(invoice_id)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(ClientSecretExchange {
            session,
            token,
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
    async fn email_attempts_cool_down_per_invoice_and_are_listed_for_the_merchant(pool: PgPool) {
        let repo = PayerSessionRepository::new(pool.clone());
        let row = invoice(&pool, "cooldown", email_policy()).await;
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
        // A provider send failure abandons its attempt and must not preserve
        // the cooldown for a code that was never delivered.
        repo.abandon_email_verification(first.id).await.unwrap();
        repo.begin_email_verification(second.id, cooldown)
            .await
            .unwrap();
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
        assert!(completion.invoice_completed_at.is_some());
        assert!(completion.session.facts().email);
        assert!(completion.session.satisfies(PayerPolicyMode::VerifiedEmail));
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

        // The merchant sees every attempt on the invoice, oldest first, with
        // nothing but its kind, status, and times.
        let attempts = repo
            .attempts_for_invoice(row.account_id, row.id)
            .await
            .unwrap();
        let listed: Vec<(&str, &str, bool)> = attempts
            .iter()
            .map(|attempt| {
                (
                    attempt.kind.as_str(),
                    attempt.status.as_str(),
                    attempt.verified_at.is_some(),
                )
            })
            .collect();
        assert_eq!(
            listed,
            [
                ("email", "abandoned", false),
                ("email", "pending", false),
                ("email", "approved", true),
            ]
        );
        assert!(
            repo.attempts_for_invoice(Uuid::from_u128(999), row.id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn wallet_challenges_are_per_session_expire_and_are_consumed_by_binding(pool: PgPool) {
        let repo = PayerSessionRepository::new(pool.clone());
        let row = invoice(&pool, "challenge", PayerPolicy::Permissionless).await;
        let session = repo.create(row.id, PAYER_SESSION_TTL).await.unwrap();
        let found = repo
            .find_active(&session.token, row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.wallet_challenge(Utc::now()), None);

        let first = repo
            .issue_wallet_challenge(session.id, Duration::from_secs(600))
            .await
            .unwrap();
        let second = repo
            .issue_wallet_challenge(session.id, Duration::from_secs(600))
            .await
            .unwrap();
        assert_ne!(first.nonce, second.nonce, "each challenge is fresh");
        let found = repo
            .find_active(&session.token, row.id)
            .await
            .unwrap()
            .unwrap();
        // Only the latest challenge stands, and only until it expires.
        assert_eq!(found.wallet_challenge(Utc::now()), Some(second));
        assert_eq!(
            found.wallet_challenge(second.expires_at + Duration::from_secs(1)),
            None
        );
        assert!(second.expires_at <= session.expires_at);

        // The binding consumes it; a bound invoice lists the wallet attempt.
        crate::invoices::tests::bind_for_test(
            &pool,
            row.id,
            &crate::invoices::tests::TEST_PAYER_KEY,
        )
        .await;
        let attempts = repo
            .attempts_for_invoice(row.account_id, row.id)
            .await
            .unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].kind, "wallet");
        assert_eq!(attempts[0].status, "approved");
        assert!(attempts[0].verified_at.is_some());

        // An expired session cannot be challenged.
        sqlx::query(
            "UPDATE payer_sessions SET created_at = now() - interval '2 hours', expires_at = now() - interval '1 hour' WHERE id = $1",
        )
        .bind(session.id)
        .execute(&pool)
        .await
        .unwrap();
        assert!(matches!(
            repo.issue_wallet_challenge(session.id, Duration::from_secs(600))
                .await,
            Err(sqlx::Error::RowNotFound)
        ));
    }

    fn merchant_policy() -> PayerPolicy {
        PayerPolicy::MerchantSession {
            payer_reference: "user_123".into(),
        }
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn a_client_secret_is_hashed_single_use_and_opens_only_its_own_invoice(pool: PgPool) {
        let repo = PayerSessionRepository::new(pool.clone());
        let row = invoice(&pool, "merchant", merchant_policy()).await;
        let other = invoice(&pool, "merchant-other", merchant_policy()).await;
        let minted = repo
            .create_client_secret(row.id, row.account_id, CLIENT_SECRET_TTL)
            .await
            .unwrap();

        assert!(minted.secret.starts_with(CLIENT_SECRET_PREFIX));
        assert_eq!(
            URL_SAFE_NO_PAD
                .decode(&minted.secret[CLIENT_SECRET_PREFIX.len()..])
                .unwrap()
                .len(),
            32
        );
        let (stored, used): (Vec<u8>, Option<DateTime<Utc>>) =
            sqlx::query_as("SELECT secret_hash, used_at FROM payer_client_secrets WHERE id = $1")
                .bind(minted.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, Sha256::digest(minted.secret.as_bytes()).to_vec());
        assert!(used.is_none());
        assert!(minted.expires_at > Utc::now() + Duration::from_secs(14 * 60));
        assert!(minted.expires_at <= Utc::now() + CLIENT_SECRET_TTL);

        // The secret opens the invoice it was minted for and nothing else.
        assert!(matches!(
            repo.exchange_client_secret(&minted.secret, other.id, PAYER_SESSION_TTL)
                .await,
            Err(ExchangeClientSecretError::Unknown)
        ));
        assert!(matches!(
            repo.exchange_client_secret("cs_not-a-secret", row.id, PAYER_SESSION_TTL)
                .await,
            Err(ExchangeClientSecretError::Unknown)
        ));

        let exchange = repo
            .exchange_client_secret(&minted.secret, row.id, PAYER_SESSION_TTL)
            .await
            .unwrap();
        assert!(exchange.invoice_completed_at.is_some());
        assert!(exchange.session.merchant_session_verified_at.is_some());
        assert!(exchange.session.email_verified_at.is_none());
        assert!(exchange.session.payer_ref.is_none());
        assert!(exchange.session.satisfies(PayerPolicyMode::MerchantSession));
        assert!(!exchange.session.satisfies(PayerPolicyMode::VerifiedEmail));
        // The minted session is a normal payer session for this invoice only.
        let found = repo
            .find_active(&exchange.token, row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, exchange.session.id);
        assert!(
            repo.find_active(&exchange.token, other.id)
                .await
                .unwrap()
                .is_none()
        );
        let (used, session_id): (Option<DateTime<Utc>>, Option<Uuid>) = sqlx::query_as(
            "SELECT used_at, payer_session_id FROM payer_client_secrets WHERE id = $1",
        )
        .bind(minted.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(used.is_some());
        assert_eq!(session_id, Some(exchange.session.id));

        // Spent: a second exchange mints nothing, and the first session lives on.
        assert!(matches!(
            repo.exchange_client_secret(&minted.secret, row.id, PAYER_SESSION_TTL)
                .await,
            Err(ExchangeClientSecretError::Used)
        ));
        let sessions: i64 =
            sqlx::query_scalar("SELECT count(*) FROM payer_sessions WHERE invoice_id = $1")
                .bind(row.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(sessions, 1);
        let completed: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM invoices WHERE verification_completed_at IS NOT NULL",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(completed, vec![row.id]);

        // The merchant sees one approved merchant-session attempt.
        let attempts = repo
            .attempts_for_invoice(row.account_id, row.id)
            .await
            .unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].kind, "merchant_session");
        assert_eq!(attempts[0].status, "approved");
        assert!(attempts[0].verified_at.is_some());
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn expired_client_secrets_are_refused_and_fresh_ones_reopen_terminal_invoices(
        pool: PgPool,
    ) {
        let repo = PayerSessionRepository::new(pool.clone());
        let row = invoice(&pool, "merchant", merchant_policy()).await;
        let stale = repo
            .create_client_secret(row.id, row.account_id, CLIENT_SECRET_TTL)
            .await
            .unwrap();
        sqlx::query("UPDATE payer_client_secrets SET created_at = now() - interval '20 minutes', expires_at = now() - interval '5 minutes' WHERE id = $1")
            .bind(stale.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            repo.exchange_client_secret(&stale.secret, row.id, PAYER_SESSION_TTL)
                .await,
            Err(ExchangeClientSecretError::Expired)
        ));
        // Minting again does not disturb the earlier secret's record, and
        // two live secrets are each good exactly once.
        let first = repo
            .create_client_secret(row.id, row.account_id, CLIENT_SECRET_TTL)
            .await
            .unwrap();
        let second = repo
            .create_client_secret(row.id, row.account_id, CLIENT_SECRET_TTL)
            .await
            .unwrap();
        assert_ne!(first.secret, second.secret);
        let opened = repo
            .exchange_client_secret(&first.secret, row.id, PAYER_SESSION_TTL)
            .await
            .unwrap();
        assert!(opened.invoice_completed_at.is_some());

        // Settled: a fresh secret still opens a receipt session, but the
        // invoice's own completion is already recorded and never moves. Only
        // a bound invoice can have settled, so bind the payer's wallet first.
        crate::invoices::tests::bind_for_test(
            &pool,
            row.id,
            &crate::invoices::tests::TEST_PAYER_KEY,
        )
        .await;
        sqlx::query("UPDATE invoices SET status = 'fulfilled', settlement_tx_hash = $2, settled_at = now() WHERE id = $1")
            .bind(row.id)
            .bind([0x44u8; 32].as_slice())
            .execute(&pool)
            .await
            .unwrap();
        let receipt = repo
            .exchange_client_secret(&second.secret, row.id, PAYER_SESSION_TTL)
            .await
            .unwrap();
        assert!(receipt.invoice_completed_at.is_none());
        assert!(receipt.session.satisfies(PayerPolicyMode::MerchantSession));
        let completed_at: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT verification_completed_at FROM invoices WHERE id = $1")
                .bind(row.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(completed_at, opened.invoice_completed_at);
        let attempts = repo
            .attempts_for_invoice(row.account_id, row.id)
            .await
            .unwrap();
        // Two exchanges and the wallet binding between them.
        assert_eq!(attempts.len(), 3);
        assert_eq!(
            attempts
                .iter()
                .filter(|attempt| attempt.kind == "merchant_session")
                .count(),
            2
        );
        assert!(
            attempts
                .iter()
                .all(|attempt| attempt.kind == "merchant_session" || attempt.kind == "wallet")
        );
    }
}
