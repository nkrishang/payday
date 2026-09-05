use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

/// Previous keys remain usable for 24 hours after rotation. This conservative,
/// fixed window gives deployments time to switch credentials without leaving a
/// long-lived second credential.
pub const API_KEY_GRACE_HOURS: i64 = 24;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountId(pub Uuid);

#[derive(Debug, sqlx::FromRow)]
pub struct ApiKeyMetadata {
    pub account_id: Uuid,
    pub hint: Option<String>,
    pub generation: i64,
    pub created_at: sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,
    pub rotated_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    pub previous_key_expires_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    pub revoked_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    /// The mailbox the account signs in with, once one has been proven.
    pub email: Option<String>,
    /// The account's own EVM wallet — the embedded wallet its identity
    /// provider created — checksummed; `None` until a session has carried one.
    pub wallet_address: Option<String>,
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
    #[error("account is disabled")]
    AccountDisabled,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Why an identity could not be mapped to an account.
#[derive(Debug, thiserror::Error)]
pub enum ProvisionAccountError {
    #[error("account is disabled")]
    AccountDisabled,
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
            r#"SELECT id FROM accounts
               WHERE disabled_at IS NULL AND (
                   api_key_hash = $1 OR
                   (previous_api_key_hash = $1 AND previous_api_key_expires_at > now())
               )"#,
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
        self.issue_api_key_inner(
            issuer,
            subject,
            expected_generation,
            authentication_event_id,
            key,
            None,
        )
        .await
    }

    pub async fn issue_api_key_with_email(
        &self,
        issuer: &str,
        subject: &str,
        expected_generation: Option<i64>,
        authentication_event_id: &str,
        key: &str,
        verified_email: &str,
    ) -> Result<IssuedApiKey, IssueApiKeyError> {
        self.issue_api_key_inner(
            issuer,
            subject,
            expected_generation,
            authentication_event_id,
            key,
            Some(verified_email),
        )
        .await
    }

    async fn issue_api_key_inner(
        &self,
        issuer: &str,
        subject: &str,
        expected_generation: Option<i64>,
        authentication_event_id: &str,
        key: &str,
        verified_email: Option<&str>,
    ) -> Result<IssuedApiKey, IssueApiKeyError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1), hashtext($2))")
            .bind(issuer)
            .bind(subject)
            .execute(&mut *tx)
            .await?;
        if let Some((account_id, generation, disabled)) = sqlx::query_as::<_, (Uuid, i64, bool)>(
            r#"SELECT account_id, api_key_generation, disabled_at IS NOT NULL
               FROM account_identities
               JOIN accounts ON accounts.id = account_identities.account_id
               WHERE issuer = $1 AND subject = $2"#,
        )
        .bind(issuer)
        .bind(subject)
        .fetch_optional(&mut *tx)
        .await?
        {
            if disabled {
                return Err(IssueApiKeyError::AccountDisabled);
            }
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
            // A dashboard-provisioned identity already has this row, keyless,
            // before its first key: `id = $3` still matches and takes this
            // branch, so "the row already existed" cannot stand in for
            // "a key already existed". Only a real previous key gets a grace
            // window, rotated_at, or an honest `replaced_previous_key`; the
            // `CASE` already tells the two apart, so read it back rather than
            // asserting `true` unconditionally.
            let (generation, replaced_previous_key) = sqlx::query_as::<_, (i64, bool)>(
                r#"UPDATE accounts
                   SET previous_api_key_hash = api_key_hash,
                       previous_api_key_expires_at = CASE WHEN api_key_hash IS NULL THEN NULL
                           ELSE now() + make_interval(hours => $4) END,
                       api_key_hash = $1, api_key_hint = $2, key_created_at = now(),
                       api_key_generation = api_key_generation + 1,
                       key_rotated_at = CASE WHEN api_key_hash IS NULL THEN key_rotated_at ELSE now() END,
                       key_revoked_at = NULL, email = COALESCE($5, email)
                   WHERE id = $3
                   RETURNING api_key_generation, previous_api_key_expires_at IS NOT NULL"#,
            )
            .bind(hash.as_slice())
            .bind(key_hint(key))
            .bind(account_id)
            .bind(API_KEY_GRACE_HOURS as i32)
            .bind(verified_email)
            .fetch_one(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(IssuedApiKey {
                account_id: AccountId(account_id),
                generation,
                replaced_previous_key,
            });
        }

        if expected_generation.is_some() {
            return Err(IssueApiKeyError::GenerationConflict);
        }
        let id = Uuid::now_v7();
        let hash: [u8; 32] = Sha256::digest(key.as_bytes()).into();
        sqlx::query(
            "INSERT INTO accounts (id, api_key_hash, api_key_hint, email) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(hash.as_slice())
        .bind(key_hint(key))
        .bind(verified_email)
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

    /// The account behind a verified identity, created on first sight. A
    /// dashboard session never holds an API key, so the account starts with
    /// none; the key routes can add one later through the usual rotation.
    ///
    /// `wallet_address` is the wallet the identity provider currently reports
    /// for this identity. It is recorded on creation and kept current on every
    /// later sight, so an embedded wallet that was still being created at the
    /// very first sign-in is picked up as soon as a session carries it.
    pub async fn find_or_provision_by_identity(
        &self,
        issuer: &str,
        subject: &str,
        verified_email: &str,
        wallet_address: Option<&str>,
    ) -> Result<AccountId, ProvisionAccountError> {
        if let Some(existing) = self.identity_account(issuer, subject, &self.pool).await? {
            let account = existing?;
            self.adopt_wallet(account, wallet_address).await?;
            return Ok(account);
        }
        // Two first sign-ins for one identity race here; the same lock the
        // key issuance takes serializes them so exactly one account appears.
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1), hashtext($2))")
            .bind(issuer)
            .bind(subject)
            .execute(&mut *tx)
            .await?;
        if let Some(existing) = self.identity_account(issuer, subject, &mut *tx).await? {
            let account = existing?;
            tx.commit().await?;
            self.adopt_wallet(account, wallet_address).await?;
            return Ok(account);
        }
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO accounts (id, email, wallet_address) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(verified_email)
            .bind(wallet_address)
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
        tx.commit().await?;
        Ok(AccountId(id))
    }

    /// Records the wallet a session reports when it differs from what is
    /// stored. A session that carries none leaves the stored one alone: the
    /// provider not mentioning a wallet is not the same as the wallet going
    /// away, and the row must never lose the address deposits settle to.
    async fn adopt_wallet(
        &self,
        account: AccountId,
        wallet_address: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let Some(wallet_address) = wallet_address else {
            return Ok(());
        };
        sqlx::query(
            "UPDATE accounts SET wallet_address = $1 WHERE id = $2 AND wallet_address IS DISTINCT FROM $1",
        )
        .bind(wallet_address)
        .bind(account.0)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn identity_account<'e, E: sqlx::PgExecutor<'e>>(
        &self,
        issuer: &str,
        subject: &str,
        executor: E,
    ) -> Result<Option<Result<AccountId, ProvisionAccountError>>, sqlx::Error> {
        let found = sqlx::query_as::<_, (Uuid, bool)>(
            r#"SELECT account_id, disabled_at IS NOT NULL
               FROM account_identities
               JOIN accounts ON accounts.id = account_identities.account_id
               WHERE issuer = $1 AND subject = $2"#,
        )
        .bind(issuer)
        .bind(subject)
        .fetch_optional(executor)
        .await?;
        Ok(found.map(|(id, disabled)| {
            if disabled {
                Err(ProvisionAccountError::AccountDisabled)
            } else {
                Ok(AccountId(id))
            }
        }))
    }

    pub async fn metadata(&self, account: AccountId) -> Result<ApiKeyMetadata, sqlx::Error> {
        sqlx::query_as(
            "SELECT id AS account_id, api_key_hint AS hint, api_key_generation AS generation, key_created_at AS created_at, key_rotated_at AS rotated_at, previous_api_key_expires_at AS previous_key_expires_at, key_revoked_at AS revoked_at, email, wallet_address FROM accounts WHERE id = $1",
        )
        .bind(account.0)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn has_verified_email(&self, account: AccountId) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar("SELECT email IS NOT NULL FROM accounts WHERE id = $1")
            .bind(account.0)
            .fetch_one(&self.pool)
            .await
    }

    pub async fn revoke_api_key(
        &self,
        issuer: &str,
        subject: &str,
        expected_generation: i64,
        authentication_event_id: &str,
    ) -> Result<AccountId, IssueApiKeyError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1), hashtext($2))")
            .bind(issuer)
            .bind(subject)
            .execute(&mut *tx)
            .await?;
        let row = sqlx::query_as::<_, (Uuid, i64, bool)>(
            r#"SELECT account_id, api_key_generation, disabled_at IS NOT NULL
               FROM account_identities JOIN accounts ON accounts.id = account_id
               WHERE issuer = $1 AND subject = $2"#,
        )
        .bind(issuer)
        .bind(subject)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((account_id, generation, disabled)) = row else {
            return Err(IssueApiKeyError::GenerationConflict);
        };
        if disabled {
            return Err(IssueApiKeyError::AccountDisabled);
        }
        if generation != expected_generation {
            return Err(IssueApiKeyError::GenerationConflict);
        }
        let inserted = sqlx::query("INSERT INTO account_key_authentication_events (account_id, event_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(account_id).bind(authentication_event_id).execute(&mut *tx).await?;
        if inserted.rows_affected() == 0 {
            return Err(IssueApiKeyError::AuthenticationEventAlreadyUsed);
        }
        sqlx::query("UPDATE accounts SET api_key_hash = NULL, api_key_hint = NULL, previous_api_key_hash = NULL, previous_api_key_expires_at = NULL, key_revoked_at = now() WHERE id = $1")
            .bind(account_id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(AccountId(account_id))
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
    async fn rotation_keeps_the_previous_key_until_grace_expires(pool: PgPool) {
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
        assert_eq!(
            repo.authenticate(FIRST_KEY).await.unwrap(),
            Some(first.account_id)
        );
        assert_eq!(
            repo.authenticate(SECOND_KEY).await.unwrap(),
            Some(first.account_id)
        );
        sqlx::query(
            "UPDATE accounts SET previous_api_key_expires_at = now() - interval '1 second' WHERE id = $1",
        )
        .bind(first.account_id.0)
        .execute(&repo.pool)
        .await
        .unwrap();
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
        assert_eq!(
            repo.authenticate(FIRST_KEY).await.unwrap(),
            Some(first.account_id)
        );
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

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn revocation_immediately_invalidates_current_and_grace_keys(pool: PgPool) {
        let repo = AccountRepository::new(pool);
        let first = repo
            .issue_api_key("issuer", "email|one", None, "event-1", FIRST_KEY)
            .await
            .unwrap();
        repo.issue_api_key("issuer", "email|one", Some(1), "event-2", SECOND_KEY)
            .await
            .unwrap();
        repo.revoke_api_key("issuer", "email|one", 2, "event-3")
            .await
            .unwrap();
        assert_eq!(repo.authenticate(FIRST_KEY).await.unwrap(), None);
        assert_eq!(repo.authenticate(SECOND_KEY).await.unwrap(), None);
        let metadata = repo.metadata(first.account_id).await.unwrap();
        assert!(metadata.revoked_at.is_some());
        assert!(metadata.previous_key_expires_at.is_none());
    }

    const WALLET: &str = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
    const OTHER_WALLET: &str = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC";

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn identity_is_provisioned_once_without_a_key_and_can_take_one_later(pool: PgPool) {
        let repo = AccountRepository::new(pool.clone());
        // The very first sign-in may arrive before the provider has finished
        // creating the wallet; the account exists without one.
        let first = repo
            .find_or_provision_by_identity("issuer", "email|one", "one@example.com", None)
            .await
            .unwrap();
        assert!(repo.metadata(first).await.unwrap().wallet_address.is_none());
        let again = repo
            .find_or_provision_by_identity(
                "issuer",
                "email|one",
                "changed@example.com",
                Some(WALLET),
            )
            .await
            .unwrap();
        assert_eq!(first, again, "the identity maps to one account");
        assert_eq!(
            repo.find_by_identity("issuer", "email|one").await.unwrap(),
            Some(first)
        );
        let metadata = repo.metadata(first).await.unwrap();
        assert!(
            metadata.hint.is_none(),
            "a dashboard account starts keyless"
        );
        assert_eq!(metadata.generation, 1);
        assert_eq!(
            metadata.wallet_address.as_deref(),
            Some(WALLET),
            "the wallet is adopted as soon as a session carries it"
        );
        assert!(repo.has_verified_email(first).await.unwrap());
        assert_eq!(
            metadata.email.as_deref(),
            Some("one@example.com"),
            "the first verified email sticks"
        );

        // A later session naming no wallet does not forget the one stored; a
        // later session naming another one moves it.
        repo.find_or_provision_by_identity("issuer", "email|one", "one@example.com", None)
            .await
            .unwrap();
        assert_eq!(
            repo.metadata(first)
                .await
                .unwrap()
                .wallet_address
                .as_deref(),
            Some(WALLET)
        );
        repo.find_or_provision_by_identity(
            "issuer",
            "email|one",
            "one@example.com",
            Some(OTHER_WALLET),
        )
        .await
        .unwrap();
        assert_eq!(
            repo.metadata(first)
                .await
                .unwrap()
                .wallet_address
                .as_deref(),
            Some(OTHER_WALLET)
        );

        // A first issue runs from the current generation, as for any
        // existing identity — but this is a first key, not a rotation, since
        // a dashboard-provisioned identity starts keyless.
        let issued = repo
            .issue_api_key("issuer", "email|one", Some(1), "event-1", FIRST_KEY)
            .await
            .unwrap();
        assert_eq!(issued.account_id, first);
        assert_eq!(issued.generation, 2);
        assert!(!issued.replaced_previous_key, "nothing existed to replace");
        assert_eq!(repo.authenticate(FIRST_KEY).await.unwrap(), Some(first));
        let metadata = repo.metadata(first).await.unwrap();
        assert!(
            metadata.previous_key_expires_at.is_none(),
            "no previous key means no grace window"
        );
        assert!(metadata.rotated_at.is_none(), "issued, not rotated");

        // An existing keyed identity is found, never duplicated.
        let keyed = repo
            .issue_api_key("issuer", "email|two", None, "event-2", SECOND_KEY)
            .await
            .unwrap();
        assert_eq!(
            repo.find_or_provision_by_identity("issuer", "email|two", "two@example.com", None)
                .await
                .unwrap(),
            keyed.account_id
        );

        sqlx::query("UPDATE accounts SET disabled_at = now() WHERE id = $1")
            .bind(first.0)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            repo.find_or_provision_by_identity(
                "issuer",
                "email|one",
                "one@example.com",
                Some(WALLET)
            )
            .await,
            Err(ProvisionAccountError::AccountDisabled)
        ));
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn disabled_accounts_cannot_authenticate_rotate_or_revoke(pool: PgPool) {
        let repo = AccountRepository::new(pool.clone());
        let account = repo
            .issue_api_key("issuer", "email|one", None, "event-1", FIRST_KEY)
            .await
            .unwrap();
        sqlx::query("UPDATE accounts SET disabled_at = now() WHERE id = $1")
            .bind(account.account_id.0)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(repo.authenticate(FIRST_KEY).await.unwrap(), None);
        assert!(matches!(
            repo.issue_api_key("issuer", "email|one", Some(1), "event-2", SECOND_KEY)
                .await,
            Err(IssueApiKeyError::AccountDisabled)
        ));
        assert!(matches!(
            repo.revoke_api_key("issuer", "email|one", 1, "event-3")
                .await,
            Err(IssueApiKeyError::AccountDisabled)
        ));
    }
}
