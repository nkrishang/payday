//! Merchant-owned customer records (product plan §4.4). Issued invoices
//! snapshot the party they were billed to, so editing a customer never
//! changes an invoice.

use sqlx::PgPool;
use sqlx::types::chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::AccountId;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DbCustomer {
    pub id: Uuid,
    pub account_id: Uuid,
    pub name: String,
    pub email: Option<String>,
    pub details: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct CreateCustomerInput {
    pub id: Uuid,
    pub account_id: AccountId,
    pub name: String,
    pub email: Option<String>,
    pub details: Option<String>,
}

#[derive(Clone)]
pub struct CustomerRepository {
    pool: PgPool,
}

impl CustomerRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, input: &CreateCustomerInput) -> Result<DbCustomer, sqlx::Error> {
        sqlx::query_as::<_, DbCustomer>(
            r#"INSERT INTO customers (id, account_id, name, email, details)
               VALUES ($1, $2, $3, $4, $5)
               RETURNING *"#,
        )
        .bind(input.id)
        .bind(input.account_id.0)
        .bind(&input.name)
        .bind(&input.email)
        .bind(&input.details)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn get_for_account(
        &self,
        account: AccountId,
        id: Uuid,
    ) -> Result<Option<DbCustomer>, sqlx::Error> {
        sqlx::query_as::<_, DbCustomer>("SELECT * FROM customers WHERE account_id = $1 AND id = $2")
            .bind(account.0)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    /// An account's customers, newest first, plus one row beyond `limit` so
    /// the caller can tell whether another page exists (as invoice listing
    /// does). A `starting_after` cursor from another account matches nothing.
    pub async fn list_for_account(
        &self,
        account: AccountId,
        limit: u32,
        starting_after: Option<Uuid>,
    ) -> Result<Vec<DbCustomer>, sqlx::Error> {
        sqlx::query_as::<_, DbCustomer>(
            r#"SELECT candidate.* FROM customers candidate
               LEFT JOIN customers cursor ON cursor.account_id = $1 AND cursor.id = $2
               WHERE candidate.account_id = $1
                 AND ($2 IS NULL OR (candidate.created_at, candidate.id) < (cursor.created_at, cursor.id))
               ORDER BY candidate.created_at DESC, candidate.id DESC LIMIT $3"#,
        )
        .bind(account.0)
        .bind(starting_after)
        .bind(i64::from(limit) + 1)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn update(
        &self,
        account: AccountId,
        id: Uuid,
        name: String,
        email: Option<String>,
        details: Option<String>,
    ) -> Result<Option<DbCustomer>, sqlx::Error> {
        sqlx::query_as::<_, DbCustomer>(
            r#"UPDATE customers SET name = $3, email = $4, details = $5, updated_at = now()
               WHERE account_id = $1 AND id = $2
               RETURNING *"#,
        )
        .bind(account.0)
        .bind(id)
        .bind(name)
        .bind(email)
        .bind(details)
        .fetch_optional(&self.pool)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn account(pool: &PgPool, id: u128) -> AccountId {
        let id = Uuid::from_u128(id);
        sqlx::query(
            "INSERT INTO accounts (id, api_key_hash, api_key_hint) VALUES ($1, $2, 'hint')",
        )
        .bind(id)
        .bind(id.as_bytes().repeat(2))
        .execute(pool)
        .await
        .unwrap();
        AccountId(id)
    }

    fn input(account: AccountId, name: &str) -> CreateCustomerInput {
        CreateCustomerInput {
            id: Uuid::now_v7(),
            account_id: account,
            name: name.into(),
            email: None,
            details: None,
        }
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn customers_are_scoped_to_their_account(pool: PgPool) {
        let mine = account(&pool, 1).await;
        let theirs = account(&pool, 2).await;
        let repo = CustomerRepository::new(pool);
        let first = repo.create(&input(mine, "Acme")).await.unwrap();
        let second = repo.create(&input(mine, "Globex")).await.unwrap();
        let other = repo.create(&input(theirs, "Initech")).await.unwrap();

        let listed = repo.list_for_account(mine, 20, None).await.unwrap();
        assert_eq!(
            listed.iter().map(|row| row.id).collect::<Vec<_>>(),
            [second.id, first.id],
            "newest first, nobody else's rows"
        );
        let page = repo.list_for_account(mine, 1, None).await.unwrap();
        assert_eq!(page.len(), 2, "one extra row signals another page");
        let next = repo
            .list_for_account(mine, 1, Some(second.id))
            .await
            .unwrap();
        assert_eq!(
            next.iter().map(|row| row.id).collect::<Vec<_>>(),
            [first.id]
        );
        assert!(
            repo.list_for_account(mine, 20, Some(other.id))
                .await
                .unwrap()
                .is_empty(),
            "a foreign cursor matches nothing"
        );

        assert!(
            repo.get_for_account(mine, other.id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            repo.get_for_account(theirs, other.id)
                .await
                .unwrap()
                .unwrap()
                .name,
            "Initech"
        );

        assert!(
            repo.update(mine, other.id, "Renamed".into(), None, None)
                .await
                .unwrap()
                .is_none()
        );
        let updated = repo
            .update(
                theirs,
                other.id,
                "Initech LLC".into(),
                Some("ap@initech.example".into()),
                Some("Net 30".into()),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.name, "Initech LLC");
        assert_eq!(updated.email.as_deref(), Some("ap@initech.example"));
        assert_eq!(updated.details.as_deref(), Some("Net 30"));
        assert!(updated.updated_at >= other.updated_at);
        assert_eq!(
            repo.get_for_account(theirs, other.id)
                .await
                .unwrap()
                .unwrap()
                .name,
            "Initech LLC"
        );
    }
}
