//! Database row type and repository for invoices.
//!
//! All conversions between domain types and DB column types happen here.
//! Handlers and application logic never see SQL or DB rows.

use alloy_primitives::{Address, B256, U256};
use sqlx::PgPool;
use uuid::Uuid;

use gateway_core::{
    Amount, BeneficiaryAddress, ChainId, FactoryAddress, InvoiceId, PaymentAddress, Salt,
    TokenAddress,
};

/// Database row representing one invoice.
#[allow(dead_code)] // some fields not used in responses yet
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DbInvoice {
    pub id: Uuid,
    pub idempotency_key: String,
    pub chain_id: i64,
    pub factory_address: Vec<u8>,
    pub token_address: Vec<u8>,
    pub token_decimals: i16,
    pub beneficiary_address: Vec<u8>,
    pub amount: String,
    pub salt: Vec<u8>,
    pub payment_address: Vec<u8>,
    pub status: String,
    pub created_at: sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,
    pub updated_at: sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,
}

impl DbInvoice {
    /// Convert DB row to domain types.
    pub fn to_domain(&self) -> gateway_core::Invoice {
        gateway_core::Invoice {
            id: InvoiceId(self.id),
            chain_id: ChainId(self.chain_id as u64),
            token: TokenAddress(Address::from_slice(&self.token_address)),
            beneficiary: BeneficiaryAddress(Address::from_slice(&self.beneficiary_address)),
            factory: FactoryAddress(Address::from_slice(&self.factory_address)),
            amount: Amount(U256::from_str_radix(&self.amount, 10).expect("invalid amount in DB")),
            salt: Salt(B256::from_slice(&self.salt)),
            payment_address: PaymentAddress(Address::from_slice(&self.payment_address)),
            status: match self.status.as_str() {
                "created" => gateway_core::InvoiceStatus::Created,
                "funded" => gateway_core::InvoiceStatus::Funded,
                "deploying" => gateway_core::InvoiceStatus::Deploying,
                "fulfilled" => gateway_core::InvoiceStatus::Fulfilled,
                "failed" => gateway_core::InvoiceStatus::Failed,
                other => panic!("unknown invoice status in DB: {other}"),
            },
        }
    }
}

#[derive(Clone)]
pub struct InvoiceRepository {
    pool: PgPool,
}

/// Input for creating an invoice in the DB.
pub struct CreateInvoiceInput {
    pub id: Uuid,
    pub idempotency_key: String,
    pub chain_id: u64,
    pub factory_address: [u8; 20],
    pub token_address: [u8; 20],
    pub token_decimals: u8,
    pub beneficiary_address: [u8; 20],
    pub amount: String,
    pub salt: [u8; 32],
    pub payment_address: [u8; 20],
}

impl InvoiceRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert a new invoice. Returns the row if inserted, or None if a row with
    /// the same idempotency_key already exists (caller must handle the conflict).
    pub async fn insert(
        &self,
        input: &CreateInvoiceInput,
    ) -> Result<Option<DbInvoice>, sqlx::Error> {
        let row = sqlx::query_as::<_, DbInvoice>(
            r#"
            INSERT INTO invoices
                (id, idempotency_key, chain_id, factory_address, token_address,
                 token_decimals, beneficiary_address, amount, salt, payment_address, status)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'created')
            ON CONFLICT (idempotency_key) DO NOTHING
            RETURNING *
            "#,
        )
        .bind(input.id)
        .bind(&input.idempotency_key)
        .bind(input.chain_id as i64)
        .bind(&input.factory_address)
        .bind(&input.token_address)
        .bind(input.token_decimals as i16)
        .bind(&input.beneficiary_address)
        .bind(&input.amount)
        .bind(&input.salt)
        .bind(&input.payment_address)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row)
    }

    /// Fetch an existing invoice by its idempotency key.
    pub async fn find_by_idempotency_key(
        &self,
        key: &str,
    ) -> Result<Option<DbInvoice>, sqlx::Error> {
        sqlx::query_as::<_, DbInvoice>(r#"SELECT * FROM invoices WHERE idempotency_key = $1"#)
            .bind(key)
            .fetch_optional(&self.pool)
            .await
    }

    /// Fetch an invoice by its ID.
    pub async fn find_by_id(&self, id: Uuid) -> Result<Option<DbInvoice>, sqlx::Error> {
        sqlx::query_as::<_, DbInvoice>(r#"SELECT * FROM invoices WHERE id = $1"#)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }
}
