//! Issuer identities and the payout addresses they may settle to.
//!
//! The merchant's own side of an invoice, kept once rather than retyped. An
//! issued invoice still snapshots the party and the address it was issued
//! with, so editing an identity here never changes an invoice already out in
//! the world — the same rule customers follow.

use sqlx::PgPool;
use sqlx::types::chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::AccountId;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DbIssuer {
    pub id: Uuid,
    pub account_id: Uuid,
    pub name: String,
    pub contact_email: String,
    pub details: Option<String>,
    /// When the contact mailbox was proven. `None` until a code lands.
    pub email_verified_at: Option<DateTime<Utc>>,
    pub email_code_sent_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DbPayoutAddress {
    pub id: Uuid,
    pub account_id: Uuid,
    /// EIP-55 checksummed; the caller canonicalizes before it gets here.
    pub address: String,
    pub label: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// One association, flattened for the listing that loads every identity's
/// addresses in a single round trip.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DbIssuerPayoutAddress {
    pub issuer_id: Uuid,
    pub id: Uuid,
    pub account_id: Uuid,
    pub address: String,
    pub label: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct CreateIssuerInput {
    pub id: Uuid,
    pub account_id: AccountId,
    pub name: String,
    pub contact_email: String,
    pub details: Option<String>,
}

pub struct CreatePayoutAddressInput {
    pub id: Uuid,
    pub account_id: AccountId,
    pub address: String,
    pub label: Option<String>,
}

/// The index that keeps one name per account, so a caller can tell a duplicate
/// name apart from any other write failure.
pub const ISSUER_NAME_UNIQUE: &str = "issuers_account_name_unique";

/// Whether a write failed because the name is already an identity's.
pub fn is_duplicate_issuer_name(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.constraint() == Some(ISSUER_NAME_UNIQUE))
}

/// Why a code could not be sent right now.
#[derive(Debug, PartialEq, Eq)]
pub enum StartIssuerEmailError {
    NoSuchIssuer,
    AlreadyVerified,
    /// Another code went out this recently; the caller reports the wait.
    TooSoon(DateTime<Utc>),
}

#[derive(Clone)]
pub struct IssuerRepository {
    pool: PgPool,
}

impl IssuerRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, input: &CreateIssuerInput) -> Result<DbIssuer, sqlx::Error> {
        sqlx::query_as::<_, DbIssuer>(
            r#"INSERT INTO issuers (id, account_id, name, contact_email, details)
               VALUES ($1, $2, $3, $4, $5)
               RETURNING *"#,
        )
        .bind(input.id)
        .bind(input.account_id.0)
        .bind(&input.name)
        .bind(&input.contact_email)
        .bind(&input.details)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn get_for_account(
        &self,
        account: AccountId,
        id: Uuid,
    ) -> Result<Option<DbIssuer>, sqlx::Error> {
        sqlx::query_as::<_, DbIssuer>("SELECT * FROM issuers WHERE account_id = $1 AND id = $2")
            .bind(account.0)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    /// Newest first, with one row beyond `limit` so the caller can tell
    /// whether another page exists — the convention every listing here uses.
    pub async fn list_for_account(
        &self,
        account: AccountId,
        limit: u32,
        starting_after: Option<Uuid>,
    ) -> Result<Vec<DbIssuer>, sqlx::Error> {
        sqlx::query_as::<_, DbIssuer>(
            r#"SELECT candidate.* FROM issuers candidate
               LEFT JOIN issuers cursor ON cursor.account_id = $1 AND cursor.id = $2
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

    /// Replaces the editable fields. A different contact address is a
    /// different claim, so the proof and the cooldown go with the old one.
    pub async fn update(
        &self,
        account: AccountId,
        id: Uuid,
        name: String,
        contact_email: String,
        details: Option<String>,
    ) -> Result<Option<DbIssuer>, sqlx::Error> {
        sqlx::query_as::<_, DbIssuer>(
            r#"UPDATE issuers SET
                   name = $3,
                   details = $5,
                   email_verified_at = CASE WHEN contact_email = $4 THEN email_verified_at END,
                   email_code_sent_at = CASE WHEN contact_email = $4 THEN email_code_sent_at END,
                   contact_email = $4,
                   updated_at = now()
               WHERE account_id = $1 AND id = $2
               RETURNING *"#,
        )
        .bind(account.0)
        .bind(id)
        .bind(name)
        .bind(contact_email)
        .bind(details)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn delete(&self, account: AccountId, id: Uuid) -> Result<bool, sqlx::Error> {
        sqlx::query("DELETE FROM issuers WHERE account_id = $1 AND id = $2")
            .bind(account.0)
            .bind(id)
            .execute(&self.pool)
            .await
            .map(|done| done.rows_affected() > 0)
    }

    /// Whether anything was issued under this identity. The link is what makes
    /// "these requests are that identity's" answerable after a rename, so an
    /// identity history refers to is not deleted out from under it.
    pub async fn has_invoices(&self, account: AccountId, id: Uuid) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM invoices WHERE account_id = $1 AND issuer_id = $2)",
        )
        .bind(account.0)
        .bind(id)
        .fetch_one(&self.pool)
        .await
    }

    /// Claims the right to send a code, stamping the attempt in the same
    /// statement that checks the cooldown so two requests cannot both win.
    pub async fn start_email_verification(
        &self,
        account: AccountId,
        id: Uuid,
        cooldown_seconds: i64,
    ) -> Result<DbIssuer, StartIssuerEmailError> {
        let claimed = sqlx::query_as::<_, DbIssuer>(
            r#"UPDATE issuers SET email_code_sent_at = now()
               WHERE account_id = $1 AND id = $2
                 AND email_verified_at IS NULL
                 AND (email_code_sent_at IS NULL
                      OR email_code_sent_at <= now() - make_interval(secs => $3::double precision))
               RETURNING *"#,
        )
        .bind(account.0)
        .bind(id)
        .bind(cooldown_seconds as f64)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| StartIssuerEmailError::NoSuchIssuer)?;
        if let Some(row) = claimed {
            return Ok(row);
        }
        // Nothing was claimed: say which of the three reasons it was.
        match self.get_for_account(account, id).await {
            Ok(Some(row)) if row.email_verified_at.is_some() => {
                Err(StartIssuerEmailError::AlreadyVerified)
            }
            Ok(Some(row)) => Err(StartIssuerEmailError::TooSoon(
                row.email_code_sent_at.unwrap_or_else(Utc::now),
            )),
            _ => Err(StartIssuerEmailError::NoSuchIssuer),
        }
    }

    /// Records the proof. Conditioned on the address so a code confirmed
    /// after the merchant edited the mailbox does not vouch for the new one.
    pub async fn mark_email_verified(
        &self,
        account: AccountId,
        id: Uuid,
        contact_email: &str,
    ) -> Result<Option<DbIssuer>, sqlx::Error> {
        sqlx::query_as::<_, DbIssuer>(
            r#"UPDATE issuers SET email_verified_at = now(), updated_at = now()
               WHERE account_id = $1 AND id = $2 AND contact_email = $3
               RETURNING *"#,
        )
        .bind(account.0)
        .bind(id)
        .bind(contact_email)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn create_payout_address(
        &self,
        input: &CreatePayoutAddressInput,
    ) -> Result<DbPayoutAddress, sqlx::Error> {
        sqlx::query_as::<_, DbPayoutAddress>(
            r#"INSERT INTO payout_addresses (id, account_id, address, label)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT (account_id, address) DO UPDATE
                   SET label = COALESCE(EXCLUDED.label, payout_addresses.label)
               RETURNING *"#,
        )
        .bind(input.id)
        .bind(input.account_id.0)
        .bind(&input.address)
        .bind(&input.label)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn list_payout_addresses(
        &self,
        account: AccountId,
    ) -> Result<Vec<DbPayoutAddress>, sqlx::Error> {
        sqlx::query_as::<_, DbPayoutAddress>(
            r#"SELECT * FROM payout_addresses WHERE account_id = $1
               ORDER BY created_at DESC, id DESC"#,
        )
        .bind(account.0)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_payout_address(
        &self,
        account: AccountId,
        id: Uuid,
    ) -> Result<Option<DbPayoutAddress>, sqlx::Error> {
        sqlx::query_as::<_, DbPayoutAddress>(
            "SELECT * FROM payout_addresses WHERE account_id = $1 AND id = $2",
        )
        .bind(account.0)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn delete_payout_address(
        &self,
        account: AccountId,
        id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query("DELETE FROM payout_addresses WHERE account_id = $1 AND id = $2")
            .bind(account.0)
            .bind(id)
            .execute(&self.pool)
            .await
            .map(|done| done.rows_affected() > 0)
    }

    /// The addresses one identity may settle to, oldest association first so
    /// the first one a merchant attached stays the obvious default.
    pub async fn addresses_for_issuer(
        &self,
        account: AccountId,
        issuer: Uuid,
    ) -> Result<Vec<DbPayoutAddress>, sqlx::Error> {
        sqlx::query_as::<_, DbPayoutAddress>(
            r#"SELECT address.* FROM payout_addresses address
               JOIN issuer_payout_addresses link
                 ON link.payout_address_id = address.id AND link.account_id = $1
               WHERE link.issuer_id = $2
               ORDER BY link.created_at, address.id"#,
        )
        .bind(account.0)
        .bind(issuer)
        .fetch_all(&self.pool)
        .await
    }

    /// Every identity's addresses in one query, so listing them is not a
    /// round trip per row.
    pub async fn addresses_for_issuers(
        &self,
        account: AccountId,
        issuers: &[Uuid],
    ) -> Result<Vec<DbIssuerPayoutAddress>, sqlx::Error> {
        sqlx::query_as::<_, DbIssuerPayoutAddress>(
            r#"SELECT link.issuer_id, address.* FROM payout_addresses address
               JOIN issuer_payout_addresses link
                 ON link.payout_address_id = address.id AND link.account_id = $1
               WHERE link.issuer_id = ANY($2)
               ORDER BY link.created_at, address.id"#,
        )
        .bind(account.0)
        .bind(issuers)
        .fetch_all(&self.pool)
        .await
    }

    /// Replaces which addresses an identity settles to, in one transaction so
    /// a half-applied set is never observable. Order is preserved as the
    /// association order, which is what the listing reads back.
    pub async fn set_payout_addresses(
        &self,
        account: AccountId,
        issuer: Uuid,
        addresses: &[Uuid],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM issuer_payout_addresses WHERE account_id = $1 AND issuer_id = $2")
            .bind(account.0)
            .bind(issuer)
            .execute(&mut *tx)
            .await?;
        for (position, address) in addresses.iter().enumerate() {
            sqlx::query(
                r#"INSERT INTO issuer_payout_addresses
                       (account_id, issuer_id, payout_address_id, created_at)
                   VALUES ($1, $2, $3, now() + make_interval(secs => $4::double precision))
                   ON CONFLICT (issuer_id, payout_address_id) DO NOTHING"#,
            )
            .bind(account.0)
            .bind(issuer)
            .bind(address)
            .bind(position as f64 / 1000.0)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    /// Attaches an address to an identity. Idempotent, and impossible across
    /// accounts: the composite foreign keys refuse the row.
    pub async fn attach_payout_address(
        &self,
        account: AccountId,
        issuer: Uuid,
        payout_address: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"INSERT INTO issuer_payout_addresses (account_id, issuer_id, payout_address_id)
               VALUES ($1, $2, $3)
               ON CONFLICT (issuer_id, payout_address_id) DO NOTHING"#,
        )
        .bind(account.0)
        .bind(issuer)
        .bind(payout_address)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    pub async fn detach_payout_address(
        &self,
        account: AccountId,
        issuer: Uuid,
        payout_address: Uuid,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query(
            r#"DELETE FROM issuer_payout_addresses
               WHERE account_id = $1 AND issuer_id = $2 AND payout_address_id = $3"#,
        )
        .bind(account.0)
        .bind(issuer)
        .bind(payout_address)
        .execute(&self.pool)
        .await
        .map(|done| done.rows_affected() > 0)
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

    fn issuer(account: AccountId, name: &str, email: &str) -> CreateIssuerInput {
        CreateIssuerInput {
            id: Uuid::now_v7(),
            account_id: account,
            name: name.into(),
            contact_email: email.into(),
            details: None,
        }
    }

    fn address(account: AccountId, address: &str) -> CreatePayoutAddressInput {
        CreatePayoutAddressInput {
            id: Uuid::now_v7(),
            account_id: account,
            address: address.into(),
            label: Some("Treasury".into()),
        }
    }

    const WALLET: &str = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
    const OTHER_WALLET: &str = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC";

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn issuers_are_scoped_to_their_account_and_page_newest_first(pool: PgPool) {
        let mine = account(&pool, 1).await;
        let theirs = account(&pool, 2).await;
        let repo = IssuerRepository::new(pool);

        let first = repo
            .create(&issuer(mine, "Acme", "billing@acme.example"))
            .await
            .unwrap();
        let second = repo
            .create(&issuer(mine, "Acme EU", "eu@acme.example"))
            .await
            .unwrap();
        let other = repo
            .create(&issuer(theirs, "Initech", "ap@initech.example"))
            .await
            .unwrap();

        assert!(first.email_verified_at.is_none(), "unproven when created");
        let listed = repo.list_for_account(mine, 20, None).await.unwrap();
        assert_eq!(
            listed.iter().map(|row| row.id).collect::<Vec<_>>(),
            [second.id, first.id]
        );
        assert!(
            repo.get_for_account(mine, other.id)
                .await
                .unwrap()
                .is_none(),
            "nobody else's identity"
        );
        assert!(
            repo.list_for_account(mine, 20, Some(other.id))
                .await
                .unwrap()
                .is_empty(),
            "a foreign cursor matches nothing"
        );
        assert!(!repo.delete(mine, other.id).await.unwrap());
        assert!(repo.delete(theirs, other.id).await.unwrap());
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn a_code_is_claimed_once_per_cooldown_and_proves_only_the_address_it_went_to(
        pool: PgPool,
    ) {
        let mine = account(&pool, 1).await;
        let repo = IssuerRepository::new(pool);
        let row = repo
            .create(&issuer(mine, "Acme", "billing@acme.example"))
            .await
            .unwrap();

        repo.start_email_verification(mine, row.id, 60)
            .await
            .unwrap();
        assert!(
            matches!(
                repo.start_email_verification(mine, row.id, 60).await,
                Err(StartIssuerEmailError::TooSoon(_))
            ),
            "a second code inside the window is refused"
        );
        // A zero-second cooldown is the same statement with the window open.
        repo.start_email_verification(mine, row.id, 0)
            .await
            .unwrap();

        assert!(
            repo.mark_email_verified(mine, row.id, "someone@else.example")
                .await
                .unwrap()
                .is_none(),
            "a code cannot vouch for an address the identity does not carry"
        );
        let verified = repo
            .mark_email_verified(mine, row.id, "billing@acme.example")
            .await
            .unwrap()
            .unwrap();
        assert!(verified.email_verified_at.is_some());
        assert_eq!(
            repo.start_email_verification(mine, row.id, 0).await.err(),
            Some(StartIssuerEmailError::AlreadyVerified)
        );
        assert_eq!(
            repo.start_email_verification(mine, Uuid::now_v7(), 0)
                .await
                .err(),
            Some(StartIssuerEmailError::NoSuchIssuer)
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn one_name_per_account_however_it_is_cased(pool: PgPool) {
        let mine = account(&pool, 1).await;
        let theirs = account(&pool, 2).await;
        let repo = IssuerRepository::new(pool);

        let first = repo
            .create(&issuer(mine, "Payday", "billing@payday.sh"))
            .await
            .unwrap();
        for taken in ["Payday", "payday", "  PAYDAY  "] {
            let refused = repo
                .create(&issuer(mine, taken, "other@payday.sh"))
                .await
                .expect_err(taken);
            assert!(is_duplicate_issuer_name(&refused), "{taken}");
        }
        // Another account's list is its own.
        repo.create(&issuer(theirs, "Payday", "billing@payday.sh"))
            .await
            .unwrap();
        // And a rename onto a name already taken is refused the same way.
        let second = repo
            .create(&issuer(mine, "Payday EU", "eu@payday.sh"))
            .await
            .unwrap();
        let clash = repo
            .update(
                mine,
                second.id,
                "payday".into(),
                "eu@payday.sh".into(),
                None,
            )
            .await
            .expect_err("rename onto a taken name");
        assert!(is_duplicate_issuer_name(&clash));
        // Renaming to its own name is not a clash with itself.
        repo.update(
            mine,
            first.id,
            "Payday".into(),
            "billing@payday.sh".into(),
            None,
        )
        .await
        .unwrap()
        .unwrap();
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn changing_the_contact_address_drops_the_proof(pool: PgPool) {
        let mine = account(&pool, 1).await;
        let repo = IssuerRepository::new(pool);
        let row = repo
            .create(&issuer(mine, "Acme", "billing@acme.example"))
            .await
            .unwrap();
        repo.mark_email_verified(mine, row.id, "billing@acme.example")
            .await
            .unwrap()
            .unwrap();

        let renamed = repo
            .update(
                mine,
                row.id,
                "Acme Inc.".into(),
                "billing@acme.example".into(),
                Some("Reg 4471".into()),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(
            renamed.email_verified_at.is_some(),
            "the same address keeps its proof"
        );

        let moved = repo
            .update(
                mine,
                row.id,
                "Acme Inc.".into(),
                "support@acme.example".into(),
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(
            moved.email_verified_at.is_none() && moved.email_code_sent_at.is_none(),
            "a new address is a new claim, and starts unproven"
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn addresses_attach_to_identities_within_one_account_only(pool: PgPool) {
        let mine = account(&pool, 1).await;
        let theirs = account(&pool, 2).await;
        let repo = IssuerRepository::new(pool);

        let acme = repo
            .create(&issuer(mine, "Acme", "billing@acme.example"))
            .await
            .unwrap();
        let acme_eu = repo
            .create(&issuer(mine, "Acme EU", "eu@acme.example"))
            .await
            .unwrap();
        let intruder = repo
            .create(&issuer(theirs, "Initech", "ap@initech.example"))
            .await
            .unwrap();

        let treasury = repo
            .create_payout_address(&address(mine, WALLET))
            .await
            .unwrap();
        let second = repo
            .create_payout_address(&address(mine, OTHER_WALLET))
            .await
            .unwrap();

        // The same address twice is the same row, not a duplicate.
        let again = repo
            .create_payout_address(&address(mine, WALLET))
            .await
            .unwrap();
        assert_eq!(again.id, treasury.id);
        assert_eq!(repo.list_payout_addresses(mine).await.unwrap().len(), 2);

        repo.attach_payout_address(mine, acme.id, treasury.id)
            .await
            .unwrap();
        repo.attach_payout_address(mine, acme.id, second.id)
            .await
            .unwrap();
        // Attaching twice is not an error and not a second row.
        repo.attach_payout_address(mine, acme.id, treasury.id)
            .await
            .unwrap();
        repo.attach_payout_address(mine, acme_eu.id, treasury.id)
            .await
            .unwrap();

        let attached = repo.addresses_for_issuer(mine, acme.id).await.unwrap();
        assert_eq!(
            attached.iter().map(|row| row.id).collect::<Vec<_>>(),
            [treasury.id, second.id],
            "oldest association first"
        );
        assert_eq!(
            repo.addresses_for_issuer(mine, acme_eu.id)
                .await
                .unwrap()
                .len(),
            1,
            "one wallet can serve several identities"
        );

        assert!(
            repo.attach_payout_address(theirs, intruder.id, treasury.id)
                .await
                .is_err(),
            "an address cannot be attached to another account's identity"
        );

        assert!(
            repo.detach_payout_address(mine, acme.id, second.id)
                .await
                .unwrap()
        );
        assert!(
            !repo
                .detach_payout_address(mine, acme.id, second.id)
                .await
                .unwrap()
        );
        assert_eq!(
            repo.addresses_for_issuer(mine, acme.id)
                .await
                .unwrap()
                .len(),
            1
        );

        // Deleting the address takes its associations with it.
        assert!(repo.delete_payout_address(mine, treasury.id).await.unwrap());
        assert!(
            repo.addresses_for_issuer(mine, acme.id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            repo.addresses_for_issuer(mine, acme_eu.id)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
