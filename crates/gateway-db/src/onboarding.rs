//! The onboarding walkthrough's one real, self-funded deposit request: at
//! most one per account, ever (see migration 0016's comment for why the
//! table's shape is the enforcement).

use sqlx::PgPool;
use uuid::Uuid;

use crate::AccountId;

/// What claiming the one-shot slot found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingClaim {
    /// Nobody had claimed this account's slot yet; proceed to verify and pay.
    Claimed,
    /// This account already claimed the slot for this same payment, but no
    /// transfer was recorded yet (a crash between claiming and broadcasting,
    /// or a bare retry) — safe to attempt the transfer again.
    PendingRetry,
    /// This account already claimed the slot for this same payment and the
    /// transfer already broadcast; hand back the same hash rather than
    /// sending a second one.
    AlreadySubmitted(String),
    /// This account's one-shot slot belongs to a *different* payment.
    Conflict,
}

#[derive(Clone)]
pub struct OnboardingDemoPaymentRepository {
    pool: PgPool,
}

impl OnboardingDemoPaymentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Atomically claim the account's one-shot onboarding slot for
    /// `payment_id`, or report why it could not be claimed.
    pub async fn claim(
        &self,
        account: AccountId,
        payment_id: Uuid,
    ) -> Result<OnboardingClaim, sqlx::Error> {
        let inserted = sqlx::query(
            r#"
            INSERT INTO onboarding_demo_payments (account_id, payment_id)
            VALUES ($1, $2)
            ON CONFLICT (account_id) DO NOTHING
            "#,
        )
        .bind(account.0)
        .bind(payment_id)
        .execute(&self.pool)
        .await?;
        if inserted.rows_affected() == 1 {
            return Ok(OnboardingClaim::Claimed);
        }
        let (existing_payment_id, tx_hash): (Uuid, Option<String>) = sqlx::query_as(
            "SELECT payment_id, tx_hash FROM onboarding_demo_payments WHERE account_id = $1",
        )
        .bind(account.0)
        .fetch_one(&self.pool)
        .await?;
        if existing_payment_id != payment_id {
            return Ok(OnboardingClaim::Conflict);
        }
        Ok(match tx_hash {
            Some(hash) => OnboardingClaim::AlreadySubmitted(hash),
            None => OnboardingClaim::PendingRetry,
        })
    }

    /// Record the transfer's hash once it has broadcast.
    pub async fn record_tx_hash(
        &self,
        account: AccountId,
        payment_id: Uuid,
        tx_hash: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE onboarding_demo_payments SET tx_hash = $3 WHERE account_id = $1 AND payment_id = $2",
        )
        .bind(account.0)
        .bind(payment_id)
        .bind(tx_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
