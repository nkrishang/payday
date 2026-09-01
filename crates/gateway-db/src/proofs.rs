//! Read model for Proof of Payment (product plan §5.5): the credited
//! transfers and settlement details of one invoice.

use sqlx::PgPool;
use sqlx::types::chrono::{DateTime, Utc};
use uuid::Uuid;

/// One finalized USDC transfer credited toward the invoice amount.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DbSettlementTransfer {
    pub sender_address: Vec<u8>,
    pub recipient_address: Vec<u8>,
    pub amount: String,
    pub transaction_hash: Vec<u8>,
    pub block_number: i64,
}

#[derive(Debug, Clone)]
pub struct DbInvoiceSettlement {
    pub status: String,
    pub settlement_tx_hash: Option<Vec<u8>>,
    pub resolved_at_block: Option<i64>,
    pub settled_at: Option<DateTime<Utc>>,
    pub payer_policy_mode: String,
    pub verification_completed_at: Option<DateTime<Utc>>,
    /// Chain order: block, transaction, log.
    pub transfers: Vec<DbSettlementTransfer>,
}

#[derive(Clone)]
pub struct ProofRepository {
    pool: PgPool,
}

impl ProofRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Settlement details and credited observations; `None` when the invoice
    /// does not exist. Callers resolve the invoice within its account first.
    pub async fn settlement_transfers(
        &self,
        invoice_id: Uuid,
    ) -> Result<Option<DbInvoiceSettlement>, sqlx::Error> {
        type Settlement = (
            String,
            Option<Vec<u8>>,
            Option<i64>,
            Option<DateTime<Utc>>,
            String,
            Option<DateTime<Utc>>,
        );
        let Some((
            status,
            settlement_tx_hash,
            resolved_at_block,
            settled_at,
            payer_policy_mode,
            verification_completed_at,
        )) = sqlx::query_as::<_, Settlement>(
            r#"SELECT status, settlement_tx_hash, resolved_at_block, settled_at,
                      payer_policy_mode, verification_completed_at
               FROM invoices WHERE id = $1"#,
        )
        .bind(invoice_id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        let transfers = sqlx::query_as::<_, DbSettlementTransfer>(
            r#"SELECT sender_address, recipient_address, amount, transaction_hash, block_number
               FROM payment_observations
               WHERE invoice_id = $1 AND disposition = 'credited'
               ORDER BY block_number, transaction_index, log_index"#,
        )
        .bind(invoice_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(Some(DbInvoiceSettlement {
            status,
            settlement_tx_hash,
            resolved_at_block,
            settled_at,
            payer_policy_mode,
            verification_completed_at,
            transfers,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InvoiceRepository;
    use crate::invoices::tests::{account, issuance_input};

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn proof_read_model_lists_only_credited_transfers_in_chain_order(pool: PgPool) {
        let owner = account(&pool, 1).await;
        let issued = InvoiceRepository::new(pool.clone())
            .insert_issued(&issuance_input(owner, "proof", None), None)
            .await
            .unwrap()
            .row;
        let observation = |block: i64, index: i64, disposition: &str, amount: &str| {
            let reason = if disposition == "late" {
                "'invoice_expired'"
            } else {
                "NULL"
            };
            format!(
                r#"INSERT INTO payment_observations
                     (chain_id, token_address, block_number, block_hash, block_timestamp,
                      transaction_hash, transaction_index, log_index, sender_address,
                      recipient_address, invoice_id, amount, disposition, disposition_reason)
                   VALUES (1, '\x{token}', {block}, '\x{hash}', 1800000000, '\x{tx}', {index}, 0,
                      '\x{sender}', '\x{recipient}', '{invoice}', '{amount}', '{disposition}', {reason})"#,
                token = hex_bytes(&issued.token_address),
                hash = "11".repeat(32),
                tx = format!("{:02x}", block).repeat(32),
                sender = "f3".repeat(20),
                recipient = hex_bytes(&issued.payment_address),
                invoice = issued.id,
            )
        };
        for statement in [
            observation(9, 0, "credited", "1500000"),
            observation(3, 1, "credited", "1000000"),
            observation(12, 0, "late", "5"),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query(
            "UPDATE invoices SET status = 'fulfilled', settlement_tx_hash = $2, resolved_at_block = 9, settled_at = now() WHERE id = $1",
        )
        .bind(issued.id)
        .bind([9u8; 32].as_slice())
        .execute(&pool)
        .await
        .unwrap();

        let repo = ProofRepository::new(pool.clone());
        let settlement = repo.settlement_transfers(issued.id).await.unwrap().unwrap();
        assert_eq!(settlement.status, "fulfilled");
        assert_eq!(settlement.settlement_tx_hash, Some(vec![9u8; 32]));
        assert_eq!(settlement.resolved_at_block, Some(9));
        assert!(settlement.settled_at.is_some());
        assert_eq!(settlement.payer_policy_mode, "permissionless");
        assert_eq!(
            settlement
                .transfers
                .iter()
                .map(|transfer| (transfer.block_number, transfer.amount.as_str()))
                .collect::<Vec<_>>(),
            [(3, "1000000"), (9, "1500000")]
        );
        assert!(
            settlement
                .transfers
                .iter()
                .all(|transfer| transfer.recipient_address == issued.payment_address)
        );
        assert!(
            repo.settlement_transfers(Uuid::now_v7())
                .await
                .unwrap()
                .is_none()
        );
    }

    fn hex_bytes(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
