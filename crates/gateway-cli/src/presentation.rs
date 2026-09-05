use std::io::IsTerminal;
use std::path::Path;

use anstyle::{AnsiColor, Color, RgbColor, Style};
use chrono::{DateTime, Utc};
use gateway_core::{
    PayerPolicyMode, PaymentResponse, PaymentStatus, PaymentSummaryResponse, ProofOfPayment,
};
use supports_hyperlinks::Stream;

use crate::cli::ColorChoice;
use crate::client::Customer;
use crate::proof::{CheckStatus, Report};

// ── Design tokens ──────────────────────────────────────────────────

const DIVIDER: &str = "─";
const LABEL_GREY: Color = Color::Rgb(RgbColor(0x6a, 0x6a, 0x6a));
const DIVIDER_GREY: Color = Color::Rgb(RgbColor(0x44, 0x44, 0x44));
const HINT_GREY: Color = Color::Rgb(RgbColor(0x4a, 0x4a, 0x4a));

pub struct Presentation {
    color: bool,
    verbose: bool,
}

impl Presentation {
    pub fn new(choice: ColorChoice, plain: bool, verbose: bool) -> Self {
        Self::styled(choice, plain, verbose, std::io::stdout().is_terminal())
    }

    /// Styling for the failure stream, which is redirected independently of
    /// stdout and must not receive escape codes when it is not a terminal.
    pub fn for_stderr(choice: ColorChoice, plain: bool, verbose: bool) -> Self {
        Self::styled(choice, plain, verbose, std::io::stderr().is_terminal())
    }

    fn styled(choice: ColorChoice, plain: bool, verbose: bool, is_terminal: bool) -> Self {
        let color = !plain
            && match choice {
                ColorChoice::Always => true,
                ColorChoice::Never => false,
                ColorChoice::Auto => is_terminal && std::env::var_os("NO_COLOR").is_none(),
            };
        Self { color, verbose }
    }

    /// Render a failure: the first line is the message, and any further lines
    /// are next-step hints styled like the ones that follow a success.
    pub fn error(&self, message: &str) -> String {
        let mut lines = message.lines();
        let summary = lines.next().unwrap_or_default();
        let mut out = vec![format!(
            "{} {summary}",
            self.colored("error:", AnsiColor::Red)
        )];
        out.extend(lines.map(|line| self.hint(line)));
        out.join("\n")
    }

    pub fn payment(&self, payment: &PaymentResponse, created: bool) -> String {
        let mut out = Vec::new();

        // ── Header ────────────────────────────────────────────
        if created {
            out.push(self.heading(&format!("Payment created · {}", self.cyan(&payment.id))));
            out.push(String::new());

            // Share link callout
            out.push(self.kv(
                "Share",
                &self.cyan(&hyperlink(&payment.payment_url, &payment.payment_url)),
            ));
            out.push(String::new());
        }

        // ── Status row ───────────────────────────────────────
        let (label, sentence) = status_text(payment);
        let icon = status_icon(payment.status);
        let icon_str = self.paint_icon(icon, payment.status);
        let status_str = self.paint_status(label, payment.status);
        let s = status_color(payment.status);

        if created {
            out.push(format!(
                "  {icon_str}  {status_str}  {}",
                self.dim(sentence)
            ));
        } else {
            out.push(format!(
                "  {icon_str}  {status_str}  {}  {}",
                self.colored(&payment.id, s),
                self.dim(sentence),
            ));
        }
        out.push(String::new());

        // ── Amount progress ──────────────────────────────────
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
        out.push(format!("  {}", self.colored(&progress, s)));
        out.push(String::new());

        // ── Details section ──────────────────────────────────
        if created {
            out.push(self.section("Payment details"));
            out.push(self.kv(
                "Pay to",
                &format!(
                    "{}  {} · native {}{}",
                    self.cyan(&payment.address),
                    self.dim(&payment.chain.name),
                    self.dim(&payment.currency),
                    explorer_suffix(payment.address_explorer_url.as_deref(), self.color),
                ),
            ));
            out.push(self.kv("Expires", &expiry_colored(&payment.expires_at, self.color)));
            out.push(self.kv("Payout", &self.cyan(&payment.payout_address)));
            out.push(self.kv(
                "Recovery",
                &format!(
                    "{}  {}",
                    self.cyan(&payment.recovery_address),
                    self.dim("Payday recovery wallet for late or leftover funds"),
                ),
            ));
            out.extend(self.document(payment));
            out.push(String::new());

            out.push(self.wrap_dim(&format!(
                "Only Circle-issued native {} on {} (chain {}). Wrong-chain or late funds may not reach the payout wallet.",
                payment.currency, payment.chain.name, payment.chain.id
            )));
            out.push(String::new());

            out.push(self.hint(&format!("Follow it:  payday get {} --watch", payment.id)));
        } else {
            out.push(self.section("Payment details"));
            out.push(self.kv(
                "Pay to",
                &format!(
                    "{}  {} · native {}{}",
                    self.cyan(&payment.address),
                    self.dim(&payment.chain.name),
                    self.dim(&payment.currency),
                    explorer_suffix(payment.address_explorer_url.as_deref(), self.color),
                ),
            ));
            out.push(self.kv(
                "Link",
                &self.cyan(&hyperlink(&payment.payment_url, &payment.payment_url)),
            ));
            out.push(self.kv("Expires", &expiry_colored(&payment.expires_at, self.color)));
            out.push(self.kv(
                "Payout",
                &format!(
                    "{}  {} {}",
                    self.cyan(&payment.payout_address),
                    self.dim("late or leftover funds → Payday recovery"),
                    self.cyan(&payment.recovery_address),
                ),
            ));

            out.extend(self.document(payment));
            if payment.cancellation_requested_at.is_some() {
                out.push(self.kv_warn(
                    "Cancelled",
                    "advisory only; the address may still receive and route funds",
                ));
            }
            if let Some(attention) = &payment.attention {
                out.push(self.kv_alert(
                    "Action",
                    &format!("{} {}", attention.message, attention.action),
                ));
            }
            if let Some(hash) = &payment.settlement_tx_hash {
                out.push(self.kv(
                    "Settlement",
                    &explorer_link(hash, payment.settlement_explorer_url.as_deref(), self.color),
                ));
            }
            if let Some(settled_at) = &payment.settled_at {
                out.push(self.kv("Settled at", settled_at));
            }

            // ── Transfers ─────────────────────────────────────
            if !payment.transfers.is_empty() {
                out.push(String::new());
                out.push(self.section("Transfers"));
                for transfer in &payment.transfers {
                    let (amount_str, disposition_icon) = match transfer.disposition.as_str() {
                        "credited" => (
                            self.green(&format!("+{} USDC received", amount(&transfer.amount))),
                            self.green("↑"),
                        ),
                        "late" => (
                            self.yellow(&format!(
                                "{} USDC late → Payday recovery",
                                amount(&transfer.amount)
                            )),
                            self.yellow("↗"),
                        ),
                        "zero" => (
                            self.dim(&format!("{} USDC ignored", amount(&transfer.amount))),
                            self.dim("·"),
                        ),
                        other => (
                            format!("{} USDC ({other})", amount(&transfer.amount)),
                            self.dim("·"),
                        ),
                    };
                    out.push(format!(
                        "  {disposition_icon}  {amount_str}  {}  {}  {}",
                        self.dim(&short_time(&transfer.timestamp)),
                        self.dim("from"),
                        self.cyan(&transfer.sender),
                    ));
                    out.push(format!(
                        "     {}",
                        self.dim(&explorer_link(
                            &transfer.transaction_hash,
                            transfer.explorer_url.as_deref(),
                            self.color
                        ))
                    ));
                }
            }

            // ── Indexer freshness ─────────────────────────────
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
                out.push(String::new());
                out.push(self.dim(&format!(
                    "  As of block {indexed} · finalized head {block}{age}"
                )));
                out.push(self.hint("--watch to follow"));
            }
        }

        // ── Verbose details ──────────────────────────────────
        if self.verbose {
            out.push(String::new());
            out.push(self.section("Operational details"));
            out.push(self.kv("Factory", &payment.self_settlement.factory));
            out.push(self.kv("Salt", &payment.self_settlement.salt));
            out.push(self.kv("Amount base units", &payment.amount_base_units));
        }

        out.join("\n")
    }

    pub fn list(&self, payments: &[PaymentSummaryResponse]) -> String {
        if payments.is_empty() {
            return self.dim("  No invoices found.").to_string();
        }

        let mut out = Vec::new();

        // Column widths — full payment IDs are 39 chars (pay_ + UUID). The
        // bill-to and policy columns size to their content so the rare long
        // mode (`verified_identity_unattributed`) never breaks alignment.
        let id_w = 39;
        let status_w = 20;
        let amount_w = 12;
        let bill_to_w = payments
            .iter()
            .map(|payment| truncate(&payment.bill_to_name, BILL_TO_MAX).chars().count())
            .max()
            .unwrap_or(0)
            .max("BILL TO".len());
        let policy_w = payments
            .iter()
            .map(|payment| payment.payer_policy_mode.as_str().len())
            .max()
            .unwrap_or(0)
            .max("POLICY".len());

        // Header
        out.push(format!(
            "  {}  {}  {}  {}  {}  {}",
            self.dim(&pad("PAYMENT", id_w)),
            self.dim(&pad("STATUS", status_w)),
            self.dim(&pad_right("AMOUNT", amount_w)),
            self.dim(&pad("BILL TO", bill_to_w)),
            self.dim(&pad("POLICY", policy_w)),
            self.dim("HEADING"),
        ));
        // Separator matches the visual width of the columns above
        let sep_width = id_w + 2 + status_w + 2 + amount_w + 2 + bill_to_w + 2 + policy_w + 2 + 7; // "HEADING" = 7
        out.push(format!("  {}", self.dim(&DIVIDER.repeat(sep_width))));

        for payment in payments {
            let status = if payment.cancellation_requested_at.is_some()
                && payment.status == PaymentStatus::AwaitingPayment
            {
                "cancelled"
            } else {
                status_label(payment.status)
            };
            let icon = status_icon(payment.status);
            let amount_str = format!("{} USDC", amount(&payment.amount));
            let heading = payment.heading.as_deref().unwrap_or("—");

            // Build the status cell: icon + space + status text, padded to status_w
            let status_text = pad(status, status_w - 2);
            let status_cell = format!(
                "{} {}",
                self.paint_icon(icon, payment.status),
                self.paint_status(&status_text, payment.status),
            );

            out.push(format!(
                "  {}  {}  {}  {}  {}  {}",
                self.dim(&pad(&payment.id, id_w)),
                status_cell,
                self.dim(&pad_right(&amount_str, amount_w)),
                pad(&truncate(&payment.bill_to_name, BILL_TO_MAX), bill_to_w),
                self.dim(&pad(payment.payer_policy_mode.as_str(), policy_w)),
                heading,
            ));
        }

        out.join("\n")
    }

    pub fn customer(&self, customer: &Customer, created: bool) -> String {
        let mut out = Vec::new();
        if created {
            out.push(self.heading(&format!(
                "Customer created · {}",
                self.cyan(&customer.id.to_string())
            )));
            out.push(String::new());
        }
        out.push(self.kv("Customer", &self.cyan(&customer.id.to_string())));
        out.push(self.kv("Name", &customer.name));
        out.push(self.kv("Email", customer.email.as_deref().unwrap_or("—")));
        out.push(self.kv("Details", customer.details.as_deref().unwrap_or("—")));
        out.push(self.kv("Created", &short_time(&customer.created_at)));
        if created {
            out.push(String::new());
            out.push(self.hint(&format!(
                "Reference it from a `--from-file` body with \"customer_id\": \"{}\"",
                customer.id
            )));
        }
        out.join("\n")
    }

    pub fn customers(&self, customers: &[Customer]) -> String {
        if customers.is_empty() {
            return self.dim("  No customers found.").to_string();
        }
        let id_w = 36;
        let name_w = customers
            .iter()
            .map(|customer| truncate(&customer.name, BILL_TO_MAX).chars().count())
            .max()
            .unwrap_or(0)
            .max("NAME".len());
        let mut out = vec![format!(
            "  {}  {}  {}",
            self.dim(&pad("CUSTOMER", id_w)),
            self.dim(&pad("NAME", name_w)),
            self.dim("EMAIL"),
        )];
        out.push(format!(
            "  {}",
            self.dim(&DIVIDER.repeat(id_w + 2 + name_w + 2 + 5))
        ));
        for customer in customers {
            out.push(format!(
                "  {}  {}  {}",
                self.dim(&pad(&customer.id.to_string(), id_w)),
                pad(&truncate(&customer.name, BILL_TO_MAX), name_w),
                customer.email.as_deref().unwrap_or("—"),
            ));
        }
        out.join("\n")
    }

    /// After `payday proof download`: where the proof went and what it covers.
    pub fn proof_saved(&self, proof: &ProofOfPayment, path: &Path) -> String {
        [
            format!("  ✓ Proof of Payment saved to {}", path.display()),
            String::new(),
            self.kv("Payment", &self.cyan(&proof.payment_id)),
            self.kv("Address", &self.cyan(&proof.payment_address)),
            self.kv("Settlement", &proof.settlement_transaction_hash),
            self.kv(
                "Attestation",
                &format!(
                    "{} {}  {}",
                    proof.verification.payload.payer_policy_mode,
                    proof.verification.payload.result,
                    self.dim(&format!("signed by {}", proof.verification.signer)),
                ),
            ),
            String::new(),
            self.hint(&format!(
                "Verify offline: payday proof verify {} [--attachment FILE.pdf] [--trusted-attestor {}]",
                path.display(),
                proof.verification.signer
            )),
        ]
        .join("\n")
    }

    /// `payday proof verify`: one line per check, then the verdict.
    pub fn proof_report(&self, proof: &ProofOfPayment, report: &Report) -> String {
        let mut out = vec![
            self.heading(&format!(
                "Proof of Payment · {}",
                self.cyan(&proof.payment_id)
            )),
            String::new(),
        ];
        for check in &report.checks {
            let (icon, color) = match check.status {
                CheckStatus::Passed => ("✓", AnsiColor::Green),
                CheckStatus::Failed => ("✗", AnsiColor::Red),
                CheckStatus::Skipped => ("·", AnsiColor::Yellow),
            };
            let mut line = format!("  {}  {}", self.colored(icon, color), check.name);
            if let Some(detail) = &check.detail {
                line.push_str(&format!("  {}", self.dim(detail)));
            }
            out.push(line);
        }
        out.push(String::new());
        let failed = report
            .checks
            .iter()
            .filter(|check| check.status == CheckStatus::Failed)
            .count();
        out.push(if report.ok {
            format!(
                "  {}",
                self.green("Proof verified: the invoice, its payment address, and the transfers that paid it agree.")
            )
        } else {
            format!(
                "  {}",
                self.colored(
                    &format!("Proof failed {failed} check(s); do not rely on it."),
                    AnsiColor::Red
                )
            )
        });
        out.join("\n")
    }

    /// The invoice document: who issued it, who owes it, what the payer must
    /// prove before seeing it, and the commitment that binds it to the address.
    fn document(&self, payment: &PaymentResponse) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(heading) = &payment.heading {
            out.push(self.kv("Heading", heading));
        }
        out.push(self.kv("Issuer", &payment.issuer.name));
        out.push(self.kv("Bill to", &payment.bill_to.name));
        if let Some(reference) = &payment.reference {
            out.push(self.kv("Reference", reference));
        }
        let mode = payment.payer_policy.mode();
        out.push(self.kv(
            "Payer",
            &if mode == PayerPolicyMode::Permissionless {
                "anyone with the link".to_string()
            } else {
                let verification = match &payment.verification_completed_at {
                    Some(at) => format!("verified {}", short_time(at)),
                    None => "payer verifies before paying".to_string(),
                };
                format!("{}  {}", mode, self.dim(&verification))
            },
        ));
        if let Some(attachment) = &payment.attachment {
            out.push(self.kv(
                "Attachment",
                &format!(
                    "{}  {}",
                    attachment.filename,
                    self.dim(&format!(
                        "{} bytes · sha256 {}…",
                        attachment.byte_length,
                        attachment.sha256.chars().take(12).collect::<String>()
                    ))
                ),
            ));
        }
        out.push(self.kv(
            "Attribution",
            &format!(
                "{}  {}",
                payment.attribution.hash,
                self.dim(&format!("v{}", payment.attribution.version))
            ),
        ));
        if let Some(at) = &payment.likely_unsolicited_at {
            out.push(self.kv_warn(
                "Unsolicited",
                &format!(
                    "funds arrived {} before the payer verified; treat as likely unsolicited",
                    short_time(at)
                ),
            ));
        }
        out
    }

    // ── Layout helpers ──────────────────────────────────────

    fn heading(&self, text: &str) -> String {
        if self.color {
            let style = Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Cyan)))
                .bold();
            format!("{}{text}{}", style.render(), style.render_reset())
        } else {
            text.to_string()
        }
    }

    fn section(&self, title: &str) -> String {
        let label = format!(" {title} ");
        let pad_len = 50usize.saturating_sub(label.chars().count());
        let line = DIVIDER.repeat(pad_len);
        if self.color {
            let style = Style::new().fg_color(Some(DIVIDER_GREY));
            format!("{}{label}{line}{}", style.render(), style.render_reset())
        } else {
            format!("{label}{line}")
        }
    }

    fn kv(&self, key: &str, value: &str) -> String {
        let key_pad = pad(key, 12);
        if self.color {
            let key_style = Style::new().fg_color(Some(LABEL_GREY));
            format!(
                "  {}{key_pad}{}  {value}",
                key_style.render(),
                key_style.render_reset()
            )
        } else {
            format!("  {key_pad}  {value}")
        }
    }

    fn kv_warn(&self, key: &str, value: &str) -> String {
        let base = self.kv(key, "");
        if self.color {
            let val_style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
            format!(
                "{base}{}{value}{}",
                val_style.render(),
                val_style.render_reset()
            )
        } else {
            format!("{base}{value}")
        }
    }

    fn kv_alert(&self, key: &str, value: &str) -> String {
        let base = self.kv(key, "");
        if self.color {
            let val_style = Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Red)))
                .bold();
            format!(
                "{base}{}{value}{}",
                val_style.render(),
                val_style.render_reset()
            )
        } else {
            format!("{base}{value}")
        }
    }

    fn hint(&self, text: &str) -> String {
        if self.color {
            let style = Style::new().fg_color(Some(HINT_GREY));
            format!("  {}{text}{}", style.render(), style.render_reset())
        } else {
            format!("  {text}")
        }
    }

    fn dim(&self, text: &str) -> String {
        if self.color {
            let style = Style::new().dimmed();
            format!("{}{text}{}", style.render(), style.render_reset())
        } else {
            text.to_string()
        }
    }

    fn wrap_dim(&self, text: &str) -> String {
        self.dim(&format!("  {text}"))
    }

    fn green(&self, text: &str) -> String {
        if self.color {
            let style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
            format!("{}{text}{}", style.render(), style.render_reset())
        } else {
            text.to_string()
        }
    }

    fn yellow(&self, text: &str) -> String {
        if self.color {
            let style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
            format!("{}{text}{}", style.render(), style.render_reset())
        } else {
            text.to_string()
        }
    }

    fn cyan(&self, text: &str) -> String {
        if self.color {
            let style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
            format!("{}{text}{}", style.render(), style.render_reset())
        } else {
            text.to_string()
        }
    }

    fn colored(&self, text: &str, color: AnsiColor) -> String {
        if self.color {
            let style = Style::new().fg_color(Some(Color::Ansi(color)));
            format!("{}{text}{}", style.render(), style.render_reset())
        } else {
            text.to_string()
        }
    }

    fn paint_icon(&self, icon: &str, status: PaymentStatus) -> String {
        if !self.color {
            return icon.to_string();
        }
        let color = status_color(status);
        let style = Style::new().fg_color(Some(Color::Ansi(color))).bold();
        format!("{}{icon}{}", style.render(), style.render_reset())
    }

    fn paint_status(&self, label: &str, status: PaymentStatus) -> String {
        if !self.color {
            return label.to_string();
        }
        let color = status_color(status);
        let style = Style::new().fg_color(Some(Color::Ansi(color))).bold();
        format!("{}{label}{}", style.render(), style.render_reset())
    }
}

// ── Free functions ──────────────────────────────────────────

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
        PaymentStatus::Returned => "funds sent to Payday recovery",
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

fn status_icon(status: PaymentStatus) -> &'static str {
    match status {
        PaymentStatus::AwaitingPayment => "◐",
        PaymentStatus::PartiallyPaid => "◑",
        PaymentStatus::Paid => "◉",
        PaymentStatus::Settled => "✓",
        PaymentStatus::Expired => "✗",
        PaymentStatus::Returned => "↩",
        PaymentStatus::NeedsAttention => "⚠",
    }
}

fn status_color(status: PaymentStatus) -> AnsiColor {
    match status {
        PaymentStatus::Settled | PaymentStatus::Returned => AnsiColor::Green,
        PaymentStatus::NeedsAttention => AnsiColor::Red,
        PaymentStatus::Expired => AnsiColor::Red,
        PaymentStatus::Paid => AnsiColor::Cyan,
        PaymentStatus::PartiallyPaid => AnsiColor::Yellow,
        PaymentStatus::AwaitingPayment => AnsiColor::Yellow,
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

fn expiry_colored(value: &str, color: bool) -> String {
    let Ok(time) = DateTime::parse_from_rfc3339(value) else {
        return value.into();
    };
    let delta = time.signed_duration_since(Utc::now());
    let secs = delta.num_seconds();
    let (relative, urgency) = if secs <= 0 {
        ("expired".to_string(), Color::Ansi(AnsiColor::Red))
    } else if secs < 3600 {
        (
            format!("in {}m", delta.num_minutes()),
            Color::Ansi(AnsiColor::Red),
        )
    } else if secs < 6 * 3600 {
        (
            format!("in {}h {}m", delta.num_hours(), delta.num_minutes() % 60),
            Color::Ansi(AnsiColor::Yellow),
        )
    } else if delta.num_hours() > 0 {
        (
            format!("in {}h {}m", delta.num_hours(), delta.num_minutes() % 60),
            Color::Rgb(RgbColor(0x6a, 0x6a, 0x6a)),
        )
    } else {
        (
            format!("in {}m", delta.num_minutes()),
            Color::Rgb(RgbColor(0x6a, 0x6a, 0x6a)),
        )
    };
    let absolute = format!("({})", time.format("%b %-d, %H:%M UTC"));
    if color {
        let rel_style = Style::new().fg_color(Some(urgency));
        let abs_style = Style::new().dimmed();
        format!(
            "{}{relative}{}  {}{absolute}{}",
            rel_style.render(),
            rel_style.render_reset(),
            abs_style.render(),
            abs_style.render_reset(),
        )
    } else {
        format!("{relative}  {absolute}")
    }
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
                " · indexed {}s ago",
                Utc::now().signed_duration_since(time).num_seconds().max(0)
            )
        })
        .unwrap_or_default()
}

/// Widest a name column grows before its content is elided.
const BILL_TO_MAX: usize = 24;

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    let mut short: String = value.chars().take(width.saturating_sub(1)).collect();
    short.push('…');
    short
}

fn pad(value: &str, width: usize) -> String {
    let len = value.chars().count();
    if len >= width {
        value.to_string()
    } else {
        format!("{value}{}", " ".repeat(width - len))
    }
}

fn pad_right(value: &str, width: usize) -> String {
    let len = value.chars().count();
    if len >= width {
        value.to_string()
    } else {
        format!("{}{value}", " ".repeat(width - len))
    }
}

fn explorer_link(value: &str, url: Option<&str>, color: bool) -> String {
    if let Some(url) = url {
        if color {
            let style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
            format!(
                "{}{}{}",
                style.render(),
                hyperlink(url, value),
                style.render_reset()
            )
        } else {
            hyperlink(url, value)
        }
    } else {
        value.to_string()
    }
}

fn explorer_suffix(url: Option<&str>, color: bool) -> String {
    if url.is_none() {
        return String::new();
    }
    if color {
        let style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
        format!("  {}↗{}", style.render(), style.render_reset())
    } else {
        "  ↗".to_string()
    }
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
    use gateway_core::{
        AttachmentDescriptor, AttributionDto, ChainDto, IndexerFreshnessDto, Party, PayerPolicy,
        SelfSettlementDto, TokenDto, TransferDto,
    };

    #[test]
    fn errors_lead_with_the_summary_and_indent_their_hints() {
        let plain = Presentation::new(ColorChoice::Never, true, false);
        assert_eq!(
            plain.error("'pay_0198f80c' is not a complete payment ID\n→ Run `payday list`."),
            "error: 'pay_0198f80c' is not a complete payment ID\n  → Run `payday list`."
        );
        assert!(
            Presentation::new(ColorChoice::Always, false, false)
                .error("no")
                .contains("\u{1b}[")
        );
    }

    fn payment(status: PaymentStatus, received: &str) -> PaymentResponse {
        PaymentResponse {
            id: "pay_0191c8e0-5b3a-7c4d-9e2f-1a2b3c4d5e6f".into(),
            payment_url: "https://pay.payday.sh/pay/pay_0191c8e0".into(),
            address: "0x8F2aB1c4D5e6F7a8B9c0D1e2F3a4B5c6D7e8F9A0".into(),
            address_explorer_url: Some("https://monadvision.com/address/0x8F2a".into()),
            payout_address: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8".into(),
            recovery_address: "0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc".into(),
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
            issuer: Party {
                name: "Acme Corp".into(),
                email: None,
                details: None,
            },
            bill_to: Party {
                name: "Globex".into(),
                email: None,
                details: None,
            },
            notes: None,
            heading: Some("March retainer".into()),
            reference: Some("Order 1234".into()),
            customer_id: None,
            issuer_id: None,
            payer_policy: PayerPolicy::VerifiedEmail {
                expected_email: "alice@example.com".into(),
            },
            attachment: Some(AttachmentDescriptor {
                id: uuid::Uuid::from_u128(9),
                filename: "contract.pdf".into(),
                mime_type: "application/pdf".into(),
                byte_length: "1234".into(),
                sha256: format!("0x{}", "ab".repeat(32)),
                download_url: None,
            }),
            verification_completed_at: None,
            likely_unsolicited_at: None,
            attribution: AttributionDto {
                version: 1,
                hash: format!("0x{}", "cd".repeat(32)),
            },
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
        assert!(output.contains("partially paid"));
        assert!(output.contains("0.4 of 1 USDC received · 0.6 USDC remaining"));
        assert!(output.contains("Reference"));
        assert!(output.contains("Order 1234"));
        assert!(output.contains("Issuer"));
        assert!(output.contains("Acme Corp"));
        assert!(output.contains("Bill to"));
        assert!(output.contains("Globex"));
        assert!(output.contains("March retainer"));
        assert!(output.contains("verified_email"));
        assert!(output.contains("payer verifies before paying"));
        assert!(output.contains("contract.pdf"));
        assert!(output.contains("sha256 0xababababab…"));
        assert!(output.contains("Attribution"));
        assert!(output.contains(&format!("0x{}", "cd".repeat(32))));
        assert!(!output.contains("alice@example.com"));
        assert!(!output.contains("Factory"));
        assert!(!output.contains("Unsolicited"));
    }

    #[test]
    fn verification_and_unsolicited_funds_are_called_out() {
        let mut payment = payment(PaymentStatus::PartiallyPaid, "0.400000");
        payment.verification_completed_at = Some("2026-08-27T15:30:00Z".into());
        payment.likely_unsolicited_at = Some("2026-08-27T15:02:00Z".into());
        let output = Presentation {
            color: false,
            verbose: false,
        }
        .payment(&payment, false);
        assert!(output.contains("verified Aug 27 15:30 UTC"));
        assert!(output.contains("Unsolicited"));
        assert!(output.contains("likely unsolicited"));
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
        assert!(output.contains("0.4 USDC late → Payday recovery"));
        assert!(output.contains("0.4 USDC ignored"));
        assert!(!output.contains("refund"));
    }

    fn summary(
        bill_to: &str,
        mode: PayerPolicyMode,
        heading: Option<&str>,
    ) -> PaymentSummaryResponse {
        PaymentSummaryResponse {
            id: "pay_0191c8e0-5b3a-7c4d-9e2f-1a2b3c4d5e6f".into(),
            heading: heading.map(Into::into),
            bill_to_name: bill_to.into(),
            reference: Some("Order 1234".into()),
            metadata: serde_json::json!({}),
            created_at: "2026-08-27T14:00:00Z".into(),
            status: PaymentStatus::Settled,
            amount: "25.000000".into(),
            received: "25.000000".into(),
            payer_policy_mode: mode,
            customer_id: None,
            issuer_id: None,
            has_attachment: false,
            verification_completed_at: None,
            likely_unsolicited_at: None,
            cancellation_requested_at: None,
        }
    }

    #[test]
    fn list_rows_show_bill_to_policy_and_heading_aligned() {
        let payments = vec![
            summary(
                "Globex",
                PayerPolicyMode::Permissionless,
                Some("March retainer"),
            ),
            summary(
                "Initech International Holdings Ltd",
                PayerPolicyMode::VerifiedIdentityUnattributed,
                None,
            ),
        ];
        let output = Presentation {
            color: false,
            verbose: false,
        }
        .list(&payments);
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[0].contains("PAYMENT"));
        assert!(lines[0].contains("STATUS"));
        assert!(lines[0].contains("AMOUNT"));
        assert!(lines[0].contains("BILL TO"));
        assert!(lines[0].contains("POLICY"));
        assert!(lines[0].contains("HEADING"));
        assert!(lines[2].contains("25 USDC"));
        assert!(lines[2].contains("Globex"));
        assert!(lines[2].contains("permissionless"));
        assert!(lines[2].ends_with("March retainer"));
        assert!(lines[3].contains("Initech International H…"));
        assert!(lines[3].contains("verified_identity_unattributed"));
        assert!(lines[3].ends_with("—"));
        // Both rows place each column at the same character offset even
        // though the elided name carries a multi-byte ellipsis.
        let column = |line: &str, needle: &str| line[..line.find(needle).unwrap()].chars().count();
        assert_eq!(column(lines[2], "March retainer"), column(lines[3], "—"));
        assert_eq!(
            column(lines[2], "permissionless"),
            column(lines[3], "verified_identity_unattributed")
        );
    }

    #[test]
    fn list_empty_shows_message() {
        let output = Presentation {
            color: false,
            verbose: false,
        }
        .list(&[]);
        assert!(output.contains("No invoices found"));
    }

    #[test]
    fn customers_render_as_a_record_and_a_table() {
        let customer = Customer {
            id: uuid::Uuid::from_u128(7),
            name: "Globex".into(),
            email: Some("billing@globex.example".into()),
            details: None,
            created_at: "2026-08-27T14:00:00Z".into(),
            updated_at: "2026-08-27T14:00:00Z".into(),
        };
        let plain = Presentation {
            color: false,
            verbose: false,
        };
        let record = plain.customer(&customer, true);
        assert!(record.contains("Customer created"));
        assert!(record.contains("Globex"));
        assert!(record.contains("billing@globex.example"));
        assert!(record.contains("Details       —"));
        assert!(record.contains("customer_id"));
        let table = plain.customers(std::slice::from_ref(&customer));
        assert!(table.contains("CUSTOMER"));
        assert!(table.contains(&customer.id.to_string()));
        assert!(plain.customers(&[]).contains("No customers found"));
    }

    #[test]
    fn proof_report_marks_each_check_and_fails_loudly() {
        let proof = crate::proof::fixture::proof();
        let plain = Presentation {
            color: false,
            verbose: false,
        };
        let (checks, verified) = crate::proof::offline(&proof, None, &[]);
        assert!(verified.is_some());
        let report = Report { checks, ok: true };
        let output = plain.proof_report(&proof, &report);
        assert!(output.contains(&proof.payment_id));
        assert!(output.contains("✓  Canonical invoice hashes to the attribution hash"));
        assert!(output.contains("·  Attachment matches"));
        assert!(output.contains("Proof verified"));

        let (checks, verified) =
            crate::proof::offline(&proof, None, &[alloy_primitives::Address::repeat_byte(1)]);
        assert!(verified.is_none());
        let report = Report { checks, ok: false };
        let output = plain.proof_report(&proof, &report);
        assert!(output.contains("✗  Verification attestation"));
        assert!(output.contains("Proof failed 1 check(s)"));

        let saved = plain.proof_saved(&proof, Path::new("proof.json"));
        assert!(saved.contains("saved to proof.json"));
        assert!(saved.contains("payday proof verify proof.json"));
        assert!(saved.contains(&proof.verification.signer));
    }

    #[test]
    fn created_payment_shares_link_and_address() {
        let output = Presentation {
            color: false,
            verbose: false,
        }
        .payment(&payment(PaymentStatus::AwaitingPayment, "0.000000"), true);
        assert!(output.contains("Payment created"));
        assert!(output.contains("Share"));
        assert!(output.contains("Pay to"));
        assert!(output.contains("0x8F2aB1c4D5e6F7a8B9c0D1e2F3a4B5c6D7e8F9A0"));
        assert!(output.contains("Recovery"));
        assert!(output.contains("Payday recovery wallet"));
        assert!(output.contains("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"));
        assert!(output.contains("Issuer"));
        assert!(output.contains("Bill to"));
        assert!(!output.contains("Refund"));
    }
}
