use std::io::IsTerminal;

use anstyle::{AnsiColor, Color, Style};
use chrono::{DateTime, Utc};
use gateway_core::{PaymentResponse, PaymentStatus, PaymentSummaryResponse};
use supports_hyperlinks::Stream;

use crate::cli::ColorChoice;

pub struct Presentation {
    color: bool,
    verbose: bool,
}

impl Presentation {
    pub fn new(choice: ColorChoice, plain: bool, verbose: bool) -> Self {
        let color = !plain
            && match choice {
                ColorChoice::Always => true,
                ColorChoice::Never => false,
                ColorChoice::Auto => {
                    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
                }
            };
        Self { color, verbose }
    }

    pub fn payment(&self, payment: &PaymentResponse, created: bool) -> String {
        let (label, sentence) = status_text(payment);
        let mark = self.paint("●", status_color(payment.status));
        let title = if created {
            format!("✓ Payment {} created · {sentence}", payment.id)
        } else {
            format!("Payment {}  {mark} {label}", payment.id)
        };
        let progress = match payment.status {
            PaymentStatus::AwaitingPayment => {
                format!("Awaiting {} {}", amount(&payment.amount), payment.currency)
            }
            PaymentStatus::PartiallyPaid => format!(
                "{} of {} {} received · {} {} remaining",
                amount(&payment.received),
                amount(&payment.amount),
                payment.currency,
                amount(&payment.remaining),
                payment.currency
            ),
            _ => format!(
                "{} of {} {} received",
                amount(&payment.received),
                amount(&payment.amount),
                payment.currency
            ),
        };
        let mut lines = if created {
            vec![
                format!(
                    "Share    {}",
                    hyperlink(&payment.payment_url, &payment.payment_url)
                ),
                title,
                progress,
                String::new(),
            ]
        } else {
            vec![title, progress, String::new()]
        };
        if created {
            lines.extend([
                format!(
                    "Address  {}{}",
                    payment.address,
                    explorer_suffix(payment.address_explorer_url.as_deref())
                ),
                format!(
                    "Wallet   ethereum:{}@{}/transfer?address={}&uint256={}",
                    payment.token.address,
                    payment.chain.id,
                    payment.address,
                    payment.amount_base_units
                ),
                format!(
                    "Only Circle-issued native {} on {} (chain {}). Wrong-chain or late funds may not reach the payout wallet.",
                    payment.currency, payment.chain.name, payment.chain.id
                ),
                format!("Follow it:  payday get {} --watch", payment.id),
            ]);
        } else {
            lines.extend([
                format!(
                    "Pay to     {}  {} · native {} only{}",
                    payment.address,
                    payment.chain.name,
                    payment.currency,
                    explorer_suffix(payment.address_explorer_url.as_deref())
                ),
                format!(
                    "Link       {}",
                    hyperlink(&payment.payment_url, &payment.payment_url)
                ),
                format!("Expires    {}", expiry(&payment.expires_at)),
                format!(
                    "Payout     {}  late or leftover funds → {}",
                    payment.payout_address, payment.refund_address
                ),
            ]);
            if let Some(reference) = payment.reference.as_ref().or(payment.memo.as_ref()) {
                lines.push(format!("Reference  {reference}"));
            }
            if payment.cancellation_requested_at.is_some() {
                lines.push(
                    "Cancelled  advisory only; the address may still receive and route funds"
                        .into(),
                );
            }
            if let Some(attention) = &payment.attention {
                lines.push(format!(
                    "Action     {} {}",
                    attention.message, attention.action
                ));
            }
            if let Some(hash) = &payment.settlement_tx_hash {
                lines.push(format!(
                    "Settlement {}",
                    explorer_link(hash, payment.settlement_explorer_url.as_deref())
                ));
            }
            if !payment.transfers.is_empty() {
                lines.extend([String::new(), "Transfers".into()]);
                lines.extend(payment.transfers.iter().map(|transfer| {
                    let description = match transfer.disposition.as_str() {
                        "credited" => format!("+{} USDC received", amount(&transfer.amount)),
                        "late" => format!("{} USDC late → refund wallet", amount(&transfer.amount)),
                        "zero" => format!("{} USDC ignored", amount(&transfer.amount)),
                        other => format!("{} USDC ({other})", amount(&transfer.amount)),
                    };
                    format!(
                        "{}  {:<28} from {}  {}",
                        short_time(&transfer.timestamp),
                        description,
                        shorten(&transfer.sender),
                        explorer_link(&transfer.transaction_hash, transfer.explorer_url.as_deref())
                    )
                }));
            }
            if let Some(block) = &payment.indexer_freshness.last_finalized_block {
                let age = payment
                    .indexer_freshness
                    .cursor_updated_at
                    .as_deref()
                    .map(cursor_age)
                    .unwrap_or_default();
                let indexed = payment
                    .as_of
                    .as_ref()
                    .map(|as_of| as_of.block.as_str())
                    .or(payment.indexer_freshness.last_indexed_block.as_deref())
                    .unwrap_or("unknown");
                lines.extend([
                    String::new(),
                    format!(
                        "As of block {indexed} · finalized head {block}{age} · --watch to follow"
                    ),
                ]);
            }
        }
        if self.verbose {
            lines.extend([
                String::new(),
                format!("Factory           {}", payment.self_settlement.factory),
                format!("Salt              {}", payment.self_settlement.salt),
                format!("Amount base units {}", payment.amount_base_units),
            ]);
        }
        lines.join("\n")
    }

    pub fn list(&self, payments: &[PaymentSummaryResponse]) -> String {
        if payments.is_empty() {
            return "No payments found.".into();
        }
        let mut lines = vec![format!(
            "{:<21} {:<18} {:>12}  REFERENCE",
            "PAYMENT", "STATUS", "AMOUNT"
        )];
        lines.extend(payments.iter().map(|payment| {
            let status = if payment.cancellation_requested_at.is_some()
                && payment.status == PaymentStatus::AwaitingPayment
            {
                "cancelled"
            } else {
                status_label(payment.status)
            };
            format!(
                "{:<21} {:<18} {:>12}  {}",
                shorten(&payment.id),
                status,
                format!("{} USDC", amount(&payment.amount)),
                payment
                    .reference
                    .as_deref()
                    .or(payment.memo.as_deref())
                    .unwrap_or("—")
            )
        }));
        lines.join("\n")
    }

    fn paint(&self, value: &str, color: AnsiColor) -> String {
        if !self.color {
            return value.into();
        }
        let style = Style::new().fg_color(Some(Color::Ansi(color))).bold();
        format!("{}{value}{}", style.render(), style.render_reset())
    }
}

fn status_text(payment: &PaymentResponse) -> (&'static str, &'static str) {
    if payment.cancellation_requested_at.is_some()
        && payment.status == PaymentStatus::AwaitingPayment
    {
        return ("cancelled", "cancelled (advisory)");
    }
    let label = status_label(payment.status);
    let sentence = match payment.status {
        PaymentStatus::AwaitingPayment => "awaiting payment",
        PaymentStatus::PartiallyPaid => "partially paid",
        PaymentStatus::Paid => "payment confirmed; settlement queued",
        PaymentStatus::Settled => "paid to payout wallet",
        PaymentStatus::Expired => "expired; recovery pending",
        PaymentStatus::Returned => "funds sent to refund wallet",
        PaymentStatus::NeedsAttention => "automatic settlement needs attention",
    };
    (label, sentence)
}

fn status_label(status: PaymentStatus) -> &'static str {
    match status {
        PaymentStatus::AwaitingPayment => "awaiting payment",
        PaymentStatus::PartiallyPaid => "partially paid",
        PaymentStatus::Paid => "payment confirmed",
        PaymentStatus::Settled => "paid",
        PaymentStatus::Expired => "expired",
        PaymentStatus::Returned => "returned",
        PaymentStatus::NeedsAttention => "needs attention",
    }
}

fn status_color(status: PaymentStatus) -> AnsiColor {
    match status {
        PaymentStatus::Settled | PaymentStatus::Returned => AnsiColor::Green,
        PaymentStatus::NeedsAttention => AnsiColor::Red,
        _ => AnsiColor::Yellow,
    }
}

fn amount(value: &str) -> String {
    let trimmed = value.trim_end_matches('0').trim_end_matches('.');
    if value.contains('.') && trimmed.is_empty() {
        "0".into()
    } else if value.contains('.') {
        trimmed.into()
    } else {
        value.into()
    }
}

fn expiry(value: &str) -> String {
    let Ok(time) = DateTime::parse_from_rfc3339(value) else {
        return value.into();
    };
    let delta = time.signed_duration_since(Utc::now());
    let relative = if delta.num_seconds() <= 0 {
        "expired".into()
    } else if delta.num_hours() > 0 {
        format!("in {} h {} m", delta.num_hours(), delta.num_minutes() % 60)
    } else {
        format!("in {} m", delta.num_minutes())
    };
    format!("{relative}  ({})", time.format("%b %-d, %H:%M UTC"))
}

fn short_time(value: &str) -> String {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.format("%b %-d %H:%M UTC").to_string())
        .unwrap_or_else(|_| value.into())
}

fn cursor_age(value: &str) -> String {
    DateTime::parse_from_rfc3339(value)
        .map(|time| {
            format!(
                " · indexed {} s ago",
                Utc::now().signed_duration_since(time).num_seconds().max(0)
            )
        })
        .unwrap_or_default()
}

fn shorten(value: &str) -> String {
    if value.len() > 13 {
        format!("{}…{}", &value[..8], &value[value.len() - 4..])
    } else {
        value.into()
    }
}

fn explorer_link(value: &str, url: Option<&str>) -> String {
    url.map(|url| hyperlink(url, url))
        .unwrap_or_else(|| shorten(value))
}

fn explorer_suffix(url: Option<&str>) -> String {
    url.map(|url| format!("  {}", hyperlink(url, url)))
        .unwrap_or_default()
}

fn hyperlink(url: &str, label: &str) -> String {
    if supports_hyperlinks::on(Stream::Stdout) {
        format!("\x1b]8;;{url}\x1b\\{label}\x1b]8;;\x1b\\")
    } else {
        label.into()
    }
}

pub fn terminal(status: PaymentStatus) -> bool {
    matches!(
        status,
        PaymentStatus::Settled | PaymentStatus::Returned | PaymentStatus::NeedsAttention
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gateway_core::{ChainDto, IndexerFreshnessDto, SelfSettlementDto, TokenDto, TransferDto};

    fn payment(status: PaymentStatus, received: &str) -> PaymentResponse {
        PaymentResponse {
            id: "pay_0191c8e0-5b3a-7c4d-9e2f-1a2b3c4d5e6f".into(),
            payment_url: "https://pay.payday.sh/pay/pay_0191c8e0?token=test".into(),
            address: "0x8F2aB1c4D5e6F7a8B9c0D1e2F3a4B5c6D7e8F9A0".into(),
            address_explorer_url: Some("https://monadvision.com/address/0x8F2a".into()),
            payout_address: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8".into(),
            refund_address: "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC".into(),
            expires_at: "2030-03-17T17:46:40Z".into(),
            expires_in: Some(86_400),
            amount: "1.000000".into(),
            amount_base_units: "1000000".into(),
            received: received.into(),
            received_base_units: "400000".into(),
            remaining: "0.600000".into(),
            remaining_base_units: "600000".into(),
            currency: "USDC".into(),
            fee_amount: "0".into(),
            fee_amount_base_units: "0".into(),
            net_amount: "1".into(),
            net_amount_base_units: "1000000".into(),
            status,
            token: TokenDto {
                symbol: "USDC".into(),
                address: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603".into(),
                decimals: 6,
            },
            chain: ChainDto {
                id: "143".into(),
                name: "Monad".into(),
            },
            settlement_tx_hash: None,
            settlement_explorer_url: None,
            settled_at: None,
            settled_block: None,
            self_settlement: SelfSettlementDto {
                factory: "0xfactory".into(),
                salt: "0xsalt".into(),
            },
            attention: None,
            memo: Some("Order 1234".into()),
            reference: Some("Order 1234".into()),
            metadata: serde_json::json!({}),
            created_at: "2026-08-27T14:00:00Z".into(),
            updated_at: "2026-08-27T15:00:00Z".into(),
            paid_at: None,
            paid_at_block: None,
            expired_at: None,
            cancellation_requested_at: None,
            transfers: vec![],
            indexer_freshness: IndexerFreshnessDto {
                last_indexed_block: Some("12345678".into()),
                last_finalized_block: Some("12345678".into()),
                cursor_updated_at: None,
            },
            as_of: None,
        }
    }

    #[test]
    fn partial_payment_tells_the_story_without_internals() {
        let output = Presentation {
            color: false,
            verbose: false,
        }
        .payment(&payment(PaymentStatus::PartiallyPaid, "0.400000"), false);
        assert!(output.contains("● partially paid"));
        assert!(output.contains("0.4 of 1 USDC received · 0.6 USDC remaining"));
        assert!(output.contains("Reference  Order 1234"));
        assert!(!output.contains("Factory"));
    }

    #[test]
    fn transfer_disposition_is_never_presented_as_credit() {
        let mut payment = payment(PaymentStatus::PartiallyPaid, "0.400000");
        let transfer = |disposition: &str| TransferDto {
            timestamp: "2026-08-27T15:02:00Z".into(),
            amount: "0.400000".into(),
            amount_base_units: "400000".into(),
            sender: "0xa1b200000000000000000000000000000000c3d4".into(),
            transaction_hash: "0x9b1e00000000000000000000000000000000000000000000000000000000a0f2"
                .into(),
            explorer_url: Some("https://monadvision.com/tx/0x9b1e".into()),
            block: "123".into(),
            disposition: disposition.into(),
            collected: false,
        };
        payment.transfers = vec![transfer("credited"), transfer("late"), transfer("zero")];
        let output = Presentation {
            color: false,
            verbose: false,
        }
        .payment(&payment, false);
        assert!(output.contains("+0.4 USDC received"));
        assert!(output.contains("0.4 USDC late → refund wallet"));
        assert!(output.contains("0.4 USDC ignored"));
    }
}
