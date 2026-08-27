use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountId(pub Uuid);

#[derive(Debug, sqlx::FromRow)]
pub struct ApiKeyMetadata {
    pub hint: String,
    pub generation: i64,
    pub created_at: sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,
    pub rotated_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct IssuedApiKey {
    pub account_id: AccountId,
    pub generation: i64,
    pub replaced_previous_key: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum IssueApiKeyError {
    #[error("authentication event was already used")]
    AuthenticationEventAlreadyUsed,
    #[error("API key generation changed")]
    GenerationConflict,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Clone)]
pub struct AccountRepository {
    pool: PgPool,
}

impl AccountRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn authenticate(&self, key: &str) -> Result<Option<AccountId>, sqlx::Error> {
        let hash: [u8; 32] = Sha256::digest(key.as_bytes()).into();
        sqlx::query_scalar(
            "SELECT id FROM accounts WHERE api_key_hash = $1 AND disabled_at IS NULL",
        )
        .bind(hash.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map(|id| id.map(AccountId))
    }

    pub async fn issue_api_key(
        &self,
        issuer: &str,
        subject: &str,
        expected_generation: Option<i64>,
        authentication_event_id: &str,
        key: &str,
    ) -> Result<IssuedApiKey, IssueApiKeyError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1), hashtext($2))")
            .bind(issuer)
            .bind(subject)
            .execute(&mut *tx)
            .await?;
        if let Some((account_id, generation)) = sqlx::query_as::<_, (Uuid, i64)>(
            r#"SELECT account_id, api_key_generation
               FROM account_identities
               JOIN accounts ON accounts.id = account_identities.account_id
               WHERE issuer = $1 AND subject = $2"#,
        )
        .bind(issuer)
        .bind(subject)
        .fetch_optional(&mut *tx)
        .await?
        {
            if expected_generation != Some(generation) {
                return Err(IssueApiKeyError::GenerationConflict);
            }
            let inserted = sqlx::query(
                "INSERT INTO account_key_authentication_events (account_id, event_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            )
            .bind(account_id)
            .bind(authentication_event_id)
            .execute(&mut *tx)
            .await?;
            if inserted.rows_affected() == 0 {
                return Err(IssueApiKeyError::AuthenticationEventAlreadyUsed);
            }

            let hash: [u8; 32] = Sha256::digest(key.as_bytes()).into();
            let generation = sqlx::query_scalar(
                r#"UPDATE accounts
                   SET api_key_hash = $1, api_key_hint = $2,
                       api_key_generation = api_key_generation + 1, key_rotated_at = now()
                   WHERE id = $3 AND disabled_at IS NULL
                   RETURNING api_key_generation"#,
            )
            .bind(hash.as_slice())
            .bind(key_hint(key))
            .bind(account_id)
            .fetch_one(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(IssuedApiKey {
                account_id: AccountId(account_id),
                generation,
                replaced_previous_key: true,
            });
        }

        if expected_generation.is_some() {
            return Err(IssueApiKeyError::GenerationConflict);
        }
        let id = Uuid::now_v7();
        let hash: [u8; 32] = Sha256::digest(key.as_bytes()).into();
        sqlx::query("INSERT INTO accounts (id, api_key_hash, api_key_hint) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(hash.as_slice())
            .bind(key_hint(key))
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO account_identities (issuer, subject, account_id) VALUES ($1, $2, $3)",
        )
        .bind(issuer)
        .bind(subject)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO account_key_authentication_events (account_id, event_id) VALUES ($1, $2)",
        )
        .bind(id)
        .bind(authentication_event_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(IssuedApiKey {
            account_id: AccountId(id),
            generation: 1,
            replaced_previous_key: false,
        })
    }

    pub async fn find_by_identity(
        &self,
        issuer: &str,
        subject: &str,
    ) -> Result<Option<AccountId>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT account_id FROM account_identities WHERE issuer = $1 AND subject = $2",
        )
        .bind(issuer)
        .bind(subject)
        .fetch_optional(&self.pool)
        .await
        .map(|id| id.map(AccountId))
    }

    pub async fn metadata(&self, account: AccountId) -> Result<ApiKeyMetadata, sqlx::Error> {
        sqlx::query_as(
            "SELECT api_key_hint AS hint, api_key_generation AS generation, key_created_at AS created_at, key_rotated_at AS rotated_at FROM accounts WHERE id = $1",
        )
        .bind(account.0)
        .fetch_one(&self.pool)
        .await
    }
}

fn key_hint(key: &str) -> String {
    let suffix = key
        .chars()
        .rev()
        .take(6)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("…{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST_KEY: &str = "first-key-0123456789abcdef0123456789abcdef";
    const SECOND_KEY: &str = "second-key-0123456789abcdef0123456789abcdef";
    const THIRD_KEY: &str = "third-key-0123456789abcdef0123456789abcdef";
    const FOURTH_KEY: &str = "fourth-key-0123456789abcdef0123456789abcdef";

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn reissue_replaces_the_only_key_for_an_identity(pool: PgPool) {
        let repo = AccountRepository::new(pool);
        let first = repo
            .issue_api_key(
                "https://issuer.example/",
                "email|one",
                None,
                "event-1",
                FIRST_KEY,
            )
            .await
            .unwrap();
        assert!(!first.replaced_previous_key);
        assert_eq!(
            repo.authenticate(FIRST_KEY).await.unwrap(),
            Some(first.account_id)
        );

        let second = repo
            .issue_api_key(
                "https://issuer.example/",
                "email|one",
                Some(1),
                "event-2",
                SECOND_KEY,
            )
            .await
            .unwrap();
        assert!(second.replaced_previous_key);
        assert_eq!(second.account_id, first.account_id);
        assert_eq!(second.generation, 2);
        assert_eq!(repo.authenticate(FIRST_KEY).await.unwrap(), None);
        assert_eq!(
            repo.authenticate(SECOND_KEY).await.unwrap(),
            Some(first.account_id)
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn authentication_event_can_only_be_used_once(pool: PgPool) {
        let repo = AccountRepository::new(pool);
        let first = repo
            .issue_api_key(
                "https://issuer.example/",
                "email|one",
                None,
                "event-1",
                FIRST_KEY,
            )
            .await
            .unwrap();
        repo.issue_api_key(
            "https://issuer.example/",
            "email|one",
            Some(1),
            "event-2",
            SECOND_KEY,
        )
        .await
        .unwrap();
        let replay = repo
            .issue_api_key(
                "https://issuer.example/",
                "email|one",
                Some(2),
                "event-1",
                THIRD_KEY,
            )
            .await;
        assert!(matches!(
            replay,
            Err(IssueApiKeyError::AuthenticationEventAlreadyUsed)
        ));
        assert_eq!(repo.authenticate(FIRST_KEY).await.unwrap(), None);
        assert_eq!(
            repo.authenticate(SECOND_KEY).await.unwrap(),
            Some(first.account_id)
        );
        assert_eq!(repo.authenticate(THIRD_KEY).await.unwrap(), None);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn concurrent_create_only_requests_do_not_replace_the_winner(pool: PgPool) {
        let repo = AccountRepository::new(pool);
        let first = repo.issue_api_key(
            "https://issuer.example/",
            "email|one",
            None,
            "event-1",
            FIRST_KEY,
        );
        let second = repo.issue_api_key(
            "https://issuer.example/",
            "email|one",
            None,
            "event-2",
            SECOND_KEY,
        );
        let (first, second) = tokio::join!(first, second);
        let (winner, winner_key, loser_event, loser_key) = match (first, second) {
            (Ok(winner), Err(IssueApiKeyError::GenerationConflict)) => {
                (winner, FIRST_KEY, "event-2", SECOND_KEY)
            }
            (Err(IssueApiKeyError::GenerationConflict), Ok(winner)) => {
                (winner, SECOND_KEY, "event-1", FIRST_KEY)
            }
            results => panic!("expected one creation and one conflict, got {results:?}"),
        };
        assert_eq!(
            repo.authenticate(winner_key).await.unwrap(),
            Some(winner.account_id)
        );
        assert_eq!(repo.authenticate(loser_key).await.unwrap(), None);

        let retry = repo
            .issue_api_key(
                "https://issuer.example/",
                "email|one",
                Some(1),
                loser_event,
                loser_key,
            )
            .await
            .unwrap();
        assert_eq!(retry.generation, 2);
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn stale_replacement_does_not_change_key_or_consume_event(pool: PgPool) {
        let repo = AccountRepository::new(pool);
        let account = repo
            .issue_api_key(
                "https://issuer.example/",
                "email|one",
                None,
                "event-1",
                FIRST_KEY,
            )
            .await
            .unwrap();
        repo.issue_api_key(
            "https://issuer.example/",
            "email|one",
            Some(1),
            "event-2",
            SECOND_KEY,
        )
        .await
        .unwrap();

        let stale = repo
            .issue_api_key(
                "https://issuer.example/",
                "email|one",
                Some(1),
                "event-3",
                THIRD_KEY,
            )
            .await;
        assert!(matches!(stale, Err(IssueApiKeyError::GenerationConflict)));
        assert_eq!(
            repo.authenticate(SECOND_KEY).await.unwrap(),
            Some(account.account_id)
        );
        assert_eq!(repo.authenticate(THIRD_KEY).await.unwrap(), None);

        let retry = repo
            .issue_api_key(
                "https://issuer.example/",
                "email|one",
                Some(2),
                "event-3",
                FOURTH_KEY,
            )
            .await
            .unwrap();
        assert_eq!(retry.generation, 3);
    }
}
