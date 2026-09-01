use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct NotificationEvent {
    pub id: Uuid,
    pub invoice_id: Uuid,
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

    pub async fn claim(&self) -> Result<Option<NotificationEvent>, sqlx::Error> {
        sqlx::query_as(
            r#"UPDATE notification_outbox SET
                   leased_until = now() + interval '60 seconds', attempts = attempts + 1
               WHERE id = (
                   SELECT id FROM notification_outbox
                   WHERE next_attempt_at <= now()
                     AND (leased_until IS NULL OR leased_until < now())
                     AND ((email IS NOT NULL AND delivered_at IS NULL)
                       OR (email IS NULL AND missing_email_reported_at IS NULL))
                   ORDER BY next_attempt_at FOR UPDATE SKIP LOCKED LIMIT 1)
               RETURNING id, invoice_id, reason, email, attempts"#,
        )
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

    pub async fn retry(&self, id: Uuid, attempts: i32, error: &str) -> Result<(), sqlx::Error> {
        let delay = 2_i64.pow(attempts.clamp(1, 10) as u32).min(900);
        sqlx::query("UPDATE notification_outbox SET leased_until=NULL,next_attempt_at=now()+make_interval(secs=>$2),last_error=$3 WHERE id=$1")
            .bind(id).bind(delay as f64).bind(error).execute(&self.pool).await.map(drop)
    }
}
