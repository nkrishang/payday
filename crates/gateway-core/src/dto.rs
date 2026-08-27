//! Wire DTOs shared between the HTTP server (`gatewayd`) and the CLI client
//! (`gateway-cli`).
//!
//! These are the single, authoritative definition of the invoice HTTP wire
//! shape. Both sides depend on this module, so the compiler enforces that the
//! server and client agree on field names and types — a renamed field or a new
//! status can no longer drift between them.
//!
//! DTOs are deliberately kept distinct from the domain [`Invoice`] and from
//! database rows: they are all-`String` on the wire, and the domain model is
//! projected onto them via the conversions below.

use alloy_primitives::utils::format_units;
use serde::{Deserialize, Serialize};

use crate::{Invoice, USDC_DECIMALS};

/// Parameters for `POST /v1/invoices`.
///
/// The server deserializes this from the request body; the CLI serializes it
/// into one — hence both `Serialize` and `Deserialize`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateInvoiceRequest {
    pub chain_id: String,
    pub token_address: String,
    pub beneficiary_address: String,
    pub amount: String,
    pub expiration_timestamp: String,
    pub recovery_address: String,
}

/// Full invoice payment instructions and settlement state returned by the API.
///
/// The server serializes this; the CLI deserializes it — hence both directions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvoiceResponse {
    pub id: String,
    pub chain_id: String,
    pub factory_address: String,
    pub token: TokenDto,
    pub beneficiary_address: String,
    pub expiration_timestamp: String,
    pub recovery_address: String,
    pub amount: String,
    pub amount_base_units: String,
    pub salt: String,
    /// Single-use deposit address: it is only valid while `status` is
    /// `created`. Anything sent after settlement is forwarded to
    /// `recovery_address`, never to the beneficiary.
    pub payment_address: String,
    pub status: String,
    /// Finalized USDC credited toward `amount` so far, human-readable.
    pub received: String,
    pub received_base_units: String,
    /// Transaction that deployed the payment contract, when this service sent it.
    pub execute_tx_hash: Option<String>,
    /// Block at which the invoice became `fulfilled` or `recovered`.
    pub resolved_at_block: Option<String>,
    /// Why automatic sweeping stopped, when it did.
    pub blocked_reason: Option<String>,
}

/// Token metadata nested inside [`InvoiceResponse`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenDto {
    pub address: String,
    pub kind: String,
    pub decimals: u8,
}

impl From<Invoice> for InvoiceResponse {
    fn from(inv: Invoice) -> Self {
        let decimals = USDC_DECIMALS;
        let human = |units| format_units(units, decimals).unwrap_or_default();

        InvoiceResponse {
            id: inv.id.0.to_string(),
            chain_id: inv.chain_id.0.to_string(),
            factory_address: inv.factory.0.to_checksum(None),
            token: TokenDto {
                address: inv.token.0.to_checksum(None),
                kind: "erc20".to_string(),
                decimals,
            },
            beneficiary_address: inv.beneficiary.0.to_checksum(None),
            expiration_timestamp: inv.expiration_timestamp.to_string(),
            recovery_address: inv.recovery.0.to_checksum(None),
            amount: human(inv.amount.0),
            amount_base_units: inv.amount.0.to_string(),
            salt: inv.salt.0.to_string(),
            payment_address: inv.payment_address.0.to_checksum(None),
            status: inv.status.as_str().to_string(),
            received: human(inv.received.0),
            received_base_units: inv.received.0.to_string(),
            execute_tx_hash: inv.execute_tx_hash.map(|hash| hash.to_string()),
            resolved_at_block: inv.resolved_at_block.map(|block| block.to_string()),
            blocked_reason: inv.blocked_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{B256, U256, address};

    use super::*;
    use crate::{
        Amount, BeneficiaryAddress, ChainId, FactoryAddress, InvoiceStatus, RecoveryAddress,
        TokenAddress,
    };

    #[test]
    fn response_projects_settlement_state_with_human_readable_amounts() {
        let mut invoice = Invoice::new(
            FactoryAddress(address!("0x0000000000000000000000000000000000000001")),
            ChainId(143),
            TokenAddress(address!("0x754704Bc059F8C67012fEd69BC8A327a5aafb603")),
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8")),
            Amount(U256::from(1_500_000u64)),
            1_900_000_000,
            RecoveryAddress(address!("0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC")),
        );
        invoice.status = InvoiceStatus::Fulfilled;
        invoice.received = Amount(U256::from(1_750_000u64));
        invoice.execute_tx_hash = Some(B256::repeat_byte(0xAB));
        invoice.resolved_at_block = Some(42);

        let response = InvoiceResponse::from(invoice);
        assert_eq!(response.amount, "1.500000");
        assert_eq!(response.amount_base_units, "1500000");
        assert_eq!(response.received, "1.750000");
        assert_eq!(response.received_base_units, "1750000");
        assert_eq!(response.status, "fulfilled");
        assert_eq!(
            response.execute_tx_hash.as_deref(),
            Some("0xabababababababababababababababababababababababababababababababab")
        );
        assert_eq!(response.resolved_at_block.as_deref(), Some("42"));
        assert_eq!(response.blocked_reason, None);
        assert_eq!(response.chain_id, "143");
        assert_eq!(response.token.decimals, 6);
    }

    #[test]
    fn optional_fields_serialize_as_null_before_settlement() {
        let invoice = Invoice::new(
            FactoryAddress(address!("0x0000000000000000000000000000000000000001")),
            ChainId(1),
            TokenAddress(address!("0x0000000000000000000000000000000000000002")),
            BeneficiaryAddress(address!("0x0000000000000000000000000000000000000003")),
            Amount(U256::from(1u64)),
            1_900_000_000,
            RecoveryAddress(address!("0x0000000000000000000000000000000000000004")),
        );
        let json = serde_json::to_value(InvoiceResponse::from(invoice)).unwrap();
        assert_eq!(json["status"], "created");
        assert_eq!(json["received_base_units"], "0");
        assert!(json["execute_tx_hash"].is_null());
        assert!(json["resolved_at_block"].is_null());
        assert!(json["blocked_reason"].is_null());
    }
}
