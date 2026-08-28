use std::fmt;
use std::str::FromStr;

use alloy_primitives::{B256, U256};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    Amount, BeneficiaryAddress, ChainId, FactoryAddress, PaymentAddress, RecoveryAddress, Salt,
    TokenAddress, generate_salt, predict_payment_address,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InvoiceId(pub Uuid);

impl fmt::Display for InvoiceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pay_{}", self.0)
    }
}

/// Validate a customer-facing `pay_…` ID or unambiguous prefix and return the
/// UUID portion used by the account-scoped database lookup.
pub fn payment_id_prefix(value: &str) -> Option<&str> {
    let suffix = value.strip_prefix("pay_")?;
    if suffix.is_empty() || suffix.len() > 36 {
        return None;
    }
    let canonical = "00000000-0000-0000-0000-000000000000".as_bytes();
    suffix
        .bytes()
        .zip(canonical)
        .all(|(byte, template)| {
            if *template == b'-' {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
        .then_some(suffix)
}

pub fn generate_invoice_id() -> InvoiceId {
    InvoiceId(Uuid::now_v7())
}

/// Invoice lifecycle.
///
/// ```text
/// created ──(finalized credit ≥ amount)──▶ funded ──(claimed)──▶ deploying ──▶ fulfilled
///    │                                       │                        │
///    └──(chain time > expiration)──▶ expired ◀┘                       └──▶ recovered
///                                       │                                    ▲
///                                       └──(balance swept after expiry)──────┘
/// ```
///
/// `blocked` is reached from `deploying` or `expired` when a sweep fails for
/// a reason the worker classifies as permanent. Funds arriving after
/// `fulfilled`/`recovered` are forwarded to the recovery address without
/// changing the status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InvoiceStatus {
    Created,
    Funded,
    Deploying,
    Fulfilled,
    Recovered,
    Expired,
    Blocked,
}

/// Error returned when parsing an [`InvoiceStatus`] from its string form.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("unknown invoice status: {0}")]
pub struct InvoiceStatusParseError(pub String);

impl InvoiceStatus {
    pub const ALL: [InvoiceStatus; 7] = [
        InvoiceStatus::Created,
        InvoiceStatus::Funded,
        InvoiceStatus::Deploying,
        InvoiceStatus::Fulfilled,
        InvoiceStatus::Recovered,
        InvoiceStatus::Expired,
        InvoiceStatus::Blocked,
    ];

    /// The canonical lowercase string form used on the wire and in the database.
    /// This is the single source of truth for the string mapping; [`Display`],
    /// [`FromStr`], and the DB CHECK constraint all agree with it.
    pub fn as_str(&self) -> &'static str {
        match self {
            InvoiceStatus::Created => "created",
            InvoiceStatus::Funded => "funded",
            InvoiceStatus::Deploying => "deploying",
            InvoiceStatus::Fulfilled => "fulfilled",
            InvoiceStatus::Recovered => "recovered",
            InvoiceStatus::Expired => "expired",
            InvoiceStatus::Blocked => "blocked",
        }
    }

    /// Whether the invoice's own payment has reached its final destination.
    /// Late transfers can still be collected from a terminal invoice.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            InvoiceStatus::Fulfilled | InvoiceStatus::Recovered | InvoiceStatus::Blocked
        )
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
        InvoiceStatus::ALL
            .into_iter()
            .find(|status| status.as_str() == s)
            .ok_or_else(|| InvoiceStatusParseError(s.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invoice {
    pub id: InvoiceId,
    pub chain_id: ChainId,
    pub token: TokenAddress,
    pub beneficiary: BeneficiaryAddress,
    pub expiration_timestamp: u64,
    pub recovery: RecoveryAddress,
    pub factory: FactoryAddress,
    pub amount: Amount,
    pub salt: Salt,
    pub payment_address: PaymentAddress,
    pub status: InvoiceStatus,
    /// Finalized transfers credited toward `amount`, in base units.
    pub received: Amount,
    /// Transaction that deployed the `Payment` contract, when this service sent it.
    pub execute_tx_hash: Option<B256>,
    /// Block at which the invoice reached `fulfilled` or `recovered`.
    pub resolved_at_block: Option<u64>,
    /// Timestamp of that finalized block.
    pub settled_at_timestamp: Option<u64>,
    /// Why automatic sweeping stopped for this invoice, if it did.
    pub blocked_reason: Option<String>,
    /// When the customer asked clients to stop using this invoice. This is
    /// advisory metadata and does not alter the immutable payment contract.
    pub cancellation_requested_at: Option<String>,
}

impl Invoice {
    pub fn new(
        factory: FactoryAddress,
        chain_id: ChainId,
        token: TokenAddress,
        beneficiary: BeneficiaryAddress,
        amount: Amount,
        expiration_timestamp: u64,
        recovery: RecoveryAddress,
    ) -> Self {
        let salt = generate_salt();
        Invoice {
            id: generate_invoice_id(),
            chain_id,
            token,
            beneficiary,
            expiration_timestamp,
            recovery,
            factory,
            amount,
            salt,
            payment_address: predict_payment_address(
                factory,
                token,
                amount,
                beneficiary,
                expiration_timestamp,
                recovery,
                salt,
            ),
            status: InvoiceStatus::Created,
            received: Amount(U256::ZERO),
            execute_tx_hash: None,
            resolved_at_block: None,
            settled_at_timestamp: None,
            blocked_reason: None,
            cancellation_requested_at: None,
        }
    }

    /// Recompute the counterfactual address from the stored parameters. A
    /// mismatch means the row no longer describes the address payers were
    /// given and must never be swept.
    pub fn address_matches_parameters(&self) -> bool {
        predict_payment_address(
            self.factory,
            self.token,
            self.amount,
            self.beneficiary,
            self.expiration_timestamp,
            self.recovery,
            self.salt,
        ) == self.payment_address
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::*;

    #[test]
    fn public_payment_ids_are_prefixed_and_validate_canonical_prefixes() {
        let id = InvoiceId(Uuid::parse_str("0198f80c-8d2f-7dc1-a369-90556a64f700").unwrap());
        assert_eq!(id.to_string(), "pay_0198f80c-8d2f-7dc1-a369-90556a64f700");
        assert_eq!(
            payment_id_prefix("pay_0198f80c-8d2f"),
            Some("0198f80c-8d2f")
        );
        for invalid in ["", "pay_", "0198f80c", "pay_0198F", "pay_0198-", "pay_%"] {
            assert_eq!(payment_id_prefix(invalid), None, "{invalid}");
        }
    }
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
        Amount, BeneficiaryAddress, ChainId, FactoryAddress, RecoveryAddress, TokenAddress,
        predict_payment_address,
    };
    use alloy_primitives::{U256, address};

    fn sample_invoice() -> Invoice {
        Invoice::new(
            FactoryAddress(address!("0x0000000000000000000000000000000000000001")),
            ChainId(1),
            TokenAddress(address!("0xA0b86a91E6Dc7c5bE5d7B8f9cC2D9eF1a3B4c5D6")),
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01")),
            Amount(U256::from(100)),
            1_900_000_000,
            RecoveryAddress(address!("0x0000000000000000000000000000000000000002")),
        )
    }

    #[test]
    fn status_str_roundtrips_for_all_variants() {
        for status in InvoiceStatus::ALL {
            let parsed = status.as_str().parse::<InvoiceStatus>().unwrap();
            assert_eq!(parsed, status);
            // Display and as_str agree.
            assert_eq!(status.to_string(), status.as_str());
        }
    }

    #[test]
    fn status_from_str_rejects_unknown_and_retired_values() {
        for retired in ["bogus", "failed", "FULFILLED"] {
            assert_eq!(
                retired.parse::<InvoiceStatus>().unwrap_err(),
                InvoiceStatusParseError(retired.to_string())
            );
        }
    }

    #[test]
    fn terminal_statuses_are_exactly_the_settled_and_given_up_states() {
        let terminal: Vec<_> = InvoiceStatus::ALL
            .into_iter()
            .filter(InvoiceStatus::is_terminal)
            .collect();
        assert_eq!(
            terminal,
            [
                InvoiceStatus::Fulfilled,
                InvoiceStatus::Recovered,
                InvoiceStatus::Blocked
            ]
        );
    }

    #[test]
    fn new_invoice_has_created_status_and_no_operational_state() {
        let invoice = sample_invoice();
        assert_eq!(invoice.status, InvoiceStatus::Created);
        assert_eq!(invoice.received, Amount(U256::ZERO));
        assert_eq!(invoice.execute_tx_hash, None);
        assert_eq!(invoice.resolved_at_block, None);
        assert_eq!(invoice.blocked_reason, None);
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
        let expiration_timestamp = 1_900_000_000;
        let recovery = RecoveryAddress(address!("0x0000000000000000000000000000000000000002"));

        let invoice = Invoice::new(
            factory,
            chain_id,
            token,
            beneficiary,
            amount,
            expiration_timestamp,
            recovery,
        );

        assert_eq!(invoice.factory, factory);
        assert_eq!(invoice.chain_id, chain_id);
        assert_eq!(invoice.token, token);
        assert_eq!(invoice.beneficiary, beneficiary);
        assert_eq!(invoice.amount, amount);
        assert_eq!(invoice.expiration_timestamp, expiration_timestamp);
        assert_eq!(invoice.recovery, recovery);
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
            invoice.expiration_timestamp,
            invoice.recovery,
            invoice.salt,
        );

        assert_eq!(invoice.payment_address, expected);
        assert!(invoice.address_matches_parameters());
    }

    #[test]
    fn tampered_parameters_no_longer_match_the_stored_address() {
        let mut invoice = sample_invoice();
        invoice.beneficiary =
            BeneficiaryAddress(address!("0x0000000000000000000000000000000000000009"));
        assert!(!invoice.address_matches_parameters());
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
        let expiration_timestamp = 1_900_000_000;
        let recovery = RecoveryAddress(address!("0x0000000000000000000000000000000000000002"));

        let a = Invoice::new(
            factory,
            chain_id,
            token,
            beneficiary,
            amount,
            expiration_timestamp,
            recovery,
        );
        let b = Invoice::new(
            factory,
            chain_id,
            token,
            beneficiary,
            amount,
            expiration_timestamp,
            recovery,
        );

        assert_ne!(a.salt, b.salt, "salts must differ");
        assert_ne!(a.id, b.id, "IDs must differ");
        assert_ne!(
            a.payment_address, b.payment_address,
            "addresses must differ"
        );
    }

    #[test]
    fn expiration_and_recovery_are_address_parameters() {
        let invoice = sample_invoice();
        let later_expiration = predict_payment_address(
            invoice.factory,
            invoice.token,
            invoice.amount,
            invoice.beneficiary,
            invoice.expiration_timestamp + 1,
            invoice.recovery,
            invoice.salt,
        );
        let other_recovery = predict_payment_address(
            invoice.factory,
            invoice.token,
            invoice.amount,
            invoice.beneficiary,
            invoice.expiration_timestamp,
            RecoveryAddress(address!("0x0000000000000000000000000000000000000003")),
            invoice.salt,
        );

        assert_ne!(invoice.payment_address, later_expiration);
        assert_ne!(invoice.payment_address, other_recovery);
    }
}
