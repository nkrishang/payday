//! The onboarding walkthrough's one real, self-funded deposit request: at
//! most one per account, ever (see migration 0016's comment for why the
//! table's shape is the enforcement).

use alloy_primitives::B256;
use gum_bus::Publisher;
use gum_contracts::{CorrelationId, ExecutionCommand, OnboardingPaymentCommand};
use sqlx::PgPool;
use uuid::Uuid;

use crate::AccountId;

/// What claiming the one-shot slot found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingClaim {
    /// Nobody had claimed this account's slot yet; proceed to verify and pay.
    Claimed(Uuid),
    /// This account already claimed the slot for this same payment, but no
    /// transfer was recorded yet (a crash between claiming and broadcasting,
    /// or a bare retry) — safe to attempt the transfer again.
    PendingRetry(Uuid),
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
    publisher: Publisher,
}

impl OnboardingDemoPaymentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            publisher: Publisher::new("gum-server"),
        }
    }

    /// Atomically claim the account's one-shot slot and publish its stable
    /// execution job. A crash leaves both committed or neither; retries
    /// publish the same job id and payload, which the bus deduplicates.
    #[allow(clippy::too_many_arguments)]
    pub async fn claim_and_publish(
        &self,
        account: AccountId,
        payment_id: Uuid,
        chain_id: u64,
        payer: alloy_primitives::Address,
        token: alloy_primitives::Address,
        recipient: alloy_primitives::Address,
        amount: alloy_primitives::U256,
    ) -> Result<OnboardingClaim, gum_bus::BusError> {
        let mut tx = self.pool.begin().await?;
        let job_id = Uuid::now_v7();
        let inserted = sqlx::query(
            r#"
            INSERT INTO onboarding_demo_payments (account_id, payment_id, job_id)
            VALUES ($1, $2, $3)
            ON CONFLICT (account_id) DO NOTHING
            "#,
        )
        .bind(account.0)
        .bind(payment_id)
        .bind(job_id)
        .execute(&mut *tx)
        .await?;
        let claimed = inserted.rows_affected() == 1;
        let (existing_payment_id, existing_job_id, tx_hash): (Uuid, Uuid, Option<String>) =
            sqlx::query_as(
                "SELECT payment_id, job_id, tx_hash FROM onboarding_demo_payments WHERE account_id = $1 FOR UPDATE",
            )
            .bind(account.0)
            .fetch_one(&mut *tx)
            .await?;
        if existing_payment_id != payment_id {
            return Ok(OnboardingClaim::Conflict);
        }
        if let Some(hash) = tx_hash {
            return Ok(OnboardingClaim::AlreadySubmitted(hash));
        }
        let command = OnboardingPaymentCommand {
            job_id: existing_job_id,
            chain_id,
            payer,
            token,
            recipient,
            amount,
        };
        self.publisher
            .publish(
                &mut tx,
                &ExecutionCommand::OnboardingPayment(command),
                CorrelationId::new(),
                None,
            )
            .await?;
        tx.commit().await?;
        Ok(if claimed {
            OnboardingClaim::Claimed(existing_job_id)
        } else {
            OnboardingClaim::PendingRetry(existing_job_id)
        })
    }

    pub async fn submitted_hash(&self, job_id: Uuid) -> Result<Option<B256>, sqlx::Error> {
        let value: Option<String> =
            sqlx::query_scalar("SELECT tx_hash FROM onboarding_demo_payments WHERE job_id = $1")
                .bind(job_id)
                .fetch_optional(&self.pool)
                .await?
                .flatten();
        value
            .map(|hash| {
                hash.parse()
                    .map_err(|error| sqlx::Error::Decode(Box::new(error)))
            })
            .transpose()
    }

    pub async fn record_submission(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        job_id: Uuid,
        tx_hash: B256,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE onboarding_demo_payments SET tx_hash = COALESCE(tx_hash, $2) WHERE job_id = $1",
        )
        .bind(job_id)
        .bind(tx_hash.to_string())
        .execute(&mut **tx)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{account, insert_bound, issuance_input};
    use alloy_primitives::{Address, U256};

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn retries_reuse_one_job_and_one_durable_command(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let invoice = insert_bound(&pool, &issuance_input(owner, "onboarding", None), None).await;
        let repo = OnboardingDemoPaymentRepository::new(pool.clone());
        let schedule = || {
            repo.claim_and_publish(
                owner,
                invoice.id,
                1,
                Address::repeat_byte(1),
                Address::repeat_byte(2),
                Address::repeat_byte(3),
                U256::from(1_000_000),
            )
        };

        let OnboardingClaim::Claimed(first) = schedule().await.unwrap() else {
            panic!()
        };
        let OnboardingClaim::PendingRetry(second) = schedule().await.unwrap() else {
            panic!()
        };
        assert_eq!(first, second);
        let messages: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM bus.messages WHERE topic = 'execution.commands' AND deduplication_key = $1",
        )
        .bind(first.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(messages, 1);
    }
}
