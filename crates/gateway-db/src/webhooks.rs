use crate::AccountId;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use sqlx::types::chrono::{DateTime, Utc};
use uuid::Uuid;

pub const MAX_ATTEMPTS: i32 = 12;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WebhookEndpoint {
    pub id: Uuid,
    pub account_id: Uuid,
    pub url: String,
    pub secret_ciphertext: Vec<u8>,
    pub secret_cipher_version: i16,
    pub secret_key_id: String,
    pub created_at: DateTime<Utc>,
    pub disabled_at: Option<DateTime<Utc>>,
}
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WebhookEvent {
    pub id: Uuid,
    pub event_type: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WebhookDelivery {
    pub id: Uuid,
    pub event_id: Uuid,
    pub endpoint_id: Uuid,
    pub attempt_count: i32,
    pub next_attempt_at: DateTime<Utc>,
    pub state: String,
    pub delivered_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WebhookAttempt {
    pub delivery_id: Uuid,
    pub attempt_number: i32,
    pub attempted_at: DateTime<Utc>,
    pub duration_ms: i64,
    pub status: Option<i32>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeliveryClaim {
    pub delivery_id: Uuid,
    pub attempt_number: i32,
    pub lease_token: Uuid,
    pub url: String,
    pub secret_ciphertext: Vec<u8>,
    pub secret_cipher_version: i16,
    pub event_id: Uuid,
    pub event_type: String,
    pub payload: Value,
}

#[derive(Clone)]
pub struct WebhookRepository {
    pool: PgPool,
}
impl WebhookRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub async fn add(
        &self,
        account: AccountId,
        url: &str,
        secret: &[u8],
        ciphertext: &[u8],
    ) -> Result<WebhookEndpoint, sqlx::Error> {
        let hash: [u8; 32] = Sha256::digest(secret).into();
        sqlx::query_as("INSERT INTO webhook_endpoints(id,account_id,url,secret_hash,secret_ciphertext) VALUES($1,$2,$3,$4,$5) RETURNING id,account_id,url,secret_ciphertext,secret_cipher_version,secret_key_id,created_at,disabled_at")
            .bind(Uuid::now_v7()).bind(account.0).bind(url).bind(hash.as_slice()).bind(ciphertext).fetch_one(&self.pool).await
    }
    pub async fn list(&self, account: AccountId) -> Result<Vec<WebhookEndpoint>, sqlx::Error> {
        sqlx::query_as("SELECT id,account_id,url,secret_ciphertext,secret_cipher_version,secret_key_id,created_at,disabled_at FROM webhook_endpoints WHERE account_id=$1 AND disabled_at IS NULL ORDER BY id")
            .bind(account.0).fetch_all(&self.pool).await
    }
    pub async fn get(
        &self,
        account: AccountId,
        id: Uuid,
    ) -> Result<Option<WebhookEndpoint>, sqlx::Error> {
        sqlx::query_as("SELECT id,account_id,url,secret_ciphertext,secret_cipher_version,secret_key_id,created_at,disabled_at FROM webhook_endpoints WHERE account_id=$1 AND id=$2")
            .bind(account.0).bind(id).fetch_optional(&self.pool).await
    }
    pub async fn disable(&self, account: AccountId, id: Uuid) -> Result<bool, sqlx::Error> {
        Ok(sqlx::query("UPDATE webhook_endpoints SET disabled_at=COALESCE(disabled_at,now()) WHERE account_id=$1 AND id=$2").bind(account.0).bind(id).execute(&self.pool).await?.rows_affected()==1)
    }
    pub async fn test_event(
        &self,
        account: AccountId,
        endpoint: Uuid,
    ) -> Result<Option<Uuid>, sqlx::Error> {
        let event = Uuid::now_v7();
        let delivery = Uuid::now_v7();
        let mut tx = self.pool.begin().await?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM webhook_endpoints WHERE account_id=$1 AND id=$2 AND disabled_at IS NULL)").bind(account.0).bind(endpoint).fetch_one(&mut *tx).await?;
        if !exists {
            return Ok(None);
        }
        let payload = serde_json::json!({"version":"2026-08-01","id":gateway_core::WebhookEventId(event).to_string(),"type":"webhook.test","occurred_at":gateway_core::rfc3339(Utc::now()),"data":{"test":true}});
        sqlx::query("INSERT INTO webhook_events(id,account_id,event_type,payload,fanout) VALUES($1,$2,'webhook.test',$3,false)").bind(event).bind(account.0).bind(payload).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO webhook_deliveries(id,event_id,endpoint_id) VALUES($1,$2,$3)")
            .bind(delivery)
            .bind(event)
            .bind(endpoint)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(delivery))
    }
    /// One delivery the account owns, for cursor validation and reads.
    pub async fn delivery(
        &self,
        account: AccountId,
        id: Uuid,
    ) -> Result<Option<WebhookDelivery>, sqlx::Error> {
        sqlx::query_as("SELECT d.id,d.event_id,d.endpoint_id,d.attempt_count,d.next_attempt_at,d.state,d.delivered_at,d.created_at FROM webhook_deliveries d JOIN webhook_endpoints e ON e.id=d.endpoint_id WHERE e.account_id=$1 AND d.id=$2")
            .bind(account.0).bind(id).fetch_optional(&self.pool).await
    }
    /// Newest first, keyed on the UUIDv7 id so the cursor is the id of the
    /// last row shown. Returns up to `limit + 1` rows so the caller can tell
    /// whether a next page exists. `endpoint` narrows to one endpoint's
    /// deliveries, disabled or not: history outlives the endpoint.
    pub async fn deliveries(
        &self,
        account: AccountId,
        endpoint: Option<Uuid>,
        starting_after: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<WebhookDelivery>, sqlx::Error> {
        sqlx::query_as("SELECT d.id,d.event_id,d.endpoint_id,d.attempt_count,d.next_attempt_at,d.state,d.delivered_at,d.created_at FROM webhook_deliveries d JOIN webhook_endpoints e ON e.id=d.endpoint_id WHERE e.account_id=$1 AND ($2::uuid IS NULL OR d.endpoint_id=$2) AND ($3::uuid IS NULL OR d.id<$3) ORDER BY d.id DESC LIMIT $4")
            .bind(account.0).bind(endpoint).bind(starting_after).bind(limit + 1).fetch_all(&self.pool).await
    }
    pub async fn attempts(&self, delivery: Uuid) -> Result<Vec<WebhookAttempt>, sqlx::Error> {
        sqlx::query_as("SELECT delivery_id,attempt_number,attempted_at,duration_ms,status,error FROM webhook_delivery_attempts WHERE delivery_id=$1 ORDER BY attempt_number").bind(delivery).fetch_all(&self.pool).await
    }
    /// Every attempt of every listed delivery in one query, ordered by
    /// delivery then attempt number, so a page of deliveries is two queries
    /// rather than one per row.
    pub async fn attempts_for(
        &self,
        deliveries: &[Uuid],
    ) -> Result<Vec<WebhookAttempt>, sqlx::Error> {
        sqlx::query_as("SELECT delivery_id,attempt_number,attempted_at,duration_ms,status,error FROM webhook_delivery_attempts WHERE delivery_id=ANY($1) ORDER BY delivery_id,attempt_number").bind(deliveries).fetch_all(&self.pool).await
    }
    pub async fn claim(&self) -> Result<Option<DeliveryClaim>, sqlx::Error> {
        let lease_token = Uuid::now_v7();
        sqlx::query_as("WITH q AS (SELECT d.id FROM webhook_deliveries d JOIN webhook_endpoints ep ON ep.id=d.endpoint_id WHERE d.state='pending' AND d.next_attempt_at<=now() AND (d.lease_until IS NULL OR d.lease_until<now()) AND ep.disabled_at IS NULL ORDER BY d.next_attempt_at FOR UPDATE SKIP LOCKED LIMIT 1) UPDATE webhook_deliveries d SET attempt_count=attempt_count+1,lease_until=now()+interval '60 seconds',lease_token=$1 FROM q,webhook_endpoints ep,webhook_events ev WHERE d.id=q.id AND ep.id=d.endpoint_id AND ev.id=d.event_id RETURNING d.id delivery_id,d.attempt_count attempt_number,d.lease_token,ep.url,ep.secret_ciphertext,ep.secret_cipher_version,ev.id event_id,ev.event_type,ev.payload")
            .bind(lease_token)
            .fetch_optional(&self.pool)
            .await
    }
    pub async fn complete(
        &self,
        claim: &DeliveryClaim,
        attempted_at: DateTime<Utc>,
        duration_ms: i64,
        status: Option<u16>,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let success = status.is_some_and(|s| (200..300).contains(&s));
        let terminal = !success && claim.attempt_number >= MAX_ATTEMPTS;
        sqlx::query("INSERT INTO webhook_delivery_attempts(delivery_id,attempt_number,attempted_at,duration_ms,status,error) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING").bind(claim.delivery_id).bind(claim.attempt_number).bind(attempted_at).bind(duration_ms).bind(status.map(i32::from)).bind(error.map(|x|&x[..x.len().min(500)])).execute(&mut *tx).await?;
        sqlx::query("UPDATE webhook_deliveries SET state=CASE WHEN $3 THEN 'delivered' WHEN $4 THEN 'failed' ELSE 'pending' END,delivered_at=CASE WHEN $3 THEN now() ELSE delivered_at END,next_attempt_at=CASE WHEN NOT $3 AND NOT $4 THEN now()+make_interval(secs=>LEAST(3600,pow(2,attempt_count)::int)) ELSE next_attempt_at END,lease_until=NULL,lease_token=NULL WHERE id=$1 AND lease_token=$2 AND state='pending'")
            .bind(claim.delivery_id).bind(claim.lease_token).bind(success).bind(terminal).execute(&mut *tx).await?;
        tx.commit().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn account(pool: &PgPool) -> AccountId {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO accounts(id,api_key_hash,api_key_hint) VALUES($1,$2,'test')")
            .bind(id)
            .bind([0_u8; 32].as_slice())
            .execute(pool)
            .await
            .unwrap();
        AccountId(id)
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn test_event_has_one_target_and_no_invoice(pool: PgPool) {
        let account = account(&pool).await;
        let repo = WebhookRepository::new(pool.clone());
        let first = repo
            .add(account, "https://one.example/hook", b"one", &[1; 16])
            .await
            .unwrap();
        repo.add(account, "https://two.example/hook", b"two", &[2; 16])
            .await
            .unwrap();

        let delivery = repo.test_event(account, first.id).await.unwrap().unwrap();
        let targets: Vec<(Uuid,)> = sqlx::query_as("SELECT endpoint_id FROM webhook_deliveries WHERE event_id=(SELECT event_id FROM webhook_deliveries WHERE id=$1)")
            .bind(delivery).fetch_all(&pool).await.unwrap();
        assert_eq!(targets, vec![(first.id,)]);
        let invoice: Option<Uuid> = sqlx::query_scalar("SELECT invoice_id FROM webhook_events WHERE id=(SELECT event_id FROM webhook_deliveries WHERE id=$1)")
            .bind(delivery).fetch_one(&pool).await.unwrap();
        assert!(invoice.is_none());
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn completion_appends_immutable_attempt_history(pool: PgPool) {
        let account = account(&pool).await;
        let repo = WebhookRepository::new(pool.clone());
        let endpoint = repo
            .add(account, "https://one.example/hook", b"one", &[1; 16])
            .await
            .unwrap();
        let delivery = repo
            .test_event(account, endpoint.id)
            .await
            .unwrap()
            .unwrap();
        let claim = repo.claim().await.unwrap().unwrap();
        assert_eq!(claim.delivery_id, delivery);
        repo.complete(&claim, Utc::now(), 42, Some(503), Some("unavailable"))
            .await
            .unwrap();
        let attempts = repo.attempts(delivery).await.unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].status, Some(503));
        assert_eq!(attempts[0].duration_ms, 42);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn expired_claim_cannot_overwrite_its_replacement(pool: PgPool) {
        let account = account(&pool).await;
        let repo = WebhookRepository::new(pool.clone());
        let endpoint = repo
            .add(account, "https://one.example/hook", b"one", &[1; 16])
            .await
            .unwrap();

        for stale_result in [Some(204), Some(503)] {
            let delivery = repo
                .test_event(account, endpoint.id)
                .await
                .unwrap()
                .unwrap();
            let stale = repo.claim().await.unwrap().unwrap();
            sqlx::query(
                "UPDATE webhook_deliveries SET lease_until=now()-interval '1 second' WHERE id=$1",
            )
            .bind(delivery)
            .execute(&pool)
            .await
            .unwrap();
            let current = repo.claim().await.unwrap().unwrap();
            assert_ne!(stale.lease_token, current.lease_token);
            assert_eq!(current.attempt_number, stale.attempt_number + 1);

            if stale_result == Some(204) {
                repo.complete(&stale, Utc::now(), 1, stale_result, None)
                    .await
                    .unwrap();
                repo.complete(&current, Utc::now(), 1, Some(503), Some("retry"))
                    .await
                    .unwrap();
                let state: String =
                    sqlx::query_scalar("SELECT state FROM webhook_deliveries WHERE id=$1")
                        .bind(delivery)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                assert_eq!(state, "pending");
            } else {
                repo.complete(&current, Utc::now(), 1, Some(204), None)
                    .await
                    .unwrap();
                repo.complete(&stale, Utc::now(), 1, stale_result, Some("stale"))
                    .await
                    .unwrap();
                let state: String =
                    sqlx::query_scalar("SELECT state FROM webhook_deliveries WHERE id=$1")
                        .bind(delivery)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                assert_eq!(state, "delivered");
            }
            assert_eq!(repo.attempts(delivery).await.unwrap().len(), 2);
        }
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn disabled_endpoint_url_can_be_added_again(pool: PgPool) {
        let account = account(&pool).await;
        let repo = WebhookRepository::new(pool);
        let first = repo
            .add(account, "https://one.example/hook", b"one", &[1; 16])
            .await
            .unwrap();
        assert!(repo.disable(account, first.id).await.unwrap());
        let second = repo
            .add(account, "https://one.example/hook", b"two", &[2; 16])
            .await
            .unwrap();
        assert_ne!(first.id, second.id);
    }
}
