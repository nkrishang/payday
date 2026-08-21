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

use crate::{Invoice, NATIVE_TOKEN_DECIMALS};

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
}

/// Full invoice payment instructions returned by the API.
///
/// The server serializes this; the CLI deserializes it — hence both directions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvoiceResponse {
    pub id: String,
    pub chain_id: String,
    pub factory_address: String,
    pub token: TokenDto,
    pub beneficiary_address: String,
    pub amount: String,
    pub amount_base_units: String,
    pub salt: String,
    pub payment_address: String,
    pub status: String,
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
        // Milestone is native-token-only, so decimals are the native default.
        // Correcting this for ERC-20s (using the invoice's real token decimals)
        // is tracked separately.
        let decimals = NATIVE_TOKEN_DECIMALS;
        let amount_human = format_units(inv.amount.0, decimals).unwrap_or_default();
        let kind = if inv.token.is_native() {
            "native"
        } else {
            "erc20"
        };

        InvoiceResponse {
            id: inv.id.0.to_string(),
            chain_id: inv.chain_id.0.to_string(),
            factory_address: inv.factory.0.to_checksum(None),
            token: TokenDto {
                address: inv.token.0.to_checksum(None),
                kind: kind.to_string(),
                decimals,
            },
            beneficiary_address: inv.beneficiary.0.to_checksum(None),
            amount: amount_human,
            amount_base_units: inv.amount.0.to_string(),
            salt: inv.salt.0.to_string(),
            payment_address: inv.payment_address.0.to_checksum(None),
            status: inv.status.as_str().to_string(),
        }
    }
}
