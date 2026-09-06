use sqlx::PgPool;
use uuid::Uuid;

/// Who an outbox row is addressed to. Each recipient has its own sender and
/// its own message: a merchant hears about a payout that needs attention, a
/// payer hears about the deposit request they were named on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationRecipient {
    Merchant,
    Payer,
}

impl NotificationRecipient {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merchant => "merchant",
            Self::Payer => "payer",
        }
    }
}

/// The one reason a payer row is written: the request was just issued.
pub const PAYER_DEPOSIT_REQUEST_ISSUED: &str = "deposit_request_issued";

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct NotificationEvent {
    pub id: Uuid,
    pub invoice_id: Uuid,
    pub recipient: String,
    pub reason: String,
    pub email: Option<String>,
    pub attempts: i32,
}

#[derive(Clone)]
pub struct NotificationRepository {
    pool: PgPool,
}

impl NotificationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Lease the next deliverable row addressed to one of `recipients`. A
    /// deployment without a sender for some recipient leaves those rows
    /// untouched rather than burning their attempts.
    pub async fn claim(
        &self,
        recipients: &[NotificationRecipient],
    ) -> Result<Option<NotificationEvent>, sqlx::Error> {
        let recipients: Vec<&str> = recipients.iter().map(|r| r.as_str()).collect();
        sqlx::query_as(
            r#"UPDATE notification_outbox SET
                   leased_until = now() + interval '60 seconds', attempts = attempts + 1
               WHERE id = (
                   SELECT id FROM notification_outbox
                   WHERE next_attempt_at <= now()
                     AND recipient = ANY($1)
                     AND abandoned_at IS NULL
                     AND (leased_until IS NULL OR leased_until < now())
                     AND ((email IS NOT NULL AND delivered_at IS NULL)
                       OR (email IS NULL AND missing_email_reported_at IS NULL))
                   ORDER BY next_attempt_at FOR UPDATE SKIP LOCKED LIMIT 1)
               RETURNING id, invoice_id, recipient, reason, email, attempts"#,
        )
        .bind(recipients)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn report_missing_email(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        sqlx::query(
            "UPDATE notification_outbox SET missing_email_reported_at=now(), leased_until=NULL WHERE id=$1 AND missing_email_reported_at IS NULL",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map(|result| result.rows_affected() == 1)
    }

    pub async fn complete(&self, id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE notification_outbox SET delivered_at=COALESCE(delivered_at,now()), leased_until=NULL, last_error=NULL WHERE id=$1",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map(drop)
    }

    /// Stop trying: the provider refused the message for a reason no retry
    /// can change. The row keeps the error for whoever asks why.
    pub async fn abandon(&self, id: Uuid, error: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE notification_outbox SET abandoned_at=COALESCE(abandoned_at,now()), leased_until=NULL, last_error=$2 WHERE id=$1",
        )
        .bind(id)
        .bind(error)
        .execute(&self.pool)
        .await
        .map(drop)
    }

    pub async fn retry(&self, id: Uuid, attempts: i32, error: &str) -> Result<(), sqlx::Error> {
        let delay = 2_i64.pow(attempts.clamp(1, 10) as u32).min(900);
        sqlx::query("UPDATE notification_outbox SET leased_until=NULL,next_attempt_at=now()+make_interval(secs=>$2),last_error=$3 WHERE id=$1")
            .bind(id).bind(delay as f64).bind(error).execute(&self.pool).await.map(drop)
    }
}
