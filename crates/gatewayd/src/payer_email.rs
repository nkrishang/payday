//! The one email a payer receives from Payday: the deposit request they
//! were named on, sent to the `payer.email` the merchant gave at issuance.
//!
//! The message is rendered here, in Payday's own design, and delivered
//! through Resend (the account Auth0 already sends its codes from) by the
//! dispatcher that drains the notification outbox. Nothing in the request
//! path waits on Resend: issuance queues the row, and the dispatcher sends
//! it, retrying transient failures on the outbox's backoff.

use std::fmt;

use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use serde::Serialize;

/// Where a payer's questions go, and where the email comes from unless a
/// deployment says otherwise (`PAYDAY_PAYER_EMAIL_FROM`).
pub const CONTACT_ADDRESS: &str = "contact@payday.sh";
pub const DEFAULT_FROM: &str = "Payday <contact@payday.sh>";

const RESEND_SEND_URL: &str = "https://api.resend.com/emails";

/// Everything the payer's email says. Every string is merchant-supplied
/// free text, escaped where it lands in HTML; the deposit URL is Payday's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepositRequestEmail {
    pub issuer_name: String,
    pub payer_name: String,
    /// Trimmed decimal USDC, as the API presents it.
    pub amount: String,
    pub heading: Option<String>,
    pub reference: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub deposit_url: String,
}

impl DepositRequestEmail {
    pub fn subject(&self) -> String {
        match &self.heading {
            Some(heading) => format!(
                "{} sent you a deposit request: {heading} ({} USDC)",
                self.issuer_name, self.amount
            ),
            None => format!(
                "{} sent you a deposit request for {} USDC",
                self.issuer_name, self.amount
            ),
        }
    }

    /// "6 September 2026, 14:05 UTC": readable anywhere, honest about the
    /// zone, and never mistaken for a local time.
    fn expires_at_text(&self) -> String {
        self.expires_at.format("%-d %B %Y, %H:%M UTC").to_string()
    }

    pub fn text(&self) -> String {
        let mut lines = vec![
            format!("Hi {},", self.payer_name),
            String::new(),
            format!(
                "{} has issued you a deposit request through Payday for {} USDC.",
                self.issuer_name, self.amount
            ),
            String::new(),
        ];
        if let Some(heading) = &self.heading {
            lines.push(format!("For: {heading}"));
        }
        if let Some(reference) = &self.reference {
            lines.push(format!("Reference: {reference}"));
        }
        lines.push(format!("Amount: {} USDC", self.amount));
        lines.push(format!("Expires: {}", self.expires_at_text()));
        lines.push(String::new());
        lines.push(
            "Open the deposit request to review the details and pay from your wallet:".into(),
        );
        lines.push(self.deposit_url.clone());
        lines.push(String::new());
        lines.push(
            "The request is issued by the sender named above; Payday only provides the deposit page and settlement. If you were not expecting it, you can simply ignore this email."
                .into(),
        );
        lines.push(String::new());
        lines.push(format!(
            "Questions? Email us at {CONTACT_ADDRESS} and quote the request link."
        ));
        lines.push(String::new());
        lines.push("— Payday".into());
        lines.join("\n")
    }

    pub fn html(&self) -> String {
        let issuer = escape(&self.issuer_name);
        let payer = escape(&self.payer_name);
        let amount = escape(&self.amount);
        let url = escape(&self.deposit_url);
        let expires = escape(&self.expires_at_text());
        let mut rows = String::new();
        if let Some(heading) = &self.heading {
            rows.push_str(&detail_row("For", &escape(heading), false));
        }
        if let Some(reference) = &self.reference {
            rows.push_str(&detail_row("Reference", &escape(reference), true));
        }
        rows.push_str(&detail_row("Amount", &format!("{amount} USDC"), false));
        rows.push_str(&detail_row("Expires", &expires, false));
        let subject = escape(&self.subject());
        let preheader = escape(&format!(
            "{} is requesting {} USDC. Review and pay from your wallet.",
            self.issuer_name, self.amount
        ));

        format!(
            r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="dark">
<meta name="supported-color-schemes" content="dark">
<title>{subject}</title>
</head>
<body style="margin:0;padding:0;background-color:#070707;color:#f6f2ea;font-family:'Albert Sans',-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;">
<div style="display:none;max-height:0;overflow:hidden;opacity:0;color:transparent;">{preheader}</div>
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="background-color:#070707;">
<tr><td align="center" style="padding:32px 16px 48px;">
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="max-width:480px;">
<tr><td style="padding:0 4px 20px;">
<span style="font-size:22px;font-weight:700;letter-spacing:-0.02em;color:#f6f2ea;">Payday</span>
</td></tr>
<tr><td style="background-color:#121311;border:1px solid #232420;border-radius:16px;padding:32px 28px;">
<p style="margin:0 0 8px;font-size:13px;letter-spacing:0.04em;text-transform:uppercase;color:#8b8a84;">Deposit request</p>
<h1 style="margin:0 0 20px;font-size:22px;line-height:1.3;font-weight:600;color:#f6f2ea;">{issuer} has sent you a deposit request</h1>
<p style="margin:0 0 20px;font-size:15px;line-height:1.6;color:#b0afa9;">Hi {payer}, <span style="color:#f6f2ea;">{issuer}</span> is requesting <span style="color:#f6f2ea;">{amount} USDC</span> through Payday. Open the request to review the details and pay from your wallet.</p>
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin:0 0 24px;border-top:1px solid #232420;">
{rows}</table>
<table role="presentation" cellpadding="0" cellspacing="0" border="0" style="margin:0 0 24px;">
<tr><td style="background-color:#a3d277;border-radius:10px;">
<a href="{url}" style="display:inline-block;padding:14px 22px;font-size:15px;font-weight:600;color:#0f0f0e;text-decoration:none;">Open deposit request</a>
</td></tr>
</table>
<p style="margin:0 0 12px;font-size:13px;line-height:1.6;color:#8b8a84;">Or paste this link into your browser:<br><a href="{url}" style="color:#a3d277;text-decoration:none;word-break:break-all;">{url}</a></p>
<p style="margin:0;font-size:13px;line-height:1.6;color:#8b8a84;">The request is issued by {issuer}; Payday provides the deposit page and settlement. If you were not expecting it, you can simply ignore this email.</p>
</td></tr>
<tr><td style="padding:20px 4px 0;font-size:12px;line-height:1.6;color:#8b8a84;">
Questions? Email <a href="mailto:{contact}" style="color:#b0afa9;text-decoration:underline;">{contact}</a> and quote the request link.
</td></tr>
</table>
</td></tr>
</table>
</body>
</html>
"##,
            contact = CONTACT_ADDRESS,
        )
    }
}

fn detail_row(label: &str, value: &str, mono: bool) -> String {
    let font = if mono {
        "font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:13px;"
    } else {
        "font-size:14px;"
    };
    format!(
        "<tr><td style=\"padding:12px 0;border-bottom:1px solid #232420;font-size:13px;color:#8b8a84;\">{label}</td><td align=\"right\" style=\"padding:12px 0;border-bottom:1px solid #232420;{font}color:#f6f2ea;\">{value}</td></tr>\n"
    )
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Why a send did not happen, and whether trying again could change that.
#[derive(Debug)]
pub enum SendError {
    /// Resend refused the message itself: a malformed or unroutable address,
    /// an unverified sender. The same request will fail the same way.
    Rejected { status: StatusCode, message: String },
    /// Rate limiting, a provider outage, a network fault: retry later.
    Transient(String),
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected { status, message } => {
                write!(f, "resend rejected the email ({status}): {message}")
            }
            Self::Transient(message) => write!(f, "resend send failed: {message}"),
        }
    }
}

#[derive(Serialize)]
struct SendRequest<'a> {
    from: &'a str,
    to: [&'a str; 1],
    reply_to: &'a str,
    subject: &'a str,
    html: &'a str,
    text: &'a str,
}

/// A thin client for Resend's one endpoint Payday uses.
#[derive(Clone)]
pub struct ResendClient {
    http: reqwest::Client,
    api_key: String,
    from: String,
}

impl ResendClient {
    pub fn new(api_key: String, from: String) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("reqwest client"),
            api_key,
            from,
        }
    }

    /// Send one message. `idempotency_key` is the outbox row's id, so a
    /// retry after an ambiguous failure (a timeout past Resend's accept)
    /// cannot deliver the same email twice.
    pub async fn send(
        &self,
        idempotency_key: &str,
        to: &str,
        subject: &str,
        html: &str,
        text: &str,
    ) -> Result<(), SendError> {
        let response = self
            .http
            .post(RESEND_SEND_URL)
            .bearer_auth(&self.api_key)
            .header("Idempotency-Key", idempotency_key)
            .json(&SendRequest {
                from: &self.from,
                to: [to],
                reply_to: CONTACT_ADDRESS,
                subject,
                html,
                text,
            })
            .send()
            .await
            .map_err(|error| SendError::Transient(error.to_string()))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|json| json.get("message")?.as_str().map(str::to_owned))
            .unwrap_or_else(|| body.chars().take(200).collect());
        // 429 and 5xx are Resend asking for patience; every other 4xx is
        // Resend saying this message will never go.
        if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            Err(SendError::Transient(format!("{status}: {message}")))
        } else {
            Err(SendError::Rejected { status, message })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn email() -> DepositRequestEmail {
        DepositRequestEmail {
            issuer_name: "Acme <Studios> & Co".into(),
            payer_name: "Globex".into(),
            amount: "1250.5".into(),
            heading: Some("March retainer".into()),
            reference: Some("INV-001".into()),
            expires_at: DateTime::parse_from_rfc3339("2026-09-07T14:05:00Z")
                .unwrap()
                .with_timezone(&Utc),
            deposit_url: "https://payday.sh/pay/0199a1b2-c3d4-7e5f-8a6b-7c8d9e0f1a2b".into(),
        }
    }

    #[test]
    fn subject_names_the_issuer_the_reason_and_the_amount() {
        assert_eq!(
            email().subject(),
            "Acme <Studios> & Co sent you a deposit request: March retainer (1250.5 USDC)"
        );
        let mut plain = email();
        plain.heading = None;
        assert_eq!(
            plain.subject(),
            "Acme <Studios> & Co sent you a deposit request for 1250.5 USDC"
        );
    }

    #[test]
    fn html_escapes_merchant_text_and_carries_the_link_and_contact() {
        let html = email().html();
        assert!(html.contains("Acme &lt;Studios&gt; &amp; Co has sent you a deposit request"));
        assert!(!html.contains("<Studios>"));
        assert!(html.contains("Hi Globex,"));
        assert!(html.contains(">1250.5 USDC<"));
        assert!(html.contains("March retainer"));
        assert!(html.contains("INV-001"));
        assert!(html.contains("7 September 2026, 14:05 UTC"));
        assert_eq!(
            html.matches("href=\"https://payday.sh/pay/0199a1b2-c3d4-7e5f-8a6b-7c8d9e0f1a2b\"")
                .count(),
            2
        );
        assert!(html.contains("Open deposit request"));
        assert!(html.contains("mailto:contact@payday.sh"));
        assert!(html.contains("background-color:#a3d277"));
    }

    #[test]
    fn html_omits_rows_that_have_no_value() {
        let mut bare = email();
        bare.heading = None;
        bare.reference = None;
        let html = bare.html();
        assert!(!html.contains(">For<"));
        assert!(!html.contains(">Reference<"));
        assert!(html.contains(">Amount<"));
        assert!(html.contains(">Expires<"));
    }

    #[test]
    fn text_alternative_says_the_same_things() {
        let text = email().text();
        assert!(text.starts_with("Hi Globex,\n"));
        assert!(text.contains(
            "Acme <Studios> & Co has issued you a deposit request through Payday for 1250.5 USDC."
        ));
        assert!(text.contains("For: March retainer\nReference: INV-001\nAmount: 1250.5 USDC\nExpires: 7 September 2026, 14:05 UTC"));
        assert!(text.contains("\nhttps://payday.sh/pay/0199a1b2-c3d4-7e5f-8a6b-7c8d9e0f1a2b\n"));
        assert!(text.contains("Email us at contact@payday.sh"));
    }

    #[test]
    fn send_request_wire_shape_matches_resend() {
        let json = serde_json::to_value(SendRequest {
            from: DEFAULT_FROM,
            to: ["payer@example.com"],
            reply_to: CONTACT_ADDRESS,
            subject: "s",
            html: "<p>h</p>",
            text: "t",
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "from": "Payday <contact@payday.sh>",
                "to": ["payer@example.com"],
                "reply_to": "contact@payday.sh",
                "subject": "s",
                "html": "<p>h</p>",
                "text": "t"
            })
        );
    }
}
