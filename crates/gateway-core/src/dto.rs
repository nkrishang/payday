//! Customer-facing HTTP types. Internal invoice terminology stops at this seam.

use alloy_primitives::utils::format_units;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::{Invoice, InvoiceStatus, USDC_DECIMALS};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePaymentRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_address: Option<String>,
    pub payout_address: String,
    pub amount: String,
    /// Lifetime in seconds. Idempotent retries retain the original deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refund_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default = "empty_metadata")]
    pub metadata: serde_json::Value,
}

fn empty_metadata() -> serde_json::Value {
    serde_json::json!({})
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferDto {
    pub timestamp: String,
    pub amount: String,
    pub amount_base_units: String,
    pub sender: String,
    pub transaction_hash: String,
    pub explorer_url: Option<String>,
    pub block: String,
    pub disposition: String,
    pub collected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexerFreshnessDto {
    pub last_indexed_block: Option<String>,
    pub last_finalized_block: Option<String>,
    pub cursor_updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AsOfDto {
    pub block: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentResponse {
    pub id: String,
    /// Shareable, payment-scoped page for the payer.
    pub payment_url: String,
    pub address: String,
    pub address_explorer_url: Option<String>,
    pub payout_address: String,
    pub refund_address: String,
    pub expires_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<u64>,
    pub amount: String,
    pub amount_base_units: String,
    pub received: String,
    pub received_base_units: String,
    pub remaining: String,
    pub remaining_base_units: String,
    pub currency: String,
    pub fee_amount: String,
    pub fee_amount_base_units: String,
    pub net_amount: String,
    pub net_amount_base_units: String,
    pub status: PaymentStatus,
    pub token: TokenDto,
    pub chain: ChainDto,
    pub settlement_tx_hash: Option<String>,
    pub settlement_explorer_url: Option<String>,
    pub settled_at: Option<String>,
    pub settled_block: Option<String>,
    pub self_settlement: SelfSettlementDto,
    pub attention: Option<AttentionDto>,
    pub memo: Option<String>,
    pub reference: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
    pub paid_at: Option<String>,
    pub paid_at_block: Option<String>,
    pub expired_at: Option<String>,
    pub cancellation_requested_at: Option<String>,
    pub transfers: Vec<TransferDto>,
    pub indexer_freshness: IndexerFreshnessDto,
    pub as_of: Option<AsOfDto>,
}

/// Payment instructions and finalized status safe to expose to one payer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayerPaymentResponse {
    pub id: String,
    pub chain: ChainDto,
    pub token: TokenDto,
    pub amount: String,
    pub amount_base_units: String,
    pub received: String,
    pub received_base_units: String,
    pub remaining: String,
    pub remaining_base_units: String,
    pub address: String,
    pub expires_at: String,
    /// Gateway wall-clock time used by clients to render the deadline without
    /// trusting the payer device's clock.
    pub server_timestamp: String,
    pub status: PaymentStatus,
    /// Whether the gateway still considers this address payable.
    pub payable: bool,
    /// EIP-681 request for the amount still due, absent after the deadline or
    /// after the payment has left the payable state.
    pub payment_uri: Option<String>,
    pub address_explorer_url: Option<String>,
    pub settlement_tx_hash: Option<String>,
    pub settlement_explorer_url: Option<String>,
    /// Safety guidance shown only when payout needs operator attention.
    pub payer_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentSummaryResponse {
    pub id: String,
    pub memo: Option<String>,
    pub reference: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: String,
    pub status: PaymentStatus,
    pub amount: String,
    pub received: String,
    pub cancellation_requested_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentListResponse {
    pub payments: Vec<PaymentSummaryResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelPaymentResponse {
    pub payment: PaymentResponse,
    pub advisory: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentStatus {
    AwaitingPayment,
    PartiallyPaid,
    Paid,
    Settled,
    Expired,
    Returned,
    NeedsAttention,
}
impl PaymentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingPayment => "awaiting_payment",
            Self::PartiallyPaid => "partially_paid",
            Self::Paid => "paid",
            Self::Settled => "settled",
            Self::Expired => "expired",
            Self::Returned => "returned",
            Self::NeedsAttention => "needs_attention",
        }
    }
}

impl std::str::FromStr for PaymentStatus {
    type Err = ();
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        [
            Self::AwaitingPayment,
            Self::PartiallyPaid,
            Self::Paid,
            Self::Settled,
            Self::Expired,
            Self::Returned,
            Self::NeedsAttention,
        ]
        .into_iter()
        .find(|status| status.as_str() == value)
        .ok_or(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenDto {
    pub symbol: String,
    pub address: String,
    pub decimals: u8,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainDto {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfSettlementDto {
    pub factory: String,
    pub salt: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionDto {
    pub code: String,
    pub message: String,
    pub action: String,
}

impl PaymentResponse {
    pub fn from_invoice(inv: Invoice, expires_in: Option<u64>) -> Self {
        let human = |units| format_units(units, USDC_DECIMALS).unwrap_or_default();
        let remaining = inv.amount.0.saturating_sub(inv.received.0);
        let status = payment_status(&inv);
        let attention = inv.blocked_reason.as_deref().map(attention);
        Self {
            id: inv.id.to_string(),
            payment_url: String::new(),
            address: inv.payment_address.0.to_checksum(None),
            address_explorer_url: None,
            payout_address: inv.beneficiary.0.to_checksum(None),
            refund_address: inv.recovery.0.to_checksum(None),
            expires_at: DateTime::<Utc>::from_timestamp(inv.expiration_timestamp as i64, 0)
                .expect("validated timestamp")
                .to_rfc3339_opts(SecondsFormat::Secs, true),
            expires_in,
            amount: human(inv.amount.0),
            amount_base_units: inv.amount.0.to_string(),
            received: human(inv.received.0),
            received_base_units: inv.received.0.to_string(),
            remaining: human(remaining),
            remaining_base_units: remaining.to_string(),
            currency: "USDC".into(),
            fee_amount: "0".into(),
            fee_amount_base_units: "0".into(),
            net_amount: human(inv.amount.0),
            net_amount_base_units: inv.amount.0.to_string(),
            status,
            token: TokenDto {
                symbol: "USDC".into(),
                address: inv.token.0.to_checksum(None),
                decimals: USDC_DECIMALS,
            },
            chain: ChainDto {
                id: inv.chain_id.0.to_string(),
                name: chain_name(inv.chain_id.0).into(),
            },
            settlement_tx_hash: inv.execute_tx_hash.map(|h| h.to_string()),
            settlement_explorer_url: None,
            settled_at: inv.settled_at_timestamp.and_then(|t| {
                DateTime::<Utc>::from_timestamp(t as i64, 0)
                    .map(|d| d.to_rfc3339_opts(SecondsFormat::Secs, true))
            }),
            settled_block: inv.resolved_at_block.map(|b| b.to_string()),
            self_settlement: SelfSettlementDto {
                factory: inv.factory.0.to_checksum(None),
                salt: inv.salt.0.to_string(),
            },
            attention,
            memo: None,
            reference: None,
            metadata: serde_json::json!({}),
            created_at: String::new(),
            updated_at: String::new(),
            paid_at: None,
            paid_at_block: None,
            expired_at: None,
            cancellation_requested_at: inv.cancellation_requested_at,
            transfers: Vec::new(),
            indexer_freshness: IndexerFreshnessDto {
                last_indexed_block: None,
                last_finalized_block: None,
                cursor_updated_at: None,
            },
            as_of: None,
        }
    }
}

fn payment_status(inv: &Invoice) -> PaymentStatus {
    if inv.blocked_reason.is_some() || inv.status == InvoiceStatus::Blocked {
        return PaymentStatus::NeedsAttention;
    }
    match inv.status {
        InvoiceStatus::Created if inv.received.0.is_zero() => PaymentStatus::AwaitingPayment,
        InvoiceStatus::Created => PaymentStatus::PartiallyPaid,
        InvoiceStatus::Funded | InvoiceStatus::Deploying => PaymentStatus::Paid,
        InvoiceStatus::Fulfilled => PaymentStatus::Settled,
        InvoiceStatus::Expired => PaymentStatus::Expired,
        InvoiceStatus::Recovered => PaymentStatus::Returned,
        InvoiceStatus::Blocked => PaymentStatus::NeedsAttention,
    }
}
fn chain_name(id: u64) -> &'static str {
    match id {
        1 => "Ethereum",
        143 => "Monad",
        10_143 => "Monad Testnet",
        31_337 => "Local",
        _ => "Unknown",
    }
}
fn attention(code: &str) -> AttentionDto {
    let (message, action) = match code {
        "beneficiary_blacklisted" => (
            "Circle has blacklisted the payout address.",
            "Contact support to provide a compliant payout address.",
        ),
        "recovery_blacklisted" => (
            "Circle has blacklisted the refund address.",
            "Resolve the blacklist with Circle, then contact support to retry.",
        ),
        "payment_address_blacklisted" => (
            "Circle has blacklisted the payment address.",
            "Contact support; do not send additional funds.",
        ),
        "balance_below_amount" => (
            "The payment address balance is lower than the confirmed amount.",
            "Contact support so Payday can investigate safely.",
        ),
        _ => (
            "Automatic settlement has paused.",
            "Funds remain safe at the payment address; contact support.",
        ),
    };
    AttentionDto {
        code: code.into(),
        message: message.into(),
        action: action.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Amount, BeneficiaryAddress, ChainId, FactoryAddress, RecoveryAddress, TokenAddress,
    };
    use alloy_primitives::{U256, address};

    fn payment(status: InvoiceStatus, received: u64, blocked: Option<&str>) -> PaymentResponse {
        let mut invoice = Invoice::new(
            FactoryAddress(address!("0000000000000000000000000000000000000001")),
            ChainId(143),
            TokenAddress(address!("754704Bc059F8C67012fEd69BC8A327a5aafb603")),
            BeneficiaryAddress(address!("70997970C51812dc3A010C7d01b50e0d17dc79C8")),
            Amount(U256::from(1_000_000)),
            1_900_000_000,
            RecoveryAddress(address!("3C44CdDdB6a900fa2b585dd299e03d12FA4293BC")),
        );
        invoice.status = status;
        invoice.received = Amount(U256::from(received));
        invoice.blocked_reason = blocked.map(str::to_owned);
        PaymentResponse::from_invoice(invoice, None)
    }

    #[test]
    fn projects_every_customer_status_and_blocking_takes_precedence() {
        for (internal, received, expected) in [
            (InvoiceStatus::Created, 0, PaymentStatus::AwaitingPayment),
            (InvoiceStatus::Created, 1, PaymentStatus::PartiallyPaid),
            (InvoiceStatus::Funded, 1_000_000, PaymentStatus::Paid),
            (InvoiceStatus::Deploying, 1_000_000, PaymentStatus::Paid),
            (InvoiceStatus::Fulfilled, 1_000_000, PaymentStatus::Settled),
            (InvoiceStatus::Expired, 400_000, PaymentStatus::Expired),
            (InvoiceStatus::Recovered, 400_000, PaymentStatus::Returned),
            (InvoiceStatus::Blocked, 1, PaymentStatus::NeedsAttention),
        ] {
            assert_eq!(payment(internal, received, None).status, expected);
        }
        let blocked = payment(
            InvoiceStatus::Fulfilled,
            1_000_000,
            Some("beneficiary_blacklisted"),
        );
        assert_eq!(blocked.status, PaymentStatus::NeedsAttention);
        assert!(
            blocked
                .attention
                .unwrap()
                .action
                .contains("compliant payout address")
        );
    }

    #[test]
    fn wire_shape_keeps_customer_vocabulary_and_list_summaries_bounded() {
        let payment = payment(InvoiceStatus::Created, 0, None);
        let json = serde_json::to_value(&payment).unwrap();
        assert!(json["id"].as_str().unwrap().starts_with("pay_"));
        assert_eq!(json["status"], "awaiting_payment");
        assert_eq!(json["currency"], "USDC");
        assert!(json.get("beneficiary_address").is_none());

        let summary = PaymentSummaryResponse {
            id: payment.id,
            memo: None,
            reference: None,
            metadata: serde_json::json!({}),
            created_at: String::new(),
            status: payment.status,
            amount: payment.amount,
            received: payment.received,
            cancellation_requested_at: None,
        };
        let summary = serde_json::to_value(summary).unwrap();
        assert!(summary.get("transfers").is_none());
        assert!(summary.get("self_settlement").is_none());
    }
}
