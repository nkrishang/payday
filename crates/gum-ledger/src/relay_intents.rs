//! Cross-chain payments through Relay, one row per quote (migration 0008).
//!
//! `gum-server` inserts an intent when it quotes and marks it `sent` when the
//! page reports the origin transaction. The destination chain's indexer
//! worker polls Relay for pending intents and resolves them: `filled` only
//! when the origin chain's own receipt proves the attested wallet paid the
//! quoted amount (migration 0009: attribution is evidence, not Relay's or
//! the page's word), or `failed`/`refunded`.
//!
//! The intent is what keeps the solver's transfer from being flagged as
//! likely unsolicited. While a `sent` intent is pending, the crediting path
//! (`InvoiceRepository::apply_finalized_range`) parks a transfer from
//! an unknown sender for the quoted amount instead of flagging it; the
//! resolution here either attributes the parked transfer to the intent or
//! unparks it into the ordinary flagging path. Only `sent` intents park: a
//! quote nobody sent must not shield a stranger's transfer. A `quoted` or
//! `expired` intent whose exact amount was already observed is still polled
//! (reconciliation), so a fill that lands before the report is attributed
//! anyway; it parks nothing.
//!
//! ```text
//! quoted ─▶ sent ─▶ filled
//! quoted ─▶ expired ─┐
//! sent   ─▶ failed | refunded
//! quoted|expired ─▶ filled | failed | refunded   (reconciliation)
//! ```

use std::time::Duration;

use alloy_primitives::{Address, B256, U256};
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayIntentStatus {
    Quoted,
    Sent,
    Filled,
    Failed,
    Refunded,
    Expired,
}

impl RelayIntentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Quoted => "quoted",
            Self::Sent => "sent",
            Self::Filled => "filled",
            Self::Failed => "failed",
            Self::Refunded => "refunded",
            Self::Expired => "expired",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "quoted" => Self::Quoted,
            "sent" => Self::Sent,
            "filled" => Self::Filled,
            "failed" => Self::Failed,
            "refunded" => Self::Refunded,
            "expired" => Self::Expired,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct DbRelayIntent {
    pub id: Uuid,
    pub invoice_id: Uuid,
    pub request_id: Vec<u8>,
    pub origin_chain_id: i64,
    pub destination_chain_id: i64,
    pub origin_currency: Vec<u8>,
    pub payer_wallet: Vec<u8>,
    pub quoted_in_amount: String,
    pub quoted_out_amount: String,
    pub status: String,
    pub origin_tx_hash: Option<Vec<u8>>,
    pub verified_origin_tx_hash: Option<Vec<u8>>,
    pub fill_tx_hashes: Vec<Vec<u8>>,
    pub relay_status: Option<String>,
    pub attribution_source: Option<String>,
    pub next_check_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
}

impl DbRelayIntent {
    pub fn request_id(&self) -> Option<B256> {
        B256::try_from(self.request_id.as_slice()).ok()
    }

    pub fn payer_wallet(&self) -> Option<Address> {
        Address::try_from(self.payer_wallet.as_slice()).ok()
    }

    pub fn origin_tx_hash(&self) -> Option<B256> {
        self.origin_tx_hash
            .as_deref()
            .and_then(|hash| B256::try_from(hash).ok())
    }

    /// The origin transaction the origin chain's receipt vouched for. This
    /// is the only hash a proof may name; the reported one is a hint.
    pub fn verified_origin_tx_hash(&self) -> Option<B256> {
        self.verified_origin_tx_hash
            .as_deref()
            .and_then(|hash| B256::try_from(hash).ok())
    }

    pub fn fill_tx_hashes(&self) -> Vec<B256> {
        self.fill_tx_hashes
            .iter()
            .filter_map(|hash| B256::try_from(hash.as_slice()).ok())
            .collect()
    }
}

/// What a quote records.
#[derive(Debug, Clone)]
pub struct NewRelayIntent {
    pub id: Uuid,
    pub invoice_id: Uuid,
    pub request_id: B256,
    pub origin_chain_id: u64,
    pub destination_chain_id: u64,
    pub origin_currency: Address,
    pub payer_wallet: Address,
    pub quoted_in_amount: U256,
    pub quoted_out_amount: U256,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkSent {
    /// The report is recorded: the intent is `sent`, or was already past
    /// that and the report matched what was recorded (idempotent retries).
    Sent,
    /// The intent already recorded a different origin transaction.
    HashConflict,
    NotFound,
}

/// What resolving an intent did to the transfers parked on its invoice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Resolution {
    /// Parked transfers now attributed to the intent.
    pub attributed: usize,
    /// Parked transfers handed back to the likely-unsolicited path.
    pub unparked: usize,
}

/// What filling an intent came to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FillResolution {
    /// The intent is filled and the attribution is done.
    Resolved(Resolution),
    /// The intent was not in a state that can fill any more.
    #[default]
    NotPending,
    /// Another intent's verified origin transaction is this one: one origin
    /// transaction pays for one intent.
    OriginTransactionClaimed,
}

#[derive(Clone)]
pub struct RelayIntentRepository {
    pool: PgPool,
}

impl RelayIntentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, intent: &NewRelayIntent) -> Result<DbRelayIntent, sqlx::Error> {
        sqlx::query_as::<_, DbRelayIntent>(
            r#"
            INSERT INTO relay_intents
                (id, invoice_id, request_id, origin_chain_id, destination_chain_id,
                 origin_currency, payer_wallet, quoted_in_amount, quoted_out_amount,
                 status, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'quoted', $10)
            RETURNING *
            "#,
        )
        .bind(intent.id)
        .bind(intent.invoice_id)
        .bind(intent.request_id.as_slice())
        .bind(intent.origin_chain_id as i64)
        .bind(intent.destination_chain_id as i64)
        .bind(intent.origin_currency.as_slice())
        .bind(intent.payer_wallet.as_slice())
        .bind(intent.quoted_in_amount.to_string())
        .bind(intent.quoted_out_amount.to_string())
        .bind(intent.expires_at)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn find(&self, id: Uuid) -> Result<Option<DbRelayIntent>, sqlx::Error> {
        sqlx::query_as::<_, DbRelayIntent>("SELECT * FROM relay_intents WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    /// The intent the page is following: the most recent actual attempt
    /// (sent or filled) over newer unused quotes, so an abandoned quote
    /// cannot hide a payment in flight.
    pub async fn latest_for_invoice(
        &self,
        invoice_id: Uuid,
    ) -> Result<Option<DbRelayIntent>, sqlx::Error> {
        sqlx::query_as::<_, DbRelayIntent>(
            r#"
            SELECT * FROM relay_intents WHERE invoice_id = $1
            ORDER BY (status = 'sent') DESC,
                     (sent_at IS NOT NULL OR status = 'filled') DESC,
                     COALESCE(sent_at, resolved_at, created_at) DESC,
                     id DESC
            LIMIT 1
            "#,
        )
        .bind(invoice_id)
        .fetch_optional(&self.pool)
        .await
    }

    /// The page reported the origin transaction: the intent is `sent` and
    /// the poll starts. Idempotent for the same identity and hash, whatever
    /// state the intent reached — a report may arrive after the invoice
    /// funded, or twice over a flaky connection. A report never reopens a
    /// terminal attempt and never replaces a recorded hash. A compare-and-set
    /// on the intent alone; the invoice row is never locked here.
    pub async fn mark_sent(
        &self,
        id: Uuid,
        invoice_id: Uuid,
        payer_wallet: Address,
        origin_tx_hash: B256,
    ) -> Result<MarkSent, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let Some(mut intent): Option<DbRelayIntent> = sqlx::query_as(
            r#"
            SELECT * FROM relay_intents
            WHERE id = $1 AND invoice_id = $2 AND payer_wallet = $3
            FOR UPDATE
            "#,
        )
        .bind(id)
        .bind(invoice_id)
        .bind(payer_wallet.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        else {
            return Ok(MarkSent::NotFound);
        };
        let outcome = match intent.origin_tx_hash() {
            Some(recorded) if recorded != origin_tx_hash => MarkSent::HashConflict,
            None if intent.status == "filled" => {
                // The report of the transaction whose receipt filled the
                // intent: recording it changes nothing the poller proved.
                if intent.verified_origin_tx_hash() == Some(origin_tx_hash) {
                    intent.origin_tx_hash = Some(origin_tx_hash.as_slice().to_vec());
                    sqlx::query("UPDATE relay_intents SET origin_tx_hash = $2 WHERE id = $1")
                        .bind(id)
                        .bind(origin_tx_hash.as_slice())
                        .execute(&mut *tx)
                        .await?;
                    MarkSent::Sent
                } else {
                    MarkSent::HashConflict
                }
            }
            // Reported already, or a terminal attempt with the recorded hash:
            // nothing to change.
            _ => MarkSent::Sent,
        };
        if outcome == MarkSent::HashConflict {
            tx.rollback().await?;
            return Ok(outcome);
        }
        if matches!(intent.status.as_str(), "quoted" | "expired") {
            sqlx::query(
                r#"
                UPDATE relay_intents
                SET status = 'sent', origin_tx_hash = $2, sent_at = now(),
                    next_check_at = now(), resolved_at = NULL
                WHERE id = $1
                "#,
            )
            .bind(id)
            .bind(origin_tx_hash.as_slice())
            .execute(&mut *tx)
            .await?;
        } else if intent.origin_tx_hash.is_none() {
            // A late report for a terminal attempt: keep it for the record
            // without reopening anything.
            sqlx::query("UPDATE relay_intents SET origin_tx_hash = $2 WHERE id = $1")
                .bind(id)
                .bind(origin_tx_hash.as_slice())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(outcome)
    }

    /// How many `sent` intents into `destination_chain_id` await Relay.
    pub async fn pending_count(&self, destination_chain_id: u64) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT count(*) FROM relay_intents WHERE destination_chain_id = $1 AND status = 'sent'",
        )
        .bind(destination_chain_id as i64)
        .fetch_one(&self.pool)
        .await
    }

    /// Intents on `destination_chain_id` whose next check is due, oldest
    /// first: `sent` intents, and `quoted`/`expired` ones whose exact amount
    /// was already observed unattributed on this chain (the solver can land
    /// its fill before the page's report arrives; reconciliation follows it
    /// without waiting for a report, and unparks nothing on its own). The
    /// reconciliation horizon is a day past quote expiry, so abandoned
    /// quotes stop being polled.
    pub async fn due_for_poll(
        &self,
        destination_chain_id: u64,
        limit: i64,
    ) -> Result<Vec<DbRelayIntent>, sqlx::Error> {
        sqlx::query_as::<_, DbRelayIntent>(
            r#"
            SELECT r.* FROM relay_intents r
            WHERE r.destination_chain_id = $1
              AND COALESCE(r.next_check_at, r.created_at) <= now()
              AND (
                  r.status = 'sent'
                  OR (
                      r.status IN ('quoted', 'expired')
                      AND r.expires_at >= now() - interval '1 day'
                      AND EXISTS (
                          SELECT 1 FROM payment_observations o
                          JOIN invoices i ON i.id = o.invoice_id
                          WHERE o.invoice_id = r.invoice_id
                            AND o.relay_intent_id IS NULL
                            AND o.chain_id = r.destination_chain_id
                            AND o.token_address = i.token_address
                            AND o.recipient_address = i.payment_address
                            AND o.sender_address <> r.payer_wallet
                            AND o.disposition IN ('credited', 'late')
                            AND o.amount::numeric > 0
                            AND o.amount::numeric = r.quoted_out_amount::numeric
                      )
                  )
              )
            ORDER BY COALESCE(r.next_check_at, r.created_at), r.id
            LIMIT $2
            "#,
        )
        .bind(destination_chain_id as i64)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn defer(
        &self,
        id: Uuid,
        next_check_at: DateTime<Utc>,
        relay_status: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE relay_intents
            SET next_check_at = $2, relay_status = COALESCE($3, relay_status)
            WHERE id = $1 AND status IN ('quoted', 'sent', 'expired')
            "#,
        )
        .bind(id)
        .bind(next_check_at)
        .bind(relay_status)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Quotes nobody sent, past their expiry: forgotten.
    pub async fn expire_quoted(&self, destination_chain_id: u64) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE relay_intents
            SET status = 'expired', resolved_at = now()
            WHERE destination_chain_id = $1 AND status = 'quoted' AND expires_at < now()
            "#,
        )
        .bind(destination_chain_id as i64)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Relay filled the request. Requires the verified origin transaction —
    /// the one whose receipt proved the attested wallet paid — records the
    /// fill, then, under the invoice row's lock, attributes every matching
    /// transfer the fill made and unparks the rest unless another sent
    /// intent may still explain them. Attribution matches one log: a
    /// transaction hash admits only the observation for the exact quoted
    /// amount, from the solver, on the invoice's own token, and never one
    /// another intent already owns.
    pub async fn resolve_filled(
        &self,
        id: Uuid,
        fill_tx_hashes: &[B256],
        verified_origin_tx_hash: B256,
        relay_status: &str,
    ) -> Result<FillResolution, sqlx::Error> {
        if fill_tx_hashes.is_empty() {
            return Ok(FillResolution::NotPending);
        }
        let mut tx = self.pool.begin().await?;
        let Some(intent) = lock_intent(&mut tx, id).await? else {
            return Ok(FillResolution::NotPending);
        };
        if !matches!(intent.status.as_str(), "quoted" | "sent" | "expired") {
            tx.rollback().await?;
            return Ok(FillResolution::NotPending);
        }
        let fills: Vec<Vec<u8>> = fill_tx_hashes
            .iter()
            .map(|hash| hash.as_slice().to_vec())
            .collect();
        let claimed = sqlx::query(
            r#"
            UPDATE relay_intents
            SET status = 'filled', fill_tx_hashes = $2,
                verified_origin_tx_hash = $3, relay_status = $4,
                attribution_source = 'receipt',
                next_check_at = NULL, resolved_at = now()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&fills)
        .bind(verified_origin_tx_hash.as_slice())
        .bind(relay_status)
        .execute(&mut *tx)
        .await;
        if let Err(sqlx::Error::Database(error)) = &claimed
            && error.is_unique_violation()
        {
            tx.rollback().await?;
            return Ok(FillResolution::OriginTransactionClaimed);
        }
        claimed?;
        let payer_wallet = intent.payer_wallet.clone();
        let quoted_out_amount = intent.quoted_out_amount.clone();
        let attributed = sqlx::query(
            r#"
            UPDATE payment_observations o
            SET relay_intent_id = $1, relay_parked_at = NULL
            FROM invoices i
            WHERE i.id = $2 AND o.invoice_id = i.id
              AND o.relay_intent_id IS NULL
              AND o.transaction_hash = ANY($3::bytea[])
              AND o.chain_id = i.chain_id
              AND o.token_address = i.token_address
              AND o.recipient_address = i.payment_address
              AND o.sender_address <> $4
              AND o.disposition IN ('credited', 'late')
              AND o.amount::numeric > 0
              AND o.amount::numeric = $5::numeric
            "#,
        )
        .bind(id)
        .bind(intent.invoice_id)
        .bind(&fills)
        .bind(payer_wallet.as_slice())
        .bind(quoted_out_amount)
        .execute(&mut *tx)
        .await?
        .rows_affected() as usize;
        let unparked = unpark_unless_pending(&mut tx, intent.invoice_id).await?;
        tx.commit().await?;
        Ok(FillResolution::Resolved(Resolution {
            attributed,
            unparked,
        }))
    }

    /// Relay failed or refunded the request, or the origin evidence named
    /// somebody else: nothing the intent parked is the payer's. Unreported
    /// quotes reconcile the same way, so a payment attempt Relay never
    /// filled cannot shield a stranger's transfer either.
    pub async fn resolve_failed(
        &self,
        id: Uuid,
        status: RelayIntentStatus,
        relay_status: &str,
    ) -> Result<Resolution, sqlx::Error> {
        debug_assert!(matches!(
            status,
            RelayIntentStatus::Failed | RelayIntentStatus::Refunded
        ));
        let mut tx = self.pool.begin().await?;
        let Some(intent) = lock_intent(&mut tx, id).await? else {
            return Ok(Resolution::default());
        };
        if !matches!(intent.status.as_str(), "quoted" | "sent" | "expired") {
            tx.rollback().await?;
            return Ok(Resolution::default());
        }
        sqlx::query(
            r#"
            UPDATE relay_intents
            SET status = $2, relay_status = $3, next_check_at = NULL, resolved_at = now()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(status.as_str())
        .bind(relay_status)
        .execute(&mut *tx)
        .await?;
        let unparked = unpark_unless_pending(&mut tx, intent.invoice_id).await?;
        tx.commit().await?;
        Ok(Resolution {
            attributed: 0,
            unparked,
        })
    }

    /// `sent` intents into `destination_chain_id` older than `cutoff` that
    /// Relay never resolved: failed, and whatever they parked goes back to
    /// the flagging path. Long, since a delayed fill that lands after this
    /// is flagged for good.
    pub async fn stale_sent(
        &self,
        destination_chain_id: u64,
        cutoff: Duration,
    ) -> Result<Vec<DbRelayIntent>, sqlx::Error> {
        sqlx::query_as::<_, DbRelayIntent>(
            r#"
            SELECT * FROM relay_intents
            WHERE destination_chain_id = $1 AND status = 'sent'
              AND sent_at < now() - make_interval(secs => $2)
            "#,
        )
        .bind(destination_chain_id as i64)
        .bind(cutoff.as_secs_f64())
        .fetch_all(&self.pool)
        .await
    }
}

async fn lock_intent(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Option<DbRelayIntent>, sqlx::Error> {
    // Lock order everywhere: invoices before relay_intents before
    // payment_observations, the same as the crediting path.
    let Some(invoice_id): Option<Uuid> =
        sqlx::query_scalar("SELECT invoice_id FROM relay_intents WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
    else {
        return Ok(None);
    };
    sqlx::query("SELECT id FROM invoices WHERE id = $1 FOR UPDATE")
        .bind(invoice_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query_as::<_, DbRelayIntent>("SELECT * FROM relay_intents WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await
}

/// Hand every still-parked transfer of the invoice back to the ordinary
/// path, unless another sent intent may still be the one that made them.
/// Flagging is the chain time of the earliest such transfer, as the
/// crediting path would have stamped it; the trigger raises the webhook.
async fn unpark_unless_pending(
    tx: &mut Transaction<'_, Postgres>,
    invoice_id: Uuid,
) -> Result<usize, sqlx::Error> {
    let pending: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM relay_intents WHERE invoice_id = $1 AND status = 'sent')",
    )
    .bind(invoice_id)
    .fetch_one(&mut **tx)
    .await?;
    if pending {
        return Ok(0);
    }
    let earliest: Option<i64> = sqlx::query_scalar(
        r#"
        WITH parked AS (
            UPDATE payment_observations
            SET relay_parked_at = NULL
            WHERE invoice_id = $1 AND relay_parked_at IS NOT NULL
            RETURNING block_timestamp
        )
        SELECT min(block_timestamp) FROM parked
        "#,
    )
    .bind(invoice_id)
    .fetch_one(&mut **tx)
    .await?;
    let Some(earliest) = earliest else {
        return Ok(0);
    };
    let flagged = sqlx::query(
        r#"
        UPDATE invoices
        SET likely_unsolicited_at = CASE
                WHEN likely_unsolicited_at IS NULL THEN to_timestamp($2)
                ELSE likely_unsolicited_at
            END,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(invoice_id)
    .bind(earliest as f64)
    .execute(&mut **tx)
    .await?;
    Ok(flagged.rows_affected() as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invoices::tests::{account, insert_bound, issuance_input, test_payer_wallet};
    use crate::{DbInvoice, InvoiceRepository, PaymentObservation, ProofRepository};

    const CHAIN: u64 = 1;
    const ORIGIN: u64 = 8453;

    fn solver() -> Address {
        Address::repeat_byte(0x50)
    }

    fn address(row: &DbInvoice) -> Address {
        Address::from_slice(row.payment_address.as_ref().unwrap())
    }

    fn token(row: &DbInvoice) -> Address {
        Address::from_slice(row.token_address.as_ref().unwrap())
    }

    fn amount(row: &DbInvoice) -> U256 {
        U256::from_str_radix(&row.amount, 10).unwrap()
    }

    fn observation(
        sender: Address,
        row: &DbInvoice,
        amount: U256,
        hash: B256,
        block: u64,
    ) -> PaymentObservation {
        PaymentObservation {
            token: token(row),
            block_number: block,
            block_hash: B256::repeat_byte(block as u8),
            block_timestamp: block * 100,
            transaction_hash: hash,
            transaction_index: 0,
            log_index: 0,
            sender,
            recipient: address(row),
            amount,
        }
    }

    fn intent(row: &DbInvoice, request: u8) -> NewRelayIntent {
        NewRelayIntent {
            id: Uuid::now_v7(),
            invoice_id: row.id,
            request_id: B256::repeat_byte(request),
            origin_chain_id: ORIGIN,
            destination_chain_id: CHAIN,
            origin_currency: Address::repeat_byte(0x0c),
            payer_wallet: test_payer_wallet(),
            quoted_in_amount: amount(row) + U256::from(20_000u64),
            quoted_out_amount: amount(row),
            expires_at: Utc::now() + chrono::Duration::minutes(15),
        }
    }

    /// One finalized range on the test chain ending at `block`, with the
    /// cursor the previous call left (ranges here are one block apart).
    async fn apply(
        repo: &InvoiceRepository,
        _row: &DbInvoice,
        block: u64,
        cursor_block: Option<u64>,
        observations: &[PaymentObservation],
    ) {
        let cursor = cursor_block.map(|block| crate::IndexerCursor {
            block,
            block_hash: B256::repeat_byte(block as u8),
            block_timestamp: Some(block * 100),
        });
        repo.apply_finalized_range(
            CHAIN,
            cursor,
            block,
            B256::repeat_byte(block as u8),
            block * 100,
            observations,
        )
        .await
        .unwrap();
    }

    async fn parked(pool: &PgPool, hash: B256) -> (bool, Option<Uuid>) {
        sqlx::query_as::<_, (bool, Option<Uuid>)>(
            "SELECT relay_parked_at IS NOT NULL, relay_intent_id FROM payment_observations WHERE transaction_hash = $1",
        )
        .bind(hash.as_slice())
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn flagged_at(repo: &InvoiceRepository, id: Uuid) -> Option<i64> {
        repo.find_by_id(id)
            .await
            .unwrap()
            .unwrap()
            .likely_unsolicited_at
            .map(|at| at.timestamp())
    }

    /// The solver's transfer for a sent intent is parked, not flagged, and
    /// the fill attributes it; the proof then reads the attribution. A
    /// quote nobody reported as sent shields nothing.
    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn a_sent_intent_parks_the_solvers_transfer_and_the_fill_attributes_it(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let invoices = InvoiceRepository::new(pool.clone());
        let intents = RelayIntentRepository::new(pool.clone());
        let quoted_only =
            insert_bound(&pool, &issuance_input(owner, "quoted-only", None), None).await;
        let paid = insert_bound(&pool, &issuance_input(owner, "relayed", None), None).await;

        // A quote alone explains nothing: the transfer is flagged as usual.
        intents.insert(&intent(&quoted_only, 0x01)).await.unwrap();
        let unexplained = B256::repeat_byte(0xA1);
        apply(
            &invoices,
            &quoted_only,
            10,
            None,
            &[observation(
                solver(),
                &quoted_only,
                amount(&quoted_only),
                unexplained,
                10,
            )],
        )
        .await;
        assert_eq!(flagged_at(&invoices, quoted_only.id).await, Some(1_000));
        assert_eq!(parked(&pool, unexplained).await, (false, None));

        // Reported as sent: the transfer for the quoted amount is parked
        // and the invoice funds without a flag.
        let sent = intents.insert(&intent(&paid, 0x02)).await.unwrap();
        assert_eq!(
            intents
                .mark_sent(
                    sent.id,
                    paid.id,
                    test_payer_wallet(),
                    B256::repeat_byte(0xB2)
                )
                .await
                .unwrap(),
            MarkSent::Sent
        );
        let fill = B256::repeat_byte(0xA2);
        apply(
            &invoices,
            &paid,
            11,
            Some(10),
            &[observation(solver(), &paid, amount(&paid), fill, 11)],
        )
        .await;
        assert_eq!(flagged_at(&invoices, paid.id).await, None);
        assert_eq!(parked(&pool, fill).await, (true, None));
        let funded = invoices.find_by_id(paid.id).await.unwrap().unwrap();
        assert_eq!(funded.status, "funded");
        // A transfer for another amount from the same solver is not parked:
        // the intent explains exactly the quoted amount.
        let other = B256::repeat_byte(0xA3);
        apply(
            &invoices,
            &paid,
            12,
            Some(11),
            &[observation(solver(), &paid, U256::from(7u64), other, 12)],
        )
        .await;
        assert_eq!(parked(&pool, other).await, (false, None));
        assert_eq!(flagged_at(&invoices, paid.id).await, Some(1_200));

        // The fill: the parked transfer is the intent's. The origin
        // transaction is the one whose receipt proved the payment.
        let resolution = intents
            .resolve_filled(sent.id, &[fill], B256::repeat_byte(0xB2), "success")
            .await
            .unwrap();
        assert_eq!(
            resolution,
            FillResolution::Resolved(Resolution {
                attributed: 1,
                unparked: 0
            })
        );
        assert_eq!(parked(&pool, fill).await, (false, Some(sent.id)));
        let filled = intents.find(sent.id).await.unwrap().unwrap();
        assert_eq!(filled.status, "filled");
        assert_eq!(filled.attribution_source.as_deref(), Some("receipt"));
        assert_eq!(
            filled.verified_origin_tx_hash(),
            Some(B256::repeat_byte(0xB2))
        );
        assert_eq!(filled.fill_tx_hashes(), vec![fill]);
        assert!(filled.resolved_at.is_some());
        // Resolving again is a no-op.
        assert_eq!(
            intents
                .resolve_filled(sent.id, &[fill], B256::repeat_byte(0xB2), "success")
                .await
                .unwrap(),
            FillResolution::NotPending
        );

        // The proof's read model carries the attribution for that transfer only.
        let transfers = ProofRepository::new(pool.clone())
            .settlement_transfers(paid.id)
            .await
            .unwrap();
        assert_eq!(transfers.len(), 2);
        assert_eq!(
            transfers[0].relay_request_id.as_deref(),
            Some(B256::repeat_byte(0x02).as_slice())
        );
        assert_eq!(transfers[0].relay_origin_chain_id, Some(ORIGIN as i64));
        assert_eq!(
            transfers[0].relay_origin_tx_hash.as_deref(),
            Some(B256::repeat_byte(0xB2).as_slice())
        );
        assert_eq!(
            transfers[0].relay_payer_wallet.as_deref(),
            Some(test_payer_wallet().as_slice())
        );
        assert!(transfers[1].relay_request_id.is_none());
    }

    /// A fill Relay reports before the destination worker sees the transfer
    /// is attributed at credit time, by its hash.
    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn a_fill_known_first_attributes_the_transfer_when_it_is_credited(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let invoices = InvoiceRepository::new(pool.clone());
        let intents = RelayIntentRepository::new(pool.clone());
        let paid = insert_bound(&pool, &issuance_input(owner, "relayed-first", None), None).await;
        let sent = intents.insert(&intent(&paid, 0x03)).await.unwrap();
        intents
            .mark_sent(
                sent.id,
                paid.id,
                test_payer_wallet(),
                B256::repeat_byte(0xB3),
            )
            .await
            .unwrap();
        let fill = B256::repeat_byte(0xA4);
        assert_eq!(
            intents
                .resolve_filled(sent.id, &[fill], B256::repeat_byte(0xB3), "success")
                .await
                .unwrap(),
            FillResolution::Resolved(Resolution::default())
        );
        apply(
            &invoices,
            &paid,
            10,
            None,
            &[observation(solver(), &paid, amount(&paid), fill, 10)],
        )
        .await;
        assert_eq!(parked(&pool, fill).await, (false, Some(sent.id)));
        assert_eq!(flagged_at(&invoices, paid.id).await, None);
        assert_eq!(
            invoices.find_by_id(paid.id).await.unwrap().unwrap().status,
            "funded"
        );
    }

    /// A failed or refunded intent hands what it parked back to the
    /// flagging path, at the transfer's chain time, and the webhook follows.
    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn a_failed_intent_unparks_into_the_likely_unsolicited_path(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let invoices = InvoiceRepository::new(pool.clone());
        let intents = RelayIntentRepository::new(pool.clone());
        let paid = insert_bound(&pool, &issuance_input(owner, "relay-failed", None), None).await;
        let sent = intents.insert(&intent(&paid, 0x04)).await.unwrap();
        intents
            .mark_sent(
                sent.id,
                paid.id,
                test_payer_wallet(),
                B256::repeat_byte(0xB4),
            )
            .await
            .unwrap();
        let stranger = B256::repeat_byte(0xA5);
        apply(
            &invoices,
            &paid,
            10,
            None,
            &[observation(solver(), &paid, amount(&paid), stranger, 10)],
        )
        .await;
        assert_eq!(parked(&pool, stranger).await, (true, None));
        assert_eq!(flagged_at(&invoices, paid.id).await, None);

        // A second sent intent keeps the transfer parked when the first fails.
        let again = intents.insert(&intent(&paid, 0x05)).await.unwrap();
        intents
            .mark_sent(
                again.id,
                paid.id,
                test_payer_wallet(),
                B256::repeat_byte(0xB5),
            )
            .await
            .unwrap();
        assert_eq!(
            intents
                .resolve_failed(sent.id, RelayIntentStatus::Refunded, "refund")
                .await
                .unwrap(),
            Resolution::default()
        );
        assert_eq!(parked(&pool, stranger).await, (true, None));
        assert_eq!(flagged_at(&invoices, paid.id).await, None);

        // The last one failing unparks it: flagged at chain time.
        assert_eq!(
            intents
                .resolve_failed(again.id, RelayIntentStatus::Failed, "depositor_mismatch")
                .await
                .unwrap(),
            Resolution {
                attributed: 0,
                unparked: 1
            }
        );
        assert_eq!(parked(&pool, stranger).await, (false, None));
        assert_eq!(flagged_at(&invoices, paid.id).await, Some(1_000));
        let events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM webhook_events WHERE invoice_id = $1 AND event_type = 'deposit_request.likely_unsolicited'",
        )
        .bind(paid.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(events, 1);
        assert_eq!(
            intents.find(again.id).await.unwrap().unwrap().status,
            "failed"
        );
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn marking_sent_is_a_compare_and_set_and_quotes_expire(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let intents = RelayIntentRepository::new(pool.clone());
        let paid = insert_bound(&pool, &issuance_input(owner, "cas", None), None).await;
        let first = intents.insert(&intent(&paid, 0x06)).await.unwrap();
        let second = intents.insert(&intent(&paid, 0x07)).await.unwrap();
        assert_eq!(
            intents
                .latest_for_invoice(paid.id)
                .await
                .unwrap()
                .unwrap()
                .id,
            second.id
        );
        let origin = B256::repeat_byte(0xB6);
        assert_eq!(
            intents
                .mark_sent(first.id, paid.id, test_payer_wallet(), origin)
                .await
                .unwrap(),
            MarkSent::Sent
        );
        // A report already recorded answers the same again: idempotent.
        assert_eq!(
            intents
                .mark_sent(first.id, paid.id, test_payer_wallet(), origin)
                .await
                .unwrap(),
            MarkSent::Sent
        );
        // The wallet is part of the identity.
        assert_eq!(
            intents
                .mark_sent(first.id, paid.id, Address::repeat_byte(0x77), origin)
                .await
                .unwrap(),
            MarkSent::NotFound
        );
        assert_eq!(
            intents
                .mark_sent(
                    Uuid::nil(),
                    paid.id,
                    test_payer_wallet(),
                    B256::repeat_byte(0xB7)
                )
                .await
                .unwrap(),
            MarkSent::NotFound
        );
        // Another invoice's id does not reach the intent.
        let other = insert_bound(&pool, &issuance_input(owner, "cas-other", None), None).await;
        assert_eq!(
            intents
                .mark_sent(
                    second.id,
                    other.id,
                    test_payer_wallet(),
                    B256::repeat_byte(0xB8)
                )
                .await
                .unwrap(),
            MarkSent::NotFound
        );
        // An intent that recorded a hash keeps it: a different report is a
        // conflict, not an overwrite.
        assert_eq!(
            intents
                .mark_sent(
                    first.id,
                    paid.id,
                    test_payer_wallet(),
                    B256::repeat_byte(0xB9)
                )
                .await
                .unwrap(),
            MarkSent::HashConflict
        );
        assert_eq!(intents.due_for_poll(CHAIN, 10).await.unwrap().len(), 1);
        assert!(intents.due_for_poll(ORIGIN, 10).await.unwrap().is_empty());

        // Unsent quotes are forgotten once expired; sent ones are not.
        sqlx::query("UPDATE relay_intents SET expires_at = now() - interval '1 minute'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(intents.expire_quoted(CHAIN).await.unwrap(), 1);
        assert_eq!(
            intents.find(second.id).await.unwrap().unwrap().status,
            "expired"
        );
        assert_eq!(
            intents.find(first.id).await.unwrap().unwrap().status,
            "sent"
        );
        assert!(
            intents
                .stale_sent(CHAIN, Duration::from_secs(3600))
                .await
                .unwrap()
                .is_empty()
        );
        sqlx::query("UPDATE relay_intents SET sent_at = now() - interval '2 days'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            intents
                .stale_sent(CHAIN, Duration::from_secs(3600))
                .await
                .unwrap()
                .len(),
            1
        );
        // An expired quote can still report the transaction it already sent:
        // expiration stops new sends, not the record of a broadcast.
        assert_eq!(
            intents
                .mark_sent(
                    second.id,
                    paid.id,
                    test_payer_wallet(),
                    B256::repeat_byte(0xBA)
                )
                .await
                .unwrap(),
            MarkSent::Sent
        );
        let reported = intents.find(second.id).await.unwrap().unwrap();
        assert_eq!(reported.status, "sent");
        assert_eq!(reported.origin_tx_hash(), Some(B256::repeat_byte(0xBA)));
        assert!(reported.resolved_at.is_none());
    }

    /// An unverified report reserves nothing: two intents may carry the same
    /// reported hash, and a verified origin transaction fills exactly one
    /// intent — the loser attributes nothing.
    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn unverified_origin_hash_does_not_reserve_a_transaction(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let intents = RelayIntentRepository::new(pool.clone());
        let one = insert_bound(&pool, &issuance_input(owner, "squat-one", None), None).await;
        let two = insert_bound(&pool, &issuance_input(owner, "squat-two", None), None).await;
        let first = intents.insert(&intent(&one, 0x0A)).await.unwrap();
        let second = intents.insert(&intent(&two, 0x0B)).await.unwrap();
        let origin = B256::repeat_byte(0xBB);
        // The same reported hash on two intents is accepted on both: the
        // report is a hint, not a claim.
        assert_eq!(
            intents
                .mark_sent(first.id, one.id, test_payer_wallet(), origin)
                .await
                .unwrap(),
            MarkSent::Sent
        );
        assert_eq!(
            intents
                .mark_sent(second.id, two.id, test_payer_wallet(), origin)
                .await
                .unwrap(),
            MarkSent::Sent
        );
        let fill = B256::repeat_byte(0xFC);
        assert_eq!(
            intents
                .resolve_filled(first.id, &[fill], origin, "success")
                .await
                .unwrap(),
            FillResolution::Resolved(Resolution {
                attributed: 0,
                unparked: 0
            })
        );
        // The second intent's verification finds the transaction already
        // owned: it fails instead of attributing.
        let other_fill = B256::repeat_byte(0xFD);
        assert_eq!(
            intents
                .resolve_filled(second.id, &[other_fill], origin, "success")
                .await
                .unwrap(),
            FillResolution::OriginTransactionClaimed
        );
        assert_eq!(
            intents.find(second.id).await.unwrap().unwrap().status,
            "sent"
        );
    }

    /// The database itself refuses a filled intent without verified origin
    /// evidence: `receipt` and the verified hash are the only attribution.
    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn filled_requires_verified_origin_evidence(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let intents = RelayIntentRepository::new(pool.clone());
        let paid = insert_bound(&pool, &issuance_input(owner, "evidence", None), None).await;
        let intent = intents.insert(&intent(&paid, 0x0C)).await.unwrap();
        for (source, verified) in [
            (None::<&str>, None::<&[u8]>),
            (Some("relay_api"), Some(B256::repeat_byte(0xCD).as_slice())),
            (Some("receipt"), None),
        ] {
            let done = sqlx::query(
                "UPDATE relay_intents SET status = 'filled', attribution_source = $2, verified_origin_tx_hash = $3 WHERE id = $1",
            )
            .bind(intent.id)
            .bind(source)
            .bind(verified)
            .execute(&pool)
            .await;
            assert!(done.is_err(), "{source:?} must not fill");
        }
    }

    /// Attribution matches one log, not a transaction: of the logs a fill's
    /// transaction left on the invoice, only the one for the exact quoted
    /// amount belongs to the intent, and the two orderings — observed first
    /// or filled first — must agree.
    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn attribution_matches_the_quoted_log_whichever_comes_first(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let invoices = InvoiceRepository::new(pool.clone());
        let intents = RelayIntentRepository::new(pool.clone());
        let first = insert_bound(&pool, &issuance_input(owner, "log-first", None), None).await;
        let second = insert_bound(&pool, &issuance_input(owner, "log-second", None), None).await;
        let origin = B256::repeat_byte(0xBE);
        let origin_b = B256::repeat_byte(0xBD);

        // One destination transaction, two transfers to the same invoice:
        // the quoted amount, and dust. (Each invoice's fill is its own
        // transaction; observation identity is chain + token + tx + log.)
        let quoted = amount(&first);
        let fill = B256::repeat_byte(0xAF);
        let both = observation(solver(), &first, quoted, fill, 10);
        let mut dust = observation(solver(), &first, U256::from(7u64), fill, 10);
        dust.log_index = 1;

        let a = intents.insert(&intent(&first, 0x0D)).await.unwrap();
        intents
            .mark_sent(a.id, first.id, test_payer_wallet(), origin)
            .await
            .unwrap();
        // Filled first: at credit time, only the quoted log is attributed.
        assert_eq!(
            intents
                .resolve_filled(a.id, &[fill], origin, "success")
                .await
                .unwrap(),
            FillResolution::Resolved(Resolution {
                attributed: 0,
                unparked: 0
            })
        );
        apply(&invoices, &first, 10, None, &[both.clone(), dust.clone()]).await;
        let owners: Vec<(i64, Option<Uuid>)> = sqlx::query_as(
            "SELECT log_index, relay_intent_id FROM payment_observations WHERE transaction_hash = $1 ORDER BY log_index",
        )
        .bind(fill.as_slice())
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(owners, vec![(0, Some(a.id)), (1, None)]);

        // Observed first: the parked path parks only the quoted amount; the
        // resolution then attributes exactly that log. This invoice's fill
        // is a different transaction, later on the chain cursor.
        let fill_b = B256::repeat_byte(0xFB);
        let b = intents.insert(&intent(&second, 0x0E)).await.unwrap();
        intents
            .mark_sent(b.id, second.id, test_payer_wallet(), origin_b)
            .await
            .unwrap();
        let mut quoted_b = observation(solver(), &second, quoted, fill_b, 11);
        quoted_b.block_timestamp = 1_100;
        let mut dust_b = observation(solver(), &second, U256::from(7u64), fill_b, 11);
        dust_b.log_index = 1;
        dust_b.block_timestamp = 1_100;
        apply(&invoices, &second, 11, Some(10), &[quoted_b, dust_b]).await;
        let parked_row: Option<Uuid> = sqlx::query_scalar(
            "SELECT relay_intent_id FROM payment_observations WHERE transaction_hash = $1 AND log_index = 0 AND invoice_id = $2",
        )
        .bind(fill_b.as_slice())
        .bind(second.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(parked_row, None);
        let parked_flag: bool = sqlx::query_scalar(
            "SELECT relay_parked_at IS NOT NULL FROM payment_observations WHERE transaction_hash = $1 AND log_index = 0 AND invoice_id = $2",
        )
        .bind(fill_b.as_slice())
        .bind(second.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(parked_flag);
        assert_eq!(
            intents
                .resolve_filled(b.id, &[fill_b], origin_b, "success")
                .await
                .unwrap(),
            FillResolution::Resolved(Resolution {
                attributed: 1,
                unparked: 0
            })
        );
        let dust_owner: Option<Uuid> = sqlx::query_scalar(
            "SELECT relay_intent_id FROM payment_observations WHERE transaction_hash = $1 AND log_index = 1 AND invoice_id = $2",
        )
        .bind(fill_b.as_slice())
        .bind(second.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(dust_owner, None);
    }

    /// A fill observed before any report is attributed by reconciliation:
    /// an unreported quote whose exact amount was seen is polled, filled on
    /// evidence, and its late report is accepted without reopening anything.
    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn a_never_reported_quote_is_reconciled_when_its_amount_was_seen(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let invoices = InvoiceRepository::new(pool.clone());
        let intents = RelayIntentRepository::new(pool.clone());
        let paid = insert_bound(&pool, &issuance_input(owner, "reconciled", None), None).await;
        let quote = intents.insert(&intent(&paid, 0x0F)).await.unwrap();

        // The solver's transfer for exactly the quoted amount lands, but the
        // page never reports: flagged (a quote explains nothing), never parked.
        let fill = B256::repeat_byte(0xAE);
        apply(
            &invoices,
            &paid,
            10,
            None,
            &[observation(solver(), &paid, amount(&paid), fill, 10)],
        )
        .await;
        assert_eq!(flagged_at(&invoices, paid.id).await, Some(1_000));
        assert_eq!(parked(&pool, fill).await, (false, None));

        // Reconciliation picks the quote up and the evidence fills it.
        assert_eq!(intents.due_for_poll(CHAIN, 10).await.unwrap().len(), 1);
        assert_eq!(
            intents
                .resolve_filled(quote.id, &[fill], B256::repeat_byte(0xBF), "success")
                .await
                .unwrap(),
            FillResolution::Resolved(Resolution {
                attributed: 1,
                unparked: 0
            })
        );
        assert_eq!(parked(&pool, fill).await, (false, Some(quote.id)));
        // The historical flag stands; attribution changed nothing about it.
        assert_eq!(flagged_at(&invoices, paid.id).await, Some(1_000));
        // The late report of the same transaction is accepted, idempotently.
        assert_eq!(
            intents
                .mark_sent(
                    quote.id,
                    paid.id,
                    test_payer_wallet(),
                    B256::repeat_byte(0xBF)
                )
                .await
                .unwrap(),
            MarkSent::Sent
        );
        assert_eq!(
            intents.find(quote.id).await.unwrap().unwrap().status,
            "filled"
        );
        // A quote without a matching observation shields nothing and is not
        // polled; an unused one is invisible next to a real attempt.
        let unused = intents.insert(&intent(&paid, 0x10)).await.unwrap();
        assert_eq!(intents.due_for_poll(CHAIN, 10).await.unwrap().len(), 0);
        assert_eq!(
            intents
                .latest_for_invoice(paid.id)
                .await
                .unwrap()
                .unwrap()
                .id,
            quote.id
        );
        let _ = unused;
    }
}
