use serde::{Deserialize, Serialize};

mod salt;
pub use salt::*;

mod amount;
pub use amount::*;

mod invoice_id;
pub use invoice_id::*;

mod deterministic_address;
pub use deterministic_address::*;

mod address;
pub use address::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChainId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InvoiceStatus {
    Created,
    Funded,
    Deploying,
    Fulfilled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invoice {
    pub id: InvoiceId,
    pub chain_id: ChainId,
    pub token: TokenAddress,
    pub beneficiary: Beneficiary,
    pub factory: FactoryAddress,
    pub amount: Amount,
    pub salt: Salt,
    pub payment_address: PaymentAddress,
    pub status: InvoiceStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserInvoice {
    pub id: InvoiceId,
    pub chain_id: ChainId,
    pub token: TokenAddress,
    pub beneficiary: Beneficiary,
    pub amount: Amount,
    pub payment_address: PaymentAddress,
    pub status: InvoiceStatus,
}
