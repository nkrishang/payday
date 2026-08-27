//! Customer-facing HTTP types. Internal invoice terminology stops at this seam.

use alloy_primitives::utils::format_units;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::{Invoice, InvoiceStatus, USDC_DECIMALS};

/// Parameters for `POST /v1/payments`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePaymentRequest {
    pub chain_id: String,
    pub token_address: String,
    pub payout_address: String,
    pub amount: String,
    /// Lifetime in seconds. Replays with the same idempotency key retain the
    /// deadline established by the first request.
    pub expires_in: u64,
    pub refund_address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentResponse {
    pub id: String,
    pub address: String,
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
    pub status: PaymentStatus,
    pub token: TokenDto,
    pub chain: ChainDto,
    pub settlement_tx_hash: Option<String>,
    /// Timestamp of the finalized settlement block.
    pub settled_at: Option<String>,
    pub settled_block: Option<String>,
    pub self_settlement: SelfSettlementDto,
    pub attention: Option<AttentionDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentListResponse {
    pub payments: Vec<PaymentResponse>,
    pub next_cursor: Option<String>,
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
        let status = if inv.blocked_reason.is_some() || inv.status == InvoiceStatus::Blocked {
            PaymentStatus::NeedsAttention
        } else {
            match inv.status {
                InvoiceStatus::Created if inv.received.0.is_zero() => {
                    PaymentStatus::AwaitingPayment
                }
                InvoiceStatus::Created => PaymentStatus::PartiallyPaid,
                InvoiceStatus::Funded | InvoiceStatus::Deploying => PaymentStatus::Paid,
                InvoiceStatus::Fulfilled => PaymentStatus::Settled,
                InvoiceStatus::Expired => PaymentStatus::Expired,
                InvoiceStatus::Recovered => PaymentStatus::Returned,
                InvoiceStatus::Blocked => PaymentStatus::NeedsAttention,
            }
        };
        let attention = inv.blocked_reason.as_deref().map(attention);
        let expires_at = DateTime::<Utc>::from_timestamp(inv.expiration_timestamp as i64, 0)
            .expect("validated payment timestamp")
            .to_rfc3339_opts(SecondsFormat::Secs, true);

        Self {
            id: inv.id.to_string(),
            address: inv.payment_address.0.to_checksum(None),
            payout_address: inv.beneficiary.0.to_checksum(None),
            refund_address: inv.recovery.0.to_checksum(None),
            expires_at,
            expires_in,
            amount: human(inv.amount.0),
            amount_base_units: inv.amount.0.to_string(),
            received: human(inv.received.0),
            received_base_units: inv.received.0.to_string(),
            remaining: human(remaining),
            remaining_base_units: remaining.to_string(),
            currency: "USDC".into(),
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
            settlement_tx_hash: inv.execute_tx_hash.map(|hash| hash.to_string()),
            settled_at: inv.settled_at_timestamp.and_then(|timestamp| {
                DateTime::<Utc>::from_timestamp(timestamp as i64, 0)
                    .map(|time| time.to_rfc3339_opts(SecondsFormat::Secs, true))
            }),
            settled_block: inv.resolved_at_block.map(|block| block.to_string()),
            self_settlement: SelfSettlementDto {
                factory: inv.factory.0.to_checksum(None),
                salt: inv.salt.0.to_string(),
            },
            attention,
        }
    }
}

fn chain_name(id: u64) -> &'static str {
    match id {
        1 => "Ethereum",
        143 => "Monad",
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
        assert_eq!(
            payment(
                InvoiceStatus::Fulfilled,
                1_000_000,
                Some("retries_exhausted")
            )
            .status,
            PaymentStatus::NeedsAttention
        );
    }

    #[test]
    fn wire_shape_uses_merchant_vocabulary() {
        let json = serde_json::to_value(payment(InvoiceStatus::Created, 0, None)).unwrap();
        assert!(json["id"].as_str().unwrap().starts_with("pay_"));
        assert_eq!(json["status"], "awaiting_payment");
        assert_eq!(json["currency"], "USDC");
        assert_eq!(json["chain"]["name"], "Monad");
        assert!(json.get("beneficiary_address").is_none());
        assert!(json.get("expires_in").is_none());
    }
}
