use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    Amount, BeneficiaryAddress, ChainId, FactoryAddress, PaymentAddress, Salt, TokenAddress,
    generate_salt, predict_payment_address,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InvoiceId(pub Uuid);

pub fn generate_invoice_id() -> InvoiceId {
    InvoiceId(Uuid::now_v7())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InvoiceStatus {
    Created,
    Funded,
    Deploying,
    Fulfilled,
    Failed,
}

/// Error returned when parsing an [`InvoiceStatus`] from its string form.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("unknown invoice status: {0}")]
pub struct InvoiceStatusParseError(pub String);

impl InvoiceStatus {
    /// The canonical lowercase string form used on the wire and in the database.
    /// This is the single source of truth for the string mapping; [`Display`],
    /// [`FromStr`], and the DB CHECK constraint all agree with it.
    pub fn as_str(&self) -> &'static str {
        match self {
            InvoiceStatus::Created => "created",
            InvoiceStatus::Funded => "funded",
            InvoiceStatus::Deploying => "deploying",
            InvoiceStatus::Fulfilled => "fulfilled",
            InvoiceStatus::Failed => "failed",
        }
    }
}

impl fmt::Display for InvoiceStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for InvoiceStatus {
    type Err = InvoiceStatusParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "created" => Ok(InvoiceStatus::Created),
            "funded" => Ok(InvoiceStatus::Funded),
            "deploying" => Ok(InvoiceStatus::Deploying),
            "fulfilled" => Ok(InvoiceStatus::Fulfilled),
            "failed" => Ok(InvoiceStatus::Failed),
            other => Err(InvoiceStatusParseError(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invoice {
    pub id: InvoiceId,
    pub chain_id: ChainId,
    pub token: TokenAddress,
    pub beneficiary: BeneficiaryAddress,
    pub factory: FactoryAddress,
    pub amount: Amount,
    pub salt: Salt,
    pub payment_address: PaymentAddress,
    pub status: InvoiceStatus,
}

impl Invoice {
    pub fn new(
        factory: FactoryAddress,
        chain_id: ChainId,
        token: TokenAddress,
        beneficiary: BeneficiaryAddress,
        amount: Amount,
    ) -> Self {
        let salt = generate_salt();
        Invoice {
            id: generate_invoice_id(),
            chain_id,
            token,
            beneficiary,
            factory,
            amount,
            salt,
            payment_address: predict_payment_address(factory, token, amount, beneficiary, salt),
            status: InvoiceStatus::Created,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::*;
    use uuid::Version;

    #[test]
    fn two_ids_differ() {
        let a = generate_invoice_id();
        let b = generate_invoice_id();

        assert_ne!(a, b);
    }

    #[test]
    fn ids_are_version_7() {
        let id = generate_invoice_id();
        assert_eq!(id.0.get_version(), Some(Version::SortRand));
    }

    #[test]
    fn ids_are_chronologically_sortable() {
        // The whole point of UUIDv7 over v4: later timestamps sort after
        // earlier ones. Sleep a few ms to guarantee a timestamp boundary.
        let a = generate_invoice_id();
        thread::sleep(Duration::from_millis(5));
        let b = generate_invoice_id();

        assert!(a.0 < b.0, "earlier ID should sort before later ID");
    }

    use crate::{
        Amount, BeneficiaryAddress, ChainId, FactoryAddress, TokenAddress, predict_payment_address,
    };
    use alloy_primitives::{U256, address};

    fn sample_invoice() -> Invoice {
        Invoice::new(
            FactoryAddress(address!("0x0000000000000000000000000000000000000001")),
            ChainId(1),
            TokenAddress(address!("0xA0b86a91E6Dc7c5bE5d7B8f9cC2D9eF1a3B4c5D6")),
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01")),
            Amount(U256::from(100)),
        )
    }

    #[test]
    fn status_str_roundtrips_for_all_variants() {
        for status in [
            InvoiceStatus::Created,
            InvoiceStatus::Funded,
            InvoiceStatus::Deploying,
            InvoiceStatus::Fulfilled,
            InvoiceStatus::Failed,
        ] {
            let parsed = status.as_str().parse::<InvoiceStatus>().unwrap();
            assert_eq!(parsed, status);
            // Display and as_str agree.
            assert_eq!(status.to_string(), status.as_str());
        }
    }

    #[test]
    fn status_from_str_rejects_unknown() {
        let err = "bogus".parse::<InvoiceStatus>().unwrap_err();
        assert_eq!(err, InvoiceStatusParseError("bogus".to_string()));
    }

    #[test]
    fn new_invoice_has_created_status() {
        let invoice = sample_invoice();
        assert_eq!(invoice.status, InvoiceStatus::Created);
    }

    #[test]
    fn new_invoice_has_valid_uuidv7_id() {
        let invoice = sample_invoice();
        assert_eq!(invoice.id.0.get_version(), Some(Version::SortRand));
    }

    #[test]
    fn new_invoice_stores_all_input_fields() {
        let factory = FactoryAddress(address!("0x0000000000000000000000000000000000000001"));
        let chain_id = ChainId(1);
        let token = TokenAddress(address!("0xA0b86a91E6Dc7c5bE5d7B8f9cC2D9eF1a3B4c5D6"));
        let beneficiary =
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01"));
        let amount = Amount(U256::from(100));

        let invoice = Invoice::new(factory, chain_id, token, beneficiary, amount);

        assert_eq!(invoice.factory, factory);
        assert_eq!(invoice.chain_id, chain_id);
        assert_eq!(invoice.token, token);
        assert_eq!(invoice.beneficiary, beneficiary);
        assert_eq!(invoice.amount, amount);
    }

    #[test]
    fn new_invoice_payment_address_matches_predict() {
        // The address stored on the invoice must equal what predict_payment_address
        // returns for the same inputs — this is the integration test that ties
        // generate_salt, generate_invoice_id, and predict_payment_address together.
        let invoice = sample_invoice();

        let expected = predict_payment_address(
            invoice.factory,
            invoice.token,
            invoice.amount,
            invoice.beneficiary,
            invoice.salt,
        );

        assert_eq!(invoice.payment_address, expected);
    }

    #[test]
    fn two_invoices_with_same_inputs_get_different_addresses() {
        // Because the salt is random per invoice, identical economic parameters
        // must still produce different payment addresses. This verifies the salt
        // is actually unique per call and is wired into address derivation.
        let factory = FactoryAddress(address!("0x0000000000000000000000000000000000000001"));
        let chain_id = ChainId(1);
        let token = TokenAddress(address!("0xA0b86a91E6Dc7c5bE5d7B8f9cC2D9eF1a3B4c5D6"));
        let beneficiary =
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01"));
        let amount = Amount(U256::from(100));

        let a = Invoice::new(factory, chain_id, token, beneficiary, amount);
        let b = Invoice::new(factory, chain_id, token, beneficiary, amount);

        assert_ne!(a.salt, b.salt, "salts must differ");
        assert_ne!(a.id, b.id, "IDs must differ");
        assert_ne!(
            a.payment_address, b.payment_address,
            "addresses must differ"
        );
    }
}
